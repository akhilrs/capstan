//! The launcher's interface for the rest of the daemon: `LauncherService` (what `LauncherApi` of src/commands/shared.ts and
//! the public methods of `Launcher` in src/launcher.ts offer), its value types (src/launcher/shared.ts) and a recording
//! stub with scripted results.
//!
//! Every method blocks; the launcher runs one operation at a time on its own thread and takes the kernel per ledger
//! step, never across Herdr, git or setup commands.

use capstan_herdr::api::{CaptureOutcome, HookResult, PromptAnswer, RelayOutcome};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;
use std::sync::Mutex;

pub const STEP_BUDGET_MS: u64 = 60_000;
pub const CLEANUP_BUDGET_MS: u64 = 30_000;
pub const ADOPT_BUDGET_MS: u64 = 20_000;
pub const MAX_WAITING_OPERATIONS: usize = 1;

/// `LauncherError`: a refusal with a code; the command layer shows `code: message`, and `not_configured` as itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LauncherError {
    pub code: String,
    pub message: String,
}

impl LauncherError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for LauncherError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for LauncherError {}

pub type LauncherResult<T> = Result<T, LauncherError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchState {
    Started,
    Running,
    Blocked,
    NeedsRestart,
    Failed,
}

impl LaunchState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::NeedsRestart => "needs_restart",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HubStatus {
    Opened,
    Present,
    Failed,
}

impl HubStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Opened => "opened",
            Self::Present => "present",
            Self::Failed => "failed",
        }
    }
}

fn put(map: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        map.insert(key.to_string(), value);
    }
}

/// `LaunchResult`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaunchResult {
    pub state: Option<LaunchState>,
    pub agent_id: Option<String>,
    pub pane_id: Option<String>,
    pub generation: Option<i64>,
    pub hint: Option<String>,
    pub reason: Option<String>,
    pub step: Option<String>,
    pub hub: Option<HubStatus>,
    pub missing_env: Option<Vec<String>>,
    pub warning: Option<String>,
}

impl LaunchResult {
    /// The JSON the command answers with: members in declaration order, absent ones left out.
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        put(&mut map, "state", self.state.map(|s| json!(s.as_str())));
        put(
            &mut map,
            "agentId",
            self.agent_id.as_ref().map(|v| json!(v)),
        );
        put(&mut map, "paneId", self.pane_id.as_ref().map(|v| json!(v)));
        put(&mut map, "generation", self.generation.map(|v| json!(v)));
        put(&mut map, "hint", self.hint.as_ref().map(|v| json!(v)));
        put(&mut map, "reason", self.reason.as_ref().map(|v| json!(v)));
        put(&mut map, "step", self.step.as_ref().map(|v| json!(v)));
        put(&mut map, "hub", self.hub.map(|v| json!(v.as_str())));
        put(
            &mut map,
            "missingEnv",
            self.missing_env.as_ref().map(|v| json!(v)),
        );
        put(&mut map, "warning", self.warning.as_ref().map(|v| json!(v)));
        Value::Object(map)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Pane,
    Tab,
}

impl Placement {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pane => "pane",
            Self::Tab => "tab",
        }
    }
}

/// `SpawnResult`.
#[derive(Clone, Debug, PartialEq)]
pub struct SpawnResult {
    /// "started" or "blocked".
    pub state: String,
    pub agent_id: String,
    pub pane_id: String,
    pub worktree_path: String,
    pub branch: String,
    pub placement: Placement,
    pub placement_note: Option<String>,
    pub hint: Option<String>,
    pub missing_env: Option<Vec<String>>,
    pub warning: Option<String>,
}

impl SpawnResult {
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("state".into(), json!(self.state));
        map.insert("agentId".into(), json!(self.agent_id));
        map.insert("paneId".into(), json!(self.pane_id));
        map.insert("worktreePath".into(), json!(self.worktree_path));
        map.insert("branch".into(), json!(self.branch));
        map.insert("placement".into(), json!(self.placement.as_str()));
        put(
            &mut map,
            "placementNote",
            self.placement_note.as_ref().map(|v| json!(v)),
        );
        put(&mut map, "hint", self.hint.as_ref().map(|v| json!(v)));
        put(
            &mut map,
            "missingEnv",
            self.missing_env.as_ref().map(|v| json!(v)),
        );
        put(&mut map, "warning", self.warning.as_ref().map(|v| json!(v)));
        Value::Object(map)
    }
}

