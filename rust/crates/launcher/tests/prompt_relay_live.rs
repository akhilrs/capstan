//! A real Claude Code worker blocked on a permission prompt is captured and answered through the PM's `cstan prompt`
//! commands; an answer for a prompt that is no longer the shown one types nothing (replaces test/prompt-relay-live.test.ts;
//! the command-level cases of test/prompt-relay-commands.test.ts and prompt-relay-ledger.test.ts are the daemon's and the
//! kernel's replays, named in docs/test-map/live.md).
//!
//! It needs the real `claude` with a login in the operator's home; the agents get a private copy of the sign-in in a scratch
//! `CLAUDE_CONFIG_DIR` (nothing is linked; the stand-in cannot draw a permission prompt). Without it a test says
//! `SKIPPED LOUDLY` and passes. The Herdr server, the project and the session are the throwaway ones of the harness; the
//! operator's own Herdr sessions are never touched.
//!
//! Run by hand after `cargo build -p cstan-front`:
//! `CSTAN_LIVE=1 cargo test -p capstan-launcher --test prompt_relay_live -- --ignored --test-threads=1`

#[path = "../../herdr/tests/common/live_env.rs"]
mod live_env;

use live_env::{text, until, Live, Options};
use serde_json::Value;

#[test]
#[ignore = "drives a real Herdr server and a real Claude Code; run by hand: CSTAN_LIVE=1 cargo test -p capstan-launcher --test prompt_relay_live -- --ignored"]
fn a_real_claude_worker_blocked_on_a_permission_prompt_is_captured_and_answered_and_a_stale_relay_types_nothing(
) {
    let Some(mut live) = Live::start(&Options {
        prompt_relay: true,
        linked_login: true,
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
    assert_eq!(spawned["state"], "started", "{spawned}");
    let agent = spawned["agentId"].as_str().unwrap().to_string();
    let pane = spawned["paneId"].as_str().unwrap().to_string();
    let worktree = std::path::PathBuf::from(spawned["worktreePath"].as_str().unwrap());
    live.wait_status(&pane, "idle", 90);

    let ask = |file: &str| {
        live.herdr(&[
            "agent",
            "prompt",
            &live.herdr_agent_name(&agent),
            &format!("Run exactly this shell command and nothing else: touch {file}"),
        ])
        .unwrap();
        live.wait_status(&pane, "blocked", 90);
    };
    let show = || -> Value { live.cstan_as_ok(&pm_token, &["prompt", "show", &agent]) };
    let answer = |args: &[&str]| {
        let mut all = vec!["prompt", "answer"];
        all.extend_from_slice(args);
        live.cstan_as(&pm_token, &all)
    };

    // First prompt: capture, answer Yes with the captured hash, and the worker leaves the blocked state.
    ask("relay-one.txt");
    let first = show();
    assert!(
        first["prompt"]
            .as_str()
            .unwrap()
            .contains("touch relay-one.txt"),
        "{first}"
    );
    let options = first["options"].as_array().unwrap();
    let numbers: Vec<u64> = options
        .iter()
        .map(|o| o["number"].as_u64().unwrap())
        .collect();
    assert_eq!(numbers, (1..=options.len() as u64).collect::<Vec<_>>());
    let yes = options
        .iter()
        .find(|o| o["text"] == "Yes")
        .unwrap_or_else(|| panic!("{options:?}"))["number"]
        .as_u64()
        .unwrap()
        .to_string();
    let (relay_one, hash_one) = (
        first["relayId"].as_str().unwrap().to_string(),
        first["hash"].as_str().unwrap().to_string(),
    );
    let answered = answer(&[&relay_one, "--hash", &hash_one, "option", &yes]);
    assert!(answered.status.success(), "{}", text(&answered));
    until("the worker to leave the blocked state", 90, || {
        (live.agent_status(&pane).as_deref() != Some("blocked")).then_some(())
    });
    until("the permitted command to have run", 90, || {
        worktree.join("relay-one.txt").exists().then_some(())
    });

    // The first relay opens nothing again: once it is answered, and against a different prompt.
    let repeat = answer(&[&relay_one, "--hash", &hash_one, "option", &yes]);
    assert!(!repeat.status.success(), "{}", text(&repeat));
    until("the worker to settle", 90, || {
        matches!(live.agent_status(&pane).as_deref(), Some("idle" | "done")).then_some(())
    });
    ask("relay-two.txt");
    let second = show();
    let (relay_two, hash_two) = (
        second["relayId"].as_str().unwrap().to_string(),
        second["hash"].as_str().unwrap().to_string(),
    );
    assert_ne!(relay_two, relay_one);
    let stale = answer(&[&relay_one, "--hash", &hash_one, "option", &yes]);
    assert!(!stale.status.success(), "{}", text(&stale));
    assert!(
        live.screen(&pane).contains("touch relay-two.txt"),
        "the second prompt is still open: nothing was typed"
    );

    // The second prompt answered with text through the open field: the worker is told not to run it.
    let options = second["options"].as_array().unwrap();
    assert!(
        options
            .iter()
            .any(|o| o["text"] == "No" && o["acceptsText"] == true),
        "{options:?}"
    );
    let texted = answer(&[
        &relay_two,
        "--hash",
        &hash_two,
        "text",
        "do not touch files",
    ]);
    assert!(texted.status.success(), "{}", text(&texted));
    until("the worker to leave the blocked state again", 90, || {
        (live.agent_status(&pane).as_deref() != Some("blocked")).then_some(())
    });
    assert!(
        !worktree.join("relay-two.txt").exists(),
        "the refused command did not run"
    );

    live.stop_daemon();
    live.close();
}
