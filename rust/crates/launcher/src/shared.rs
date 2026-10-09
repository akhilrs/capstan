//! Constants, types and the error shared by the launcher and its collaborators (src/launcher/shared.ts), and the branch
//! naming of src/conventions.ts the launcher uses.

use crate::api::{HubStatus, LauncherError, SetupOutcome};
use crate::kernel::Ledger;
use capstan_config::RoleConfig;
use capstan_herdr::api::{shell_quote, AdapterError, LauncherAdapter};
use capstan_kernel::KernelError;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

pub const START_TIMEOUT_MS: u64 = 30_000;
pub const START_ATTEMPTS: u32 = 3;
pub const START_RETRY_DELAY_MS: u64 = 1_500;
pub const DIALOG_TIMEOUT_MS: u64 = 10_000;
pub const ID_ATTEMPTS: u32 = 3;
/// Keeps a sync error to one short line in the refusal.
pub const MAX_SYNC_REASON_CHARS: usize = 200;
pub const MAX_NOTE_LENGTH: usize = 200;
pub const SETUP_OUTPUT_CHARS: usize = 2000;

// ------------------------------------------------------------------------------------------------ errors

/// What any step of an operation can fail with: the launcher's own refusal, an adapter (Herdr) failure or a ledger
/// failure. `message` is the `message` of the Node error and `js_string` is `String(error)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpError {
    Launcher(LauncherError),
    Adapter(AdapterError),
    Kernel { name: String, message: String },
}

pub type OpResult<T> = Result<T, OpError>;

impl OpError {
    pub fn launcher(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Launcher(LauncherError::new(code, message))
    }

    pub fn message(&self) -> String {
        match self {
            Self::Launcher(e) => e.message.clone(),
            Self::Adapter(e) => e.message().to_string(),
            Self::Kernel { message, .. } => message.clone(),
        }
    }

    /// `String(error)`: the class name, a colon and the message.
    pub fn js_string(&self) -> String {
        match self {
            Self::Launcher(e) => format!("LauncherError: {}", e.message),
            Self::Adapter(e) => format!("{}: {}", e.name(), e.message()),
            Self::Kernel { name, message } => format!("{name}: {message}"),
        }
    }

    pub fn adapter(&self) -> Option<&AdapterError> {
        match self {
            Self::Adapter(e) => Some(e),
            _ => None,
        }
    }

    /// Whether this is a Herdr failure with exactly this code.
    pub fn herdr_code_is(&self, code: &str) -> bool {
        self.adapter().and_then(AdapterError::herdr_code) == Some(code)
    }
}

impl From<LauncherError> for OpError {
    fn from(error: LauncherError) -> Self {
        Self::Launcher(error)
    }
}

impl From<AdapterError> for OpError {
    fn from(error: AdapterError) -> Self {
        Self::Adapter(error)
    }
}

impl From<KernelError> for OpError {
    fn from(error: KernelError) -> Self {
        Self::Kernel {
            name: error.name().to_string(),
            message: error.message(),
        }
    }
}

impl From<OpError> for LauncherError {
    /// How an error leaves the launcher: a launcher refusal as it is, anything else with the error's class (or the Herdr
    /// code) as its code.
    fn from(error: OpError) -> Self {
        match error {
            OpError::Launcher(e) => e,
            OpError::Adapter(e) => {
                let code = e.herdr_code().unwrap_or_else(|| e.name()).to_string();
                LauncherError::new(code, e.message())
            }
            OpError::Kernel { name, message } => LauncherError::new(name, message),
        }
    }
}

// ------------------------------------------------------------------------------------------------ records

/// `AgentRecord`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentRec {
    pub agent_id: String,
    pub role_name: String,
    pub kind: String,
    pub seat_id: String,
    pub actor_id: String,
    pub generation: i64,
    pub state: String,
}

impl AgentRec {
    pub fn from_value(value: &Value) -> Option<Self> {
        if !value.is_object() {
            return None;
        }
        Some(Self {
            agent_id: text(value, "agentId")?,
            role_name: text(value, "roleName")?,
            kind: text(value, "kind")?,
            seat_id: text(value, "seatId").unwrap_or_default(),
            actor_id: text(value, "actorId").unwrap_or_default(),
            generation: value.get("generation").and_then(Value::as_i64).unwrap_or(0),
            state: text(value, "state")?,
        })
    }

    pub fn active(&self) -> bool {
        self.state == "active"
    }
}

/// `AgentPaneRecord`: one row of `agentPanes`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneRow {
    pub agent_id: String,
    pub workspace_id: Option<String>,
    pub pane_id: Option<String>,
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
    pub base_sha: Option<String>,
    pub generation: i64,
    pub task_ref: Option<String>,
    pub task_title: Option<String>,
}