/// `SpawnOptions`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpawnOptions {
    pub base_sha: Option<String>,
    pub seed: Option<String>,
    /// `<plan-id>/<package-id>` or a requirement ref id: the branch is named after it.
    pub task: Option<String>,
    pub type_: Option<String>,
    pub title: Option<String>,
    /// The report or integration a reviewer looks at.
    pub review_target: Option<String>,
    /// Continue this branch name (a replacement's).
    pub branch: Option<String>,
    pub record_task_ref: Option<String>,
    pub record_task_title: Option<String>,
}

/// `BranchRenameResult`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchRenameResult {
    pub branch: Option<String>,
    pub renamed: bool,
    pub note: Option<String>,
}

/// `ObserveResult`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObserveResult {
    pub agent_id: String,
    pub role_name: String,
    pub kind: String,
    pub state: String,
    /// Herdr's own state for the pane, or None when it could not be read.
    pub agent_status: Option<String>,
    pub text: String,
}

impl ObserveResult {
    pub fn to_value(&self) -> Value {
        json!({
            "agentId": self.agent_id,
            "roleName": self.role_name,
            "kind": self.kind,
            "state": self.state,
            "agentStatus": self.agent_status,
            "text": self.text,
        })
    }
}

/// `ReleaseOutcome`: None where Node has null.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseOutcome {
    pub pane_closed: Option<bool>,
    pub worktree_removed: Option<bool>,
    pub branch_kept: Option<bool>,
}

/// `ReleaseResult`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseResult {
    pub outcome: ReleaseOutcome,
    pub agent_id: String,
    pub branch: Option<String>,
    pub cancelled_message_ids: Vec<String>,
}

impl ReleaseResult {
    pub fn to_value(&self) -> Value {
        json!({
            "paneClosed": self.outcome.pane_closed,
            "worktreeRemoved": self.outcome.worktree_removed,
            "branchKept": self.outcome.branch_kept,
            "state": "released",
            "agentId": self.agent_id,
            "branch": self.branch,
            "cancelledMessageIds": self.cancelled_message_ids,
        })
    }
}

/// The new agent of a replacement.
#[derive(Clone, Debug, PartialEq)]
pub struct Replaced {
    pub spawn: SpawnResult,
    pub predecessor: String,
    pub base_sha: String,
    /// "predecessor" or "head".
    pub base_source: String,
    pub predecessor_worktree_removed: Option<bool>,
    pub cancelled_message_ids: Vec<String>,
    pub kept_ref: Option<String>,
    pub replacement_recorded: bool,
}

/// `ReplaceResult`: the new agent, or why nothing was started.
#[derive(Clone, Debug, PartialEq)]
pub enum ReplaceResult {
    Replaced(Box<Replaced>),
    Blocked { predecessor: String, reason: String },
}

impl ReplaceResult {
    pub fn state(&self) -> &str {
        match self {
            Self::Replaced(done) => &done.spawn.state,
            Self::Blocked { .. } => "blocked",
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            Self::Blocked {
                predecessor,
                reason,
            } => json!({"state": "blocked", "predecessor": predecessor, "reason": reason}),
            Self::Replaced(done) => {
                let Value::Object(mut map) = done.spawn.to_value() else {
                    unreachable!("a spawn result is an object")
                };
                map.insert("predecessor".into(), json!(done.predecessor));
                map.insert("baseSha".into(), json!(done.base_sha));
                map.insert("baseSource".into(), json!(done.base_source));
                map.insert(
                    "predecessorWorktreeRemoved".into(),
                    json!(done.predecessor_worktree_removed),
                );
                map.insert(
                    "cancelledMessageIds".into(),
                    json!(done.cancelled_message_ids),
                );
                put(
                    &mut map,
                    "keptRef",
                    done.kept_ref.as_ref().map(|v| json!(v)),
                );
                map.insert(
                    "replacementRecorded".into(),
                    json!(done.replacement_recorded),
                );
                Value::Object(map)
            }
        }
    }
}

