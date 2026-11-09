//! The shapes, names and constants of a resolved capstan.toml (`src/config/types.ts`).

pub const CONFIG_FILE_NAME: &str = "capstan.toml";
/// The optional, untracked file laid over `capstan.toml`: machine and personal settings.
pub const LOCAL_CONFIG_FILE_NAME: &str = "capstan.local.toml";
pub const DEFAULT_WAIT_TIMEOUT_SECONDS: i64 = 90;
pub const MAX_WAIT_TIMEOUT_SECONDS: i64 = 3600;
pub const DEFAULT_HERDR_SESSION: &str = "default";

pub const ROLE_KINDS: [&str; 4] = ["PM", "Developer", "Verifier", "Supervisor"];
pub const HOST_KINDS: [&str; 4] = ["claude", "codex", "omp", "pi"];
pub const PERMISSION_MODES: [&str; 4] = ["default", "acceptEdits", "plan", "auto"];

pub const PLAN_REVIEW_MODES: [&str; 3] = ["high_risk", "always", "never"];
pub const DEFAULT_ARCHITECT_ROLE: &str = "architect";
pub const DEFAULT_ARCHITECT_MAX_PACKAGES: i64 = 8;
pub const MAX_ARCHITECT_PACKAGES: i64 = 20;
pub const DEFAULT_ARCHITECT_MAX_REVIEW_ROUNDS: i64 = 5;
pub const MAX_ARCHITECT_REVIEW_ROUNDS: i64 = 20;
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
/// Read-only commands that run no project code, used when `operator.timed_safe_commands` is absent. Each entry's
/// write or exec options are blocked by the always-approve words and dangerous short letters (see the policy tests).
pub const DEFAULT_TIMED_SAFE_COMMANDS: [&str; 22] = [
    "git status",
    "git log",
    "git diff",
    "git show",
    "git ls-files",
    "git rev-parse",
    "git blame",
    "cstan status",
    "cstan ping",
    "ls",
    "pwd",
    "whoami",
    "uname",
    "df",
    "du",
    "wc",
    "head",
    "tail",
    "cat",
    "grep",
    "uptime",
    "free",
];
/// Commands and predicates an auto-approval window never answers, used when `operator.approval_floor` is absent.
pub const DEFAULT_APPROVAL_FLOOR: [&str; 12] = [
    "git push --force",
    "git push -f",
    "git push --force-with-lease",
    "git push --force-if-includes",
    "git push --delete",
    "git push -d",
    "git push --mirror",
    "git push --prune",
    "@push-rewrite-refspec",
    "@delete-outside-project",
    "@write-outside-project",
    "@controller-restart",
];
/// The predicates an `operator.approval_floor` entry may name besides a command prefix.
pub const APPROVAL_FLOOR_PREDICATES: [&str; 4] = [
    "@push-rewrite-refspec",
    "@delete-outside-project",
    "@write-outside-project",
    "@controller-restart",
];
/// The tail of the warning for a removed timed-grant key; the warning names the key first.
pub const TIMED_GRANT_REMOVED: &str =
    "is ignored: timed grants were replaced by auto-approval windows (operator.auto_window_max_minutes, operator.approval_floor)";
pub const TOOL_PROMPT_DISCLOSURE: &str =
    "Web fetch, MCP and other non-file tool prompts are answered without a floor check.";
