//! The Operator service over a real kernel in a scratch project: an approved proposal runs once, with a scrubbed
//! environment; auto rules, session grants and full auto approve what Node approves.

mod common;

use capstan_config::AutoWindowKind;
use capstan_operator::api::{
    DecideInput, Decision, FullAutoOnInput, FullAutoStatus, GrantKind, OperatorService,
    ProposalKind, ProposeInput, RestartCoordinator, SessionGrantInput, StubRestart,
};
use capstan_operator::service::{
    run_command, OperatorLedger, OperatorServiceOptions, ProcessOperatorService, RunOptions,
    RunResult, Runner,
};
use common::*;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

type Environment = Vec<(String, String)>;

struct Setup {
    world: World,
    service: Arc<ProcessOperatorService>,
    runs: Arc<AtomicUsize>,
    seen: Arc<std::sync::Mutex<Vec<Environment>>>,
}

fn setup(overrides: impl FnOnce(&mut capstan_config::Operator)) -> Setup {
    setup_with(overrides, None)
}

fn setup_with(
    overrides: impl FnOnce(&mut capstan_config::Operator),
    restart: Option<Arc<dyn capstan_operator::api::RestartCoordinator>>,
) -> Setup {
    let world = World::new();
    let runs = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let runner: Runner = {
        let (runs, seen) = (runs.clone(), seen.clone());
        Arc::new(move |options: RunOptions| -> RunResult {
            runs.fetch_add(1, Ordering::SeqCst);
            seen.lock().unwrap().push(options.environment.clone());
            run_command(options)
        })
    };
    let started = std::time::Instant::now();
    let service = Arc::new(ProcessOperatorService::new(OperatorServiceOptions {
        ledger: world.ledger.clone(),
        config: operator_config(overrides),
        controller_credential: OWNER.into(),
        project_root: world.root.clone(),
        // A caller that forgot to remove the agent variables: the service must still keep them from the command.
        environment: Arc::new(|| {
            vec![
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("CAPSTAN_TOKEN".into(), "agent-token-0123456789".into()),
                ("CAPSTAN_SOCKET".into(), "/tmp/agent.sock".into()),
                ("HOME".into(), "/nonexistent".into()),
            ]
        }),
        run_command: Some(runner),
        restart,
        notify_pm: None,
        processes: None,
        monotonic_now: Some(Arc::new(move || {
            1_000_000 + started.elapsed().as_millis() as i64
        })),
        orphan_kill_wait_ms: None,
        log: None,
    }));
    Setup {
        world,
        service,
        runs,
        seen,
    }
}

impl Setup {
    fn propose(&self, command: &str) -> Value {
        self.service
            .propose(
                self.world.developer(),
                ProposeInput {
                    kind: ProposalKind::Command,
                    command: command.into(),
                    reason: "check it".into(),
                    force_restart: false,
                },
            )
            .expect("proposed")
    }

    fn approve(&self, proposal: &Value, session: Option<SessionGrantInput>) -> Value {
        self.service
            .decide(
                self.world.pm(),
                DecideInput {
                    proposal_id: proposal["proposalId"].as_str().unwrap().into(),
                    decision: Decision::Approve,
                    hash: Some(proposal["commandSha"].as_str().unwrap()[..12].into()),
                    note: None,
                    session,
                },
            )
            .expect("approved")
    }