/// `LauncherStatus`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LauncherStatus {
    /// (agent id, reason, worktree path)
    pub cleanup_failed: Vec<(String, String, Option<String>)>,
    /// (agent id, pane id)
    pub orphan_panes: Vec<(String, String)>,
}

impl LauncherStatus {
    pub fn to_value(&self) -> Value {
        json!({
            "cleanupFailed": self.cleanup_failed.iter().map(|(agent, reason, path)| {
                let mut entry = Map::new();
                entry.insert("agentId".into(), json!(agent));
                entry.insert("reason".into(), json!(reason));
                put(&mut entry, "worktreePath", path.as_ref().map(|v| json!(v)));
                Value::Object(entry)
            }).collect::<Vec<_>>(),
            "orphanPanes": self.orphan_panes.iter().map(|(agent, pane)| json!({"agentId": agent, "paneId": pane})).collect::<Vec<_>>(),
        })
    }
}

/// `SetupOutcome`: how a worktree setup or teardown command ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetupOutcome {
    Ok,
    Failed {
        exit_code: Option<i32>,
        output: String,
    },
    Timeout,
}

/// What `answer_prompt` takes besides the agent: the hash it expects, the answer, and the hook that runs before the
/// first key (it records the relay in the ledger and may refuse by returning an error).
pub struct AnswerPromptRequest<'a> {
    pub prompt_sha: &'a str,
    pub answer: &'a PromptAnswer,
    pub before_type: &'a mut dyn FnMut() -> HookResult,
}

/// The launcher as the commands, reviews, supervision and the operator see it.
pub trait LauncherService: Send + Sync {
    /// Starts the PM in the hub workspace (`launchPm`).
    fn launch_pm(&self) -> LauncherResult<LaunchResult>;
    /// Replaces the PM's pane with a fresh PM (`restartPm`).
    fn restart_pm(&self) -> LauncherResult<LaunchResult>;
    /// Starts a worker of `role_name` in its own worktree.
    fn spawn(&self, role_name: &str, options: &SpawnOptions) -> LauncherResult<SpawnResult>;
    /// Names an assignee's branch after its task when it still has no commit and no report; never fails for a kept branch.
    fn rename_branch_for_task(
        &self,
        agent_id: &str,
        task: &str,
    ) -> LauncherResult<BranchRenameResult>;
    /// Ends a worker and frees its pane, worktree and, without commits, its branch.
    fn release(&self, agent_id: &str) -> LauncherResult<ReleaseResult>;
    /// Releases a worker and starts a new agent of the same role whose prompt carries a seed built from the ledger.
    fn replace(&self, agent_id: &str) -> LauncherResult<ReplaceResult>;
    /// The recent screen of an active agent (a read: it does not wait for other launcher operations); also what `peek` uses.
    fn observe(&self, agent_id: &str, lines: usize) -> LauncherResult<ObserveResult>;
    /// Reads the blocking permission prompt of an active worker; nothing is typed.
    fn capture_prompt(&self, agent_id: &str) -> LauncherResult<CaptureOutcome>;
    /// Types an answer when the screen still hashes to `prompt_sha`.
    fn answer_prompt(
        &self,
        agent_id: &str,
        request: AnswerPromptRequest<'_>,
    ) -> LauncherResult<RelayOutcome>;
    /// Sends one Esc to a worker Herdr shows working; true when sent.
    fn interrupt(&self, agent_id: &str) -> LauncherResult<bool>;
    fn status(&self) -> LauncherStatus;
    /// The environment an approved Operator command runs in.
    fn operator_environment(&self) -> BTreeMap<String, String>;
    /// How many spawn, release or replace operations are running or waiting; a restart waits until it is zero.
    fn in_flight_operations(&self) -> usize;
    /// Re-registers the panes recorded before a daemon restart (run in the background before the driver starts).
    fn adopt_all(&self) -> LauncherResult<()>;
    /// Runs the worktree teardown command of a released worker's worktree.
    fn teardown(&self, agent_id: &str, worktree_path: &str) -> LauncherResult<SetupOutcome>;
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A `LauncherService` with a call log and scripted results (`script` queues the next answer of a method; a method
/// with nothing scripted answers a plausible default).
#[derive(Default)]
pub struct StubLauncher {
    state: Mutex<StubLauncherState>,
}

#[derive(Default)]
struct StubLauncherState {
    calls: Vec<String>,
    errors: HashMap<String, VecDeque<LauncherError>>,
    spawns: VecDeque<SpawnResult>,
    counter: u64,
    in_flight: usize,
}

impl StubLauncher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn calls(&self) -> Vec<String> {
        lock(&self.state).calls.clone()
    }

    /// The next call of `method` fails with `error`.
    pub fn fail_next(&self, method: &str, error: LauncherError) {
        lock(&self.state)
            .errors
            .entry(method.to_string())
            .or_default()
            .push_back(error);
    }

    /// The next `spawn` answers this result.
    pub fn script_spawn(&self, result: SpawnResult) {
        lock(&self.state).spawns.push_back(result);
    }

    pub fn set_in_flight(&self, count: usize) {
        lock(&self.state).in_flight = count;
    }

    fn record(&self, line: String, method: &str) -> LauncherResult<()> {
        self.enter(line, method).map(drop)
    }

    fn enter(
        &self,
        line: String,
        method: &str,
    ) -> LauncherResult<std::sync::MutexGuard<'_, StubLauncherState>> {
        let mut state = lock(&self.state);
        state.calls.push(line);
        match state.errors.get_mut(method).and_then(VecDeque::pop_front) {
            Some(error) => Err(error),
            None => Ok(state),
        }
    }
}