pub const FLOOR_DISCLOSURE: &str =
    "The floor is not a sandbox; scripts and shells are approved unchecked during a window.";
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
pub const DEFAULT_LEDGER_RETENTION_DAYS: i64 = 30;
pub const MAX_LEDGER_RETENTION_DAYS: i64 = 3650;
pub const DEFAULT_LEDGER_CACHE_MB: i64 = 64;
pub const MAX_LEDGER_CACHE_MB: i64 = 1024;
pub const DEFAULT_LEDGER_MMAP_MB: i64 = 256;
pub const MAX_LEDGER_MMAP_MB: i64 = 4096;
pub const DEFAULT_BACKUP_MAX_AGE_DAYS: i64 = 14;
pub const MAX_BACKUP_MAX_AGE_DAYS: i64 = 3650;
pub const DEFAULT_STATUS_CACHE_MS: i64 = 2000;
pub const MAX_STATUS_CACHE_MS: i64 = 60_000;
pub const DEFAULT_SNAPSHOT_READS_PER_MINUTE: i64 = 0;
pub const MAX_SNAPSHOT_READS_PER_MINUTE: i64 = 600;
pub const DEFAULT_MAX_CONNECTIONS: i64 = 64;
pub const DEFAULT_CALLER_CONNECTIONS: i64 = 32;
pub const DEFAULT_WAIT_CONNECTIONS: i64 = 32;
pub const DEFAULT_WAIT_PER_CALLER: i64 = 4;
pub const DEFAULT_REPEAT_WINDOW_SECONDS: i64 = 60;
pub const DEFAULT_BUSY_RETRY_MS: i64 = 500;
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
/// The rules of `PM_DEFAULT_DENY` the launcher drops when it installs the memory write guard (which then refuses every
/// Write and Edit outside the PM's memory folder). Any other `deny` is used as written.
pub const PM_GUARDED_DENY: [&str; 2] = ["Write", "Edit"];
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
pub const MAX_MAX_ACTIVE: i64 = 1000;
pub const DEFAULT_PM_STALE_MINUTES: i64 = 10;
pub const DEFAULT_PM_STUCK_MINUTES: i64 = 15;

#[derive(Clone, Debug, PartialEq)]
pub struct Notifications {
    pub herdr: bool,
    pub fallback: bool,
    pub pm_stale_minutes: i64,
    /// Minutes before a PM that waits is called stuck; 0 turns the check off, else 2 to 240.
    pub pm_stuck_minutes: i64,
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

/// The `[liveness]` table: the check that finds an agent whose session has died.
#[derive(Clone, Debug, PartialEq)]
pub struct Liveness {
    pub enabled: bool,
    pub check_seconds: i64,
    pub confirm_checks: i64,
    pub start_grace_seconds: i64,
    /// The longest a live tool process of a silent agent keeps the stall notice away.
    pub stall_child_max_seconds: i64,
}

/// Which messages wake the PM (`messages.wake`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakePolicy {
    Action,
    All,
}

impl WakePolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            WakePolicy::Action => "action",
            WakePolicy::All => "all",
        }
    }
}

/// The class of a message (`messages.send_default`, `messages.notice_class`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageClassName {
    Action,
    Info,
}

impl MessageClassName {
    pub fn as_str(self) -> &'static str {
        match self {
            MessageClassName::Action => "action",
            MessageClassName::Info => "info",
        }
    }
}

pub const DEFAULT_INFO_WAKE_AFTER_SECONDS: i64 = 600;
/// The default body limit of a message sent with `cstan send` (`messages.max_body_bytes`).
pub const DEFAULT_MAX_BODY_BYTES: i64 = 4096;
/// The largest `messages.max_body_bytes`: the daemon's send body limit (Herdr's text limit minus the frame reserve).
pub const MAX_BODY_BYTES_LIMIT: i64 = 15360;
/// The most folders in `roles.<name>.write_dirs`.
pub const MAX_WRITE_DIRS: usize = 8;
pub const MAX_INFO_WAKE_AFTER_SECONDS: i64 = 604_800;

/// Every notice kind of the system with whether `messages.notice_class` may change its class. The names and flags
/// are those of the kernel's `NOTICE_KINDS`.
pub const MESSAGE_NOTICE_KINDS: &[(&str, bool)] = &[
    ("finding_raised", false),
    ("finding_escalated", false),
    ("finding_intervention", false),
    ("finding_resolved", true),
    ("finding_cancelled", true),
    ("cleanup_offer", false),
    ("cleanup_done", true),
    ("verify_finished", true),
    ("report_verified", false),
    ("review_request", false),
    ("review_verdict", false),
    ("plan_notice", false),
    ("plan_handover", false),
    ("architect_changed", true),
    ("plan_review_failed", false),
    ("work_package", false),
    ("work_package_cancelled", false),
    ("delivery_problem", false),
    ("agent_stalled", false),
    ("agent_blocked", false),
    ("agent_lost", false),
    ("input_blocked", false),
    ("integration_conflict", false),
    ("supervision_check", false),
    ("pause_notice", true),
    ("operator_proposal", false),
    ("operator_proposal_update", true),
    ("operator_grant_update", true),
    ("operator_run_update", true),
    ("restart_result", true),
    ("operator_notice", true),
    ("operator_message", false),
    ("auto_approved", true),
    ("auto_answered", true),
    ("approval_window", false),
    ("approval_floor_hit", false),
];

