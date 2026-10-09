//! The Operator's interface for the rest of the daemon: `OperatorService` (src/operator.ts: proposals, decisions, session
//! grants, full auto, the worker that runs approved commands) and `RestartCoordinator` (src/restart.ts), with
//! `OperatorError` and recording stubs.
//!
//! Records (proposals, grants) travel as the camelCase JSON the kernel stores (`serde_json::Value`), as they do in the
//! kernel crate. Every method blocks; the worker that runs commands has its own thread and takes the kernel per ledger
//! step, never while a command runs.

use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::Mutex;

pub const ORPHAN_KILL_WAIT_MS: u64 = 5000;
pub const OPERATOR_TICK_MS: u64 = 5000;
pub const OPERATOR_NOTICE_MAX_BYTES: usize = 16 * 1024;

/// `OperatorError`: a refusal the service names with a code; the command layer shows `code: message`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperatorError {
    pub code: String,
    pub message: String,
}

impl OperatorError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for OperatorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for OperatorError {}

pub type OperatorResult<T> = Result<T, OperatorError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProposalKind {
    Command,
    Restart,
}

impl ProposalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::Restart => "restart",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Approve,
    Deny,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrantKind {
    Exact,
    Prefix,
}

impl GrantKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Prefix => "prefix",
        }
    }
}

pub struct ProposeInput {
    pub kind: ProposalKind,
    pub command: String,
    pub reason: String,
    pub force_restart: bool,
}

/// Approve and allow the same text (exact) or the same leading words (prefix) again for the session.
pub struct SessionGrantInput {
    pub kind: GrantKind,
    pub text: Option<String>,
}

pub struct DecideInput {
    pub proposal_id: String,
    pub decision: Decision,
    pub hash: Option<String>,
    pub note: Option<String>,
    pub session: Option<SessionGrantInput>,
}

#[derive(Clone, Debug, Default)]
pub struct ListFilter {
    pub states: Option<Vec<String>>,
    pub proposer_agent_id: Option<String>,
    pub limit: Option<usize>,
}

/// `FullAutoStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FullAutoStatus {
    Off,
    On {
        minutes: i64,
        remaining_seconds: i64,
    },
}

pub struct FullAutoOnInput {
    pub minutes: Option<i64>,
    pub asked_user: String,
}

/// `OperatorService`.
pub trait OperatorService: Send + Sync {
    fn propose(&self, credential: &str, input: ProposeInput) -> OperatorResult<Value>;
    fn decide(&self, credential: &str, input: DecideInput) -> OperatorResult<Value>;
    fn cancel(&self, credential: &str, proposal_id: &str) -> OperatorResult<Value>;
    fn show(&self, proposal_id: &str) -> Option<Value>;
    fn list(&self, filter: &ListFilter) -> Vec<Value>;
    /// Session grants that are still in force.
    fn grants(&self) -> Vec<Value>;
    /// Every grant, ended ones included, newest first.
    fn list_grants(&self) -> Vec<Value>;
    fn revoke_grant(&self, credential: &str, grant_id: &str) -> OperatorResult<Value>;
    /// Switches full auto on for `minutes` (the configured default when omitted) after the PM asked the user.
    fn full_auto_on(
        &self,
        credential: &str,
        input: FullAutoOnInput,
    ) -> OperatorResult<FullAutoStatus>;
    /// Immediate. Does nothing when full auto is already off.
    fn full_auto_off(&self, credential: &str) -> OperatorResult<FullAutoStatus>;
    fn full_auto_status(&self) -> FullAutoStatus;
    /// Runs approved proposals now; returns when the worker has nothing left to start.
    fn drain(&self);
    /// Startup: abandons runs that were running when the controller stopped and clears stray process groups;
    /// `skip_restarts_with_plan` names the restart proposals whose helper is still working.
    fn recover(&self, skip_restarts_with_plan: Option<&dyn Fn(&str) -> bool>);
    /// One pass of expiry, orphan re-checks and the worker.
    fn tick(&self);
    fn start(&self, interval_ms: Option<u64>);
    /// Kills a running command's process group and waits for the worker.
    fn stop(&self);
}

/// What a restart preflight answers: a warning for the PM notice, or none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RestartPreflight {
    pub warning: Option<String>,
}

/// `RestartCoordinator`.
pub trait RestartCoordinator: Send + Sync {
    /// Refuses with a code when a restart could not start now; returns a warning for the PM notice, or none.
    fn preflight(&self) -> OperatorResult<RestartPreflight>;
    /// Starts the restart of an approved restart proposal (its JSON record).
    fn run(&self, proposal: &Value) -> OperatorResult<()>;
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// An `OperatorService` with a call log; every answer is scripted with `script` (a method with nothing scripted fails
/// with `not_implemented`, or answers an empty result for the reads).
#[derive(Default)]
pub struct StubOperator {
    state: Mutex<StubOperatorState>,
}

#[derive(Default)]
struct StubOperatorState {
    calls: Vec<String>,
    answers: HashMap<String, VecDeque<OperatorResult<Value>>>,
    auto: Option<FullAutoStatus>,
}

impl StubOperator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn calls(&self) -> Vec<String> {
        lock(&self.state).calls.clone()
    }

    /// Queues the answer of the next call of `method` (propose, decide, cancel, revoke_grant).
    pub fn script(&self, method: &str, answer: OperatorResult<Value>) {
        lock(&self.state)
            .answers
            .entry(method.to_string())
            .or_default()
            .push_back(answer);
    }

    pub fn set_full_auto(&self, status: FullAutoStatus) {
        lock(&self.state).auto = Some(status);
    }

