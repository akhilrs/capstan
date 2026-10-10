//! Replays every local transcript (tests/local-transcripts, written by test/cli-local-transcript-export.ts from the Node
//! CLI): the commands that run on the local machine or against a real daemon of a scratch project. Each runs in this
//! process with the transcript's argv, environment and directory layout; what it printed (normalised the way the Node
//! export normalises it), the files it created and the commits it made are compared with what Node did.
mod support;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread;

use serde_json::{json, Value};
use support::*;

struct Entry {
    kind: &'static str,
    mode: String,
    bytes: Option<Vec<u8>>,
}

/// `snapshot`: every file below `directory`, in the order the Node export walks them; `.git` is a single directory entry.
fn snapshot(directory: &Path) -> Vec<(String, Entry)> {
    fn walk(base: &Path, current: &Path, out: &mut Vec<(String, Entry)>) {
        let mut names: Vec<_> = std::fs::read_dir(current)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        for name in names {
            let full = current.join(&name);
            let relative = full
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let metadata = std::fs::symlink_metadata(&full).unwrap();
            let mode = format!(
                "{:04o}",
                std::os::unix::fs::PermissionsExt::mode(&metadata.permissions()) & 0o777
            );
            if metadata.is_dir() {
                out.push((
                    relative,
                    Entry {
                        kind: "dir",
                        mode,
                        bytes: None,
                    },
                ));
                if name != ".git" {
                    walk(base, &full, out);
                }
            } else if metadata.is_file() {
                out.push((
                    relative,
                    Entry {
                        kind: "file",
                        mode,
                        bytes: Some(std::fs::read(&full).unwrap()),
                    },
                ));
            }
        }
    }
    let mut out = Vec::new();
    walk(directory, directory, &mut out);
    out
}

fn git_output(directory: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .env_clear()
        .env("PATH", HOST_PATH)
        .env("HOME", "/nonexistent")
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn commits_of(directory: &Path) -> Value {
    let mut entries = Vec::new();
    if let Some(log) = git_output(directory, &["log", "--format=%H%x00%s"]) {
        for row in log.split('\n').filter(|l| !l.is_empty()) {
            let (hash, subject) = row.split_once('\0').unwrap();
            let tree =
                git_output(directory, &["ls-tree", "-r", "--name-only", hash]).unwrap_or_default();
            let files: Vec<&str> = tree.split('\n').filter(|l| !l.is_empty()).collect();
            entries.push(json!({"subject": subject, "files": files}));
        }
    }
    Value::Array(entries)
}

fn env_of(value: &Value, root: &str) -> Vec<(OsString, OsString)> {
    value
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| {
            (
                OsString::from(k),
                OsString::from(substitute(v.as_str().unwrap(), root)),
            )
        })
        .collect()
}

fn replay(
    file: &Path,
    index: usize,
    overlays: &capstan_parity_overlay::Overlays,
) -> Result<(), String> {
    let transcript: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    let name = string(&transcript, "name").to_string();
    let scratch = Scratch::new("local", index);
    replay_in(file, &transcript, &name, &scratch, overlays).map_err(|e| format!("{name}: {e}"))
}

