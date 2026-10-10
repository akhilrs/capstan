//! The adapter drives a real Herdr session with a stand-in agent (replaces the stand-in half of test/herdr-live.test.ts and
//! the live checks of test/panes.test.ts): a clean agent shell, the trust dialog, guarded sends that defer and send, the
//! input line, the PM pane that is never typed into, and the worktree's removal. The Herdr server, the scratch HOME and the
//! `capstan-test-*` session are the harness's (tests/common/live_env.rs); the operator's own sessions are never touched.
//!
//! Run by hand: `CSTAN_LIVE=1 cargo test -p capstan-herdr --test live_agent -- --ignored --test-threads=1`
//! Without CSTAN_LIVE=1 (or herdr, python3, git) it says `SKIPPED LOUDLY` and passes.

#[path = "common/live_env.rs"]
mod live_env;

use capstan_herdr::adapter::{Adapter, AdapterOptions};
use capstan_herdr::api::*;
use capstan_herdr::claude_args::{build_agent_environment, claude_arguments, ClaudeRoleSettings};
use capstan_herdr::runner::{ProcessRunner, RunnerOptions};
use live_env::{until, Live, Options};
use std::cell::RefCell;
use std::sync::{Arc, Mutex};

const TOKEN: &str = "tok_LIVE_0123456789ABCDEFGHIJKLMNOPQRSTUV";

fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

