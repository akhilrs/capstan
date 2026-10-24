//! The keys every table of a capstan.toml accepts. Each slice is the list its table's `reject_unknown_keys` call prints,
//! so this registry is the parser's own list; `accepted_keys` turns it into the dotted paths the starter must document.
use crate::cache::CACHE_PRESETS;
use crate::types::ROLE_KINDS;

/// The keys the top-level refusal prints.
pub const TOP_LEVEL_KEYS: &[&str] = &[
    "schema_version",
    "project",
    "herdr_session",
    "notifications",
    "timers",
    "supervision",
    "liveness",
    "messages",
    "architect",
    "operator",
    "researcher",
    "mcp_servers",
    "prompt_relay",
    "nexora",
    "defaults",
    "limits",
    "ledger",
    "layout",
    "worktree",
    "daemon",
    "env",
    "hosts",
    "roles",
];
/// Accepted at the top level but left out of the refusal text, which the frozen corpus pins.
pub const UNPRINTED_TOP_LEVEL_KEYS: &[&str] = &["cleanup", "cache", "verify", "review", "design"];

pub const PROJECT_KEYS: &[&str] = &["name"];
pub const NOTIFICATION_KEYS: &[&str] = &["herdr", "fallback", "pm_stale_minutes"];
pub const SUPERVISION_KEYS: &[&str] = &["enabled", "check_seconds"];
pub const LIVENESS_KEYS: &[&str] = &[
    "enabled",
    "check_seconds",
    "confirm_checks",
    "start_grace_seconds",
    "stall_child_max_seconds",
];
pub const MESSAGES_KEYS: &[&str] = &[
    "wake",
    "send_default",
    "info_wake_after_seconds",
    "auto_ack_info",
    "notice_class",
];
pub const LIMITS_KEYS: &[&str] = &["max_workers"];
pub const LEDGER_KEYS: &[&str] = &["keep_migration_backups", "retention_days"];
pub const LAYOUT_KEYS: &[&str] = &[
    "spawn",
    "split",
    "pm_width_percent",
    "min_pane_columns",
    "min_pane_rows",
];
pub const WORKTREE_KEYS: &[&str] = &[
    "setup",
    "setup_timeout_seconds",
    "teardown",
    "teardown_timeout_seconds",
];
pub const DAEMON_KEYS: &[&str] = &["implementation"];
pub const CLEANUP_KEYS: &[&str] = &["branches", "worktrees", "include_unlanded"];
pub const ENV_KEYS: &[&str] = &["pass"];
pub const HOST_KEYS: &[&str] = &[
    "kind",
    "command",
    "shell_command_timeout_seconds",
    "wait_timeout_seconds",
    "model",
    "permission_mode",
];
pub const ROLE_KEYS: &[&str] = &[
    "kind",
    "host",
    "model",
    "permission_mode",
    "allow",
    "deny",
    "hooks",
    "mcp",
    "prompt",
    "prompt_file",
    "max_active",
];
/// The keys of `[defaults]` itself and of each `[defaults.<Kind>]`.
pub const DEFAULTS_ROLE_KEYS: &[&str] = &["model", "permission_mode"];
pub const MCP_SERVER_KEYS: &[&str] = &["command", "args"];
pub const NEXORA_KEYS: &[&str] = &["track", "default_action"];
pub const ARCHITECT_KEYS: &[&str] = &[
    "enabled",
    "role",
    "plan_review",
    "reviewer_role",
    "max_packages",
    "max_review_rounds",
    "plan_amend",
    "count_toward_worker_limit",
    "high_risk_triggers",
];
pub const OPERATOR_KEYS: &[&str] = &[
    "enabled",
    "role",
    "auto_approve",
    "auto_approve_prefix",
    "timeout_seconds",
    "max_timeout_seconds",
    "output_tail_bytes",
    "proposal_ttl_minutes",
    "approval_ttl_minutes",
    "max_pending_proposals",
    "count_toward_worker_limit",
    "restart_health_timeout_seconds",
    "restart_idle_wait_seconds",
    "session_grant_max_minutes",
    "full_auto_default_minutes",
    "full_auto_max_minutes",
    "timed_grant_default_minutes",
    "timed_grant_max_minutes",
    "timed_safe_commands",
    "approval_floor",
    "auto_window_max_minutes",
    "auto_window_tool_prompts",
];
pub const RESEARCHER_KEYS: &[&str] = &["enabled", "role", "output_dir", "user_agent"];
pub const PROMPT_RELAY_KEYS: &[&str] = &["enabled", "capture_ttl_seconds"];
pub const CACHE_KEYS: &[&str] = &[
    "presets",
    "detect",
    "root",
    "scope",
    "max_size_gb",
    "max_age_days",
    "rust",
    "python",
    "node",
    "go",
    "entries",
    "setup_reuse",
];
pub const CACHE_SETUP_REUSE_KEYS: &[&str] = &["key_files", "paths"];
pub const CACHE_ENTRY_KEYS: &[&str] = &["dirs", "set", "requires", "prune", "stop", "max_size_gb"];
pub const CACHE_RUST_KEYS: &[&str] = &["mode", "incremental", "max_size_gb"];
/// The keys of `[cache.python]`, `[cache.node]` and `[cache.go]`.
pub const CACHE_PRESET_KEYS: &[&str] = &["max_size_gb"];
pub const VERIFY_KEYS: &[&str] = &[
    "commands",
    "env",
    "timeout_seconds",
    "max_parallel",
    "auto",
    "require",
    "output_tail_bytes",
    "max_log_bytes",
    "keep_logs",
    "worktree_hooks",
    "reap",
    "scratch_dir",
];
pub const REVIEW_KEYS: &[&str] = &[
    "warm_reviewer",
    "idle_minutes",
    "max_reviews",
    "integration",
];
pub const DESIGN_KEYS: &[&str] = &[
    "require_approval",
    "design_record",
    "design_record_mode",
    "guidance",
    "guidance_rules",
    "guidance_extra",
    "directions_min",
    "directions_max",
    "reference_collection",
];

