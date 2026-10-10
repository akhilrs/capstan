//! The shapes, names and constants of a resolved capstan.toml (`src/config/types.ts`).

pub const CONFIG_FILE_NAME: &str = "capstan.toml";
pub const DEFAULT_WAIT_TIMEOUT_SECONDS: i64 = 90;
pub const MAX_WAIT_TIMEOUT_SECONDS: i64 = 3600;
pub const DEFAULT_HERDR_SESSION: &str = "default";

pub const ROLE_KINDS: [&str; 4] = ["PM", "Developer", "Verifier", "Supervisor"];
pub const HOST_KINDS: [&str; 3] = ["claude", "codex", "omp"];
pub const PERMISSION_MODES: [&str; 4] = ["default", "acceptEdits", "plan", "auto"];

pub const PLAN_REVIEW_MODES: [&str; 3] = ["high_risk", "always", "never"];
pub const DEFAULT_ARCHITECT_ROLE: &str = "architect";
pub const DEFAULT_ARCHITECT_MAX_PACKAGES: i64 = 8;
pub const MAX_ARCHITECT_PACKAGES: i64 = 20;
pub const DEFAULT_HIGH_RISK_TRIGGERS: [&str; 4] = [
    "schema or migrations",
    "security or auth",
    "public contracts or wire formats",
    "cross-cutting changes",
];

pub const DEFAULT_OPERATOR_ROLE: &str = "operator";
pub const OPERATOR_HARD_MAX_TIMEOUT_SECONDS: i64 = 3600;
pub const MAX_OPERATOR_OUTPUT_TAIL_BYTES: i64 = 12288;
pub const OPERATOR_HARD_MAX_SESSION_MINUTES: i64 = 480;
/// Tool rules the Operator role must deny so it cannot read the state directory, tokens or the key, or hand work to a subagent.
pub const OPERATOR_REQUIRED_DENY: [&str; 8] = [
    "Write",
    "Edit",
    "NotebookEdit",
    "Agent",
    "Task",
    "Read",
    "Glob",
    "Grep",
];

pub const NEXORA_TRACK_MODES: [&str; 3] = ["ask", "always", "never"];
pub const NEXORA_DEFAULT_ACTIONS: [&str; 3] = ["create", "link", "none"];
pub const NEXORA_PROJECT_FILE: &str = ".nexora.toml";

pub const DEFAULT_RESEARCHER_ROLE: &str = "researcher";
pub const DEFAULT_RESEARCHER_OUTPUT_DIR: &str = "docs/research";
pub const DEFAULT_RESEARCHER_USER_AGENT: &str =
    "capstan-researcher/1.0 (research bot; contact: project owner)";

pub const DEFAULT_KEEP_MIGRATION_BACKUPS: i64 = 3;
pub const MAX_KEEP_MIGRATION_BACKUPS: i64 = 50;
pub const DEFAULT_MAX_WORKERS: i64 = 3;
pub const MAX_MAX_WORKERS: i64 = 16;

pub const DEFAULT_WORKTREE_SETUP_TIMEOUT_SECONDS: i64 = 600;
pub const MAX_WORKTREE_SETUP_TIMEOUT_SECONDS: i64 = 3600;
pub const MAX_WORKTREE_SETUP_CHARS: usize = 1000;
pub const MAX_WORKTREE_TEARDOWN_TIMEOUT_SECONDS: i64 = 3600;
pub const DEFAULT_WORKTREE_TEARDOWN_TIMEOUT_SECONDS: i64 = 60;

pub const MAX_PASSED_ENV_NAMES: usize = 32;
/// Passed by default already, or able to change how the agent's shell or loader behaves.
pub const RESERVED_ENV_NAMES: [&str; 29] = [
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LC_ALL",
    "TERM",
    "TMPDIR",
    "IFS",
    "PS0",
    "PS1",
    "PS2",
    "PS4",
    "PROMPT_COMMAND",
    "BASH_ENV",
    "ENV",
    "SHELLOPTS",
    "BASHOPTS",
    "GLOBIGNORE",
    "HISTFILE",
    "CDPATH",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "LD_AUDIT",
    "NODE_OPTIONS",
    "ZDOTDIR",
    "SHELL",
    "PYTHONSTARTUP",
    "PERL5OPT",
];
pub const RESERVED_ENV_PREFIXES: [&str; 4] = ["LD_", "DYLD_", "GIT_", "BASH_"];