/// The `[messages]` table: how messages are classed and when info wakes the PM.
#[derive(Clone, Debug, PartialEq)]
pub struct Messages {
    pub wake: WakePolicy,
    pub send_default: MessageClassName,
    pub info_wake_after_seconds: i64,
    pub auto_ack_info: bool,
    pub notice_class: std::collections::BTreeMap<String, MessageClassName>,
    /// The most bytes a message body may have (`messages.max_body_bytes`).
    pub max_body_bytes: i64,
}

impl Default for Messages {
    fn default() -> Self {
        Messages {
            wake: WakePolicy::Action,
            send_default: MessageClassName::Action,
            info_wake_after_seconds: DEFAULT_INFO_WAKE_AFTER_SECONDS,
            auto_ack_info: true,
            notice_class: std::collections::BTreeMap::new(),
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Architect {
    pub enabled: bool,
    pub role: String,
    pub plan_review: String,
    pub reviewer_role: Option<String>,
    pub max_packages: i64,
    pub max_review_rounds: i64,
    pub plan_amend: bool,
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
    /// True when capstan.toml writes any timed-grant key (the JSON output prints the three only then).
    pub timed_grant_written: bool,
    pub timed_grant_default_minutes: i64,
    pub timed_grant_max_minutes: i64,
    /// Whole-word command prefixes a timed grant lets run; empty disables timed grants.
    pub timed_safe_commands: Vec<String>,
    /// True when capstan.toml writes any auto-window key (the JSON output prints the three only then).
    pub auto_window_written: bool,
    /// Commands and `@` predicates a window never auto-answers; empty means no floor.
    pub approval_floor: Vec<String>,
    pub auto_window_max_minutes: i64,
    /// Whether a window also answers non-file tool prompts.
    pub auto_window_tool_prompts: bool,
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
    /// The model a role on this host takes when it writes none; `None` when not written.
    pub model: Option<String>,
    /// The permission mode a role on this host takes when it writes none; `None` when not written.
    pub permission_mode: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PromptSource {
    pub source: &'static str,
    pub path: Option<String>,
    pub hash: Option<String>,
}

/// The text a `[design]` key appended after a role's own prompt: which parts ("guidance", "project", "record", in that
/// order) and the SHA-256 of exactly the appended bytes, separators included.
#[derive(Clone, Debug, PartialEq)]
pub struct AppendedGuidance {
    pub parts: Vec<&'static str>,
    pub hash: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Role {
    pub name: String,
    pub kind: String,
    pub host: String,
    pub model: Option<String>,
    /// Where `model` came from ("role", "host", "defaults" or "none"); read by `config check` only, never printed in JSON or hashed.
    pub model_source: &'static str,
    pub permission_mode: String,
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub hooks: &'static str,
    /// PM only: the session may write its own Claude Code memory folder (through the write guard) even though
    /// `Write` and `Edit` are in `deny`. Not printed in JSON and not hashed. `false` on every other kind.
    pub memory: bool,
    pub mcp: Vec<McpServer>,
    pub prompt: PromptSource,
    /// The prompt's text, kept for the launcher; it is not part of the printed JSON.
    pub prompt_text: Option<String>,
    /// What was appended after the role's own prompt text in `prompt_text`; None when nothing was.
    pub guidance: Option<AppendedGuidance>,
    /// The most agents of this role that may be active at once; `None` is unlimited.
    pub max_active: Option<i64>,
    /// Folders (relative to the agent's worktree) a Developer role may write although `Write` and `Edit` are denied;
    /// entries as written, trailing '/' removed. Empty means none.
    pub write_dirs: Vec<String>,
    pub config_hash: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Limits {
    pub max_workers: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ledger {
    pub keep_migration_backups: i64,
    pub retention_days: i64,
    /// Page cache of the ledger connection in MiB; 0 = SQLite's own default.
    pub cache_mb: i64,
    /// Memory-mapped reads of the ledger in MiB; 0 = off.
    pub mmap_mb: i64,
    /// Days a pre-migration backup is kept; 0 = count only.
    pub backup_max_age_days: i64,
}

/// The `[daemon]` performance keys (`implementation` selects nothing and is not kept).
#[derive(Clone, Debug, PartialEq)]
pub struct DaemonTuning {
    /// How long the status and feed caches serve a reading, in ms; 0 turns them off.
    pub status_cache_ms: i64,
    /// False keeps the mutate-every-tick driver path.
    pub driver_read_first: bool,
    /// The most feed snapshot reads per minute; 0 = no limit.
    pub snapshot_reads_per_minute: i64,
    /// The most connections the daemon holds open at once.
    pub max_connections: i64,
    /// The most open non-wait connections one caller may hold.
    pub caller_connections: i64,
    /// The most open wait-class connections in all.
    pub wait_connections: i64,
    /// The most open wait-class connections one caller may hold.
    pub wait_per_caller: i64,
    /// Seconds in which a repeated send is folded into the first; 0 = off.
    pub repeat_window_seconds: i64,
    /// The retry hint, in ms, in a busy answer.
    pub busy_retry_ms: i64,
}

impl Default for DaemonTuning {
    fn default() -> Self {
        DaemonTuning {
            status_cache_ms: DEFAULT_STATUS_CACHE_MS,
            driver_read_first: true,
            snapshot_reads_per_minute: DEFAULT_SNAPSHOT_READS_PER_MINUTE,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            caller_connections: DEFAULT_CALLER_CONNECTIONS,
            wait_connections: DEFAULT_WAIT_CONNECTIONS,
            wait_per_caller: DEFAULT_WAIT_PER_CALLER,
            repeat_window_seconds: DEFAULT_REPEAT_WINDOW_SECONDS,
            busy_retry_ms: DEFAULT_BUSY_RETRY_MS,
        }
    }
}

impl DaemonTuning {
    /// The defaults for a run without a loaded config (tests, transcript replay): the same, but a send is never
    /// folded, so a replay stays exact.
    pub fn without_config() -> Self {
        DaemonTuning {
            repeat_window_seconds: 0,
            ..DaemonTuning::default()
        }
    }
}

/// The `[daemon]` limits of a loaded config, or the no-config defaults. Consumers call this and keep no literal.
pub fn daemon_limits(capstan: Option<&RoleConfig>) -> DaemonTuning {
    match capstan {
        Some(config) => config.daemon.clone(),
        None => DaemonTuning::without_config(),
    }
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CleanupBranches {
    Auto,
    #[default]
    Ask,
    Off,
}

impl CleanupBranches {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Ask => "ask",
            Self::Off => "off",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CleanupWorktrees {
    #[default]
    Release,
    Keep,
}

impl CleanupWorktrees {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::Keep => "keep",
        }
    }
}

/// The `[cleanup]` section; an absent section is the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cleanup {
    pub branches: CleanupBranches,
    pub worktrees: CleanupWorktrees,
    pub include_unlanded: bool,
}

/// A resolved capstan.toml (`CapstanConfig`).
#[derive(Clone, Debug, PartialEq)]
pub struct RoleConfig {
    pub project_name: Option<String>,
    pub herdr_session: String,
    pub notifications: Notifications,
    pub timers: Timers,
    pub supervision: Supervision,
    pub liveness: Liveness,
    pub messages: Messages,
    pub architect: Architect,
    pub operator: Operator,
    pub researcher: Researcher,
    pub mcp_servers: Vec<McpServer>,
    pub prompt_relay: PromptRelay,
    pub nexora: Nexora,
    pub limits: Limits,
    pub ledger: Ledger,
    pub daemon: DaemonTuning,
    pub layout: Layout,
    pub worktree: Option<Worktree>,
    pub cleanup: Cleanup,
    /// The `[remote]` section; the default (merge and push off) when it is absent.
    pub remote: crate::remote::Remote,
    /// The `[impact]` section; the default (first path component, no migrations, no steps) when it is absent.
    pub impact: crate::impact::ImpactConfig,
    /// The `[ntfy]` section; disabled when it is absent.
    pub ntfy: crate::ntfy::Ntfy,
    /// The `[cache]` section; `None` when it is absent.
    pub cache: Option<crate::cache::CacheConfig>,
    /// The `[verify]` section; `None` when it is absent.
    pub verify: Option<crate::verify::Verify>,
    /// The `[design]` section; the default (approval required) when it is absent.
    pub design: crate::design::Design,
    /// The `[review]` section; `Review::default()` when it is absent.
    pub review: crate::review::Review,
    /// The `[agents]` section; `Agents::default()` when it is absent. Never printed.
    pub agents: crate::agents::Agents,
    pub env_pass: Vec<String>,
    pub hosts: Vec<Host>,
    pub roles: Vec<Role>,
    pub warnings: Vec<String>,
}