    /// Waits until the run ended, whichever way (a git command in a bare scratch project may exit non-zero).
    fn wait_ended(&self, proposal: &Value) {
        let id = proposal["proposalId"].as_str().unwrap().to_string();
        for _ in 0..750 {
            let state = self.service.show(&id).map(|p| p["state"].clone());
            if matches!(
                state.as_ref().and_then(Value::as_str),
                Some("finished" | "failed")
            ) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("the run of {id} never ended");
    }

    fn wait(&self, proposal: &Value, state: &str) -> Value {
        let id = proposal["proposalId"].as_str().unwrap().to_string();
        let service = self.service.clone();
        self.world.wait_state(&move || service.show(&id), state)
    }
}

#[test]
fn an_approved_proposal_runs_once_in_the_project_root_with_a_scrubbed_environment() {
    let s = setup(|_| {});
    let proposal = s.propose("env; pwd");
    assert_eq!(proposal["state"], "proposed");
    assert_eq!(s.runs.load(Ordering::SeqCst), 0);
    s.approve(&proposal, None);
    let done = s.wait(&proposal, "finished");
    let output = done["run"]["outputTail"].as_str().unwrap().to_string();
    assert!(!output.contains("CAPSTAN_"), "{output}");
    assert!(!output.contains("agent-token"), "{output}");
    assert!(
        !output.contains("cstan"),
        "no cstan wrapper on PATH: {output}"
    );
    assert!(output.contains("PATH=/usr/bin:/bin"), "{output}");
    assert!(
        output.trim_end().ends_with(&path_str(&s.world.root)),
        "{output}"
    );
    // Nothing runs a second time, however often the worker looks.
    s.service.drain();
    s.service.tick();
    assert_eq!(s.runs.load(Ordering::SeqCst), 1);
    let environment = &s.seen.lock().unwrap()[0];
    assert!(environment
        .iter()
        .all(|(name, _)| !name.starts_with("CAPSTAN_")));
    s.service.stop();
}

#[test]
fn a_denied_proposal_never_runs() {
    let s = setup(|_| {});
    let proposal = s.propose("echo no");
    s.service
        .decide(
            s.world.pm(),
            DecideInput {
                proposal_id: proposal["proposalId"].as_str().unwrap().into(),
                decision: Decision::Deny,
                hash: None,
                note: Some("not now".into()),
                session: None,
            },
        )
        .unwrap();
    s.service.drain();
    assert_eq!(s.wait(&proposal, "denied")["state"], "denied");
    assert_eq!(s.runs.load(Ordering::SeqCst), 0);
    s.service.stop();
}

#[test]
fn a_configured_prefix_rule_approves_and_runs_a_read_only_command() {
    let s = setup(|c| c.auto_approve_prefix = vec!["ls".into()]);
    let proposal = s.propose("ls state");
    assert_eq!(proposal["state"], "approved");
    assert_eq!(proposal["autoRule"], "ls");
    s.wait(&proposal, "finished");
    // A command with a denied word needs a human even under the same rule.
    let risky = s.propose("ls -fd x");
    assert_eq!(risky["state"], "proposed");
    assert_eq!(risky["autoRule"], Value::Null);
    s.service.stop();
}

#[test]
fn a_session_grant_lets_the_same_text_run_again_without_a_decision() {
    let s = setup(|_| {});
    let first = s.propose("echo granted");
    let approved = s.approve(
        &first,
        Some(SessionGrantInput::Grant {
            kind: GrantKind::Exact,
            text: None,
        }),
    );
    let grant_id = approved["sessionGrant"]["grantId"]
        .as_str()
        .expect("a grant")
        .to_string();
    s.wait(&first, "finished");
    assert_eq!(s.service.grants().len(), 1);
    let second = s.propose("echo granted");
    assert_eq!(second["state"], "approved");
    assert_eq!(second["autoRule"], format!("session:{grant_id}"));
    // Different text, and a prefix that is not granted, still need a human.
    assert_eq!(s.propose("echo other")["state"], "proposed");
    s.wait(&second, "finished");
    s.service.revoke_grant(s.world.pm(), &grant_id).unwrap();
    assert!(s.service.grants().is_empty());
    assert_eq!(s.propose("echo granted")["state"], "proposed");
    s.service.stop();
}

#[test]
fn full_auto_approves_everything_until_it_is_switched_off() {
    let s = setup(|_| {});
    assert_eq!(s.service.full_auto_status(), FullAutoStatus::Off);
    let status = s
        .service
        .full_auto_on(
            s.world.pm(),
            FullAutoOnInput {
                minutes: Some(5),
                asked_user: "the user said yes".into(),
            },
        )
        .unwrap();
    assert!(matches!(status, FullAutoStatus::On { minutes: 5, .. }));
    let proposal = s.propose("echo whatever");
    assert_eq!(proposal["state"], "approved");
    assert_eq!(proposal["autoRule"], "full-auto");
    s.wait(&proposal, "finished");
    assert_eq!(
        s.service.full_auto_off(s.world.pm()).unwrap(),
        FullAutoStatus::Off
    );
    assert_eq!(s.propose("echo again")["state"], "proposed");
    let refused = s
        .service
        .full_auto_on(
            s.world.pm(),
            FullAutoOnInput {
                minutes: Some(100_000),
                asked_user: "yes".into(),
            },
        )
        .unwrap_err();
    assert_eq!(refused.code(), Some("full_auto_minutes"));
    s.service.stop();
}

#[test]
fn kernel_refusals_keep_their_class() {
    use capstan_kernel::KernelError;
    use capstan_operator::api::OperatorFailure;
    let s = setup(|_| {});
    // An unknown credential is an authentication error, not an operator code.
    let unauthenticated = s
        .service
        .propose(
            "no-such-credential",
            ProposeInput {
                kind: ProposalKind::Command,
                command: "ls".into(),
                reason: "r".into(),
                force_restart: false,
            },
        )
        .unwrap_err();
    assert!(
        matches!(
            unauthenticated,
            OperatorFailure::Kernel(KernelError::Authentication(_))
        ),
        "{unauthenticated:?}"
    );
    // The PM is not the Operator agent: the kernel refuses with an authorization error.
    let forbidden = s
        .service
        .propose(
            s.world.pm(),
            ProposeInput {
                kind: ProposalKind::Command,
                command: "ls".into(),
                reason: "r".into(),
                force_restart: false,
            },
        )
        .unwrap_err();
    assert!(
        matches!(
            forbidden,
            OperatorFailure::Kernel(KernelError::Authorization(_))
        ),
        "{forbidden:?}"
    );
    // An unknown proposal id is a ControllerError.
    let unknown = s
        .service
        .cancel(s.world.developer(), "no-such-proposal")
        .unwrap_err();
    assert!(
        matches!(
            unknown,
            OperatorFailure::Kernel(KernelError::Controller(_))
                | OperatorFailure::Kernel(KernelError::Type(_))
        ),
        "{unknown:?}"
    );
    s.service.stop();
}

#[test]
fn a_command_that_would_stop_the_controller_is_refused_at_propose_time() {
    let s = setup(|_| {});
    for command in [
        "cstan stop",
        "cd /work && cstan stop && cstan start && cstan status",
        "/usr/local/bin/cstan stop",
        "cstan --json stop; echo done",
        "(sleep 1 | cstan restart)",
        "echo $(cstan stop)",
        "FOO=1 nohup cstan stop",
        "cstan stop>/dev/null && cstan start",
        "cstan stop 2>&1 && cstan start",
        ">/dev/null cstan stop",
        "echo 'a;b' && cstan stop",
        "cstan stop\\\n&& cstan start",
        "echo >$(cstan stop)",
        "cat <<'EOF' > /dev/null\nhi\nEOF\ncstan stop",
    ] {
        let error = s
            .service
            .propose(
                s.world.developer(),
                ProposeInput {
                    kind: ProposalKind::Command,
                    command: command.into(),
                    reason: "restart".into(),
                    force_restart: false,
                },
            )
            .expect_err(command);
        let text = format!("{error:?}");
        assert!(
            text.contains("controller_stop_refused"),
            "{command}: {text}"
        );
        assert!(text.contains("--restart"), "{command}: {text}");
    }
}

#[test]
fn unrelated_commands_with_stop_in_them_are_not_refused() {
    for command in [
        "systemctl --user stop foo",
        "git stash",
        "cstan status",
        "echo cstan stop",
        "cstan op show stop",
        "rg 'cstan stop|cstan restart' docs",
        "grep \"cstan stop; cstan start\" notes.md",
        "echo '$(cstan stop)'",
        "echo cstan\\ stop",
        "cstan status > stop.txt",
        "cat <<'EOF'\ncstan stop\nEOF",
        "cat <<-\"EOF\"\n\tcstan restart\n\tEOF",
    ] {
        setup(|_| {}).propose(command);
    }
}

#[test]
fn a_restart_proposal_is_refused_without_a_coordinator() {
    let s = setup(|_| {});
    let refused = s
        .service
        .propose(
            s.world.developer(),
            ProposeInput {
                kind: ProposalKind::Restart,
                command: String::new(),
                reason: "restart".into(),
                force_restart: false,
            },
        )
        .unwrap_err();
    assert_eq!(refused.code(), Some("restart_not_available"));
    s.service.stop();
}

// ------------------------------------------------------------------------------------------------ approval windows

fn exact() -> Option<SessionGrantInput> {
    Some(SessionGrantInput::Grant {
        kind: GrantKind::Exact,
        text: None,
    })
}

fn window(minutes: i64) -> Option<SessionGrantInput> {
    Some(SessionGrantInput::Window {
        kind: AutoWindowKind::Minutes,
        minutes,
    })
}

fn messages(s: &Setup, agent: &str) -> Vec<String> {
    let agent = agent.to_string();
    s.world
        .ledger
        .run(Box::new(move |core| core.messages_for(&agent)))
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["body"].as_str().map(str::to_string))
        .collect()
}

