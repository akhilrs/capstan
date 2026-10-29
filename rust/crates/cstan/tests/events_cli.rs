//! `cstan events` as a process: the command line it accepts, the exit codes of a bad one, and the stop signal.
mod support;

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use support::FRONT;

fn cstan(args: &[&str]) -> std::process::Output {
    Command::new(FRONT)
        .args(args)
        .current_dir(std::env::temp_dir())
        .env_clear()
        .stdin(Stdio::null())
        .output()
        .expect("cstan runs")
}

#[test]
fn a_bad_command_line_exits_2_with_nothing_on_stdout() {
    let bad: &[&[&str]] = &[
        &["events", "--bogus"],
        &["events", "--cursor"],
        &["events", "--root"],
        &["events", "--poll-ms"],
        &["events", "--cursor", "a", "--cursor", "b"],
        &["events", "--poll-ms", "5", "--poll-ms", "6"],
        &["events", "extra"],
        &["events", "--"],
        &["events", "--json"],
        &["events", "--poll-ms", "3600001"],
        &["events", "--poll-ms", "abc"],
        &["events", "--poll-ms", "-1"],
        &["--json", "events"],
        &["EVENTS"],
    ];
    for args in bad {
        let out = cstan(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
        assert!(!out.stderr.is_empty(), "{args:?}");
    }
}

#[test]
fn events_help_exits_0() {
    let out = cstan(&["events", "--help"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("--poll-ms"));
}

#[cfg(debug_assertions)]
#[test]
fn the_stream_says_hello_and_stops_on_sigterm() {
    let dir = std::env::temp_dir().join(format!("cstan-events-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut child = Command::new(FRONT)
        .args(["events", "--poll-ms", "100", "--root"])
        .arg(&dir)
        .current_dir(&dir)
        .env_clear()
        .env("CSTAN_EVENTS_NO_PROCESS_SCAN", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("cstan runs");
    let mut lines = BufReader::new(child.stdout.take().unwrap());
    let mut first = String::new();
    lines.read_line(&mut first).unwrap();
    let hello: serde_json::Value = serde_json::from_str(first.trim_end()).expect("a JSON line");
    assert_eq!(hello["kind"], "hello", "{first}");
    assert_eq!(hello["projects"], 0, "{first}");
    // SAFETY: signals the child this test started.
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "no exit within 10 s");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(0));
    let mut rest = String::new();
    let mut last = first;
    while lines.read_line(&mut rest).unwrap() > 0 {
        last = std::mem::take(&mut rest);
    }
    let last: serde_json::Value = serde_json::from_str(last.trim_end()).unwrap();
    assert_eq!(last["kind"], "cursor");
    let _ = std::fs::remove_dir_all(Path::new(&dir));
}
