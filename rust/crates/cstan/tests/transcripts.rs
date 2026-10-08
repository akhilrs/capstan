//! Replays every transcript in tests/transcripts (written by test/cli-transcript-export.ts from the Node CLI) against a
//! fake daemon on a real socket and compares stdout, stderr and the exit code byte for byte; a transcript marked
//! `fallback` must be handed to Node without a byte reaching the daemon.
use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use capstan_wire::js::{self, Value};
use cstan_front::{run, Context, Outcome, VERSION};

fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    match value.get(key) {
        Some(Value::String(s)) => Box::leak(s.to_utf8().expect("text").into_boxed_str()),
        other => panic!("{key} is not a string: {other:?}"),
    }
}

fn strings(value: &Value, key: &str) -> Vec<String> {
    match value.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| match v {
                Value::String(s) => s.to_utf8().expect("text"),
                other => panic!("{other:?}"),
            })
            .collect(),
        other => panic!("{key} is not an array: {other:?}"),
    }
}

fn number(value: &Value, key: &str) -> f64 {
    match value.get(key) {
        Some(Value::Number(n)) => *n,
        other => panic!("{key} is not a number: {other:?}"),
    }
}

/// The bytes the fake daemon sends for a `reply` description, or `None` to send nothing.
fn reply_bytes(reply: &Value) -> Option<Vec<u8>> {
    let kind = string(reply, "kind");
    Some(match kind {
        "line" => {
            let mut bytes = string(reply, "text").as_bytes().to_vec();
            bytes.push(b'\n');
            bytes
        }
        "expanded" => {
            let mut bytes = string(reply, "head").as_bytes().to_vec();
            let text = string(reply, "text");
            for _ in 0..number(reply, "count") as usize {
                bytes.extend_from_slice(text.as_bytes());
            }
            bytes.extend_from_slice(string(reply, "tail").as_bytes());
            if matches!(reply.get("newline"), Some(Value::Bool(true))) {
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

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
        .collect()
}

struct Daemon {
    stop: Arc<AtomicBool>,
    request: Arc<Mutex<Option<String>>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Daemon {
    fn start(socket: &Path, reply: &Value) -> Daemon {
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
                            if kind != "hang"
                                && kind != "line"
                                && kind != "expanded"
                                && kind != "raw"
                            {
                                // partial, close: the connection ends now.
                                drop(stream);
                            } else {
                                held.push(stream);
                            }
                        }
                        Err(_) => thread::sleep(Duration::from_millis(1)),
                    }
                }
            })
        };
        Daemon {
            stop,
            request,
            thread: Some(thread),
        }
    }

    fn finish(mut self) -> Option<String> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.join().expect("daemon thread");
        }
        let request = self.request.lock().unwrap().clone();
        request
    }
}

fn expected_string(value: &Value, key: &str, scratch: &str) -> String {
    string(value, key)
        .replace("$ROOT", scratch)
        .replace("$VERSION", VERSION)
}