impl PaneRow {
    pub fn from_value(value: &Value) -> Option<Self> {
        Some(Self {
            agent_id: text(value, "agentId")?,
            workspace_id: text(value, "workspaceId"),
            pane_id: text(value, "paneId"),
            worktree_path: text(value, "worktreePath"),
            branch: text(value, "branch"),
            base_sha: text(value, "baseSha"),
            generation: value.get("generation").and_then(Value::as_i64).unwrap_or(0),
            task_ref: text(value, "taskRef"),
            task_title: text(value, "taskTitle"),
        })
    }
}

/// The string member `key` of an object; None when absent or not a string.
pub fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

// ------------------------------------------------------------------------------------------------ budget

/// `Budget`: the time an operation may still take; the deadline moves later by the time spent on a step with a deadline of
/// its own.
pub struct Budget {
    deadline: AtomicI64,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl Budget {
    pub fn new(now: Arc<dyn Fn() -> i64 + Send + Sync>, ms: u64) -> Self {
        let deadline = now() + ms as i64;
        Self {
            deadline: AtomicI64::new(deadline),
            now,
        }
    }

    pub fn deadline(&self) -> i64 {
        self.deadline.load(Ordering::SeqCst)
    }

    pub fn extend(&self, ms: i64) {
        self.deadline.fetch_add(ms, Ordering::SeqCst);
    }

    pub fn check(&self, step: &str) -> OpResult<()> {
        if (self.now)() > self.deadline() {
            return Err(OpError::launcher(
                "timeout",
                format!("the operation ran out of time before {step}"),
            ));
        }
        Ok(())
    }

    pub fn within(&self) -> bool {
        (self.now)() <= self.deadline()
    }
}

/// `Hub`: the long-lived hub workspace (it runs `cstan status --watch`): workers' worktrees hang under it, so a PM pane can
/// be closed and replaced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hub {
    pub status: HubStatus,
    pub workspace_id: Option<String>,
    /// The root pane of a hub made in this call; the first PM takes it.
    pub free_pm_pane: Option<String>,
}

/// What the cleanup of an agent is told about the resources it holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CleanupInfo {
    pub worktree_path: Option<String>,
    pub pane_id: Option<String>,
    pub branch: Option<String>,
    pub base_sha: Option<String>,
    /// True only when a pane move was started and its result never reached the ledger.
    pub move_may_have_happened: bool,
    /// The pane's Herdr terminal id recorded at spawn; None reads it from the ledger row, Some(None) says there is none.
    pub terminal_id: Option<Option<String>>,
    /// True only when this operation made the pane, so its id cannot have been given to another pane yet.
    pub pane_created_here: bool,
}

/// `cleanupFailed` entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanupFailure {
    pub agent_id: String,
    pub reason: String,
}

// ------------------------------------------------------------------------------------------------ git

/// What `worktreeRemove` answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeRemoval {
    pub removed: bool,
    /// Git's message when it refuses.
    pub stderr: String,
}

/// `GitRunner`: the git facts the launcher needs.
pub trait GitRunner: Send + Sync {
    fn head_sha(&self) -> Result<String, LauncherError>;
    /// Removes a worktree. Only a worktree of a legacy `capstan/` branch or of `recorded_branch` (the branch the ledger
    /// records for its agent) is forced (it may hold untracked files such as installed dependencies); any other is removed
    /// without force.
    fn worktree_remove(
        &self,
        worktree_path: &str,
        recorded_branch: Option<&str>,
    ) -> WorktreeRemoval;
    /// Whether the worktree's directory still exists; a worktree removed by hand (or by an earlier cleanup) does not.
    fn worktree_present(&self, worktree_path: &str) -> bool;
    /// `git worktree prune`: forgets worktrees whose directories are gone.
    fn prune_worktrees(&self);
    /// How many files are untracked or modified in the worktree; None when git cannot tell.
    fn worktree_dirty_count(&self, worktree_path: &str) -> Option<usize>;
    /// Atomic compare-and-delete: only when the branch still points at `sha`.
    fn delete_branch_if(&self, branch: &str, sha: &str) -> bool;
    fn worktree_by_branch(&self, branch: &str) -> Result<Option<String>, LauncherError>;
    /// Whether `branch` names a valid branch, checked by git before a worktree is created for it.
    fn branch_name_valid(&self, branch: &str) -> bool;
    /// `git branch -m` from the project root; the branch's worktree follows. `stderr` is git's message when it refuses.
    fn rename_branch(&self, from: &str, to: &str) -> WorktreeRemoval;
    /// Points `reference` (a full ref name) at `sha`.
    fn save_ref(&self, reference: &str, sha: &str) -> bool;
    /// The commit a branch points at, or None when there is no such branch.
    fn branch_tip(&self, branch: &str) -> Option<String>;
    /// Whether `sha` (40 lowercase hex characters) resolves as a commit and is an ancestor of one of the refs.
    fn reachable_commit(&self, sha: &str, from: &[String]) -> Result<bool, LauncherError>;
}

