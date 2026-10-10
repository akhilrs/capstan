//! The launcher through the daemon, against a real Herdr server in a throwaway `capstan-test-*` session (replaces the
//! black-box halves of test/launcher-live.test.ts, launcher-lifecycle, launcher-spawn and launcher-release): a PM and a
//! worker are started in real panes, mail reaches the worker, the panes are re-adopted after the daemon restarts and the
//! worker survives a PM restart; in pane mode a worker is split into the PM's tab and release closes that pane and removes
//! the worktree.
//!
//! The daemon is `cstan daemon`; the commands are the ones the Rust front end serves, and `launch`/`pm-restart` go straight
//! over the socket (see rust/crates/herdr/tests/common/live_env.rs). The stand-in for Claude Code draws the real screens.
//!
//! Run by hand, one at a time, after `cargo build -p cstan-front`:
//! `CSTAN_LIVE=1 cargo test -p capstan-launcher --test live -- --ignored --test-threads=1`
//! Without CSTAN_LIVE=1 (or herdr, python3, git, a built `cstan`) a test says `SKIPPED LOUDLY` and passes.

#[path = "../../herdr/tests/common/live_env.rs"]
mod live_env;

use live_env::{text, until, Live, Options};
use std::path::PathBuf;

fn worker_sees(live: &Live, pane: &str, needle: &str) {
    until(&format!("the worker to show {needle:?}"), 30, || {
        live.screen(pane).contains(needle).then_some(())
    });
}

#[test]
#[ignore = "drives a real Herdr server; run by hand: CSTAN_LIVE=1 cargo test -p capstan-launcher --test live -- --ignored"]
fn the_launcher_starts_a_pm_and_a_worker_delivers_readopts_after_a_restart_and_keeps_the_worker_through_a_pm_restart(
) {
    let Some(mut live) = Live::start(&Options::default()) else {
        return;
    };
    live.start_daemon();

    // Launch the PM: the stand-in stands at the real trust dialog, which only the operator answers.
    let launched = live.launch_pm();
    assert_eq!(launched["state"], "blocked", "{launched}");
    assert_eq!(launched["agentId"], "pm-1");
    assert_eq!(launched["hub"], "opened", "{launched}");
    let pm_pane = launched["paneId"].as_str().unwrap().to_string();
    live.answer_trust_dialog(&pm_pane);
    assert_eq!(
        live.agent_panes(),
        [("pm-1".to_string(), pm_pane.clone())],
        "the PM's pane carries its agent token"
    );
    // The hub's watch pane runs the real `cstan status --watch`: its first frame lists the PM and ends in the separator.
    until("the watch pane", 30, || {
        live.pane_ids()
            .iter()
            .find(|pane| {
                let screen = live.screen(pane);
                screen.contains("messages:") && screen.contains("pm-1") && screen.contains("---")
            })
            .map(|_| ())
    });
    // A second launch reports the running PM and opens nothing new.
    let again = live.launch_pm();
    assert_eq!(again["state"], "running", "{again}");
    assert_eq!(again["paneId"], pm_pane.as_str());
    assert_eq!(live.agent_panes().len(), 1);

    // Spawn a worker: a real worktree on its own branch at the recorded base sha, the dialog answered by the controller.
    let head = live.git_ok(&["rev-parse", "HEAD"]);
    let spawned = live.cstan_ok(&["spawn", "developer"]);
    assert_eq!(spawned["state"], "started", "{spawned}");
    assert_eq!(spawned["agentId"], "developer-1");
    assert_eq!(spawned["branch"], "chore/developer-1-developer");
    assert_eq!(
        live.git_ok(&["rev-parse", "chore/developer-1-developer"]),
        head
    );
    let worktrees = live.git_ok(&["worktree", "list", "--porcelain"]);
    assert!(
        worktrees.contains("branch refs/heads/chore/developer-1-developer"),
        "{worktrees}"
    );
    let worker_pane = spawned["paneId"].as_str().unwrap().to_string();
    live.wait_status(&worker_pane, "idle", 30);
    let refused = live.cstan(&["spawn", "developer"]);
    assert!(!refused.status.success());
    assert!(
        text(&refused).contains("worker_limit"),
        "{}",
        text(&refused)
    );

    // Deliver through the driver. The stand-in never answers, so the worker acknowledges each message itself, which is what
    // lets the next one be delivered.
    let worker_token = until("the worker's token", 30, || {
        live.token_in(std::path::Path::new(
            spawned["worktreePath"].as_str().unwrap(),
        ))
    });
    let one = live.cstan_ok(&["send", "developer-1", "build the first slice"]);
    worker_sees(&live, &worker_pane, "build the first slice");
    worker_sees(&live, &worker_pane, one["messageId"].as_str().unwrap());
    live.cstan_as_ok(&worker_token, &["ack", one["messageId"].as_str().unwrap()]);

    // A daemon restart: a second daemon re-adopts the recorded panes and delivery continues.
    live.stop_daemon();
    let panes_before = live.agent_panes();
    assert_eq!(panes_before.len(), 2, "{panes_before:?}");
    live.start_daemon();
    let relaunched = live.launch_pm();
    assert_eq!(relaunched["state"], "running", "{relaunched}");
    assert_eq!(relaunched["paneId"], pm_pane.as_str());
    assert_eq!(
        live.pane_of("developer-1").as_deref(),
        Some(worker_pane.as_str())
    );
    assert_eq!(
        live.agent_panes(),
        panes_before,
        "the agents' panes were re-adopted, none opened or closed"
    );
    let two = live.cstan_ok(&["send", "developer-1", "second slice after the restart"]);
    worker_sees(&live, &worker_pane, "second slice after the restart");
    live.cstan_as_ok(&worker_token, &["ack", two["messageId"].as_str().unwrap()]);

    // A PM restart replaces the PM pane and its token; the worker pane stays.
    let restarted = live.wire_ok("pm-restart", &[]);
    assert_eq!(restarted["state"], "blocked", "{restarted}");
    assert_eq!(restarted["generation"], 2, "{restarted}");
    let new_pm_pane = restarted["paneId"].as_str().unwrap().to_string();
    assert_ne!(new_pm_pane, pm_pane);
    live.answer_trust_dialog(&new_pm_pane);
    live.cstan_ok(&["send", "developer-1", "third slice after the PM restart"]);
    worker_sees(&live, &worker_pane, "third slice after the PM restart");
    let panes = live.pane_ids();
    assert!(
        !panes.contains(&pm_pane),
        "the old PM pane is gone: {panes:?}"
    );
    assert!(
        panes.contains(&worker_pane),
        "the worker pane stays: {panes:?}"
    );

    live.stop_daemon();
    live.close();
}