/// The tools a PM may not use unless its role sets `deny` itself.
pub const PM_DEFAULT_DENY: [&str; 5] = ["Write", "Edit", "NotebookEdit", "Agent", "Task"];
/// A Supervisor only reads and reports.
pub const SUPERVISOR_DEFAULT_DENY: [&str; 9] = [
    "Write",
    "Edit",
    "NotebookEdit",
    "Agent",
    "Task",
    "Bash(git push)",
    "Bash(git push *)",
    "Bash(herdr *)",
    "Bash(tmux *)",
];

pub const SPAWN_LAYOUTS: [&str; 2] = ["tab", "pane"];
pub const DEFAULT_PM_WIDTH_PERCENT: i64 = 60;
pub const DEFAULT_MIN_PANE_COLUMNS: i64 = 60;
pub const DEFAULT_MIN_PANE_ROWS: i64 = 12;
pub const DEFAULT_SUPERVISION_CHECK_SECONDS: i64 = 300;
pub const DEFAULT_PM_STALE_MINUTES: i64 = 10;

#[derive(Clone, Debug, PartialEq)]
pub struct Notifications {
    pub herdr: bool,
    pub fallback: bool,
    pub pm_stale_minutes: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Timers {
    pub max_deferral_seconds: i64,
    pub max_busy_deferral_seconds: i64,
    pub pm_ack_timeout_seconds: i64,
    pub pm_notify_after_seconds: i64,
    pub notify_interval_seconds: i64,
    pub stall_after_seconds: i64,
    pub worker_ack_timeout_seconds: i64,
    pub finding_check_seconds: i64,
    pub pm_wake_after_seconds: i64,
    pub pm_wake_interval_seconds: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Supervision {
    pub enabled: bool,
    pub check_seconds: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Architect {
    pub enabled: bool,
    pub role: String,
    pub plan_review: String,
    pub reviewer_role: Option<String>,
    pub max_packages: i64,
    pub count_toward_worker_limit: bool,
    pub high_risk_triggers: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Operator {
    pub configured: bool,
    pub enabled: bool,
    pub role: String,
    pub auto_approve: Vec<String>,
    pub auto_approve_prefix: Vec<String>,
    pub timeout_seconds: i64,
    pub max_timeout_seconds: i64,
    pub output_tail_bytes: i64,
    pub proposal_ttl_minutes: i64,
    pub approval_ttl_minutes: i64,
    pub max_pending_proposals: i64,
    pub count_toward_worker_limit: bool,
    pub restart_health_timeout_seconds: i64,
    pub restart_idle_wait_seconds: i64,
    pub session_grant_max_minutes: i64,
    pub full_auto_default_minutes: i64,
    pub full_auto_max_minutes: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Researcher {
    pub configured: bool,
    pub enabled: bool,
    pub role: String,
    pub output_dir: String,
    pub user_agent: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct McpServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Host {
    pub name: String,
    pub kind: String,
    pub command: String,
    pub shell_command_timeout_seconds: i64,
    pub wait_timeout_seconds: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PromptSource {
    pub source: &'static str,
    pub path: Option<String>,
    pub hash: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Role {
    pub name: String,
    pub kind: String,
    pub host: String,
    pub model: Option<String>,
    pub permission_mode: String,
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub hooks: &'static str,
    pub mcp: Vec<McpServer>,
    pub prompt: PromptSource,
    /// The prompt's text, kept for the launcher; it is not part of the printed JSON.
    pub prompt_text: Option<String>,
    pub config_hash: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Limits {
    pub max_workers: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ledger {
    pub keep_migration_backups: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub spawn: &'static str,
    pub pm_width_percent: i64,
    pub min_pane_columns: i64,
    pub min_pane_rows: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PromptRelay {
    pub present: bool,
    pub enabled: bool,
    pub capture_ttl_seconds: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Nexora {
    pub track: &'static str,
    pub default_action: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Worktree {
    pub setup: Option<String>,
    pub setup_timeout_seconds: i64,
    pub teardown: Option<String>,
    pub teardown_timeout_seconds: Option<i64>,
}

/// A resolved capstan.toml (`CapstanConfig`).
#[derive(Clone, Debug, PartialEq)]
pub struct RoleConfig {
    pub project_name: Option<String>,
    pub herdr_session: String,
    pub notifications: Notifications,
    pub timers: Timers,
    pub supervision: Supervision,
    pub architect: Architect,
    pub operator: Operator,
    pub researcher: Researcher,
    pub mcp_servers: Vec<McpServer>,
    pub prompt_relay: PromptRelay,
    pub nexora: Nexora,
    pub limits: Limits,
    pub ledger: Ledger,
    pub layout: Layout,
    pub worktree: Option<Worktree>,
    pub env_pass: Vec<String>,
    pub hosts: Vec<Host>,
    pub roles: Vec<Role>,
    pub warnings: Vec<String>,
}