fn replay(file: &Path, index: usize) -> Result<(), String> {
    let text = std::fs::read_to_string(file).unwrap();
    let transcript = js::parse(&text).unwrap();
    let name = string(&transcript, "name");
    let scratch = std::env::temp_dir().join(format!("cstan-rt-{}-{index}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();
    let scratch = std::fs::canonicalize(scratch).unwrap();
    let root = scratch.to_str().unwrap().to_string();
    let result = replay_in(&transcript, name, &scratch, &root);
    let _ = std::fs::remove_dir_all(&scratch);
    result.map_err(|e| format!("{name}: {e}"))
}

fn replay_in(transcript: &Value, name: &str, scratch: &Path, root: &str) -> Result<(), String> {
    let layout = transcript.get("layout").unwrap();
    for dir in strings(layout, "dirs") {
        std::fs::create_dir_all(scratch.join(dir)).unwrap();
    }
    for file in strings(layout, "files") {
        let path = scratch.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }
    if let Some(Value::Array(links)) = layout.get("links") {
        for link in links {
            std::os::unix::fs::symlink(
                scratch.join(string(link, "to")),
                scratch.join(string(link, "path")),
            )
            .unwrap();
        }
    }
    let cwd: PathBuf = scratch.join(string(transcript, "cwd"));
    std::fs::create_dir_all(&cwd).unwrap();
    let env: Vec<(OsString, OsString)> = match transcript.get("env") {
        Some(Value::Object(members)) => members
            .iter()
            .map(|(k, v)| match v {
                Value::String(s) => (
                    OsString::from(k.to_utf8().unwrap()),
                    OsString::from(s.to_utf8().unwrap().replace("$ROOT", root)),
                ),
                other => panic!("{other:?}"),
            })
            .collect(),
        other => panic!("{other:?}"),
    };
    let args: Vec<OsString> = strings(transcript, "argv")
        .into_iter()
        .map(|a| OsString::from(a.replace("$ROOT", root)))
        .collect();
    let daemon = match transcript.get("daemon") {
        Some(daemon @ Value::Object(_)) => {
            let socket = scratch.join(
                string(daemon, "socket")
                    .replace("$ROOT", root)
                    .strip_prefix(&format!("{root}/"))
                    .unwrap(),
            );
            Some(Daemon::start(&socket, daemon.get("reply").unwrap()))
        }
        _ => None,
    };
    let outcome = run(&Context {
        args,
        env,
        cwd,
        now_ms: number(transcript, "now"),
    });
    let request = daemon.map(Daemon::finish);
    let fallback = matches!(transcript.get("fallback"), Some(Value::Bool(true)));
    if fallback {
        return match outcome {
            Outcome::Fallback(_) if request.flatten().is_none() => Ok(()),
            Outcome::Fallback(_) => Err("fell back after a request reached the daemon".into()),
            Outcome::Done {
                stdout,
                stderr,
                exit,
            } => Err(format!(
                "answered natively (exit {exit}, stdout {:?}, stderr {:?}) but must fall back",
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            )),
        };
    }
    let Outcome::Done {
        stdout,
        stderr,
        exit,
    } = outcome
    else {
        return Err(format!("fell back ({outcome:?}) but must answer natively"));
    };
    let (want_out, want_err, want_exit) = match transcript.get("node") {
        Some(node @ Value::Object(_)) => (
            expected_string(node, "stdout", root),
            expected_string(node, "stderr", root),
            number(node, "exit") as i32,
        ),
        _ if name.starts_with("front-version") => {
            (format!("cstan-front {VERSION}\n"), String::new(), 0)
        }
        other => panic!("{name}: no expected output: {other:?}"),
    };
    let want_request = match transcript.get("request") {
        Some(Value::String(s)) => Some(s.to_utf8().unwrap().replace("$ROOT", root)),
        _ => None,
    };
    if let Some(got) = request {
        if got != want_request {
            return Err(format!("request {got:?}, want {want_request:?}"));
        }
    }
    let mut problems = Vec::new();
    if stdout != want_out.as_bytes() {
        problems.push(format!(
            "stdout {:?}, want {:?}",
            String::from_utf8_lossy(&stdout),
            want_out
        ));
    }
    if stderr != want_err.as_bytes() {
        problems.push(format!(
            "stderr {:?}, want {:?}",
            String::from_utf8_lossy(&stderr),
            want_err
        ));
    }
    if exit != want_exit {
        problems.push(format!("exit {exit}, want {want_exit}"));
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("; "))
    }
}

#[test]
fn every_transcript_replays_byte_for_byte() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/transcripts");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&directory)
        .expect("tests/transcripts")
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    assert!(
        files.len() > 100,
        "the transcripts are missing: run `npm run build && node dist/test/cli-transcript-export.js`"
    );
    let next = AtomicUsize::new(0);
    let failures = Mutex::new(Vec::new());
    thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| loop {
                let at = next.fetch_add(1, Ordering::SeqCst);
                let Some(file) = files.get(at) else { return };
                if let Err(failure) = replay(file, at) {
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
