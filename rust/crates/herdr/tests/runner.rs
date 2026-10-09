//! The process runner against stand-in `herdr` scripts: arguments, environment, exit codes and the four ways a run
//! fails (src/herdr/runner.ts).

use capstan_herdr::api::{HerdrRunner, RunOptions};
use capstan_herdr::runner::{run_json, ProcessRunner, RunnerOptions};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

/// Written under a temporary name, closed, then renamed into place: a file that is still open for writing cannot be
/// executed (ETXTBSY), and a parallel test's fork can hold a copy of the descriptor. The exec tests also take a lock.
fn script(directory: &Path, body: &str) -> String {
    use std::io::Write;
    let path = directory.join("herdr");
    let temp = directory.join(format!("herdr.tmp{}", std::process::id()));
    {
        let mut file = std::fs::File::create(&temp).unwrap();
        file.write_all(format!("#!/bin/sh\n{body}\n").as_bytes())
            .unwrap();
        file.sync_all().unwrap();
    }
    std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::rename(&temp, &path).unwrap();
    path.to_string_lossy().into_owned()
}

static EXEC: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn exclusive() -> std::sync::MutexGuard<'static, ()> {
    EXEC.lock().unwrap_or_else(|p| p.into_inner())
}

fn runner(binary: String, timeout_ms: Option<u64>) -> ProcessRunner {
    let env: HashMap<String, String> = [
        ("HOME", "/home/test"),
        ("PATH", "/usr/bin:/bin"),
        ("HERDR_SESSION", "operator"),
        ("HERDR_PANE", "w1:p1"),
        ("KEEP_ME", "yes"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    ProcessRunner::new(RunnerOptions {
        session: "capstan-test-0".into(),
        binary: Some(binary),
        timeout_ms,
        env: Some(env),
    })
    .unwrap()
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn the_session_is_always_named_and_must_be_a_valid_name() {
    let _guard = exclusive();
    for bad in ["", "-x", "a b", "a/b", &"x".repeat(65)] {
        let result = ProcessRunner::new(RunnerOptions {
            session: bad.into(),
            binary: None,
            timeout_ms: None,
            env: Some(HashMap::new()),
        });
        assert_eq!(
            result.err().as_deref(),
            Some("a Herdr session name is required"),
            "{bad:?}"
        );
    }
}

#[test]
fn arguments_follow_the_session_flag_and_herdr_variables_are_removed() {
    let _guard = exclusive();
    let directory = tempfile::tempdir().unwrap();
    let binary = script(
        directory.path(),
        "for a in \"$@\"; do echo \"arg:$a\"; done; env | sort",
    );
    let output = runner(binary, None)
        .run(
            &args(&["pane", "get", "w1:p1", "with space", ""]),
            &RunOptions::default(),
        )
        .unwrap();
    assert_eq!(output.code, 0);
    let lines: Vec<&str> = output.stdout.lines().collect();
    assert_eq!(
        &lines[..6],
        [
            "arg:--session",
            "arg:capstan-test-0",
            "arg:pane",
            "arg:get",
            "arg:w1:p1",
            "arg:with space"
        ]
    );
    assert_eq!(lines[6], "arg:");
    assert!(output.stdout.contains("KEEP_ME=yes"));
    assert!(output.stdout.contains("HOME=/home/test"));
    assert!(
        !output.stdout.contains("HERDR_"),
        "no HERDR_ variable reaches the binary"
    );
}

#[test]
fn an_exit_code_and_both_streams_come_back() {
    let _guard = exclusive();
    let directory = tempfile::tempdir().unwrap();
    let binary = script(directory.path(), "echo out; echo err >&2; exit 3");
    let output = runner(binary, None)
        .run(&args(&["x"]), &RunOptions::default())
        .unwrap();
    assert_eq!(
        (output.code, output.stdout.as_str(), output.stderr.as_str()),
        (3, "out\n", "err\n")
    );
}

#[test]
fn a_run_that_takes_too_long_is_killed_and_reported() {
    let _guard = exclusive();
    let directory = tempfile::tempdir().unwrap();
    let binary = script(directory.path(), "exec sleep 20");
    let started = Instant::now();
    let error = runner(binary.clone(), Some(300))
        .run(&args(&["pane", "read"]), &RunOptions::default())
        .unwrap_err();
    assert_eq!(
        (error.code.as_str(), error.message.as_str()),
        ("timeout", "herdr pane timed out")
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    // A call's own limit overrides the runner's.
    let error = runner(binary, Some(60_000))
        .run(
            &args(&["agent", "start"]),
            &RunOptions {
                timeout_ms: Some(200),
                stdin: None,
            },
        )
        .unwrap_err();
    assert_eq!(error.message, "herdr agent timed out");
}

#[test]
fn a_child_that_closes_its_streams_and_keeps_running_is_killed_at_the_deadline() {
    let _guard = exclusive();
    let directory = tempfile::tempdir().unwrap();
    let binary = script(directory.path(), "exec >&- 2>&-\nexec sleep 20");
    let started = Instant::now();
    let error = runner(binary, Some(300))
        .run(&args(&["pane", "read"]), &RunOptions::default())
        .unwrap_err();
    assert_eq!(
        (error.code.as_str(), error.message.as_str()),
        ("timeout", "herdr pane timed out")
    );
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn output_over_the_limit_is_an_error() {
    let _guard = exclusive();
    let directory = tempfile::tempdir().unwrap();
    let binary = script(directory.path(), "head -c 5000000 /dev/zero | tr '\\0' a");
    let error = runner(binary, None)
        .run(&args(&["x"]), &RunOptions::default())
        .unwrap_err();
    assert_eq!(
        (error.code.as_str(), error.message.as_str()),
        ("output_too_large", "herdr output exceeded the limit")
    );
}

#[test]
fn bytes_that_are_not_utf8_are_an_error_and_a_byte_order_mark_is_kept() {
    let _guard = exclusive();
    let directory = tempfile::tempdir().unwrap();
    let bad = script(directory.path(), "printf '\\377'");
    let error = runner(bad, None)
        .run(&args(&["x"]), &RunOptions::default())
        .unwrap_err();
    assert_eq!(
        (error.code.as_str(), error.message.as_str()),
        ("bad_output", "herdr wrote text that is not UTF-8")
    );
    let bom = script(
        directory.path(),
        "printf '\\357\\273\\277{\"result\":{\"a\":1}}'",
    );
    let runner = runner(bom, None);
    let output = runner.run(&args(&["x"]), &RunOptions::default()).unwrap();
    assert!(output.stdout.starts_with('\u{feff}'));
    let result = run_json(&runner, &args(&["x"]), &RunOptions::default()).unwrap();
    assert_eq!(result["a"], 1);
}

#[test]
fn a_binary_that_cannot_start_is_an_error_and_standard_input_reaches_the_binary() {
    let _guard = exclusive();
    let error = runner("/no/such/herdr".into(), None)
        .run(&args(&["x"]), &RunOptions::default())
        .unwrap_err();
    assert_eq!(error.code, "spawn");
    let directory = tempfile::tempdir().unwrap();
    let binary = script(directory.path(), "cat");
    let output = runner(binary, None)
        .run(
            &args(&["x"]),
            &RunOptions {
                timeout_ms: None,
                stdin: Some(b"typed text".to_vec()),
            },
        )
        .unwrap();
    assert_eq!(output.stdout, "typed text");
}

#[test]
fn run_json_reads_the_result_and_the_failure_of_a_real_process() {
    let _guard = exclusive();
    let directory = tempfile::tempdir().unwrap();
    let ok = script(
        directory.path(),
        "echo '{\"id\":\"x\",\"result\":{\"pane\":{\"pane_id\":\"w1:p1\"}}}'",
    );
    let result = run_json(
        &runner(ok, None),
        &args(&["pane", "get"]),
        &RunOptions::default(),
    )
    .unwrap();
    assert_eq!(result["pane"]["pane_id"], "w1:p1");
    let failing = script(
        directory.path(),
        "echo '{\"error\":{\"code\":\"pane_not_found\",\"message\":\"no such pane\"}}' >&2; exit 1",
    );
    let error = run_json(
        &runner(failing, None),
        &args(&["pane", "get"]),
        &RunOptions::default(),
    )
    .unwrap_err();
    assert_eq!(
        (error.code.as_str(), error.message.as_str()),
        ("pane_not_found", "no such pane")
    );
}