#[test]
#[ignore = "drives a real Herdr server; run by hand: CSTAN_LIVE=1 cargo test -p capstan-launcher --test live -- --ignored"]
fn pane_mode_splits_a_worker_into_the_pms_tab_and_release_closes_that_pane_and_removes_the_worktree(
) {
    let Some(mut live) = Live::start(&Options {
        layout: "pane",
        ..Options::default()
    }) else {
        return;
    };
    live.start_daemon();
    let launched = live.launch_pm();
    let pm_pane = launched["paneId"].as_str().unwrap().to_string();
    live.answer_trust_dialog(&pm_pane);
    let tab_of = |pane: &str| -> Option<String> {
        live.panes()
            .iter()
            .find(|p| p["pane_id"] == pane)
            .and_then(|p| p["tab_id"].as_str().map(str::to_string))
    };

    let spawned = live.cstan_ok(&["spawn", "developer"]);
    assert_eq!(spawned["state"], "started", "{spawned}");
    assert_eq!(spawned["placement"], "pane", "{spawned}");
    let worker_pane = spawned["paneId"].as_str().unwrap().to_string();
    let worktree = PathBuf::from(spawned["worktreePath"].as_str().unwrap());
    assert!(worktree.is_dir());
    let tab = tab_of(&pm_pane).expect("the PM's tab");
    assert_eq!(
        tab_of(&worker_pane).as_deref(),
        Some(tab.as_str()),
        "the PM and the worker share one tab"
    );

    let released = live.cstan_ok(&["release", "developer-1"]);
    assert_eq!(released["paneClosed"], true, "{released}");
    assert_eq!(released["worktreeRemoved"], true, "{released}");
    assert!(!worktree.exists());
    let panes = live.pane_ids();
    assert!(
        !panes.contains(&worker_pane),
        "the worker pane is gone: {panes:?}"
    );
    assert!(panes.contains(&pm_pane));
    assert!(!live.git_ok(&["worktree", "list"]).contains("developer-1"));

    live.stop_daemon();
    live.close();
}

fn refusal(output: &std::process::Output) -> String {
    assert!(!output.status.success(), "{}", text(output));
    text(output)
}

