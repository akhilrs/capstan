//! The Operator service over a real kernel in a scratch project: an approved proposal runs once, with a scrubbed
//! environment; auto rules, session grants and full auto approve what Node approves.

mod common;

use capstan_operator::api::{
    DecideInput, Decision, FullAutoOnInput, FullAutoStatus, GrantKind, OperatorService,
    ProposalKind, ProposeInput, SessionGrantInput,
};
use capstan_operator::service::{
    run_command, OperatorLedger, OperatorServiceOptions, ProcessOperatorService, RunOptions,
    RunResult, Runner,
};
use common::*;
use serde_json::Value;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::Arc;

type Environment = Vec<(String, String)>;

struct Setup {
    world: World,
    service: Arc<ProcessOperatorService>,
    runs: Arc<AtomicUsize>,
    seen: Arc<std::sync::Mutex<Vec<Environment>>>,
    /// Milliseconds the service's clock has been moved ahead.
    skew: Arc<AtomicI64>,
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
    let skew = Arc::new(AtomicI64::new(0));
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
        monotonic_now: Some({
            let skew = skew.clone();
            Arc::new(move || {
                1_000_000 + started.elapsed().as_millis() as i64 + skew.load(Ordering::SeqCst)
            })
        }),
        orphan_kill_wait_ms: None,
        log: None,
    }));
    Setup {
        world,
        service,
        runs,
        seen,
        skew,
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
        Some(SessionGrantInput {
            kind: GrantKind::Exact,
            text: None,
            minutes: None,
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

// ------------------------------------------------------------------------------------------------ timed grants

fn timed(minutes: Option<i64>) -> Option<SessionGrantInput> {
    Some(SessionGrantInput {
        kind: GrantKind::Timed,
        text: None,
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

/// A setup with a live timed grant of `minutes`; returns the grant id.
fn with_timed_grant(s: &Setup, minutes: i64) -> String {
    let first = s.propose("echo start");
    let approved = s.approve(&first, timed(Some(minutes)));
    s.wait(&first, "finished");
    approved["sessionGrant"]["grantId"]
        .as_str()
        .expect("a grant")
        .to_string()
}

#[test]
fn a_timed_grant_approves_and_runs_a_safe_command_and_the_pm_gets_the_notice() {
    let s = setup(|_| {});
    let grant = with_timed_grant(&s, 30);
    let proposal = s.propose("git status");
    assert_eq!(proposal["state"], "approved");
    assert_eq!(proposal["autoRule"], format!("session:{grant}"));
    s.wait_ended(&proposal);
    let done = s
        .service
        .show(proposal["proposalId"].as_str().unwrap())
        .unwrap();
    assert_eq!(done["autoRule"], format!("session:{grant}"));
    let id = proposal["proposalId"].as_str().unwrap();
    assert!(
        messages(&s, "pm-1")
            .iter()
            .any(|m| m.contains(&format!("Operator run {id} finished"))),
        "the PM got the run notice"
    );
    let ledger = s.service.list(&Default::default());
    assert!(ledger
        .iter()
        .any(|p| p["proposalId"] == proposal["proposalId"]));
    s.service.stop();
}

#[test]
fn a_timed_grant_leaves_destructive_unknown_and_unparseable_commands_to_the_pm() {
    let s = setup(|_| {});
    with_timed_grant(&s, 30);
    for command in ["git push", "rm -rf x", "npm test", "ls | $(id)"] {
        let proposal = s.propose(command);
        assert_eq!(proposal["state"], "proposed", "{command}");
        assert_eq!(proposal["autoRule"], Value::Null, "{command}");
    }
    s.service.stop();
}

#[test]
fn a_timed_grant_ends_at_its_cap_on_revoke_on_recover_and_on_release() {
    // At its cap (the in-memory deadline comes from the grant's own minutes).
    let s = setup(|_| {});
    with_timed_grant(&s, 5);
    assert_eq!(s.propose("git status")["state"], "approved");
    s.skew.fetch_add(5 * 60_000 + 1, Ordering::SeqCst);
    assert!(s.service.grants().is_empty());
    assert_eq!(s.propose("git status")["state"], "proposed");
    s.service.stop();

    // On revoke.
    let s = setup(|_| {});
    let grant = with_timed_grant(&s, 30);
    s.service.revoke_grant(s.world.pm(), &grant).unwrap();
    assert_eq!(s.propose("git status")["state"], "proposed");
    s.service.stop();

    // On recover (a restart).
    let s = setup(|_| {});
    with_timed_grant(&s, 30);
    s.service.recover(None);
    assert!(s.service.grants().is_empty());
    assert_eq!(s.propose("git status")["state"], "proposed");
    s.service.stop();

    // When the Operator is released.
    let s = setup(|_| {});
    with_timed_grant(&s, 30);
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
    assert!(s.service.grants().is_empty());
    s.service.stop();
}

#[test]
fn a_prefix_grant_wins_over_a_live_timed_grant_and_a_restart_never_matches() {
    let restart = Arc::new(capstan_operator::api::StubRestart::new());
    restart.set_preflight(Ok(Default::default()));
    let s = setup_with(|_| {}, Some(restart));
    let timed_grant = with_timed_grant(&s, 30);
    let seed = s.propose("git log --follow");
    let approved = s.approve(
        &seed,
        Some(SessionGrantInput {
            kind: GrantKind::Prefix,
            text: Some("git log".into()),
            minutes: None,
        }),
    );
    let prefix = approved["sessionGrant"]["grantId"]
        .as_str()
        .unwrap()
        .to_string();
    s.wait_ended(&seed);
    let matched = s.propose("git log -5");
    assert_eq!(matched["autoRule"], format!("session:{prefix}"));
    s.wait_ended(&matched);
    // Without a matching prefix the timed grant is the rule.
    let other = s.propose("git diff");
    assert_eq!(other["autoRule"], format!("session:{timed_grant}"));
    s.wait_ended(&other);
    let restart = s
        .service
        .propose(
            s.world.developer(),
            ProposeInput {
                kind: ProposalKind::Restart,
                command: "restart".into(),
                reason: "new build".into(),
                force_restart: false,
            },
        )
        .expect("proposed");
    assert_eq!(restart["state"], "proposed");
    assert_eq!(restart["autoRule"], Value::Null);
    s.service.stop();
}

#[test]
fn exact_and_prefix_grants_keep_the_session_cap_and_timed_grants_use_their_own_minutes() {
    let s = setup(|c| {
        c.session_grant_max_minutes = 60;
        c.timed_grant_max_minutes = 240;
    });
    let exact = s.propose("echo exact");
    let exact_grant = s.approve(
        &exact,
        Some(SessionGrantInput {
            kind: GrantKind::Exact,
            text: None,
            minutes: None,
        }),
    );
    s.wait(&exact, "finished");
    let timed_grant = with_timed_grant(&s, 120);
    let remaining = |id: &str| {
        s.service
            .grants()
            .iter()
            .find(|g| g["grantId"] == id)
            .map(|g| g["remainingSeconds"].as_i64().unwrap())
            .unwrap()
    };
    let exact_id = exact_grant["sessionGrant"]["grantId"].as_str().unwrap();
    assert!(
        (3590..=3600).contains(&remaining(exact_id)),
        "{}",
        remaining(exact_id)
    );
    assert!(
        (7190..=7200).contains(&remaining(&timed_grant)),
        "{}",
        remaining(&timed_grant)
    );
    let ledger = s.service.list_grants();
    let ledger_expiry = |id: &str| {
        ledger.iter().find(|g| g["grantId"] == id).unwrap()["expiresAt"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let (exact_at, timed_at) = (ledger_expiry(exact_id), ledger_expiry(&timed_grant));
    assert!(timed_at > exact_at, "{timed_at} vs {exact_at}");
    s.service.stop();
}

#[test]
fn a_timed_grant_is_refused_when_disabled_or_out_of_range() {
    let s = setup(|c| c.timed_safe_commands.clear());
    let proposal = s.propose("echo x");
    let refused = s
        .service
        .decide(
            s.world.pm(),
            DecideInput {
                proposal_id: proposal["proposalId"].as_str().unwrap().into(),
                decision: Decision::Approve,
                hash: Some(proposal["commandSha"].as_str().unwrap()[..12].into()),
                note: None,
                session: timed(Some(10)),
            },
        )
        .unwrap_err();
    assert_eq!(refused.code(), Some("timed_grants_disabled"));
    s.service.stop();

    let s = setup(|c| c.timed_grant_max_minutes = 60);
    let proposal = s.propose("echo x");
    for minutes in [0, 61] {
        let refused = s
            .service
            .decide(
                s.world.pm(),
                DecideInput {
                    proposal_id: proposal["proposalId"].as_str().unwrap().into(),
                    decision: Decision::Approve,
                    hash: Some(proposal["commandSha"].as_str().unwrap()[..12].into()),
                    note: None,
                    session: timed(Some(minutes)),
                },
            )
            .unwrap_err();
        assert_eq!(refused.code(), Some("timed_minutes"), "{minutes}");
    }
    s.service.stop();
}