/// Runs the worktree setup command: command, directory, timeout in milliseconds.
pub type SetupRunner = Arc<dyn Fn(&str, &str, u64) -> SetupOutcome + Send + Sync>;

/// Runs the worktree teardown command with the environment it is given.
pub type TeardownRunner =
    Arc<dyn Fn(&str, &str, u64, &BTreeMap<String, String>) -> SetupOutcome + Send + Sync>;

pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;
pub type LogFn = Arc<dyn Fn(&str, Value) + Send + Sync>;

/// `LauncherOptions`.
pub struct LauncherOptions {
    pub ledger: Ledger,
    pub adapter: Arc<dyn LauncherAdapter>,
    pub config: RoleConfig,
    pub project_root: String,
    pub cli_path: String,
    pub socket_path: String,
    /// The project (operator) credential.
    pub credential: String,
    pub node_path: Option<String>,
    pub base_environment: HashMap<String, String>,
    pub git: Option<Arc<dyn GitRunner>>,
    pub now: Option<Clock>,
    pub log: Option<LogFn>,
    /// Brings the configured roles into the ledger; the launcher calls it once more when a role is missing or out of date.
    /// The error is the message of the failure.
    pub sync_roles: Option<Arc<dyn Fn() -> Result<(), String> + Send + Sync>>,
    pub run_setup: Option<SetupRunner>,
    /// Waits between attempts to start a worker; tests stub it.
    pub sleep: Option<Arc<dyn Fn(u64) + Send + Sync>>,
    pub run_teardown: Option<TeardownRunner>,
    /// Where the process runs, for the `cstan` wrapper; None reads it from the process.
    pub site: Option<FrontEndSite>,
}

// ------------------------------------------------------------------------------------------------ the cstan wrapper

/// Where the process runs, for `front_end_path`; tests pass their own.
#[derive(Clone, Debug, Default)]
pub struct FrontEndSite {
    pub env: HashMap<String, String>,
    pub exec_path: String,
    pub sea: bool,
}

impl FrontEndSite {
    /// This process: its environment and executable, and not the standalone binary of the Node CLI.
    pub fn current() -> Self {
        Self {
            env: std::env::vars().collect(),
            exec_path: std::env::current_exe()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            sea: false,
        }
    }
}

fn executable_file(file: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c_path) = std::ffi::CString::new(file.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `c_path` is a valid NUL-terminated string for the length of the call.
    let accessible = unsafe { libc::access(c_path.as_ptr(), libc::X_OK) } == 0;
    accessible
        && std::fs::metadata(file)
            .map(|m| m.is_file())
            .unwrap_or(false)
}

fn real_or_self(file: &Path) -> std::path::PathBuf {
    std::fs::canonicalize(file).unwrap_or_else(|_| {
        if file.is_absolute() {
            file.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_default().join(file)
        }
    })
}

/// The native front end an agent's `cstan` should start, or None for the plain Node CLI: `CSTAN_FRONT_END` when it names
/// an absolute executable file, else under SEA a `cstan` beside the binary that is not the binary itself.
pub fn front_end_path(site: &FrontEndSite) -> Option<String> {
    if let Some(configured) = site.env.get("CSTAN_FRONT_END").filter(|v| !v.is_empty()) {
        let path = Path::new(configured);
        return (path.is_absolute() && executable_file(path)).then(|| configured.clone());
    }
    if !site.sea {
        return None;
    }
    let exec = Path::new(&site.exec_path);
    let sibling = exec.parent().unwrap_or(Path::new("/")).join("cstan");
    (executable_file(&sibling) && real_or_self(&sibling) != real_or_self(exec))
        .then(|| sibling.to_string_lossy().into_owned())
}

/// Shell words that run this CLI: the binary itself under SEA, otherwise node plus the CLI file.
pub fn self_invocation(node: &str, cli_path: &str, site: &FrontEndSite) -> String {
    if site.sea {
        return shell_quote(&site.exec_path);
    }
    format!("{} {}", shell_quote(node), shell_quote(cli_path))
}