/// Opens a window of `minutes` through a decision; returns its id.
fn open_window(s: &Setup, minutes: i64) -> String {
    let first = s.propose("echo start");
    let approved = s.approve(&first, window(minutes));
    s.wait(&first, "finished");
    approved["window"]["windowId"]
        .as_str()
        .expect("a window")
        .to_string()
}

fn floor_note(s: &Setup) -> bool {
    messages(s, "pm-1")
        .iter()
        .any(|m| m.contains("needs the user even inside approval window"))
}

#[test]
fn the_floor_holds_back_what_a_grant_or_the_window_would_approve() {
    let s = setup(|_| {});
    let floored = [
        "git push --force-with-lease",
        "rm -rf /srv/x",
        "bash -c \"git push -f\"",
        "cd /outside && rm -rf x",
    ];
    // An exact grant for each, made before any window exists.
    for command in floored {
        let proposal = s.propose(command);
        s.approve(&proposal, exact());
        s.wait_ended(&proposal);
    }
    assert_eq!(s.service.grants().len(), floored.len());
    let window_id = open_window(&s, 30);
    for command in floored {
        let proposal = s.propose(command);
        assert_eq!(proposal["state"], "proposed", "{command}");
        assert_eq!(proposal["autoRule"], Value::Null, "{command}");
    }
    assert!(floor_note(&s));
    // Without a grant the window approves what the floor lets through, with an info notice.
    for command in ["git push origin main", "rm -rf target", "npm publish"] {
        let proposal = s.propose(command);
        assert_eq!(proposal["state"], "approved", "{command}");
        assert_eq!(
            proposal["autoRule"],
            format!("window:{window_id}"),
            "{command}"
        );
        s.wait_ended(&proposal);
    }
    let notices = messages(&s, "pm-1");
    assert!(
        notices
            .iter()
            .any(|m| m.contains("was approved by approval window") && m.contains(&window_id)),
        "{notices:?}"
    );
    // A script the floor cannot read is approved with the reason.
    let unchecked = s.propose("bash deploy.sh");
    assert_eq!(unchecked["autoRule"], format!("window:{window_id}"));
    s.wait_ended(&unchecked);
    assert!(messages(&s, "pm-1")
        .iter()
        .any(|m| m.contains("Approved without the floor check")));
    s.service.stop();
}

