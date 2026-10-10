//! Replays every transcript in tests/transcripts (written by test/cli-transcript-export.ts from the Node CLI) against a
//! fake daemon on a real socket and compares stdout, stderr and the exit code byte for byte. The transcripts marked
//! `fallback` were commands the first Rust front end handed to Node; they are answered natively now and compared with the
//! output Node recorded for them. Where Rust deliberately differs, an overlay in tests/divergences replaces the
//! expectation of the case.
mod support;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread;

use cstan_front::VERSION;
use serde_json::Value;
use support::*;

fn expected_string(value: &Value, key: &str, scratch: &str, version: &str) -> String {
    string(value, key)
        .replace("$ROOT", scratch)
        .replace(VERSION_PLACEHOLDER, version)
}

/// What a transcript expects of the run: the overlay of the case when there is one, else what Node recorded.
fn expectation<'a>(transcript: &'a Value, overlay: Option<&'a Value>) -> Option<&'a Value> {
    overlay.or_else(|| {
        [transcript.get("node"), transcript.get("expected")]
            .into_iter()
            .flatten()
            .find(|v| v.is_object())
    })
}

fn replay(
    file: &Path,
    index: usize,
    overlays: &capstan_parity_overlay::Overlays,
) -> Result<(), String> {
    let text = std::fs::read_to_string(file).unwrap();
    let transcript: Value = serde_json::from_str(&text).unwrap();
    let name = string(&transcript, "name").to_string();
    let scratch = Scratch::new("replay", index);
    let result = replay_in(file, &transcript, &name, &scratch, overlays);
    result.map_err(|e| format!("{name}: {e}"))
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
    let cwd: PathBuf = scratch.0.join(string(transcript, "cwd"));
    std::fs::create_dir_all(&cwd).unwrap();
    let env: Vec<(OsString, OsString)> = transcript
        .get("env")
        .and_then(Value::as_object)
        .unwrap()
        .iter()
        .map(|(k, v)| {
            (
                OsString::from(k),
                OsString::from(substitute(v.as_str().unwrap(), &root)),
            )
        })
        .collect();
    let args: Vec<String> = strings(transcript, "argv")
        .into_iter()
        .map(|a| substitute(&a, &root))
        .collect();
    let daemon = match transcript.get("daemon") {
        Some(daemon @ Value::Object(_)) => {
            let socket = substitute(string(daemon, "socket"), &root);
            let relative = socket
                .strip_prefix(&format!("{root}/"))
                .unwrap()
                .to_string();
            Some(FakeDaemon::start(
                &scratch.0.join(relative),
                daemon.get("reply").unwrap(),
            ))
        }
        _ => None,
    };
    let settings = Settings {
        env,
        cwd,
        now_ms: transcript.get("now").and_then(Value::as_f64).unwrap(),
        terminal: Some(false),
    };
    let observed = run_once(&args, &settings);
    let request = daemon.and_then(FakeDaemon::finish);
    let fixture = format!(
        "transcripts/{}",
        file.file_name().unwrap().to_string_lossy()
    );
    let overlay = overlays.expected(&fixture, name);
    let Some(want) = expectation(transcript, overlay) else {
        // Node recorded nothing for this one (it needs a real project); the local transcripts hold its output. It must
        // still run to its end.
        return Ok(());
    };
    let (want_out, want_err, want_exit) = (
        expected_string(want, "stdout", &root, VERSION),
        expected_string(want, "stderr", &root, VERSION),
        want.get("exit").and_then(Value::as_i64).unwrap() as i32,
    );
    let want_request = transcript
        .get("request")
        .and_then(Value::as_str)
        .map(|s| s.replace("$ROOT", &root));
    let mut problems = Vec::new();
    if transcript.get("daemon").is_some_and(Value::is_object) && request != want_request {
        problems.push(format!("request {request:?}, want {want_request:?}"));
    }
    if observed.stdout != want_out {
        problems.push(format!("stdout {:?}, want {want_out:?}", observed.stdout));
    }
    if observed.stderr != want_err {
        problems.push(format!("stderr {:?}, want {want_err:?}", observed.stderr));
    }
    if observed.exit != Some(want_exit) {
        problems.push(format!("exit {:?}, want {want_exit}", observed.exit));
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("; "))
    }
}

fn transcript_files() -> Vec<PathBuf> {
    let directory = tests_dir().join("transcripts");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&directory)
        .expect("tests/transcripts")
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    files
}

#[test]
fn every_transcript_replays_byte_for_byte() {
    let files = transcript_files();
    assert!(
        files.len() > 100,
        "the transcripts are missing: run `npm run build && node dist/test/cli-transcript-export.js`"
    );
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
        "{} of {} transcripts differ:\n{}",
        failures.len(),
        files.len(),
        failures.join("\n")
    );
}

/// The transcripts that hold the package version: every one with the placeholder in what it expects.
fn version_transcripts() -> Vec<PathBuf> {
    transcript_files()
        .into_iter()
        .filter(|path| {
            std::fs::read_to_string(path)
                .unwrap()
                .contains(VERSION_PLACEHOLDER)
        })
        .collect()
}

#[test]
fn the_version_transcripts_replay_with_any_version() {
    let files = version_transcripts();
    let names: Vec<String> = files
        .iter()
        .map(|f| f.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    for wanted in ["fallback-version", "fallback-version-word", "front-version"] {
        assert!(names.iter().any(|n| n == wanted), "no {wanted} transcript");
    }
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap();
        // No version number is written into a transcript: only the placeholder stands for it.
        assert!(
            !text.contains(VERSION),
            "{} holds the version {VERSION}",
            file.display()
        );
        let transcript: Value = serde_json::from_str(&text).unwrap();
        let name = string(&transcript, "name");
        let node =
            expectation(&transcript, None).unwrap_or_else(|| panic!("{name}: no expected output"));
        for version in [VERSION, "0.0.1", "98.76.54-rc.1"] {
            let stdout = expected_string(node, "stdout", "/scratch", version);
            assert!(
                stdout.trim_end().ends_with(version) && !stdout.contains(VERSION_PLACEHOLDER),
                "{name}: {stdout:?} is not the output for version {version}"
            );
        }
    }
    // The front end prints the version it was built with: its output, with that version put back as the placeholder, is
    // what the transcript holds, whatever the version is.
    let observed = run_once(
        &["__front-version".to_string()],
        &Settings {
            env: vec![],
            cwd: std::env::temp_dir(),
            now_ms: 0.0,
            terminal: Some(false),
        },
    );
    let transcript: Value = serde_json::from_str(
        &std::fs::read_to_string(tests_dir().join("transcripts/front-version.json")).unwrap(),
    )
    .unwrap();
    let want = string(transcript.get("expected").unwrap(), "stdout");
    assert_eq!(observed.stdout.replace(VERSION, VERSION_PLACEHOLDER), want);
}
