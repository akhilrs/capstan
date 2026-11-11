//! `cstan report` in a workspace project: the client forwards `<repo>=<sha>` pairs and `--heads` to the daemon unchanged,
//! with the agent's token (the route is `Access::Agent`); only the daemon reads the forms.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::process::{Command, Output};

use serde_json::{json, Value};

const FRONT: &str = env!("CARGO_BIN_EXE_cstan");

/// Serves one connection, answers a report as accepted and returns the frame it read.
fn report_frame(label: &str, args: &[&str]) -> (Value, Output) {
    let dir = std::env::temp_dir().join(format!("capstan-wsreport-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let listener = UnixListener::bind(dir.join("control.sock")).unwrap();
    let thread = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).unwrap();
        let reply = json!({"ok": true, "result": {"reportId": "r-1", "state": "accepted", "duplicate": false, "announced": true, "carried": [], "warnings": []}});
        writeln!(&stream, "{reply}").unwrap();
        serde_json::from_str::<Value>(&line).unwrap()
    });
    let output = Command::new(FRONT)
        .arg("report")
        .args(args)
        .current_dir(&dir)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("CAPSTAN_TOKEN", "agent-token")
        .env("CAPSTAN_SOCKET", dir.join("control.sock"))
        .env("CAPSTAN_AGENT_ID", "developer-1")
        .output()
        .unwrap();
    let frame = thread.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (frame, output)
}

#[test]
fn pairs_and_heads_pass_the_client_unchanged() {
    let (a, b) = ("a".repeat(40), "b".repeat(40));
    let pairs = [
        format!("web={b}"),
        format!("core={a}"),
        "both repos".to_string(),
    ];
    let pair_args: Vec<&str> = pairs.iter().map(String::as_str).collect();
    for (label, args) in [
        ("pairs", pair_args),
        ("heads", vec!["--heads", "what moved"]),
        ("single", vec![a.as_str(), "one commit"]),
    ] {
        let (frame, output) = report_frame(label, &args);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{label}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(frame["command"], "report", "{label}");
        assert_eq!(frame["credential"], "agent-token", "{label}");
        assert_eq!(frame["args"], json!(args), "{label}");
    }
}