impl LauncherService for StubLauncher {
    fn launch_pm(&self) -> LauncherResult<LaunchResult> {
        self.record("launch_pm".into(), "launch_pm")?;
        Ok(LaunchResult {
            state: Some(LaunchState::Started),
            agent_id: Some("pm-1".into()),
            pane_id: Some("w1:p1".into()),
            generation: Some(1),
            hub: Some(HubStatus::Opened),
            ..LaunchResult::default()
        })
    }

    fn restart_pm(&self) -> LauncherResult<LaunchResult> {
        self.record("restart_pm".into(), "restart_pm")?;
        Ok(LaunchResult {
            state: Some(LaunchState::Started),
            agent_id: Some("pm-1".into()),
            pane_id: Some("w1:p2".into()),
            generation: Some(2),
            ..LaunchResult::default()
        })
    }

    fn spawn(&self, role_name: &str, options: &SpawnOptions) -> LauncherResult<SpawnResult> {
        let mut state = self.enter(
            format!(
                "spawn {role_name} {} {} {}",
                options.base_sha.as_deref().unwrap_or("-"),
                options.task.as_deref().unwrap_or("-"),
                options.review_target.as_deref().unwrap_or("-")
            ),
            "spawn",
        )?;
        if let Some(result) = state.spawns.pop_front() {
            return Ok(result);
        }
        state.counter += 1;
        let n = state.counter;
        Ok(SpawnResult {
            state: "started".into(),
            agent_id: format!("{role_name}-{n}"),
            pane_id: format!("w{n}:p1"),
            worktree_path: format!("/tmp/work/{role_name}-{n}"),
            branch: format!("capstan/{role_name}-{n}"),
            placement: Placement::Tab,
            placement_note: None,
            hint: None,
            missing_env: None,
            warning: None,
        })
    }

    fn rename_branch_for_task(
        &self,
        agent_id: &str,
        task: &str,
    ) -> LauncherResult<BranchRenameResult> {
        self.record(
            format!("rename_branch_for_task {agent_id} {task}"),
            "rename_branch_for_task",
        )?;
        Ok(BranchRenameResult {
            branch: None,
            renamed: false,
            note: None,
        })
    }