fn replay_in(
    file: &Path,
    transcript: &Value,
    name: &str,
    scratch: &Scratch,
    overlays: &capstan_parity_overlay::Overlays,
) -> Result<(), String> {
    let root = scratch.text();
    let layout = transcript.get("layout").unwrap();
    make_layout(layout, &scratch.0, &root);
    let cwd_name = string(transcript, "cwd");
    let cwd = scratch.0.join(cwd_name);
    std::fs::create_dir_all(&cwd).unwrap();
    let env = env_of(transcript.get("env").unwrap(), &root);
    let daemon = transcript.get("daemon").filter(|d| d.is_object());
    let scratch_daemon = daemon.is_some_and(|d| string(d, "kind") == "scratch");
    let fake = daemon.filter(|d| string(d, "kind") == "fake").map(|d| {
        let socket = substitute(string(d, "socket"), &root);
        FakeDaemon::start(Path::new(&socket), d.get("reply").unwrap())
    });
    let original = snapshot(&scratch.0);
    let real_now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64;
    let before = transcript
        .get("before")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let tty = transcript.get("tty").and_then(Value::as_bool) == Some(true);
    let step_settings = |env: Vec<(OsString, OsString)>, now_ms: f64, terminal: bool| Settings {
        env,
        cwd: cwd.clone(),
        now_ms,
        terminal: Some(terminal),
    };
    for step in &before {
        let step_env = match step.get("env") {
            Some(env) if env.is_object() => env_of(env, &root),
            _ => env.clone(),
        };
        let argv: Vec<String> = strings(step, "argv")
            .iter()
            .map(|a| substitute(a, &root))
            .collect();
        run_once(&argv, &step_settings(step_env, real_now, false));
    }
    let argv: Vec<String> = strings(transcript, "argv")
        .iter()
        .map(|a| substitute(a, &root))
        .collect();
    let now = if scratch_daemon {
        real_now
    } else {
        transcript.get("now").and_then(Value::as_f64).unwrap()
    };
    let mut observed = run_once(&argv, &step_settings(env.clone(), now, tty));
    let request = fake.and_then(FakeDaemon::finish);
    if scratch_daemon || !before.is_empty() {
        stop_scratch_daemon(&scratch.0, &step_settings(env.clone(), real_now, false));
    }
    if tty {
        // A pseudo-terminal merges the two streams and turns each line feed into a carriage return and a line feed.
        let merged = format!("{}{}", observed.stdout, observed.stderr).replace('\n', "\r\n");
        observed.stdout = merged;
        observed.stderr = String::new();
    }
    let mut secrets = Vec::new();
    for key_file in [
        scratch.0.join("proj/.capstan/operator.key"),
        cwd.join(".capstan/operator.key"),
    ] {
        if let Ok(key) = std::fs::read_to_string(&key_file) {
            let key = key.trim().to_string();
            if !key.is_empty() && !secrets.contains(&key) {
                secrets.push(key);
            }
        }
    }
    let mut normaliser = Normaliser::new(&root, secrets);
    let mut files: Option<Vec<Value>> = None;
    let mut commits: Option<serde_json::Map<String, Value>> = None;
    if transcript.get("files").is_some_and(Value::is_array) {
        let after = snapshot(&scratch.0);
        let mut list = Vec::new();
        for (relative, entry) in &after {
            let was = original.iter().find(|(p, _)| p == relative).map(|(_, e)| e);
            if was.is_some_and(|w| {
                w.kind == entry.kind && w.mode == entry.mode && w.bytes == entry.bytes
            }) {
                continue;
            }
            if relative.starts_with(".git/") && relative != ".git/info/exclude" {
                continue;
            }
            let path = normaliser.full(relative);
            let mut member = json!({"path": path, "kind": entry.kind, "mode": entry.mode});
            if let Some(bytes) = &entry.bytes {
                member["text"] = Value::String(normaliser.full(&String::from_utf8_lossy(bytes)));
            }
            list.push(member);
        }
        files = Some(list);
        let mut map = serde_json::Map::new();
        let mut dirs: Vec<String> = layout
            .get("git")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|g| string(g, "dir").to_string())
            .collect();
        dirs.push(cwd_name.to_string());
        for dir in dirs {
            let directory = scratch.0.join(&dir);
            if directory.join(".git").exists() {
                map.insert(dir, commits_of(&directory));
            }
        }
        commits = Some(map);
    }
    let got_request = request
        .map(|r| normaliser.full(&r))
        .filter(|r| !r.is_empty());
    let got_out = normaliser.full(&observed.stdout);
    let got_err = normaliser.full(&observed.stderr);

    let fixture = format!(
        "local-transcripts/{}",
        file.file_name().unwrap().to_string_lossy()
    );
    let want = overlays
        .expected(&fixture, name)
        .unwrap_or_else(|| transcript.get("node").unwrap());
    let mut problems = Vec::new();
    if string(want, "stdout") != got_out {
        problems.push(format!(
            "stdout {got_out:?}, want {:?}",
            string(want, "stdout")
        ));
    }
    if string(want, "stderr") != got_err {
        problems.push(format!(
            "stderr {got_err:?}, want {:?}",
            string(want, "stderr")
        ));
    }
    let want_signal = want.get("signal").and_then(Value::as_str);
    if want_signal != observed.signal {
        problems.push(format!(
            "signal {:?}, want {want_signal:?}",
            observed.signal
        ));
    }
    if want_signal.is_none()
        && want.get("exit").and_then(Value::as_i64).map(|e| e as i32) != observed.exit
    {
        problems.push(format!(
            "exit {:?}, want {:?}",
            observed.exit,
            want.get("exit")
        ));
    }
    if transcript
        .get("daemon")
        .is_some_and(|d| d.get("kind").and_then(Value::as_str) == Some("fake"))
    {
        let want_request = transcript
            .get("request")
            .and_then(Value::as_str)
            .map(str::to_string);
        if got_request != want_request {
            problems.push(format!("request {got_request:?}, want {want_request:?}"));
        }
    }
    if let (Some(files), Some(want_files)) =
        (&files, transcript.get("files").and_then(Value::as_array))
    {
        if files != want_files {
            problems.push(format!(
                "files {}, want {}",
                Value::Array(files.clone()),
                Value::Array(want_files.clone())
            ));
        }
    }
    if let (Some(commits), Some(want_commits)) = (
        &commits,
        transcript.get("commits").filter(|c| c.is_object()),
    ) {
        if &Value::Object(commits.clone()) != want_commits {
            problems.push(format!(
                "commits {}, want {want_commits}",
                Value::Object(commits.clone())
            ));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("; "))
    }
}

#[test]
fn every_local_transcript_replays() {
    let directory = tests_dir().join("local-transcripts");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&directory)
        .expect("tests/local-transcripts")
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    assert!(files.len() > 100, "the local transcripts are missing");
    let overlays = load_overlays();
    let next = AtomicUsize::new(0);
    let failures = Mutex::new(Vec::new());
    thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| loop {
                let at = next.fetch_add(1, Ordering::SeqCst);
                let Some(file) = files.get(at) else { return };
                if let Err(failure) = replay(file, at, &overlays) {
                    failures.lock().unwrap().push(failure);
                }
            });
        }
    });
    let mut failures = failures.into_inner().unwrap();
    failures.sort();
    assert!(
        failures.is_empty(),
        "{} of {} local transcripts differ:\n{}",
        failures.len(),
        files.len(),
        failures.join("\n")
    );
}
