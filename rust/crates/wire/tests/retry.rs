//! `call_retrying` against fake servers on a real Unix socket, with the sleep and the log injected.
use std::cell::RefCell;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use capstan_wire::{
    call_retrying, frame, frame_with_rid, AfterSend, RetryFailed, RetryPolicy, Timeout, WireError,
    MAX_FRAME_BYTES,
};

const BUSY: &str = r#"{"ok":false,"code":"controller_busy","message":"b","retryAfterMs":100}"#;
const OK: &str = r#"{"ok":true,"result":1}"#;

struct Fake {
    dir: PathBuf,
    socket: PathBuf,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn read_line(stream: &mut UnixStream) -> String {
    let mut out = Vec::new();
    let mut byte = [0u8; 1];
    while stream.read(&mut byte).unwrap_or(0) == 1 {
        if byte[0] == b'\n' {
            break;
        }
        out.push(byte[0]);
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Serves `script.len()` connections; the handler gets the connection's index and its stream after the request.
fn fake(
    name: &str,
    connections: usize,
    boot: Option<&str>,
    handler: impl Fn(usize, UnixStream) + Send + 'static,
) -> Fake {
    let dir = std::env::temp_dir().join(format!("cstan-retry-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let socket = dir.join("s.sock");
    if let Some(id) = boot {
        std::fs::write(dir.join("s.sock.boot"), id).unwrap();
    }
    let listener = UnixListener::bind(&socket).unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    thread::spawn(move || {
        for index in 0..connections {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            seen.lock().unwrap().push(read_line(&mut stream));
            handler(index, stream);
        }
    });
    Fake {
        dir,
        socket,
        requests,
    }
}

fn answer(mut stream: UnixStream, text: &str) {
    stream.write_all(text.as_bytes()).unwrap();
    stream.write_all(b"\n").unwrap();
}

struct Run {
    result: Result<capstan_wire::Retried, RetryFailed>,
    log: Vec<String>,
    sleeps: Vec<Duration>,
}

fn run(f: &Fake, command: &str, budget_ms: u64, deadline_ms: u64, lost: bool) -> Run {
    run_with(f, command, budget_ms, deadline_ms, lost, &[])
}

fn run_with(
    f: &Fake,
    command: &str,
    budget_ms: u64,
    deadline_ms: u64,
    lost: bool,
    args: &[String],
) -> Run {
    let log = RefCell::new(Vec::new());
    let sleeps = RefCell::new(Vec::new());
    let log_line = |l: &str| log.borrow_mut().push(l.to_string());
    let sleep = |d: Duration| sleeps.borrow_mut().push(d);
    let policy = RetryPolicy {
        busy_budget: Duration::from_millis(budget_ms),
        deadline: Duration::from_millis(deadline_ms),
        lost_reply: lost,
        log_line: &log_line,
        sleep: &sleep,
    };
    let frame_for = |rid: Option<&str>| match rid {
        Some(rid) => frame_with_rid("t", command, args, rid)
            .or_else(|_| frame("t", command, args))
            .unwrap(),
        None => frame("t", command, args).unwrap(),
    };
    let result = call_retrying(
        &f.socket,
        &frame_for,
        Timeout::Idle(Duration::from_secs(2)),
        &policy,
        command,
    );
    Run {
        result,
        log: log.into_inner(),
        sleeps: sleeps.into_inner(),
    }
}

fn rids(f: &Fake) -> Vec<String> {
    f.requests
        .lock()
        .unwrap()
        .iter()
        .map(|r| {
            let at = r.find("\"rid\":\"").map(|i| i + 7);
            at.map(|i| r[i..r.len() - 2].to_string())
                .unwrap_or_default()
        })
        .collect()
}

#[test]
fn busy_twice_then_ok_uses_one_rid() {
    let f = fake("busy2", 3, None, |i, s| {
        answer(s, if i < 2 { BUSY } else { OK })
    });
    let r = run(&f, "status", 30_000, 60_000, true);
    let done = r.result.unwrap();
    assert_eq!(done.line, OK.as_bytes());
    assert_eq!(done.attempts, 3);
    assert_eq!(r.log.len(), 2);
    assert!(r.log[0].starts_with("controller busy, retrying in "));
    assert!(r.log[1].ends_with("(attempt 3)"));
    let ids = rids(&f);
    assert_eq!(ids.len(), 3);
    assert!(!ids[0].is_empty() && ids.iter().all(|i| *i == ids[0]));
    assert!(ids[0].len() <= 48);
    for s in &r.sleeps {
        assert!(*s >= Duration::from_millis(100) && *s <= Duration::from_millis(120));
    }
}

#[test]
fn busy_beyond_the_budget_returns_the_busy_line() {
    let f = fake("budget", 10, None, |_, s| answer(s, BUSY));
    let r = run(&f, "status", 250, 60_000, true);
    let done = r.result.unwrap();
    assert_eq!(done.line, BUSY.as_bytes());
    let total: Duration = r.sleeps.iter().sum();
    assert_eq!(total, Duration::from_millis(250));
    assert_eq!(done.attempts as usize, r.sleeps.len() + 1);
}

#[test]
fn the_deadline_caps_the_sleeps() {
    let f = fake("deadline", 10, None, |_, s| answer(s, BUSY));
    let r = run(&f, "status", 30_000, 150, true);
    assert!(r.result.is_ok());
    let total: Duration = r.sleeps.iter().sum();
    assert!(total <= Duration::from_millis(150), "{total:?}");
}

#[test]
fn closed_with_an_unchanged_boot_id_is_retried_with_the_same_rid() {
    let f = fake("closed", 2, Some("b1"), |i, s| {
        if i == 0 {
            drop(s)
        } else {
            answer(s, OK)
        }
    });
    let r = run(&f, "status", 1000, 1000, true);
    assert_eq!(r.result.unwrap().attempts, 2);
    assert_eq!(r.sleeps, vec![Duration::from_millis(500)]);
    assert_eq!(
        r.log,
        vec!["controller did not answer, retrying (attempt 2)"]
    );
    let ids = rids(&f);
    assert_eq!(ids[0], ids[1]);
}

#[test]
fn closed_is_retried_at_most_twice() {
    let f = fake("twice", 5, Some("b1"), |_, s| drop(s));
    let r = run(&f, "status", 1000, 1000, true);
    let failed = r.result.unwrap_err();
    assert_eq!(failed.error, WireError::AfterSend(AfterSend::Closed));
    assert_eq!(failed.attempts, 3);
    assert!(!failed.restarted);
    assert_eq!(
        r.sleeps,
        vec![Duration::from_millis(500), Duration::from_millis(1000)]
    );
}

#[test]
fn a_boot_id_that_changes_after_the_first_attempt_stops_the_retry() {
    let boot_holder: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
    let holder = boot_holder.clone();
    let f = fake("restart", 3, Some("b1"), move |_, s| {
        if let Some(p) = holder.lock().unwrap().as_ref() {
            std::fs::write(p, "b2").unwrap();
        }
        drop(s)
    });
    *boot_holder.lock().unwrap() = Some(f.dir.join("s.sock.boot"));
    let r = run(&f, "status", 1000, 1000, true);
    let failed = r.result.unwrap_err();
    assert_eq!(failed.attempts, 1);
    assert!(failed.restarted);
    assert!(r.sleeps.is_empty());
}

#[test]
fn no_boot_file_means_no_retry_and_no_restart_note() {
    let f = fake("noboot", 3, None, |_, s| drop(s));
    let r = run(&f, "status", 1000, 1000, true);
    let failed = r.result.unwrap_err();
    assert_eq!((failed.attempts, failed.restarted), (1, false));
}

#[test]
fn answer_bytes_then_a_close_is_not_retried() {
    // (A reset after answer bytes maps to Io in the transport; a plain close after bytes stays "connection closed"
    // but is not asked again either, since part of an answer was read.)
    let f = fake("partial", 3, Some("b1"), |_, mut s| {
        s.write_all(b"{\"ok\":tr").unwrap();
    });
    let r = run(&f, "status", 1000, 1000, true);
    let failed = r.result.unwrap_err();
    assert_eq!(failed.attempts, 1);
    assert_eq!(failed.error, WireError::AfterSend(AfterSend::Closed));
    assert!(r.sleeps.is_empty());
}

#[test]
fn commands_that_never_retry_a_lost_reply() {
    for command in ["spawn", "wait", "notify", "decide"] {
        let f = fake(&format!("nolost-{command}"), 3, Some("b1"), |_, s| drop(s));
        let r = run(&f, command, 1000, 1000, true);
        assert_eq!(r.result.unwrap_err().attempts, 1, "{command}");
    }
    let f = fake("policy-off", 3, Some("b1"), |_, s| drop(s));
    assert_eq!(
        run(&f, "status", 1000, 1000, false)
            .result
            .unwrap_err()
            .attempts,
        1
    );
}

#[test]
fn the_code_busy_is_not_retried() {
    let line = r#"{"ok":false,"code":"busy","message":"x"}"#;
    let f = fake("codebusy", 3, None, move |_, s| answer(s, line));
    let r = run(&f, "notify", 30_000, 60_000, true);
    assert_eq!(r.result.unwrap().attempts, 1);
    assert!(r.sleeps.is_empty());
}

#[test]
fn a_frame_near_the_limit_is_sent_without_a_rid_and_not_asked_again() {
    let big = vec!["x".repeat(MAX_FRAME_BYTES - 60)];
    assert!(frame("t", "send", &big).is_ok());
    assert!(frame_with_rid("t", "send", &big, "aaaa-bbbb-cccc").is_err());
    let f = fake("big", 3, Some("b1"), |_, s| drop(s));
    let r = run_with(&f, "send", 1000, 1000, true, &big);
    assert_eq!(r.result.unwrap_err().attempts, 1);
    assert!(!f.requests.lock().unwrap()[0].contains("\"rid\""));
}

#[test]
fn a_write_failure_followed_by_a_busy_line_is_busy() {
    // The server answers busy at once and closes without reading; a big frame makes the client's write fail.
    let dir = std::env::temp_dir().join(format!("cstan-retry-epipe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let socket = dir.join("s.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let count = Arc::new(Mutex::new(0));
    let served = count.clone();
    thread::spawn(move || {
        for i in 0..2 {
            let Ok((mut s, _)) = listener.accept() else {
                return;
            };
            *served.lock().unwrap() += 1;
            if i == 0 {
                answer(s.try_clone().unwrap(), BUSY);
                drop(s);
            } else {
                read_line(&mut s);
                answer(s, OK);
            }
        }
    });
    let args = vec!["y".repeat(MAX_FRAME_BYTES - 200)];
    let f = Fake {
        dir,
        socket,
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let r = run_with(&f, "send", 30_000, 60_000, true, &args);
    let done = r.result.unwrap();
    assert_eq!(done.line, OK.as_bytes());
    assert_eq!(done.attempts, 2);
    assert_eq!(*count.lock().unwrap(), 2);
}

#[test]
fn a_connect_failure_is_not_retried() {
    let dir = std::env::temp_dir().join(format!("cstan-retry-gone-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = Fake {
        socket: dir.join("none.sock"),
        dir,
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let r = run(&f, "status", 1000, 1000, true);
    let failed = r.result.unwrap_err();
    assert!(matches!(failed.error, WireError::NotSent { .. }));
    assert_eq!(failed.attempts, 1);
}