    fn release(&self, agent_id: &str) -> LauncherResult<ReleaseResult> {
        self.record(format!("release {agent_id}"), "release")?;
        Ok(ReleaseResult {
            outcome: ReleaseOutcome {
                pane_closed: Some(true),
                worktree_removed: Some(true),
                branch_kept: Some(false),
            },
            agent_id: agent_id.to_string(),
            branch: None,
            cancelled_message_ids: Vec::new(),
        })
    }

    fn replace(&self, agent_id: &str) -> LauncherResult<ReplaceResult> {
        self.record(format!("replace {agent_id}"), "replace")?;
        Ok(ReplaceResult::Blocked {
            predecessor: agent_id.to_string(),
            reason: "the stub launcher does not replace".into(),
        })
    }

    fn observe(&self, agent_id: &str, lines: usize) -> LauncherResult<ObserveResult> {
        self.record(format!("observe {agent_id} {lines}"), "observe")?;
        Ok(ObserveResult {
            agent_id: agent_id.to_string(),
            role_name: "developer".into(),
            kind: "Developer".into(),
            state: "active".into(),
            agent_status: Some("idle".into()),
            text: String::new(),
        })
    }

    fn capture_prompt(&self, agent_id: &str) -> LauncherResult<CaptureOutcome> {
        self.record(format!("capture_prompt {agent_id}"), "capture_prompt")?;
        Ok(CaptureOutcome::Refused(
            capstan_herdr::api::RelayRefusal::NotBlocked,
        ))
    }

    fn answer_prompt(
        &self,
        agent_id: &str,
        request: AnswerPromptRequest<'_>,
    ) -> LauncherResult<RelayOutcome> {
        self.record(
            format!(
                "answer_prompt {agent_id} {} {:?}",
                request.prompt_sha, request.answer
            ),
            "answer_prompt",
        )?;
        Ok(RelayOutcome::Refused {
            reason: capstan_herdr::api::RelayRefusal::NotBlocked,
            keys: Vec::new(),
        })
    }

    fn interrupt(&self, agent_id: &str) -> LauncherResult<bool> {
        self.record(format!("interrupt {agent_id}"), "interrupt")?;
        Ok(false)
    }

    fn status(&self) -> LauncherStatus {
        lock(&self.state).calls.push("status".into());
        LauncherStatus::default()
    }

    fn operator_environment(&self) -> BTreeMap<String, String> {
        lock(&self.state).calls.push("operator_environment".into());
        BTreeMap::new()
    }

    fn in_flight_operations(&self) -> usize {
        lock(&self.state).in_flight
    }

    fn adopt_all(&self) -> LauncherResult<()> {
        self.record("adopt_all".into(), "adopt_all")?;
        Ok(())
    }

    fn teardown(&self, agent_id: &str, worktree_path: &str) -> LauncherResult<SetupOutcome> {
        self.record(format!("teardown {agent_id} {worktree_path}"), "teardown")?;
        Ok(SetupOutcome::Ok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_render_their_members_in_declaration_order() {
        let spawn = StubLauncher::new()
            .spawn("developer", &SpawnOptions::default())
            .unwrap();
        let text = spawn.to_value().to_string();
        assert!(
            text.starts_with(r#"{"state":"started","agentId":"developer-1""#),
            "{text}"
        );
        let blocked = ReplaceResult::Blocked {
            predecessor: "a".into(),
            reason: "r".into(),
        };
        assert_eq!(blocked.state(), "blocked");
    }

    #[test]
    fn the_stub_logs_calls_and_scripts_failures() {
        let launcher = StubLauncher::new();
        launcher.fail_next("release", LauncherError::new("not_configured", "no"));
        assert_eq!(launcher.release("a").unwrap_err().code, "not_configured");
        assert!(launcher.release("a").is_ok());
        assert_eq!(launcher.calls(), vec!["release a", "release a"]);
    }
}
