//! The Operator service over a real kernel in a scratch project: an approved proposal runs once, with a scrubbed
//! environment; auto rules, session grants and full auto approve what Node approves.

mod common;

use capstan_operator::api::{
    DecideInput, Decision, FullAutoOnInput, FullAutoStatus, GrantKind, OperatorService,
    ProposalKind, ProposeInput, SessionGrantInput,
};
use capstan_operator::service::{
    run_command, OperatorServiceOptions, ProcessOperatorService, RunOptions, RunResult, Runner,
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
        restart: None,
        notify_pm: None,
        processes: None,
        monotonic_now: None,
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