/// `[timers]`: key, default, minimum, maximum.
pub const TIMER_DEFAULTS: [(&str, i64, i64, i64); 10] = [
    ("max_deferral_seconds", 120, 1, 3600),
    ("max_busy_deferral_seconds", 3600, 60, 86_400),
    ("pm_ack_timeout_seconds", 600, 1, 86_400),
    ("pm_notify_after_seconds", 300, 1, 86_400),
    ("notify_interval_seconds", 600, 1, 86_400),
    ("stall_after_seconds", 900, 1, 86_400),
    ("worker_ack_timeout_seconds", 600, 1, 86_400),
    ("finding_check_seconds", 1800, 60, 86_400),
    ("pm_wake_after_seconds", 20, 0, 3600),
    ("pm_wake_interval_seconds", 120, 10, 3600),
];

/// Accepted but ignored or superseded: the starter may leave these out.
pub const DEPRECATED_PATHS: &[&str] = &[
    "layout.split",
    "daemon.implementation",
    "operator.timed_grant_default_minutes",
    "operator.timed_grant_max_minutes",
    "operator.timed_safe_commands",
];

/// One accepted key path. User-named segments are placeholders: `<name>` (hosts, roles, mcp_servers, cache.entries),
/// `<Kind>` (a role kind), `<kind>` (an overridable notice kind) and `<ENV>` (a verify.env variable).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyPath {
    pub path: String,
    pub deprecated: bool,
}

/// Every key path a capstan.toml accepts, generated from the per-table slices above. Only the table prefixes and the
/// placeholders are written here.
pub fn accepted_keys() -> Vec<KeyPath> {
    let owned = |keys: &[&str]| keys.iter().map(|key| key.to_string()).collect::<Vec<_>>();
    let top_level: Vec<&str> = TOP_LEVEL_KEYS
        .iter()
        .chain(UNPRINTED_TOP_LEVEL_KEYS)
        .copied()
        .collect();
    let timers: Vec<&str> = TIMER_DEFAULTS.iter().map(|entry| entry.0).collect();
    let mut tables: Vec<(String, Vec<String>)> = vec![
        (String::new(), owned(&top_level)),
        ("timers".into(), owned(&timers)),
        ("messages.notice_class".into(), owned(&["<kind>"])),
        ("verify.env".into(), owned(&["<ENV>"])),
    ];
    let mut add = |prefix: &str, keys: &[&str]| tables.push((prefix.into(), owned(keys)));
    add("project", PROJECT_KEYS);
    add("notifications", NOTIFICATION_KEYS);
    add("supervision", SUPERVISION_KEYS);
    add("liveness", LIVENESS_KEYS);
    add("messages", MESSAGES_KEYS);
    add("architect", ARCHITECT_KEYS);
    add("operator", OPERATOR_KEYS);
    add("researcher", RESEARCHER_KEYS);
    add("mcp_servers.<name>", MCP_SERVER_KEYS);
    add("prompt_relay", PROMPT_RELAY_KEYS);
    add("nexora", NEXORA_KEYS);
    add("defaults", DEFAULTS_ROLE_KEYS);
    add("limits", LIMITS_KEYS);
    add("ledger", LEDGER_KEYS);
    add("layout", LAYOUT_KEYS);
    add("worktree", WORKTREE_KEYS);
    add("daemon", DAEMON_KEYS);
    add("cleanup", CLEANUP_KEYS);
    add("env", ENV_KEYS);
    add("hosts.<name>", HOST_KEYS);
    add("roles.<name>", ROLE_KEYS);
    add("cache", CACHE_KEYS);
    add("cache.setup_reuse", CACHE_SETUP_REUSE_KEYS);
    add("cache.entries.<name>", CACHE_ENTRY_KEYS);
    add("verify", VERIFY_KEYS);
    add("review", REVIEW_KEYS);
    add("design", DESIGN_KEYS);
    for kind in ROLE_KINDS {
        add(&format!("defaults.{kind}"), DEFAULTS_ROLE_KEYS);
    }
    for preset in CACHE_PRESETS {
        let keys = if preset == "rust" {
            CACHE_RUST_KEYS
        } else {
            CACHE_PRESET_KEYS
        };
        add(&format!("cache.{preset}"), keys);
    }
    let prefixes: Vec<String> = tables.iter().map(|(prefix, _)| prefix.clone()).collect();
    let mut paths = Vec::new();
    for (prefix, keys) in &tables {
        for key in keys {
            let path = if prefix.is_empty() {
                key.to_string()
            } else {
                format!("{prefix}.{key}")
            };
            // A key that is itself a table (or holds user-named tables) contributes its own keys, not a path.
            let nested = format!("{path}.");
            if !prefixes
                .iter()
                .any(|other| *other == path || other.starts_with(&nested))
            {
                paths.push(path);
            }
        }
    }
    paths
        .into_iter()
        .map(|path| KeyPath {
            deprecated: DEPRECATED_PATHS.contains(&path.as_str()),
            path,
        })
        .collect()
}
