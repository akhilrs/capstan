//! Replays the wire transcripts Node exported (tests/transcripts/*.json, found at run time) through the daemon's handler layer
//! in process, and reports which steps passed, which are pending (an unported route, a connection-level step before the
//! server exists) and which failed. `CAPSTAN_DAEMON_PARITY_STRICT=1` fails on pending steps.
//!
//! The processes of the lock exclusion between this daemon and the Node daemon are here too: a `cstan-daemon` holding the
//! project makes a second one exit 4, and a Node daemon holding it makes `cstan-daemon` exit 4 (the Node side of that
//! pair, and the other direction, is in test/ledger-rust-interop.test.ts).

mod common;

use common::process::Daemon;
use common::project::{write_project, Identity};
use common::replay::{failing, replay_directory, replay_group, Report, Status};
use common::strict::strict;
use common::{private_tempdir, read_transcript, transcripts_dir};
use std::time::Duration;

fn summarize(reports: &[Report]) -> (usize, usize, usize) {
    let (mut passed, mut pending, mut failed) = (0, 0, 0);
    for report in reports {
        println!("{}", report.line());
        passed += report.count(|s| *s == Status::Passed);
        pending += report.count(|s| matches!(s, Status::Pending(_)));
        failed += report.count(|s| matches!(s, Status::Failed(_)));
    }
    println!("steps: {passed} passed, {pending} pending, {failed} failed");
    (passed, pending, failed)
}

#[test]
fn every_transcript_replays() {
    let reports = replay_directory(&transcripts_dir());
    assert!(
        !reports.is_empty(),
        "no scenarios found in {}",
        transcripts_dir().display()
    );
    let (passed, _, _) = summarize(&reports);
    let failures = failing(&reports, strict());
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(passed > 0, "no step passed");
}

#[test]
fn core_reports_its_dispatch_steps_and_marks_the_socket_steps_pending() {
    let core = replay_group(&read_transcript(&transcripts_dir().join("core.json")));
    assert!(!core.is_empty(), "core.json has no scenarios");
    let (passed, pending, failed) = summarize(&core);
    assert_eq!(failed, 0, "{}", failing(&core, false).join("\n"));
    // Auth refusals, unknown commands, malformed frames and the argument checks are the handler layer's own answers.
    assert!(passed >= 35, "only {passed} dispatch steps passed");
    // The oversized frame is a connection-level step: pending until the server exists (and still run against a real daemon
    // once it does).
    let socket_steps: Vec<&common::replay::StepReport> = core
        .iter()
        .flat_map(|report| report.steps.iter())
        .filter(|step| step.layer == "socket")
        .collect();
    assert!(
        !socket_steps.is_empty(),
        "core.json has no socket-layer step"
    );
    assert!(
        socket_steps
            .iter()
            .all(|step| matches!(step.status, Status::Pending(_) | Status::Passed)),
        "a socket step failed"
    );
    assert!(pending >= 1 || socket_steps.iter().all(|s| s.status == Status::Passed));
}

#[test]
fn strict_mode_turns_pending_steps_into_failures() {
    let core = replay_group(&read_transcript(&transcripts_dir().join("core.json")));
    let pending = core.iter().any(|r| matches!(r.status, Status::Pending(_)));
    assert!(
        failing(&core, false).is_empty(),
        "{}",
        failing(&core, false).join("\n")
    );
    // Strict fails exactly when something is pending.
    assert_eq!(!failing(&core, true).is_empty(), pending);
}

/// A project directory with a daemon holding its lock: the project files, and `cstan-daemon` waiting for the stop.
fn holding_daemon() -> (tempfile::TempDir, Daemon) {
    let dir = private_tempdir();
    let identity = Identity {
        project_id: "pholdlock".into(),
        name: "Lock Test".into(),
        credential: format!("operator-{}", "x".repeat(40)),
    };
    write_project(dir.path(), &identity);
    let mut daemon = Daemon::start(dir.path());
    let line = daemon.wait_for_line(Duration::from_secs(20), |line| {
        line.contains("serve_not_implemented") || line.contains("\"event\":\"ready\"")
    });
    assert!(
        line.is_some(),
        "the daemon did not come up: {}",
        daemon.stderr()
    );
    (dir, daemon)
}

#[test]
fn a_second_daemon_on_a_held_project_exits_4() {
    let (dir, mut first) = holding_daemon();
    let mut second = Daemon::start(dir.path());
    let code = second.wait_exit(Duration::from_secs(20));
    assert_eq!(code, Some(4), "stderr: {}", second.stderr());
    assert!(second.stderr().contains("another cooperating controller"));
    // The holder is untouched and stops cleanly, releasing the lock.
    assert_eq!(first.terminate(), Some(0));
    let mut third = Daemon::start(dir.path());
    assert!(third
        .wait_for_line(Duration::from_secs(20), |line| line
            .contains("serve_not_implemented")
            || line.contains("\"event\":\"ready\""))
        .is_some());
    assert_eq!(third.terminate(), Some(0));
}

#[test]
fn a_daemon_without_a_project_exits_3() {
    let dir = private_tempdir();
    let mut daemon = Daemon::start(dir.path());
    assert_eq!(daemon.wait_exit(Duration::from_secs(20)), Some(3));
    let stderr = daemon.stderr();
    assert!(
        stderr.contains("operator commands need the operator credential"),
        "{stderr}"
    );
}

#[test]
fn arguments_are_a_usage_error() {
    let dir = private_tempdir();
    let output = std::process::Command::new(common::process::binary())
        .arg("daemon")
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("usage: cstan-daemon"));
}