#[test]
#[ignore = "drives a real Herdr server; run by hand: CSTAN_LIVE=1 cargo test -p capstan-herdr --test live_agent -- --ignored"]
fn the_adapter_drives_a_real_isolated_session_with_a_stand_in_agent() {
    let Some(live) = Live::start(&Options {
        daemon: false,
        ..Options::default()
    }) else {
        return;
    };
    let runner = Arc::new(
        ProcessRunner::new(RunnerOptions {
            session: live.session.clone(),
            binary: None,
            timeout_ms: Some(30_000),
            env: Some(live.environment.clone()),
        })
        .unwrap(),
    );
    let mut options = AdapterOptions::new(runner);
    options.temp_root = Some(live.root.clone());
    let adapter = Adapter::new(options);
    let quiet = |_: &KeyLogEntry| {};

    // Tested on 0.9.3; any 0.9.x from 0.9.1 keeps the CLI surface the adapter uses.
    let version = adapter.version().unwrap();
    let minor: u32 = version
        .strip_prefix("herdr 0.9.")
        .and_then(|rest| rest.parse().ok())
        .unwrap_or_else(|| panic!("unsupported Herdr version: {version}"));
    assert!(minor >= 1, "the adapter needs 0.9.x with x >= 1");

    // A fresh isolated session has notifications disabled: Herdr answers exit 0 with shown:false.
    let error = adapter
        .notify("Capstan: PM message waiting", "Message m-1 is waiting")
        .unwrap_err();
    assert_eq!(
        error.herdr_code(),
        Some("notification_not_shown"),
        "{error}"
    );

    let repo = live.repo.to_string_lossy().into_owned();
    let workspace = adapter
        .create_workspace(CreateWorkspaceInput {
            cwd: &repo,
            label: "live-root",
            role: PaneRole::Pm,
        })
        .unwrap();
    let worktree = adapter
        .create_worktree(CreateWorktreeInput {
            workspace_id: &workspace.workspace_id,
            branch: "cap/live/dev-g1",
            label: "live-dev",
            base: None,
        })
        .unwrap();
    assert!(std::path::Path::new(&worktree.path).exists());

    // A clean shell holds the token and the allowlist but nothing else.
    let mut base = live.environment.clone();
    base.insert("PLANTED_SECRET".into(), "planted".into());
    let extras = pairs(&[
        ("CAPSTAN_TOKEN", TOKEN),
        ("CAPSTAN_SOCKET", "/tmp/none.sock"),
        (
            "FAKE_CLAUDE_FIXTURES",
            &live.environment["FAKE_CLAUDE_FIXTURES"],
        ),
        ("FAKE_CLAUDE_ARGS", &live.environment["FAKE_CLAUDE_ARGS"]),
        ("FAKE_CLAUDE_LOG", &live.environment["FAKE_CLAUDE_LOG"]),
    ]);
    let environment = build_agent_environment(&base, &extras, &[]).unwrap();
    adapter
        .prepare_shell(PrepareShellInput {
            pane_id: &worktree.pane_id,
            environment: &environment,
            timeout_ms: None,
        })
        .unwrap();
    let screen = adapter.read_screen(&worktree.pane_id, false, None).unwrap();
    assert!(!screen.contains(TOKEN), "the token is not on the screen");
    assert_eq!(
        adapter.read_input(&worktree.pane_id).unwrap().as_deref(),
        Some("")
    );
    live.herdr(&[
        "pane",
        "run",
        &worktree.pane_id,
        "echo planted=[${PLANTED_SECRET-unset}] token=[${CAPSTAN_TOKEN:0:8}] herdr=[${HERDR_ENV-unset}]",
    ])
    .unwrap();
    let shown = until("the environment report", 15, || {
        adapter
            .read_screen(&worktree.pane_id, false, None)
            .ok()?
            .lines()
            .find(|l| l.starts_with("planted="))
            .map(str::to_string)
    });
    assert_eq!(shown, "planted=[unset] token=[tok_LIVE] herdr=[unset]");
    until("the prompt to return", 15, || {
        (adapter.read_input(&worktree.pane_id).ok()?.as_deref() == Some("")).then_some(())
    });

    let role_arguments = claude_arguments(
        &ClaudeRoleSettings {
            model: Some("fake-model".into()),
            permission_mode: "acceptEdits".into(),
            allow: vec!["Read".into()],
            deny: vec!["Bash(rm *)".into()],
            hooks: "off".into(),
            mcp: vec![],
        },
        None,
    )
    .unwrap();
    let started = adapter
        .start_agent(StartAgentInput {
            name: "live-dev",
            kind: "claude",
            pane_id: &worktree.pane_id,
            args: &role_arguments,
            timeout_ms: Some(20_000),
            environment: None,
        })
        .unwrap();
    assert_eq!(started, StartStatus::BlockedAtStartup);

    let deferred = adapter
        .guarded_send(GuardedSendInput {
            pane_id: &worktree.pane_id,
            text: "too early",
            before_send: &mut || panic!("nothing is recorded for a deferral"),
        })
        .unwrap();
    assert!(
        matches!(
            deferred,
            SendOutcome::Deferred {
                reason: DeferralReason::AgentBlocked,
                ..
            }
        ),
        "{deferred:?}"
    );

    let sent_keys = Mutex::new(Vec::<String>::new());
    let logger = |entry: &KeyLogEntry| sent_keys.lock().unwrap().push(entry.key.clone());
    let answered = adapter
        .answer_trust_dialog(TrustDialogInput {
            pane_id: &worktree.pane_id,
            log: &logger,
            timeout_ms: None,
        })
        .unwrap();
    assert_eq!(
        answered,
        DialogOutcome::Handled {
            keys: vec!["down".into(), "enter".into()]
        }
    );
    assert_eq!(*sent_keys.lock().unwrap(), ["down", "enter"]);

    until("the agent to become idle", 30, || {
        (adapter.agent_state("live-dev").ok()?.status == "idle").then_some(())
    });
    assert_eq!(
        adapter.read_input(&worktree.pane_id).unwrap().as_deref(),
        Some("")
    );

    let recorded = RefCell::new(0);
    let sent = adapter
        .guarded_send(GuardedSendInput {
            pane_id: &worktree.pane_id,
            text: "first message",
            before_send: &mut || {
                *recorded.borrow_mut() += 1;
                Ok(())
            },
        })
        .unwrap();
    assert_eq!(sent, SendOutcome::Sent);
    assert_eq!(*recorded.borrow(), 1);
    let message_log = live.root.join("messages.log");
    until("the stand-in agent to log the message", 15, || {
        std::fs::read_to_string(&message_log)
            .ok()?
            .contains("first message")
            .then_some(())
    });
    for (text, seen) in [
        ("--help", "--help\n"),
        ("line one\nline two", "line one\nline two\n"),
    ] {
        let outcome = adapter
            .guarded_send(GuardedSendInput {
                pane_id: &worktree.pane_id,
                text,
                before_send: &mut || Ok(()),
            })
            .unwrap();
        assert_eq!(outcome, SendOutcome::Sent);
        until(&format!("the stand-in agent to log {text:?}"), 15, || {
            std::fs::read_to_string(&message_log)
                .ok()?
                .contains(&format!("{seen}---"))
                .then_some(())
        });
        until("the input line to be empty again", 15, || {
            (adapter.read_input(&worktree.pane_id).ok()?.as_deref() == Some("")).then_some(())
        });
    }
    let arguments: Vec<String> = std::fs::read_to_string(live.root.join("args.log"))
        .unwrap()
        .trim_end()
        .split('\n')
        .map(str::to_string)
        .collect();
    assert_eq!(arguments, role_arguments);

    // Text left in the input line defers a send and is cleared on request.
    live.herdr(&["pane", "send-keys", &worktree.pane_id, "l"])
        .unwrap();
    live.herdr(&["pane", "send-keys", &worktree.pane_id, "e"])
        .unwrap();
    until("typed text to show", 15, || {
        (adapter.read_input(&worktree.pane_id).ok()?.as_deref() == Some("le")).then_some(())
    });
    let blocked = adapter
        .guarded_send(GuardedSendInput {
            pane_id: &worktree.pane_id,
            text: "second",
            before_send: &mut || panic!("nothing is recorded for a deferral"),
        })
        .unwrap();
    assert!(
        matches!(
            blocked,
            SendOutcome::Deferred {
                reason: DeferralReason::InputNotEmpty,
                ..
            }
        ),
        "{blocked:?}"
    );
    let discarded = RefCell::new(Vec::<String>::new());
    let cleared = adapter
        .clear_after_deferral(ClearInput {
            pane_id: &worktree.pane_id,
            deferred_for_ms: 1.0,
            max_deferral_ms: 1.0,
            discard: &mut |text| {
                discarded.borrow_mut().push(text.to_string());
                Ok(())
            },
            log: &quiet,
        })
        .unwrap();
    assert_eq!(
        cleared,
        ClearOutcome {
            cleared: true,
            text: "le".into()
        }
    );
    assert_eq!(*discarded.borrow(), ["le"]);
    assert_eq!(
        adapter.read_input(&worktree.pane_id).unwrap().as_deref(),
        Some("")
    );

    // The PM pane is never typed into once its agent has started.
    adapter
        .start_agent(StartAgentInput {
            name: "live-pm",
            kind: "claude",
            pane_id: &workspace.pane_id,
            args: &[],
            timeout_ms: Some(20_000),
            environment: Some(&environment),
        })
        .unwrap();
    let refused = adapter
        .guarded_send(GuardedSendInput {
            pane_id: &workspace.pane_id,
            text: "never",
            before_send: &mut || panic!("nothing is recorded"),
        })
        .unwrap_err();
    assert!(matches!(refused, AdapterError::PmPane(_)), "{refused:?}");
    let dialog = adapter
        .answer_trust_dialog(TrustDialogInput {
            pane_id: &workspace.pane_id,
            log: &quiet,
            timeout_ms: None,
        })
        .unwrap_err();
    assert!(matches!(dialog, AdapterError::PmPane(_)), "{dialog:?}");

    adapter
        .remove_worktree(&worktree.workspace_id, true)
        .unwrap();
    assert!(adapter.pane_entry(&worktree.pane_id).is_none());
    adapter.close();
    live.close();
}