    fn answer(&self, line: String, method: &str) -> OperatorResult<Value> {
        let mut state = lock(&self.state);
        state.calls.push(line);
        state
            .answers
            .get_mut(method)
            .and_then(VecDeque::pop_front)
            .unwrap_or_else(|| {
                Err(OperatorError::new(
                    "not_implemented",
                    format!("the stub operator has no answer for {method}"),
                ))
            })
    }

    fn log(&self, line: String) {
        lock(&self.state).calls.push(line);
    }
}

impl OperatorService for StubOperator {
    fn propose(&self, _credential: &str, input: ProposeInput) -> OperatorResult<Value> {
        self.answer(
            format!("propose {} {}", input.kind.as_str(), input.command),
            "propose",
        )
    }

    fn decide(&self, _credential: &str, input: DecideInput) -> OperatorResult<Value> {
        self.answer(
            format!("decide {} {:?}", input.proposal_id, input.decision),
            "decide",
        )
    }

    fn cancel(&self, _credential: &str, proposal_id: &str) -> OperatorResult<Value> {
        self.answer(format!("cancel {proposal_id}"), "cancel")
    }

    fn show(&self, proposal_id: &str) -> Option<Value> {
        self.log(format!("show {proposal_id}"));
        None
    }

    fn list(&self, filter: &ListFilter) -> Vec<Value> {
        self.log(format!("list {:?}", filter.states));
        Vec::new()
    }

    fn grants(&self) -> Vec<Value> {
        self.log("grants".into());
        Vec::new()
    }

    fn list_grants(&self) -> Vec<Value> {
        self.log("list_grants".into());
        Vec::new()
    }

    fn revoke_grant(&self, _credential: &str, grant_id: &str) -> OperatorResult<Value> {
        self.answer(format!("revoke_grant {grant_id}"), "revoke_grant")
    }

    fn full_auto_on(
        &self,
        _credential: &str,
        input: FullAutoOnInput,
    ) -> OperatorResult<FullAutoStatus> {
        self.log(format!("full_auto_on {:?}", input.minutes));
        let status = FullAutoStatus::On {
            minutes: input.minutes.unwrap_or(60),
            remaining_seconds: input.minutes.unwrap_or(60) * 60,
        };
        lock(&self.state).auto = Some(status);
        Ok(status)
    }

    fn full_auto_off(&self, _credential: &str) -> OperatorResult<FullAutoStatus> {
        self.log("full_auto_off".into());
        lock(&self.state).auto = Some(FullAutoStatus::Off);
        Ok(FullAutoStatus::Off)
    }

    fn full_auto_status(&self) -> FullAutoStatus {
        self.log("full_auto_status".into());
        lock(&self.state).auto.unwrap_or(FullAutoStatus::Off)
    }

    fn drain(&self) {
        self.log("drain".into());
    }

    fn recover(&self, _skip_restarts_with_plan: Option<&dyn Fn(&str) -> bool>) {
        self.log("recover".into());
    }

    fn tick(&self) {
        self.log("tick".into());
    }

    fn start(&self, interval_ms: Option<u64>) {
        self.log(format!("start {interval_ms:?}"));
    }

    fn stop(&self) {
        self.log("stop".into());
    }
}

/// A `RestartCoordinator` that records its calls and answers the preflight and run it was given.
#[derive(Default)]
pub struct StubRestart {
    state: Mutex<StubRestartState>,
}

#[derive(Default)]
struct StubRestartState {
    calls: Vec<String>,
    preflight: Option<OperatorResult<RestartPreflight>>,
    run: Option<OperatorResult<()>>,
}

impl StubRestart {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn calls(&self) -> Vec<String> {
        lock(&self.state).calls.clone()
    }

    pub fn set_preflight(&self, answer: OperatorResult<RestartPreflight>) {
        lock(&self.state).preflight = Some(answer);
    }

    pub fn set_run(&self, answer: OperatorResult<()>) {
        lock(&self.state).run = Some(answer);
    }
}

impl RestartCoordinator for StubRestart {
    fn preflight(&self) -> OperatorResult<RestartPreflight> {
        let mut state = lock(&self.state);
        state.calls.push("preflight".into());
        state
            .preflight
            .clone()
            .unwrap_or(Ok(RestartPreflight::default()))
    }

    fn run(&self, proposal: &Value) -> OperatorResult<()> {
        let mut state = lock(&self.state);
        state.calls.push(format!(
            "run {}",
            proposal["proposalId"].as_str().unwrap_or("?")
        ));
        state.run.clone().unwrap_or(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_stub_operator_answers_what_was_scripted() {
        let operator = StubOperator::new();
        operator.script("propose", Ok(json!({"proposalId": "p1"})));
        let input = || ProposeInput {
            kind: ProposalKind::Command,
            command: "ls".into(),
            reason: "r".into(),
            force_restart: false,
        };
        assert_eq!(operator.propose("c", input()).unwrap()["proposalId"], "p1");
        assert_eq!(
            operator.propose("c", input()).unwrap_err().code,
            "not_implemented"
        );
        assert_eq!(operator.full_auto_status(), FullAutoStatus::Off);
        assert_eq!(operator.calls().len(), 3);
    }

    #[test]
    fn the_stub_restart_records_and_scripts() {
        let restart = StubRestart::new();
        assert_eq!(restart.preflight().unwrap().warning, None);
        restart.set_run(Err(OperatorError::new("no_known_good", "none")));
        assert_eq!(
            restart.run(&json!({"proposalId": "p2"})).unwrap_err().code,
            "no_known_good"
        );
        assert_eq!(restart.calls(), vec!["preflight", "run p2"]);
    }
}
