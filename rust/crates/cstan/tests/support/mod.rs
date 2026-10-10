//! What both replays share: the scratch directory a transcript starts from, the daemons it runs against, the normalisation
//! of what a run printed (the rule of test/cli-parity-harness.ts) and the overlays that replace an expectation.
#![allow(dead_code)]
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use cstan_front::watch::watch_status;
use cstan_front::{run, Context, Outcome, VERSION};
use regex::Regex;
use serde_json::Value;

/// The PATH a run gets for `$PATH`: git and the system tools, never the host's own bin directories.
pub const HOST_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
pub const VERSION_PLACEHOLDER: &str = "<VERSION>";
pub const FRONT: &str = env!("CARGO_BIN_EXE_cstan");

pub fn tests_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

pub fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{key} is not a string in {value}"))
}

pub fn strings(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{key} is not an array"))
        .iter()
        .map(|v| v.as_str().expect("text").to_string())
        .collect()
}

pub fn substitute(text: &str, scratch: &str) -> String {
    text.replace("$ROOT", scratch).replace("$PATH", HOST_PATH)
}

// ---------------------------------------------------------------------------------------------------- scratch

pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(label: &str, index: usize) -> Scratch {
        let path = std::env::temp_dir().join(format!(
            "capstan-cli-native-{label}-{}-{index}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Scratch(std::fs::canonicalize(path).unwrap())
    }

    pub fn text(&self) -> String {
        self.0.to_str().unwrap().to_string()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // A directory made private by the layout must be made writable again before it can be removed.
        let _ = Command::new("chmod")
            .args(["-R", "u+rwx"])
            .arg(&self.0)
            .status();
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git_fixture(directory: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args([
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.com",
        ])
        .args(args)
        .env_clear()
        .env("PATH", HOST_PATH)
        .env("HOME", "/nonexistent")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn mode_of(value: &Value) -> u32 {
    match value {
        Value::Number(n) => n.as_u64().expect("mode") as u32,
        Value::String(s) => u32::from_str_radix(s, 8).expect("octal mode"),
        other => panic!("mode {other}"),
    }
}

/// Makes the layout of a transcript under `scratch` (`makeLayout` of test/cli-local-transcript-export.ts).
pub fn make_layout(layout: &Value, scratch: &Path, root: &str) {
    for dir in strings(layout, "dirs") {
        std::fs::create_dir_all(scratch.join(dir)).unwrap();
    }
    for file in strings(layout, "files") {
        let path = scratch.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }
    let empty = serde_json::Map::new();
    let contents = layout
        .get("contents")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    for (file, text) in contents {
        let path = scratch.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, substitute(text.as_str().expect("text"), root)).unwrap();
    }
    let copies = layout
        .get("copy")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    for (file, source) in copies {
        let path = scratch.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::copy(
            tests_dir()
                .join("local-transcripts")
                .join(source.as_str().unwrap()),
            &path,
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    for link in layout
        .get("links")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        std::os::unix::fs::symlink(
            scratch.join(string(link, "to")),
            scratch.join(string(link, "path")),
        )
        .unwrap();
    }
    for entry in layout
        .get("git")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let directory = scratch.join(string(entry, "dir"));
        std::fs::create_dir_all(&directory).unwrap();
        git_fixture(&directory, &["init", "--quiet"]);
        if entry.get("commit").and_then(Value::as_bool) == Some(true) {
            std::fs::write(directory.join(".git/info/exclude"), ".capstan/\n").unwrap();
            git_fixture(&directory, &["add", "-A"]);
            git_fixture(
                &directory,
                &[
                    "commit",
                    "--quiet",
                    "--allow-empty",
                    "-m",
                    "chore: initial commit",
                ],
            );
        }
    }
    // Modes last: a directory made private must not stop the files above from being made.
    let modes = layout
        .get("modes")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let mut ordered: Vec<(&String, &Value)> = modes.iter().collect();
    ordered.sort_by_key(|(file, _)| std::cmp::Reverse(file.len()));
    for (file, mode) in ordered {
        let target = scratch.join(file);
        if target.exists() {
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode_of(mode)))
                .unwrap();
        }
    }
    for file in contents.keys().chain(copies.keys()) {
        if !modes.contains_key(file) {
            std::fs::set_permissions(scratch.join(file), std::fs::Permissions::from_mode(0o600))
                .unwrap();
        }
    }
}

// ----------------------------------------------------------------------------------------------- fake daemon

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
        .collect()
}