#[test]
fn without_a_window_nothing_changes_and_a_grant_or_rule_keeps_its_own_rule_inside_one() {
    let s = setup(|c| c.auto_approve_prefix = vec!["ls".into()]);
    assert_eq!(s.propose("bash deploy.sh")["state"], "proposed");
    let first = s.propose("echo granted");
    let granted = s.approve(&first, exact());
    s.wait(&first, "finished");
    let grant_id = granted["sessionGrant"]["grantId"]
        .as_str()
        .unwrap()
        .to_string();
    open_window(&s, 30);
    let again = s.propose("echo granted");
    assert_eq!(again["autoRule"], format!("session:{grant_id}"));
    assert_eq!(s.propose("ls state")["autoRule"], "ls");
    s.service.stop();
}

#[test]
fn a_restart_is_floored_by_default_and_approved_by_the_window_without_the_restart_entry() {
    let restart = Arc::new(StubRestart::new());
    let s = setup_with(|_| {}, Some(restart.clone() as Arc<dyn RestartCoordinator>));
    open_window(&s, 30);
    let propose_restart = |s: &Setup| {
        s.service
            .propose(
                s.world.developer(),
                ProposeInput {
                    kind: ProposalKind::Restart,
                    command: String::new(),
                    reason: "restart".into(),
                    force_restart: false,
                },
            )
            .unwrap()
    };
    assert_eq!(propose_restart(&s)["state"], "proposed");
    s.service.stop();

    let restart = Arc::new(StubRestart::new());
    let s = setup_with(
        |c| {
            c.approval_floor
                .retain(|entry| entry != "@controller-restart")
        },
        Some(restart.clone() as Arc<dyn RestartCoordinator>),
    );
    let window_id = open_window(&s, 30);
    let proposal = s
        .service
        .propose(
            s.world.developer(),
            ProposeInput {
                kind: ProposalKind::Restart,
                command: String::new(),
                reason: "restart".into(),
                force_restart: false,
            },
        )
        .unwrap();
    assert_eq!(proposal["state"], "approved");
    assert_eq!(proposal["autoRule"], format!("window:{window_id}"));
    s.service.stop();
}

#[test]
fn a_released_operator_part_stops_the_approvals_while_the_window_stays_listed() {
    let s = setup(|_| {});
    let window_id = open_window(&s, 30);
    assert_eq!(s.propose("echo one")["state"], "approved");
    s.world
        .ledger
        .run(Box::new(|core| {
            let context = capstan_kernel::types::MutationContext {
                credential: OWNER.into(),
                request_id: "req-release".into(),
                idempotency_key: "idem-release".into(),
                expected_version: core.state_version()?,
                input_revision: core.input_revision()?,
            };
            core.end_agent(&context, "dev-1", None)
        }))
        .unwrap();
    let shown = s.service.window().expect("still open");
    assert_eq!(shown["windowId"], window_id.as_str());
    assert_eq!(shown["operatorActive"], false);
    s.service.stop();
}