#[test]
#[ignore = "drives a real Herdr server; run by hand: CSTAN_LIVE=1 cargo test -p capstan-launcher --test live -- --ignored"]
fn the_prompt_commands_refuse_who_may_not_use_them_and_what_is_not_a_prompt_and_type_nothing() {
    let Some(mut live) = Live::start(&Options {
        prompt_relay: true,
        ..Options::default()
    }) else {
        return;
    };
    live.start_daemon();
    let launched = live.launch_pm();
    let pm_pane = launched["paneId"].as_str().unwrap().to_string();
    live.answer_trust_dialog(&pm_pane);
    let pm_token = until("the PM's token", 30, || live.token_in(&live.repo));
    let spawned = live.cstan_ok(&["spawn", "developer"]);
    let worker_pane = spawned["paneId"].as_str().unwrap().to_string();
    let worktree = PathBuf::from(spawned["worktreePath"].as_str().unwrap());
    live.wait_status(&worker_pane, "idle", 30);
    let developer_token = until("the worker's token", 30, || live.token_in(&worktree));
    let screen_before = live.screen(&worker_pane);

    // A worker at its idle input is not at a prompt: nothing is captured, nothing is typed.
    let idle = refusal(&live.cstan_as(&pm_token, &["prompt", "show", "developer-1"]));
    assert!(
        idle.contains("not_blocked: the worker is not at a permission prompt; look with cstan observe developer-1"),
        "{idle}"
    );
    // Only the active PM relays a prompt: a Developer and the operator are refused.
    let refused = refusal(&live.cstan_as(&developer_token, &["prompt", "show", "developer-1"]));
    assert!(
        refused.contains("only the active PM relays a worker's prompt"),
        "{refused}"
    );
    let refused = refusal(&live.cstan(&["prompt", "show", "developer-1"]));
    assert!(refused.contains("forbidden"), "{refused}");
    // Arguments that cannot be a request.
    let cases: [(&[&str], &str); 5] = [
        (
            &["prompt", "show", "pm-1"],
            "an agent cannot relay its own prompt",
        ),
        (&["prompt", "show", "bad id"], "the agent id is not valid"),
        (&["prompt", "show"], "prompt show needs one agent id"),
        (&["prompt", "bogus"], "prompt needs show or answer"),
        (
            &["prompt", "answer", "relay-1"],
            "prompt answer needs <relay-id> --hash <hash12>",
        ),
    ];
    for (args, message) in cases {
        let refused = refusal(&live.cstan_as(&pm_token, args));
        assert!(refused.contains(message), "{args:?}: {refused}");
    }
    // An answer for a relay that was never shown.
    let unknown = refusal(&live.cstan_as(
        &pm_token,
        &[
            "prompt",
            "answer",
            "relay-9",
            "--hash",
            "000000000000",
            "option",
            "1",
        ],
    ));
    assert!(unknown.contains("unknown_relay"), "{unknown}");
    // The worker's screen is as it was: nothing reached it.
    assert_eq!(live.screen(&worker_pane), screen_before);
    // With the relay on, status carries its section.
    let status = live.cstan_as_ok(&pm_token, &["status"]);
    assert!(status.get("promptRelay").is_some(), "{status}");

    live.stop_daemon();
    live.close();
}

#[test]
#[ignore = "drives a real Herdr server; run by hand: CSTAN_LIVE=1 cargo test -p capstan-launcher --test live -- --ignored"]
fn without_prompt_relay_both_prompt_commands_say_not_configured_and_status_has_no_section() {
    let Some(mut live) = Live::start(&Options::default()) else {
        return;
    };
    live.start_daemon();
    let launched = live.launch_pm();
    let pm_pane = launched["paneId"].as_str().unwrap().to_string();
    live.answer_trust_dialog(&pm_pane);
    let pm_token = until("the PM's token", 30, || live.token_in(&live.repo));
    for args in [
        &["prompt", "show", "developer-1"][..],
        &[
            "prompt",
            "answer",
            "relay-1",
            "--hash",
            "000000000000",
            "esc",
        ][..],
    ] {
        let refused = refusal(&live.cstan_as(&pm_token, args));
        assert!(
            refused.contains("not_configured")
                && refused
                    .contains("prompt commands need [prompt_relay] enabled = true in capstan.toml"),
            "{args:?}: {refused}"
        );
    }
    let status = live.cstan_as_ok(&pm_token, &["status"]);
    assert!(status.get("promptRelay").is_none(), "{status}");
    live.stop_daemon();
    live.close();
}