/// The bytes the fake daemon sends for a `reply` description, or `None` to send nothing.
fn reply_bytes(reply: &Value) -> Option<Vec<u8>> {
    Some(match string(reply, "kind") {
        "line" => {
            let mut bytes = string(reply, "text").as_bytes().to_vec();
            bytes.push(b'\n');
            bytes
        }
        "expanded" => {
            let mut bytes = string(reply, "head").as_bytes().to_vec();
            let text = string(reply, "text");
            for _ in 0..reply.get("count").and_then(Value::as_u64).unwrap() as usize {
                bytes.extend_from_slice(text.as_bytes());
            }
            bytes.extend_from_slice(string(reply, "tail").as_bytes());
            if reply.get("newline").and_then(Value::as_bool) == Some(true) {
                bytes.push(b'\n');
            }
            bytes
        }
        "raw" => hex(string(reply, "hex")),
        "partial" => string(reply, "text").as_bytes().to_vec(),
        "close" | "hang" => return None,
        other => panic!("unknown reply kind {other}"),
    })
}

pub struct FakeDaemon {
    stop: Arc<AtomicBool>,
    request: Arc<Mutex<Option<String>>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl FakeDaemon {
    pub fn start(socket: &Path, reply: &Value) -> FakeDaemon {
        let listener = UnixListener::bind(socket).expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let stop = Arc::new(AtomicBool::new(false));
        let request = Arc::new(Mutex::new(None));
        let kind = string(reply, "kind").to_string();
        let payload = reply_bytes(reply);
        let thread = {
            let (stop, request) = (stop.clone(), request.clone());
            thread::spawn(move || {
                let mut held = Vec::new();
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            stream.set_nonblocking(false).expect("blocking");
                            stream
                                .set_read_timeout(Some(Duration::from_secs(10)))
                                .expect("timeout");
                            let mut bytes = Vec::new();
                            let mut byte = [0u8; 1];
                            while stream.read(&mut byte).unwrap_or(0) == 1 {
                                if byte[0] == b'\n' {
                                    break;
                                }
                                bytes.push(byte[0]);
                            }
                            *request.lock().unwrap() =
                                Some(String::from_utf8_lossy(&bytes).into_owned());
                            if let Some(payload) = &payload {
                                let _ = stream.write_all(payload);
                            }
                            if matches!(kind.as_str(), "hang" | "line" | "expanded" | "raw") {
                                held.push(stream);
                            } else {
                                drop(stream);
                            }
                        }
                        Err(_) => thread::sleep(Duration::from_millis(1)),
                    }
                }
            })
        };
        FakeDaemon {
            stop,
            request,
            thread: Some(thread),
        }
    }

    /// Stops the daemon and returns the request line it read, if any.
    pub fn finish(mut self) -> Option<String> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.join().expect("daemon thread");
        }
        let request = self.request.lock().unwrap().clone();
        request
    }
}

// --------------------------------------------------------------------------------------------------- running

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    pub stdout: String,
    pub stderr: String,
    pub exit: Option<i32>,
    pub signal: Option<&'static str>,
}

pub struct Settings {
    pub env: Vec<(OsString, OsString)>,
    pub cwd: PathBuf,
    pub now_ms: f64,
    pub terminal: Option<bool>,
}

