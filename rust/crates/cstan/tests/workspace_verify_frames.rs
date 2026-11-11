//! `cstan verify <id> --repo <repo>`: the client forwards the arguments to the daemon unchanged, with the agent's token;
//! only the daemon reads `--repo`. A malformed shape ends in the client's usage error before any frame is sent.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::process::{Command, Output};

use serde_json::{json, Value};

const FRONT: &str = env!("CARGO_BIN_EXE_cstan");

fn verify_frame(label: &str, args: &[&str]) -> (Option<Value>, Output) {
    let dir = std::env::temp_dir().join(format!("capstan-wsverify-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let listener = UnixListener::bind(dir.join("control.sock")).unwrap();
    listener.set_nonblocking(true).unwrap();
    let owned: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let output = {
        let socket = dir.join("control.sock");
        let dir = dir.clone();
        let child = std::thread::spawn(move || {
            Command::new(FRONT)
                .arg("verify")
                .args(owned)
                .current_dir(&dir)
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("CAPSTAN_TOKEN", "agent-token")
                .env("CAPSTAN_SOCKET", socket)
                .env("CAPSTAN_AGENT_ID", "developer-1")
                .output()
                .unwrap()
        });
        let mut frame = None;
        for _ in 0..200 {
            if let Ok((stream, _)) = listener.accept() {
                stream.set_nonblocking(false).unwrap();
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).unwrap();
                let reply = json!({"ok": true, "result": {
                    "runId": "run-1", "state": "queued", "commitSha": "a".repeat(40), "repo": "web", "existing": false,
                    "runs": [
                        {"repo": "web", "runId": "run-1", "state": "queued", "existing": false},
                        {"repo": "core", "runId": "run-2", "state": "running", "existing": true}
                    ]
                }});
                writeln!(&stream, "{reply}").unwrap();
                frame = Some(serde_json::from_str::<Value>(&line).unwrap());
                break;
            }
            if child.is_finished() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        (frame, child.join().unwrap())
    };
    let _ = std::fs::remove_dir_all(&dir);
    output
}

#[test]
fn repo_flag_frames_pass_the_client_unchanged() {
    let (frame, output) = verify_frame("repo", &["r-1", "--repo", "web"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    assert_eq!(
        text,
        "verify run run-1 for repo web queued; cstan verify show run-1\nverify run run-2 for repo core already running; cstan verify show run-2\n"
    );
    let frame = frame.expect("a frame reached the daemon");
    assert_eq!(frame["command"], "verify");
    assert_eq!(frame["credential"], "agent-token");
    assert_eq!(frame["args"], json!(["r-1", "--repo", "web"]));
}

#[test]
fn a_misplaced_repo_flag_is_a_usage_error_and_sends_nothing() {
    for (label, args) in [
        ("show", vec!["show", "run-1", "--repo", "web"]),
        ("dangling", vec!["r-1", "--repo"]),
        ("order", vec!["--repo", "web", "r-1"]),
    ] {
        let (frame, output) = verify_frame(label, &args);
        assert!(frame.is_none(), "{label}: a frame was sent");
        assert_ne!(output.status.code(), Some(0), "{label}");
    }
}
