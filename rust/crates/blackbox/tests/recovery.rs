//! Recovery (test/recovery.test.ts, recovery-core.test.ts, commands.test.ts "kill -9"): what a daemon does at start with an
//! agent that has no pane, a daemon killed in the middle of mail, and the lost-agent record the PM reads. Observing a pane
//! that disappears needs Herdr; the loop that does it is replayed in daemon/tests/loops.rs (see docs/test-map/core.md).

use capstan_blackbox::*;
use std::io::Read;

#[test]
fn a_launching_daemon_ends_an_agent_that_has_no_pane_records_it_lost_and_tells_the_pm_once() {
    let mut w = World::builder("lostatstart")
        .config(TEAM_CONFIG)
        .launching()
        .without_pane_rows()
        .stopped()
        .agents(&[
            agent("pm-1", "pm", "PM"),
            agent("dev-1", "developer", "Developer"),
            agent("dev-2", "developer", "Developer"),
        ])
        .build();
    // Only the PM has a pane row; the developers were never started.
    let head = w.git(&["rev-parse", "HEAD"]);
    w.record_pane("pm-1", "main", &w.root.join("wt/pm-1"), &head);
    w.start();
    let log = w.command_log();
    for crashed in ["dev-1", "dev-2"] {
        assert_eq!(
            log.iter()
                .filter(
                    |e| e["command"] == "daemon:crashed_spawn" && e["detail"]["agentId"] == crashed
                )
                .count(),
            1,
            "{crashed} is found dead once: {log:?}"
        );
    }
    // Their tokens no longer work; the PM's does.
    assert_eq!(w.wire_as("dev-1", "inbox", &[])["code"], "unauthorized");
    assert_eq!(w.wire_as("pm-1", "inbox", &[])["ok"], true);
    // The crashed starts are ended in the ledger, in the operator's status as well.
    let agents = w.wire_op("status", &[])["result"]["agents"].to_string();
    assert!(
        !agents.contains("\"dev-1\"") || agents.contains("ended"),
        "{agents}"
    );
    let pm = w.as_agent("pm-1", &["inbox", "--json"]).ok().json();
    w.restart();
    let again = w.as_agent("pm-1", &["inbox", "--json"]).ok().json();
    assert_eq!(again["count"], pm["count"], "a restart repeats no notice");
    let crashed: Vec<_> = w
        .command_log()
        .into_iter()
        .filter(|e| e["command"] == "daemon:crashed_spawn")
        .collect();
    assert_eq!(
        crashed.len(),
        2,
        "the second start finds nothing left to end: {crashed:?}"
    );
    // An ended agent cannot be sent to.
    assert_eq!(
        w.wire_as("pm-1", "send", &["dev-1", "are you there"])["code"],
        "unknown_recipient"
    );
}

#[test]
fn after_kill_9_the_next_command_restarts_the_daemon_and_nothing_is_duplicated() {
    let mut w = World::standard("kill9mail");
    for n in 0..3 {
        w.as_agent("pm-1", &["send", "dev-1", &format!("message {n}")])
            .ok();
    }
    let pid = w.daemon_pid().unwrap();
    // SAFETY: the pid is this world's own daemon.
    unsafe { libc_kill(pid as i32, 9) };
    until("the daemon to die", 10, || {
        (!w.daemon_alive()).then_some(())
    });
    // The operator can still read the ledger with the daemon down; an agent's command says the controller is not running.
    assert!(w.op(&["status", "--json"]).ok().json()["projectId"].is_string());
    let down = w.as_agent("dev-1", &["inbox", "--json"]);
    assert_eq!(down.code, 4, "{}", down.text());
    assert!(
        down.stderr.contains("the controller is not running"),
        "{}",
        down.stderr
    );
    // `cstan start` finds the dead daemon's socket and pid file, replaces them and reconciles.
    let started = w.op(&["start", "--json"]).ok().json();
    assert_eq!(started["started"], true);
    assert_ne!(started["pid"].as_u64().unwrap(), pid as u64);
    let after_restart = w.op(&["status", "--json"]).ok().json();
    assert!(after_restart["projectId"].is_string());
    let inbox = w.as_agent("dev-1", &["inbox", "--json"]).ok().json();
    let bodies: Vec<&str> = inbox["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["body"].as_str().unwrap())
        .collect();
    assert_eq!(
        bodies,
        ["message 0", "message 1", "message 2"],
        "each message once, in order"
    );
    // Acks survive another crash.
    let first = inbox["messages"][0]["messageId"]
        .as_str()
        .unwrap()
        .to_string();
    w.as_agent("dev-1", &["ack", &first]).ok();
    let new_pid = started["pid"].as_u64().unwrap() as i32;
    // SAFETY: the pid is the daemon the start command above started for this scratch project.
    unsafe { libc_kill(new_pid, 9) };
    until("the second daemon to die", 10, || {
        (!std::path::Path::new(&format!("/proc/{new_pid}")).exists()).then_some(())
    });
    let restarted = w.op(&["start", "--json"]).ok().json();
    assert_eq!(restarted["started"], true);
    let after = w.as_agent("dev-1", &["inbox", "--json"]).ok().json();
    assert_eq!(
        after["count"], 2,
        "the acked one is gone and the others are still there once each"
    );
    let bodies: Vec<&str> = after["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["body"].as_str().unwrap())
        .collect();
    assert_eq!(bodies, ["message 1", "message 2"]);
    w.op(&["stop"]).ok();
}

extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, signal: i32) -> i32;
}

#[test]
fn a_wait_row_left_by_a_dead_daemon_is_closed_at_the_next_start() {
    let mut w = World::standard("deadwait");
    let mut waiting = std::process::Command::new(cstan_binary())
        .arg("wait")
        .current_dir(&w.project)
        .env_clear()
        .envs(w.environment())
        .env("CAPSTAN_TOKEN", w.token("dev-1"))
        .env("CAPSTAN_SOCKET", w.socket())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    until("the wait to register", 15, || {
        (w.open_waits("dev-1") == 1).then_some(())
    });
    let pid = w.daemon_pid().unwrap();
    // SAFETY: the pid is this world's own daemon.
    unsafe { libc_kill(pid as i32, 9) };
    until("the daemon to die", 10, || {
        (!w.daemon_alive()).then_some(())
    });
    assert_eq!(
        w.open_waits("dev-1"),
        1,
        "the dead daemon left the row open"
    );
    let _ = waiting.kill();
    let mut sink = String::new();
    let _ = waiting.stdout.take().unwrap().read_to_string(&mut sink);
    let _ = waiting.wait();
    w.start();
    assert_eq!(w.open_waits("dev-1"), 0, "the next start closes it");
    // A fresh wait works and is not superseded by the ghost.
    w.as_agent("pm-1", &["send", "dev-1", "after the crash"])
        .ok();
    assert!(w
        .as_agent("dev-1", &["wait"])
        .ok()
        .stdout
        .contains("after the crash"));
}