/// One command, in this process: what `main` would do, except that a watch stops after its first frame and `cstan-dash`
/// is not started (the program and arguments it would get are the output).
pub fn run_once(argv: &[String], settings: &Settings) -> Observed {
    let outcome = run(&Context {
        args: argv.iter().map(OsString::from).collect(),
        env: settings.env.clone(),
        cwd: settings.cwd.clone(),
        now_ms: settings.now_ms,
        cli_path: PathBuf::from(FRONT),
        terminal: settings.terminal,
    });
    let text = |bytes: Vec<u8>| String::from_utf8_lossy(&bytes).into_owned();
    match outcome {
        Outcome::Done {
            stdout,
            stderr,
            exit,
        } => Observed {
            stdout: text(stdout),
            stderr: text(stderr),
            exit: Some(exit),
            signal: None,
        },
        Outcome::Watch { plan, stderr } => {
            let mut stdout = String::new();
            watch_status(
                || cstan_front::watch::fetch_status(&plan),
                |chunk| stdout.push_str(chunk),
                |_| {},
                Duration::from_secs(plan.interval_seconds),
                Some(1),
            );
            Observed {
                stdout,
                stderr: text(stderr),
                exit: None,
                signal: Some("SIGTERM"),
            }
        }
        Outcome::Exec { plan, stderr } => Observed {
            stdout: format!(
                "exec {} {}\n",
                plan.program.display(),
                plan.args
                    .iter()
                    .map(|a| a.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            stderr: text(stderr),
            exit: Some(0),
            signal: None,
        },
        Outcome::Serve(serve) => Observed {
            stdout: format!("{serve:?}\n"),
            stderr: String::new(),
            exit: Some(0),
            signal: None,
        },
    }
}

/// Kills the daemons of the project at `project`: processes of this executable whose working directory is the project (nothing
/// else is ever signalled).
pub fn kill_daemons_of(project: &Path) {
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(pid) = name.to_str().and_then(|n| n.parse::<i32>().ok()) else {
                continue;
            };
            if std::fs::read_link(format!("/proc/{pid}/cwd")).is_ok_and(|cwd| cwd == project)
                && std::fs::read_link(format!("/proc/{pid}/exe"))
                    .is_ok_and(|exe| exe == Path::new(FRONT))
            {
                // SAFETY: the process is a daemon of this scratch project, started by this test.
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        }
    }
}

/// Stops the scratch daemon of the project in `scratch/proj`, and kills one that did not stop.
pub fn stop_scratch_daemon(scratch: &Path, settings: &Settings) {
    let project = scratch.join("proj");
    if !project.join(".capstan/state/control.sock").exists() {
        return;
    }
    let stop = Settings {
        env: settings.env.clone(),
        cwd: settings.cwd.clone(),
        now_ms: settings.now_ms,
        terminal: Some(false),
    };
    let _ = run_once(&["stop".to_string()], &stop);
    kill_daemons_of(&project);
}

// ----------------------------------------------------------------------------------------------- normalising

/// The rule of `Normaliser` in test/cli-parity-harness.ts, plus the operator key and the package version.
pub struct Normaliser {
    root: String,
    ids: BTreeMap<String, usize>,
    secrets: Vec<String>,
}

fn word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn boundary_before(text: &[u8], at: usize) -> bool {
    at == 0 || !word_byte(text[at - 1])
}

/// `TIMESTAMP_LOCAL`: `\b\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?(?![\dZ.:+-])`, as the backtracking engine
/// would match it (the regex crate has no lookahead).
fn local_timestamp_end(text: &[u8], at: usize) -> Option<usize> {
    let digits = |from: usize, n: usize| {
        (from..from + n).all(|i| text.get(i).is_some_and(u8::is_ascii_digit))
    };
    if !boundary_before(text, at)
        || !digits(at, 4)
        || text.get(at + 4) != Some(&b'-')
        || !digits(at + 5, 2)
    {
        return None;
    }
    if text.get(at + 7) != Some(&b'-')
        || !digits(at + 8, 2)
        || !matches!(text.get(at + 10), Some(b'T' | b' '))
    {
        return None;
    }
    if !digits(at + 11, 2) || text.get(at + 13) != Some(&b':') || !digits(at + 14, 2) {
        return None;
    }
    let ok = |end: usize| {
        !text
            .get(end)
            .is_some_and(|b| b.is_ascii_digit() || matches!(b, b'Z' | b'.' | b':' | b'+' | b'-'))
    };
    let minutes_end = at + 16;
    let mut candidates = Vec::new();
    if text.get(minutes_end) == Some(&b':') && digits(minutes_end + 1, 2) {
        let seconds_end = minutes_end + 3;
        if text.get(seconds_end) == Some(&b'.')
            && text.get(seconds_end + 1).is_some_and(u8::is_ascii_digit)
        {
            let mut end = seconds_end + 1;
            while text.get(end).is_some_and(u8::is_ascii_digit) {
                end += 1;
            }
            // Fewer fraction digits would leave a digit next, which the lookahead refuses.
            candidates.push(end);
        }
        candidates.push(seconds_end);
    }
    candidates.push(minutes_end);
    candidates.into_iter().find(|end| ok(*end))
}

fn replace_local_timestamps(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut at = 0;
    let mut copied = 0;
    while at < bytes.len() {
        if bytes[at].is_ascii_digit() {
            if let Some(end) = local_timestamp_end(bytes, at) {
                out.push_str(&text[copied..at]);
                out.push_str("<TS:local>");
                at = end;
                copied = end;
                continue;
            }
        }
        at += 1;
    }
    out.push_str(&text[copied..]);
    out
}

/// `normaliseVersion`: the version as a whole token.
pub fn normalise_version(text: &str, version: &str) -> String {
    let mut out = String::new();
    let mut last = 0;
    for (at, _) in text.match_indices(version) {
        let before = text[..at].chars().next_back();
        let after = text[at + version.len()..].chars().next();
        let part = |c: Option<char>| c.is_some_and(|c| c.is_ascii_digit() || c == '.');
        if at < last || part(before) || part(after) {
            continue;
        }
        out.push_str(&text[last..at]);
        out.push_str(VERSION_PLACEHOLDER);
        last = at + version.len();
    }
    out.push_str(&text[last..]);
    out
}

impl Normaliser {
    pub fn new(root: &str, secrets: Vec<String>) -> Normaliser {
        Normaliser {
            root: root.to_string(),
            ids: BTreeMap::new(),
            secrets,
        }
    }

    fn id(&mut self, value: &str) -> String {
        let next = self.ids.len() + 1;
        let ordinal = *self.ids.entry(value.to_string()).or_insert(next);
        format!("<ID#{ordinal}>")
    }

    fn replace_ids(&mut self, pattern: &Regex, text: &str) -> String {
        let mut out = String::new();
        let mut last = 0;
        for found in pattern.find_iter(text) {
            out.push_str(&text[last..found.start()]);
            out.push_str(&self.id(found.as_str()));
            last = found.end();
        }
        out.push_str(&text[last..]);
        out
    }

    pub fn text(&mut self, value: &str) -> String {
        let mut text = value.to_string();
        for secret in &self.secrets {
            text = text.replace(secret.as_str(), "<TOKEN:operator>");
        }
        let text = text.replace(&self.root, "<ROOT>");
        let timestamp_z =
            Regex::new(r"(?-u:\b)\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z(?-u:\b)").unwrap();
        let text = timestamp_z.replace_all(&text, "<TS:z>").into_owned();
        let text = replace_local_timestamps(&text);
        let uuid = Regex::new(
            r"(?-u:\b)[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}(?-u:\b)",
        )
        .unwrap();
        let project = Regex::new(r"(?-u:\b)p[0-9a-f]{32}(?-u:\b)").unwrap();
        let text = self.replace_ids(&uuid, &text);
        let text = self.replace_ids(&project, &text);
        let pid = Regex::new(r#"(?i)((?-u:\b)pid["']?\s*[:=]?\s*)\d+"#).unwrap();
        let text = pid.replace_all(&text, "${1}<PID>").into_owned();
        let age =
            Regex::new(r"((?-u:\b)PAUSED \()\d+s\)|((?-u:\b)oldest )\d+( min(?-u:\b))").unwrap();
        age.replace_all(&text, |caps: &regex::Captures| match caps.get(1) {
            Some(paused) => format!("{}<AGE>s)", paused.as_str()),
            None => format!("{}<AGE>{}", &caps[2], &caps[3]),
        })
        .into_owned()
    }

    pub fn full(&mut self, value: &str) -> String {
        let text = self.text(value);
        normalise_version(&text, VERSION)
    }
}

// ----------------------------------------------------------------------------------------------- overlays

/// The overlays of this crate's replays: `tests/divergences`, each naming a fixture below `tests` and the case it holds.
pub fn load_overlays() -> capstan_parity_overlay::Overlays {
    let cases_of = |file: &Path| -> Result<Vec<String>, String> {
        let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
        let value: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        Ok(vec![string(&value, "name").to_string()])
    };
    capstan_parity_overlay::load(&tests_dir().join("divergences"), &tests_dir(), &cases_of)
        .unwrap_or_else(|e| panic!("{e}"))
}