#[test]
fn a_revoked_window_approves_nothing_and_a_second_revoke_gives_the_kernel_error() {
    let s = setup(|_| {});
    let window_id = open_window(&s, 30);
    assert!(s.service.window().is_some());
    let ended = s.service.revoke_window(s.world.pm(), &window_id).unwrap();
    assert_eq!(ended["endedReason"], "revoked");
    assert!(s.service.window().is_none());
    assert_eq!(s.propose("echo two")["state"], "proposed");
    let again = s
        .service
        .revoke_window(s.world.pm(), &window_id)
        .unwrap_err();
    assert!(
        again.to_string().contains("window_already_ended"),
        "{again}"
    );
    let unknown = s.service.revoke_window(s.world.pm(), "win-99").unwrap_err();
    assert!(unknown.to_string().contains("does not exist"), "{unknown}");
    let history = s.service.list_windows(None);
    assert_eq!(history[0]["endedReason"], "revoked");
    s.service.stop();
}

#[test]
fn a_restart_of_the_controller_ends_the_window() {
    let s = setup(|_| {});
    open_window(&s, 30);
    s.service.recover(None);
    assert!(s.service.window().is_none());
    assert_eq!(s.service.list_windows(None)[0]["endedReason"], "restart");
    s.service.stop();
}

#[test]
fn nothing_names_the_timed_classifier_any_more() {
    let sources = [
        include_str!("../src/policy.rs"),
        include_str!("../src/service.rs"),
        include_str!("../src/api.rs"),
    ];
    for needle in [
        concat!("classify_", "safe_command"),
        concat!("Safe", "CommandSet"),
        concat!("Safe", "Verdict"),
        concat!("GrantKind::", "Timed"),
    ] {
        for source in sources {
            assert!(!source.contains(needle), "{needle} is still there");
        }
    }
}

fn ledger_rows(s: &Setup) -> (i64, i64) {
    let db = rusqlite::Connection::open(s.world.state.join("controller.sqlite")).unwrap();
    let count = |table: &str| {
        db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
    };
    (count("controller_events"), count("mutation_requests"))
}

fn expiry_events(s: &Setup) -> i64 {
    let db = rusqlite::Connection::open(s.world.state.join("controller.sqlite")).unwrap();
    db.query_row(
        "SELECT COUNT(*) FROM controller_events WHERE entity_type = 'approval_window'",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

#[test]
fn an_idle_tick_writes_no_event_and_no_request_row() {
    let s = setup(|_| {});
    s.service.tick();
    let before = ledger_rows(&s);
    for _ in 0..3 {
        s.service.tick();
    }
    assert_eq!(ledger_rows(&s), before);
}

#[test]
fn a_tick_with_an_open_window_inside_its_cap_writes_nothing() {
    let s = setup(|_| {});
    open_window(&s, 30);
    s.service.tick();
    let before = ledger_rows(&s);
    for _ in 0..3 {
        s.service.tick();
    }
    assert_eq!(ledger_rows(&s), before);
}

#[test]
fn a_tick_still_expires_a_window_that_is_past_its_cap() {
    let s = setup(|_| {});
    let id = open_window(&s, 30);
    s.service.tick();
    let before = expiry_events(&s);
    let db = rusqlite::Connection::open(s.world.state.join("controller.sqlite")).unwrap();
    // The scratch ledger's guard against moving a window's expiry is dropped: this stands in for the clock moving on.
    let triggers: Vec<String> = db
        .prepare("SELECT name FROM sqlite_master WHERE type = 'trigger' AND tbl_name = 'approval_windows'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for name in triggers {
        db.execute(&format!("DROP TRIGGER \"{name}\""), []).unwrap();
    }
    db.execute(
        "UPDATE approval_windows SET expires_at = '2000-01-01T00:00:00.000Z' WHERE window_id = ?",
        [&id],
    )
    .unwrap();
    drop(db);
    s.service.tick();
    assert!(expiry_events(&s) > before);
    let reason = |s: &Setup| -> Option<String> {
        let db = rusqlite::Connection::open(s.world.state.join("controller.sqlite")).unwrap();
        db.query_row("SELECT ended_reason FROM approval_windows", [], |row| {
            row.get(0)
        })
        .unwrap()
    };
    assert_eq!(reason(&s).as_deref(), Some("expired"));
    // Once expired, further ticks add nothing.
    let after = ledger_rows(&s);
    s.service.tick();
    assert_eq!(ledger_rows(&s), after);
}
