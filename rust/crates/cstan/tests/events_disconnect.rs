//! `cstan events` ends when its client goes away without a signal: stdin closed, stdout reader closed, SIGHUP.
mod support;

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use support::FRONT;

fn fixture(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cstan-events-disc-{name}-{}", std::process::id()));
    std::fs::create_dir_all(dir.join(".capstan")).unwrap();
    std::fs::write(dir.join(".capstan/undiscoverable"), "").unwrap();
    dir
}

fn spawn(dir: &PathBuf, stdin: Stdio, stdout: Stdio) -> Child {
    Command::new(FRONT)
        .args(["events", "--poll-ms", "1000", "--root"])
        .arg(dir)
        .current_dir(dir)
        .env_clear()
        .env("CSTAN_EVENTS_NO_PROCESS_SCAN", "1")
        .stdin(stdin)
        .stdout(stdout)
        .stderr(Stdio::null())
        .spawn()
        .expect("cstan runs")
}

/// Waits up to `limit` for the child; kills it and returns false when it is still running.
fn exits_within(child: &mut Child, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        if child.try_wait().unwrap().is_some() {
            return true;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// True when the child exited within `limit`; leaves a running child alone.
fn exits_within_keep(child: &mut Child, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if child.try_wait().unwrap().is_some() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn drain(child: &mut Child) -> std::thread::JoinHandle<()> {
    let mut out = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut out, &mut std::io::sink());
    })
}

#[test]
fn closing_stdin_ends_the_stream() {
    let dir = fixture("stdin");
    let mut child = spawn(&dir, Stdio::piped(), Stdio::piped());
    let _drain = drain(&mut child);
    std::thread::sleep(Duration::from_millis(300));
    drop(child.stdin.take());
    assert!(
        exits_within(&mut child, Duration::from_secs(2)),
        "no exit after stdin closed"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn closing_the_stdout_reader_ends_the_stream() {
    let dir = fixture("stdout");
    let mut child = spawn(&dir, Stdio::piped(), Stdio::piped());
    std::thread::sleep(Duration::from_millis(300));
    drop(child.stdout.take());
    assert!(
        exits_within(&mut child, Duration::from_secs(2)),
        "no exit after stdout closed"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn sighup_ends_the_stream() {
    let dir = fixture("hup");
    let mut child = spawn(&dir, Stdio::piped(), Stdio::piped());
    let _drain = drain(&mut child);
    std::thread::sleep(Duration::from_millis(300));
    // SAFETY: signals the child this test started.
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGHUP) };
    assert!(
        exits_within(&mut child, Duration::from_secs(2)),
        "no exit on SIGHUP"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn stdin_from_dev_null_is_not_watched() {
    let dir = fixture("null");
    let mut child = spawn(&dir, Stdio::null(), Stdio::piped());
    let _drain = drain(&mut child);
    if exits_within_keep(&mut child, Duration::from_secs(3)) {
        panic!("exited with stdin on /dev/null");
    }
    // SAFETY: signals the child this test started.
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    assert!(
        exits_within(&mut child, Duration::from_secs(2)),
        "no exit on SIGTERM"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(target_os = "linux")]
#[test]
fn the_watchers_do_not_spin() {
    let dir = fixture("cpu");
    let mut child = spawn(&dir, Stdio::piped(), Stdio::piped());
    let _drain = drain(&mut child);
    std::thread::sleep(Duration::from_secs(3));
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", child.id())).unwrap();
    // Fields after the parenthesised command name; utime and stime are the 14th and 15th overall.
    let rest: Vec<&str> = stat[stat.rfind(')').unwrap() + 2..].split(' ').collect();
    let ticks: u64 = rest[11].parse::<u64>().unwrap() + rest[12].parse::<u64>().unwrap();
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(dir);
    assert!(ticks < 30, "{ticks} CPU ticks in 3 s");
}