/// The per-agent `cstan` wrapper script put first on every agent's PATH. With a front end it starts that and tells it where
/// the Node CLI is (`CSTAN_NODE_CLI`, and `CSTAN_NODE` for a CLI file); without one it runs the Node CLI itself.
pub fn cstan_wrapper_script(node: &str, cli_path: &str, site: &FrontEndSite) -> String {
    match front_end_path(site) {
        None => format!(
            "#!/bin/sh\nexec {} \"$@\"\n",
            if site.sea {
                shell_quote(&site.exec_path)
            } else {
                format!("{} {}", shell_quote(node), shell_quote(cli_path))
            }
        ),
        Some(front) => {
            let environment = if site.sea {
                format!("CSTAN_NODE_CLI={}", shell_quote(&site.exec_path))
            } else {
                format!(
                    "CSTAN_NODE_CLI={} CSTAN_NODE={}",
                    shell_quote(cli_path),
                    shell_quote(node)
                )
            };
            format!(
                "#!/bin/sh\n{environment} exec {} \"$@\"\n",
                shell_quote(&front)
            )
        }
    }
}

// ------------------------------------------------------------------------------------------------ branch names

const BRANCH_TYPES: [&str; 9] = [
    "feat", "fix", "docs", "refactor", "perf", "test", "build", "ci", "chore",
];
const MAX_BRANCH: usize = 100;

/// `cleanId`.
fn clean_id(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut in_run = false;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push('-');
            in_run = true;
        }
    }
    let trimmed = out.trim_matches('-');
    let cut: String = trimmed.chars().take(max).collect();
    let cut = cut.trim_end_matches('-');
    if cut.is_empty() {
        "x".into()
    } else {
        cut.to_string()
    }
}

fn branch_type(kind: &str) -> &str {
    if BRANCH_TYPES.contains(&kind) {
        kind
    } else {
        "chore"
    }
}

/// `clip`.
fn clip(name: &str) -> String {
    if name.len() <= MAX_BRANCH {
        name.to_string()
    } else {
        name[..MAX_BRANCH]
            .trim_end_matches(['-', '.', '/'])
            .to_string()
    }
}

/// `workerBranchName`.
pub fn worker_branch_name(kind: &str, task_id: &str, slug: &str) -> String {
    clip(&format!(
        "{}/{}-{}",
        branch_type(kind),
        clean_id(task_id, 40),
        capstan_kernel::integrate::slugify(slug, 40)
    ))
}

/// `adHocBranchPrefix`: the start of `ad_hoc_branch_name`.
pub fn ad_hoc_branch_prefix(agent_id: &str) -> String {
    format!("chore/{}-", clean_id(agent_id, 40))
}

/// `adHocBranchName`.
pub fn ad_hoc_branch_name(agent_id: &str, slug: &str) -> String {
    clip(&format!(
        "chore/{}-{}",
        clean_id(agent_id, 40),
        capstan_kernel::integrate::slugify(slug, 40)
    ))
}

/// `reviewBranchName`.
pub fn review_branch_name(agent_id: &str, target_id: &str) -> String {
    clip(&format!(
        "chore/review-{}-{}",
        clean_id(agent_id, 40),
        clean_id(target_id, 40)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_names_follow_the_conventions() {
        assert_eq!(
            worker_branch_name("feat", "PM-159", "Rust launcher: spawn"),
            "feat/PM-159-rust-launcher-spawn"
        );
        assert_eq!(worker_branch_name("weird", "t/1", "x"), "chore/t-1-x");
        assert_eq!(
            ad_hoc_branch_name("developer-151", "Fix It"),
            "chore/developer-151-fix-it"
        );
        assert!(ad_hoc_branch_name("developer-151", "x")
            .starts_with(&ad_hoc_branch_prefix("developer-151")));
        assert_eq!(
            review_branch_name("verifier-2", "rep_1"),
            "chore/review-verifier-2-rep-1"
        );
        assert!(worker_branch_name("feat", "T-1", &"a".repeat(300)).len() <= 100);
    }

    #[test]
    fn the_wrapper_runs_the_node_cli_without_a_front_end() {
        let site = FrontEndSite {
            env: HashMap::new(),
            exec_path: "/opt/cstan".into(),
            sea: false,
        };
        assert_eq!(
            cstan_wrapper_script("/usr/bin/node", "/opt/capstan/cli.js", &site),
            "#!/bin/sh\nexec '/usr/bin/node' '/opt/capstan/cli.js' \"$@\"\n"
        );
        let sea = FrontEndSite { sea: true, ..site };
        assert_eq!(
            cstan_wrapper_script("/usr/bin/node", "/opt/capstan/cli.js", &sea),
            "#!/bin/sh\nexec '/opt/cstan' \"$@\"\n"
        );
    }
}
