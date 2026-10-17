//! A new `cstan` against a controller daemon that predates the info digest: `inbox` and `wait` keep working.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

const FRONT: &str = env!("CARGO_BIN_EXE_cstan");

struct Daemon {
    dir: PathBuf,
    frames: Arc<Mutex<Vec<Value>>>,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Serves `requests` connections. An `old` daemon refuses any argument; a new one takes `--digest` and `--limit`.
fn daemon(label: &str, old: bool, requests: usize) -> (Daemon, std::thread::JoinHandle<()>) {
    let dir =
        std::env::temp_dir().join(format!("capstan-olddaemon-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let listener = UnixListener::bind(dir.join("control.sock")).unwrap();
    let frames = Arc::new(Mutex::new(Vec::new()));
    let seen = frames.clone();
    let thread = std::thread::spawn(move || {
        for _ in 0..requests {
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            let frame: Value = serde_json::from_str(&line).unwrap();
            let args = frame["args"].as_array().map_or(0, Vec::len);
            let reply = if old && args > 0 {
                let command = frame["command"].as_str().unwrap().to_string();
                json!({"ok": false, "code": "invalid_request", "message": format!("{command} takes no arguments")})
            } else {
                let message = json!({"messageId": "m-1", "state": "delivered", "from": "pm", "fromAgentId": "pm", "actionNeeded": false, "body": "hello"});
                json!({"ok": true, "result": {"count": 1, "messages": [message], "unacked": []}})
            };
            seen.lock().unwrap().push(frame);
            let mut writer = &stream;
            writeln!(writer, "{reply}").unwrap();
        }
    });
    (Daemon { dir, frames }, thread)
}

fn run(d: &Daemon, args: &[&str]) -> Output {
    Command::new(FRONT)
        .args(args)
        .current_dir(&d.dir)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("CAPSTAN_TOKEN", "agent-token")
        .env("CAPSTAN_SOCKET", d.dir.join("control.sock"))
        .env("CAPSTAN_AGENT_ID", "developer-1")
        .output()
        .unwrap()
}

fn sent(d: &Daemon) -> Vec<Value> {
    d.frames
        .lock()
        .unwrap()
        .iter()
        .map(|f| f["args"].clone())
        .collect()
}

fn stdout(output: &Output) -> String {
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn a_new_daemon_gets_the_digest_for_inbox_and_wait() {
    for command in ["inbox", "wait"] {
        let (d, thread) = daemon(&format!("new-{command}"), false, 1);
        let text = stdout(&run(&d, &[command]));
        thread.join().unwrap();
        assert!(text.contains("hello"), "{text}");
        assert_eq!(sent(&d), vec![json!(["--digest"])]);
    }
}

#[test]
fn an_older_daemon_that_rejects_the_digest_is_asked_again_without_it() {
    for command in ["inbox", "wait"] {
        let (d, thread) = daemon(&format!("old-{command}"), true, 2);
        let text = stdout(&run(&d, &[command]));
        thread.join().unwrap();
        assert!(text.contains("hello"), "{text}");
        assert_eq!(sent(&d), vec![json!(["--digest"]), json!([])]);
    }
}

#[test]
fn wait_until_falls_back_to_a_plain_wait_on_an_older_daemon() {
    let (d, thread) = daemon("old-until", true, 2);
    let text = stdout(&run(&d, &["wait", "--until", "30s"]));
    thread.join().unwrap();
    assert!(text.contains("hello"), "{text}");
    assert_eq!(
        sent(&d),
        vec![json!(["--digest", "--limit", "30"]), json!([])]
    );
}

const WINDOW_MESSAGE: &str = "this controller predates auto-approve windows; restart it (cstan restart) or ask the user with Approve once";

#[test]
fn a_rejected_auto_window_gets_the_restart_message_and_one_request_only() {
    for args in [
        vec![
            "op",
            "decide",
            "p-1",
            "approve",
            "--hash",
            "abcdef123456",
            "--session",
            "auto=60",
        ],
        vec![
            "prompt",
            "answer",
            "r-1",
            "--hash",
            "abcdef123456",
            "option",
            "1",
            "--session",
            "auto=session",
        ],
    ] {
        let (d, thread) = daemon(&format!("window-{}", args[0]), true, 1);
        let output = run(&d, &args);
        thread.join().unwrap();
        assert_ne!(output.status.code(), Some(0));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(WINDOW_MESSAGE), "{stderr}");
        assert_eq!(sent(&d).len(), 1);
    }
}

#[test]
fn a_current_daemon_refusal_of_a_bad_window_keeps_its_own_message() {
    let dir = std::env::temp_dir().join(format!("capstan-olddaemon-bad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let listener = UnixListener::bind(dir.join("control.sock")).unwrap();
    let thread = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).unwrap();
        let reply = json!({"ok": false, "code": "invalid_request", "message": "window 0 must be from 1 to 240 minutes; use --session auto=<minutes>"});
        writeln!(&stream, "{reply}").unwrap();
    });
    let d = Daemon {
        dir,
        frames: Arc::new(Mutex::new(Vec::new())),
    };
    let output = run(
        &d,
        &[
            "prompt",
            "answer",
            "r-1",
            "--hash",
            "abcdef123456",
            "option",
            "1",
            "--session",
            "auto=0",
        ],
    );
    thread.join().unwrap();
    assert_ne!(output.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("window 0 must be from 1 to 240 minutes"),
        "{stderr}"
    );
    assert!(!stderr.contains("predates"), "{stderr}");
}
