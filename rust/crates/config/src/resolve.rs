//! Resolves the parsed document into a `RoleConfig` (`parseCapstanConfig` and the `resolve-*.ts` modules). Every check
//! runs in the order the Node loader runs it, so the first refusal is the same text.
use std::fs;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

use crate::design::GuidanceMode;
use crate::error::{invalid, ConfigError, Result};
use crate::json::{role_hash, sha256};
use crate::keys::*;
use crate::operator_policy::{
    approval_floor_entry_problem, auto_approve_rule_problem, timed_safe_command_problem,
};
use crate::primitives::*;
use crate::researcher_policy::researcher_rule_problems;
use crate::text::{len16, trim};
use crate::types::*;
use crate::value::{Item, Table};

/// `MAX_ROLE_NAME_CHARS` of `src/herdr/naming.ts`: 32 less the project slug, two dashes and four digits.
/// `[daemon] implementation = "node"` and `CSTAN_DAEMON=node`: the Node daemon is gone.
pub const DAEMON_NODE_REMOVED: &str = "daemon.implementation = \"node\" is no longer available: the Node daemon was removed and Rust is the only daemon; remove implementation from the [daemon] table";
/// `[daemon] implementation = "rust"`: accepted, selects nothing.
pub const DAEMON_RUST_WARNING: &str = "[daemon] implementation = \"rust\" is ignored: Rust is the only daemon; remove implementation from the [daemon] table";

/// `CSTAN_DAEMON=node`.
pub const DAEMON_NODE_REMOVED_ENV: &str = "CSTAN_DAEMON=node is no longer available: the Node daemon was removed and Rust is the only daemon; unset CSTAN_DAEMON";
/// `CSTAN_DAEMON=rust`.
pub const DAEMON_RUST_ENV_WARNING: &str =
    "cstan: CSTAN_DAEMON=rust is ignored: Rust is the only daemon; unset CSTAN_DAEMON";

const MAX_ROLE_NAME_CHARS: usize = 32 - 10 - 1 - 1 - 4;
const MAX_MCP_SERVERS: usize = 16;

pub fn parse_config(bytes: &[u8], project_root: &Path) -> Result<RoleConfig> {
    let home = std::env::var("HOME").ok();
    parse_config_with_home(bytes, project_root, home.as_deref())
}

/// `parse_config` with the home directory given, so a test can name one without touching the process environment.
/// An empty home counts as none.
pub fn parse_config_with_home(
    bytes: &[u8],
    project_root: &Path,
    home: Option<&str>,
) -> Result<RoleConfig> {
    let root = decode_document(bytes, CONFIG_FILE_NAME)?;
    resolve_document(&root, project_root, home)
}

/// One TOML file as a table; every refusal names `name`, the file it came from.
fn decode_document(bytes: &[u8], name: &str) -> Result<Table> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return invalid(format!("{name} exceeds {MAX_FILE_BYTES} bytes"));
    }
    let Ok(decoded) = std::str::from_utf8(bytes) else {
        return invalid(format!("{name} is not valid UTF-8"));
    };
    let source = decoded.strip_prefix('\u{feff}').unwrap_or(decoded);
    let source = source.replace("\r\n", "\n");
    // The document is what `smol-toml` makes of the text, or the place it refuses it.
    match crate::smol::parse(&source) {
        Ok(root) => Ok(root),
        Err(position) => invalid(format!(
            "{name} is not valid TOML at line {}, column {}",
            position.line, position.column
        )),
    }
}

/// The configuration of `capstan.toml` with the optional `capstan.local.toml` laid over it (tables merge key by key,
/// arrays and scalars of the local file replace). With no local file this is `parse_config_with_home` plus the `[remote]` rule below. `[remote] merge`
/// and `push` are honoured only from the local file: `true` in the tracked one is refused. A refusal of the merged result
/// names the file the offending key came from.
pub fn parse_layered_config(
    tracked: &[u8],
    local: Option<&[u8]>,
    project_root: &Path,
    home: Option<&str>,
) -> Result<RoleConfig> {
    parse_layered_config_with(tracked, local, project_root, home, true)
}

/// `parse_layered_config` for a caller that could not confirm that git leaves `capstan.local.toml` untracked
/// (`local_confirmed_untracked = false`, git missing or failing): the local file's `[remote]` `merge`, `push`, `push_url`
/// and `merge_branch` are then refused, so the host fails closed for everything that reaches a remote. Other keys of the
/// local file stay honoured.
pub fn parse_layered_config_with(
    tracked: &[u8],
    local: Option<&[u8]>,
    project_root: &Path,
    home: Option<&str>,
    local_confirmed_untracked: bool,
) -> Result<RoleConfig> {
    let base = decode_document(tracked, CONFIG_FILE_NAME)?;
    let over = match local {
        Some(local) => decode_document(local, LOCAL_CONFIG_FILE_NAME)?,
        None => Table::empty(),
    };
    for path in local_only_paths() {
        let Some((table, key)) = path.split_once('.') else {
            continue;
        };
        let Some(Item::Table(section)) = base.get(table) else {
            continue;
        };
        let value = section.get(key);
        let refusal = match (table, key) {
            ("remote", "merge" | "push") if value == Some(&Item::Bool(true)) => {
                Some(tracked_remote_refusal(key))
            }
            ("remote", "merge" | "push") => None,
            ("remote", _) if value.is_some() => Some(tracked_remote_refusal(key)),
            ("ntfy", "enabled") if value == Some(&Item::Bool(true)) => {
                Some(tracked_local_only_refusal("ntfy.enabled = true"))
            }
            ("ntfy", "enabled") => None,
            _ if value.is_some() => Some(tracked_local_only_refusal(&path)),
            _ => None,
        };
        if let Some(text) = refusal {
            return invalid(text);
        }
    }
    if !local_confirmed_untracked {
        if let Some(Item::Table(ntfy)) = over.get("ntfy") {
            let enabled = ntfy.get("enabled") == Some(&Item::Bool(true));
            let key = if enabled {
                Some("enabled")
            } else {
                LOCAL_ONLY_NTFY_VALUES
                    .into_iter()
                    .find(|key| ntfy.get(key).is_some())
            };
            if let Some(key) = key {
                return invalid(format!(
                    "ntfy.{key} is not honoured from {LOCAL_CONFIG_FILE_NAME}: git could not confirm that the file is untracked (is git installed and the project folder readable?)"
                ));
            }
        }
        if let Some(Item::Table(env)) = over.get("env") {
            if env.get("pass").is_some() {
                return invalid(format!(
                    "env.pass is not honoured from {LOCAL_CONFIG_FILE_NAME}: git could not confirm that the file is untracked (is git installed and the project folder readable?)"
                ));
            }
        }
        if let Some(Item::Table(remote)) = over.get("remote") {
            for key in ["merge", "push", "push_url", "merge_branch"] {
                let set = match remote.get(key) {
                    Some(Item::Bool(false)) | None => false,
                    Some(_) => true,
                };
                if set {
                    return invalid(format!(
                        "remote.{key} is not honoured from {LOCAL_CONFIG_FILE_NAME}: git could not confirm that the file is untracked (is git installed and the project folder readable?)"
                    ));
                }
            }
        }
    }
    let merged = base.merged_with(&over);
    resolve_document(&merged, project_root, home).map_err(|error| match local {
        // Without a local file every key is from `capstan.toml`: the refusal stays as it always was.
        Some(_) => name_source(error, &base, &over),
        None => error,
    })
}

/// The accepted key paths honoured only from the untracked local file: `remote.merge`, `remote.push` and the
/// `LOCAL_ONLY_REMOTE_VALUES`, `ntfy.enabled` and the `LOCAL_ONLY_NTFY_VALUES`, and `env.pass`. The trust record
/// leaves these out and the tracked file may not set them.
pub(crate) fn local_only_paths() -> Vec<String> {
    let mut paths: Vec<String> = ["remote.merge", "remote.push"].map(String::from).into();
    paths.extend(
        LOCAL_ONLY_REMOTE_VALUES
            .iter()
            .map(|key| format!("remote.{key}")),
    );
    paths.push("ntfy.enabled".to_string());
    paths.extend(
        LOCAL_ONLY_NTFY_VALUES
            .iter()
            .map(|key| format!("ntfy.{key}")),
    );
    paths.push("env.pass".to_string());
    paths
}

/// The `[remote]` keys that carry a destination and are honoured only from the local file in any form.
const LOCAL_ONLY_REMOTE_VALUES: [&str; 2] = ["push_url", "merge_branch"];

/// The `[ntfy]` keys that pick the server, the topic or a credential, honoured only from the local file in any form.
/// `enabled = true` is local-only as well. Display keys (`events`, `title_prefix`, `timeout_seconds`, `retries`, `event`)
/// stay shared: they choose which events notify and how they look, and send nothing anywhere while no server is set.
const LOCAL_ONLY_NTFY_VALUES: [&str; 8] = [
    "server",
    "topic",
    "user",
    "token_env",
    "token_file",
    "password_env",
    "password_file",
    "ca_file",
];

/// The refusal of a key that sends data off the host or passes host secrets to agents, set in the tracked file.
pub fn tracked_local_only_refusal(key: &str) -> String {
    format!(
        "{key} is not honoured in {CONFIG_FILE_NAME}, which is tracked in git; set it in {LOCAL_CONFIG_FILE_NAME} (untracked), so a pulled commit cannot send host secrets or notifications elsewhere"
    )
}

/// The refusal of `[remote] merge = true`, `push = true`, `push_url` or `merge_branch` in the tracked file.
pub fn tracked_remote_refusal(key: &str) -> String {
    if LOCAL_ONLY_REMOTE_VALUES.contains(&key) {
        return format!(
            "remote.{key} is not honoured in {CONFIG_FILE_NAME}, which is tracked in git; set it in {LOCAL_CONFIG_FILE_NAME} (untracked), so a pulled commit cannot send an approved merge or push elsewhere"
        );
    }
    format!(
        "remote.{key} = true is not honoured in {CONFIG_FILE_NAME}, which is tracked in git; set it in {LOCAL_CONFIG_FILE_NAME} (untracked), so a clone never arrives with it on"
    )
}

/// Appends the file a refusal's key came from. The refusal text starts with the dotted path of the key (or table) it is
/// about; the file is the local one when it holds that key, else the tracked one. A text that names no path found in
/// either file is left as it is. No refusal text carries a value, so naming a file repeats no secret.
fn name_source(error: ConfigError, base: &Table, over: &Table) -> ConfigError {
    let ConfigError::Invalid(text) = error else {
        return error;
    };
    let path: &str = text.split([' ', ':']).next().unwrap_or("");
    let valid_path = !path.is_empty()
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '[' | ']'));
    if !valid_path {
        return ConfigError::Invalid(text);
    }
    let segments: Vec<String> = path
        .split('.')
        .flat_map(|part| {
            let mut pieces = part.split('[');
            let head = pieces.next().unwrap_or("").to_string();
            let mut out = vec![head];
            out.extend(pieces.map(|p| p.trim_end_matches(']').to_string()));
            out
        })
        .filter(|segment| !segment.is_empty())
        .collect();
    let allowed: Option<Vec<&str>> = text
        .split_once("unknown key(s); allowed: ")
        .map(|(_, list)| list.split(", ").collect());
    let holds = |table: &Table| -> bool {
        let mut item = Item::Table(table.clone());
        for segment in &segments {
            let next = match &item {
                Item::Table(t) => t.get(segment).cloned(),
                Item::Array(a) => segment
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| a.get(i).cloned()),
                _ => None,
            };
            match next {
                Some(found) => item = found,
                None => return false,
            }
        }
        match (&allowed, &item) {
            (Some(allowed), Item::Table(t)) => t.keys().any(|k| !allowed.contains(&k)),
            _ => true,
        }
    };
    let file = if holds(over) {
        LOCAL_CONFIG_FILE_NAME
    } else if holds(base) {
        CONFIG_FILE_NAME
    } else {
        return ConfigError::Invalid(text);
    };
    ConfigError::Invalid(format!("{text} (from {file})"))
}

fn resolve_document(root: &Table, project_root: &Path, home: Option<&str>) -> Result<RoleConfig> {
    // `cleanup`, `remote`, `impact`, `cache`, `verify`, `review` and `design` are accepted but kept out of the list the refusal prints, which the frozen corpus pins.
    let known = Table::from_members(
        root.entries()
            .filter(|(key, _)| !UNPRINTED_TOP_LEVEL_KEYS.contains(key))
            .map(|(key, item)| (key.to_string(), item.clone()))
            .collect(),
    );
    reject_unknown_keys(&known, TOP_LEVEL_KEYS, "top level")?;
    if root.get("schema_version") != Some(&Item::Int(1)) {
        return invalid("schema_version must be the integer 1");
    }

    let project = optional_table(root.get("project"), "project")?;
    reject_unknown_keys(project, PROJECT_KEYS, "project")?;
    let project_name = optional_string(project.get("name"), "project.name", 256)?;
    if let Some(name) = project_name {
        guard_credential_shape(name, "project.name")?;
    }

    let herdr_session = optional_string(root.get("herdr_session"), "herdr_session", 64)?
        .unwrap_or(DEFAULT_HERDR_SESSION);
    if !session_ok(herdr_session) {
        return invalid(format!("herdr_session must match {SESSION_PATTERN}"));
    }

    let notification_table = optional_table(root.get("notifications"), "notifications")?;
    reject_unknown_keys(notification_table, NOTIFICATION_KEYS, "notifications")?;
    let notifications = Notifications {
        herdr: optional_boolean(notification_table.get("herdr"), "notifications.herdr", true)?,
        fallback: optional_boolean(
            notification_table.get("fallback"),
            "notifications.fallback",
            true,
        )?,
        pm_stale_minutes: optional_integer(
            notification_table.get("pm_stale_minutes"),
            "notifications.pm_stale_minutes",
            1,
            1440,
            DEFAULT_PM_STALE_MINUTES,
        )?,
        pm_stuck_minutes: optional_integer(
            notification_table.get("pm_stuck_minutes"),
            "notifications.pm_stuck_minutes",
            0,
            240,
            DEFAULT_PM_STUCK_MINUTES,
        )?,
    };
    if notifications.pm_stuck_minutes == 1 {
        return invalid("notifications.pm_stuck_minutes must be 0 (off) or 2 to 240");
    }
    if !notifications.herdr && !notifications.fallback {
        return invalid("notifications.herdr and notifications.fallback must not both be false");
    }

    let timer_table = optional_table(root.get("timers"), "timers")?;
    let timer_keys: Vec<&str> = TIMER_DEFAULTS.iter().map(|entry| entry.0).collect();
    reject_unknown_keys(timer_table, &timer_keys, "timers")?;
    let mut timer_values = [0i64; 10];
    for (slot, (key, fallback, min, max)) in timer_values.iter_mut().zip(TIMER_DEFAULTS) {
        *slot = optional_integer(
            timer_table.get(key),
            &format!("timers.{key}"),
            min,
            max,
            fallback,
        )?;
    }
    let timers = Timers {
        max_deferral_seconds: timer_values[0],
        max_busy_deferral_seconds: timer_values[1],
        pm_ack_timeout_seconds: timer_values[2],
        pm_notify_after_seconds: timer_values[3],
        notify_interval_seconds: timer_values[4],
        stall_after_seconds: timer_values[5],
        worker_ack_timeout_seconds: timer_values[6],
        finding_check_seconds: timer_values[7],
        pm_wake_after_seconds: timer_values[8],
        pm_wake_interval_seconds: timer_values[9],
    };

    let supervision_table = optional_table(root.get("supervision"), "supervision")?;
    reject_unknown_keys(supervision_table, SUPERVISION_KEYS, "supervision")?;
    let supervision = Supervision {
        enabled: optional_boolean(
            supervision_table.get("enabled"),
            "supervision.enabled",
            true,
        )?,
        check_seconds: optional_integer(
            supervision_table.get("check_seconds"),
            "supervision.check_seconds",
            60,
            3600,
            DEFAULT_SUPERVISION_CHECK_SECONDS,
        )?,
    };

    let liveness_table = optional_table(root.get("liveness"), "liveness")?;
    reject_unknown_keys(liveness_table, LIVENESS_KEYS, "liveness")?;
    let liveness = Liveness {
        enabled: optional_boolean(liveness_table.get("enabled"), "liveness.enabled", true)?,
        check_seconds: optional_integer(
            liveness_table.get("check_seconds"),
            "liveness.check_seconds",
            10,
            3600,
            30,
        )?,
        confirm_checks: optional_integer(
            liveness_table.get("confirm_checks"),
            "liveness.confirm_checks",
            2,
            100,
            3,
        )?,
        start_grace_seconds: optional_integer(
            liveness_table.get("start_grace_seconds"),
            "liveness.start_grace_seconds",
            30,
            3600,
            120,
        )?,
        stall_child_max_seconds: optional_integer(
            liveness_table.get("stall_child_max_seconds"),
            "liveness.stall_child_max_seconds",
            60,
            86_400,
            3600,
        )?,
    };

    let messages = resolve_messages(optional_table(root.get("messages"), "messages")?)?;

    let limit_table = optional_table(root.get("limits"), "limits")?;
    reject_unknown_keys(limit_table, LIMITS_KEYS, "limits")?;
    let limits = Limits {
        max_workers: optional_integer(
            limit_table.get("max_workers"),
            "limits.max_workers",
            1,
            MAX_MAX_WORKERS,
            DEFAULT_MAX_WORKERS,
        )?,
    };

    let ledger_table = optional_table(root.get("ledger"), "ledger")?;
    reject_unknown_keys(ledger_table, LEDGER_KEYS, "ledger")?;
    let ledger = Ledger {
        keep_migration_backups: optional_integer(
            ledger_table.get("keep_migration_backups"),
            "ledger.keep_migration_backups",
            1,
            MAX_KEEP_MIGRATION_BACKUPS,
            DEFAULT_KEEP_MIGRATION_BACKUPS,
        )?,
        retention_days: optional_integer(
            ledger_table.get("retention_days"),
            "ledger.retention_days",
            0,
            MAX_LEDGER_RETENTION_DAYS,
            DEFAULT_LEDGER_RETENTION_DAYS,
        )?,
        cache_mb: optional_integer(
            ledger_table.get("cache_mb"),
            "ledger.cache_mb",
            0,
            MAX_LEDGER_CACHE_MB,
            DEFAULT_LEDGER_CACHE_MB,
        )?,
        mmap_mb: optional_integer(
            ledger_table.get("mmap_mb"),
            "ledger.mmap_mb",
            0,
            MAX_LEDGER_MMAP_MB,
            DEFAULT_LEDGER_MMAP_MB,
        )?,
        backup_max_age_days: optional_integer(
            ledger_table.get("backup_max_age_days"),
            "ledger.backup_max_age_days",
            0,
            MAX_BACKUP_MAX_AGE_DAYS,
            DEFAULT_BACKUP_MAX_AGE_DAYS,
        )?,
    };

    let layout_table = optional_table(root.get("layout"), "layout")?;
    reject_unknown_keys(layout_table, LAYOUT_KEYS, "layout")?;
    let mut warnings: Vec<String> = Vec::new();
    if let Some(split) = layout_table.get("split") {
        enum_value(Some(split), "layout.split", &["auto", "right", "down"])?;
        warnings.push(
            "layout.split is ignored: the PM pane keeps the left layout.pm_width_percent of its tab and worker panes stack in the column on its right"
                .into(),
        );
    }
    let layout = Layout {
        spawn: match layout_table.get("spawn") {
            None => "tab",
            spawn => enum_value(spawn, "layout.spawn", &SPAWN_LAYOUTS)?,
        },
        pm_width_percent: optional_integer(
            layout_table.get("pm_width_percent"),
            "layout.pm_width_percent",
            30,
            80,
            DEFAULT_PM_WIDTH_PERCENT,
        )?,
        min_pane_columns: optional_integer(
            layout_table.get("min_pane_columns"),
            "layout.min_pane_columns",
            1,
            500,
            DEFAULT_MIN_PANE_COLUMNS,
        )?,
        min_pane_rows: optional_integer(
            layout_table.get("min_pane_rows"),
            "layout.min_pane_rows",
            1,
            200,
            DEFAULT_MIN_PANE_ROWS,
        )?,
    };

    let worktree = resolve_worktree(optional_table(root.get("worktree"), "worktree")?)?;

    let daemon_table = optional_table(root.get("daemon"), "daemon")?;
    reject_unknown_keys(daemon_table, DAEMON_KEYS, "daemon")?;
    // Rust is the only daemon: `[daemon]` selects nothing. "rust" is a no-op that warns, "node" is an error.
    if root.get("daemon").is_some() {
        match enum_value(
            daemon_table.get("implementation"),
            "daemon.implementation",
            &["node", "rust"],
        ) {
            Ok("node") => return invalid(DAEMON_NODE_REMOVED),
            Ok(_) => warnings.push(DAEMON_RUST_WARNING.to_string()),
            Err(error) if daemon_table.get("implementation").is_some() => return Err(error),
            Err(_) => {}
        }
    }

    let daemon = DaemonTuning {
        status_cache_ms: optional_integer(
            daemon_table.get("status_cache_ms"),
            "daemon.status_cache_ms",
            0,
            MAX_STATUS_CACHE_MS,
            DEFAULT_STATUS_CACHE_MS,
        )?,
        driver_read_first: optional_boolean(
            daemon_table.get("driver_read_first"),
            "daemon.driver_read_first",
            true,
        )?,
        snapshot_reads_per_minute: optional_integer(
            daemon_table.get("snapshot_reads_per_minute"),
            "daemon.snapshot_reads_per_minute",
            0,
            MAX_SNAPSHOT_READS_PER_MINUTE,
            DEFAULT_SNAPSHOT_READS_PER_MINUTE,
        )?,
        max_connections: optional_integer(
            daemon_table.get("max_connections"),
            "daemon.max_connections",
            8,
            1024,
            DEFAULT_MAX_CONNECTIONS,
        )?,
        caller_connections: optional_integer(
            daemon_table.get("caller_connections"),
            "daemon.caller_connections",
            1,
            1024,
            DEFAULT_CALLER_CONNECTIONS,
        )?,
        wait_connections: optional_integer(
            daemon_table.get("wait_connections"),
            "daemon.wait_connections",
            1,
            1024,
            DEFAULT_WAIT_CONNECTIONS,
        )?,
        wait_per_caller: optional_integer(
            daemon_table.get("wait_per_caller"),
            "daemon.wait_per_caller",
            1,
            1024,
            DEFAULT_WAIT_PER_CALLER,
        )?,
        repeat_window_seconds: optional_integer(
            daemon_table.get("repeat_window_seconds"),
            "daemon.repeat_window_seconds",
            0,
            600,
            DEFAULT_REPEAT_WINDOW_SECONDS,
        )?,
        busy_retry_ms: optional_integer(
            daemon_table.get("busy_retry_ms"),
            "daemon.busy_retry_ms",
            50,
            10_000,
            DEFAULT_BUSY_RETRY_MS,
        )?,
    };
    for (key, value, limit_key, limit) in [
        (
            "caller_connections",
            daemon.caller_connections,
            "max_connections",
            daemon.max_connections,
        ),
        (
            "wait_connections",
            daemon.wait_connections,
            "max_connections",
            daemon.max_connections,
        ),
        (
            "wait_per_caller",
            daemon.wait_per_caller,
            "wait_connections",
            daemon.wait_connections,
        ),
    ] {
        if value > limit {
            return invalid(format!(
                "daemon.{key} must not be above daemon.{limit_key} ({limit})"
            ));
        }
    }

    let cleanup = resolve_cleanup(optional_table(root.get("cleanup"), "cleanup")?)?;
    let remote = crate::remote::resolve_remote(optional_table(root.get("remote"), "remote")?)?;

    let impact = crate::impact::resolve_impact(optional_table(root.get("impact"), "impact")?)?;

    let env_table = optional_table(root.get("env"), "env")?;
    reject_unknown_keys(env_table, ENV_KEYS, "env")?;
    let env_pass = passed_environment_names(env_table.get("pass"))?;
    let ntfy = crate::ntfy::parse_ntfy(root.get("ntfy"), &env_pass)?;
    let cache = crate::cache::resolve_cache(root.get("cache"), project_root, &env_pass)?;

    let hosts = resolve_hosts(required_table(root.get("hosts"), "hosts")?)?;
    let mcp_servers = resolve_mcp_servers(
        optional_table(root.get("mcp_servers"), "mcp_servers")?,
        &mut warnings,
    )?;
    let role_table = required_table(root.get("roles"), "roles")?;
    let home = home.filter(|home| !home.is_empty());
    let design =
        crate::design::resolve_design(root.get("design"), project_root, home, &mut warnings)?;
    let defaults = resolve_defaults(optional_table(root.get("defaults"), "defaults")?)?;
    let roles = resolve_roles(
        role_table,
        &hosts,
        project_root,
        &defaults,
        &mcp_servers,
        &design,
        &mut warnings,
    )?;
    if design.customised
        && !roles
            .iter()
            .any(|role| role.kind == "Developer" && role.name == "designer")
    {
        warnings.push(
            "design: no Developer role named designer, so the guidance and record keys change no prompt"
                .to_string(),
        );
    }
    if roles.iter().filter(|role| role.kind == "PM").count() != 1 {
        return invalid("exactly one role must have kind PM");
    }
    let architect = resolve_architect(
        optional_table(root.get("architect"), "architect")?,
        &roles,
        &hosts,
    )?;
    let operator = resolve_operator(
        optional_table(root.get("operator"), "operator")?,
        root.get("operator").is_some(),
        &roles,
        &hosts,
        &architect,
        &mut warnings,
    )?;
    let researcher = resolve_researcher(
        optional_table(root.get("researcher"), "researcher")?,
        root.get("researcher").is_some(),
        &roles,
        &hosts,
        &architect,
        &operator,
    )?;
    let prompt_relay = resolve_prompt_relay(
        optional_table(root.get("prompt_relay"), "prompt_relay")?,
        root.get("prompt_relay").is_some(),
    )?;
    let nexora = resolve_nexora(optional_table(root.get("nexora"), "nexora")?)?;
    if nexora.track != "never" && !project_root.join(NEXORA_PROJECT_FILE).exists() {
        warnings.push(format!(
            "nexora.track is \"{}\" but {NEXORA_PROJECT_FILE} is missing from the project root, so the PM will not track work in Nexora; set track = \"never\" in [nexora] to silence this",
            nexora.track
        ));
    }

    let verify = crate::verify::resolve_verify(root.get("verify"))?;
    let review = crate::review::resolve_review(root.get("review"))?;

    Ok(RoleConfig {
        project_name: project_name.map(str::to_string),
        herdr_session: herdr_session.to_string(),
        notifications,
        timers,
        supervision,
        liveness,
        messages,
        architect,
        operator,
        researcher,
        mcp_servers,
        prompt_relay,
        nexora,
        limits,
        ledger,
        daemon,
        layout,
        worktree,
        cleanup,
        remote,
        impact,
        ntfy,
        cache,
        verify,
        design,
        review,
        env_pass,
        hosts,
        roles,
        warnings,
    })
}

fn cleanup_choice<'a>(table: &'a Table, key: &str, allowed: &[&str]) -> Result<Option<&'a str>> {
    let at = format!("cleanup.{key}");
    let Some(text) = optional_string(table.get(key), &at, 16)? else {
        return Ok(None);
    };
    if allowed.contains(&text) {
        Ok(Some(text))
    } else {
        invalid(format!("{at} must be one of {}", allowed.join(", ")))
    }
}

fn resolve_cleanup(table: &Table) -> Result<Cleanup> {
    reject_unknown_keys(table, CLEANUP_KEYS, "cleanup")?;
    let branches = match cleanup_choice(table, "branches", &["auto", "ask", "off"])? {
        Some("auto") => CleanupBranches::Auto,
        Some("off") => CleanupBranches::Off,
        _ => CleanupBranches::Ask,
    };
    let worktrees = match cleanup_choice(table, "worktrees", &["release", "keep"])? {
        Some("keep") => CleanupWorktrees::Keep,
        _ => CleanupWorktrees::Release,
    };
    let include_unlanded = optional_boolean(
        table.get("include_unlanded"),
        "cleanup.include_unlanded",
        false,
    )?;
    Ok(Cleanup {
        branches,
        worktrees,
        include_unlanded,
    })
}

fn resolve_worktree(table: &Table) -> Result<Option<Worktree>> {
    reject_unknown_keys(table, WORKTREE_KEYS, "worktree")?;
    if table.get("setup").is_none() && table.get("setup_timeout_seconds").is_some() {
        return invalid("worktree.setup_timeout_seconds is set without worktree.setup");
    }
    if table.get("teardown").is_none() && table.get("teardown_timeout_seconds").is_some() {
        return invalid("worktree.teardown_timeout_seconds is set without worktree.teardown");
    }
    if table.get("setup").is_none() && table.get("teardown").is_none() {
        return Ok(None);
    }
    let mut setup = None;
    if table.get("setup").is_some() {
        let text = required_string(
            table.get("setup"),
            "worktree.setup",
            MAX_WORKTREE_SETUP_CHARS,
            false,
        )?;
        guard_credential_shape(text, "worktree.setup")?;
        setup = Some(text.to_string());
    }
    let mut teardown = None;
    if table.get("teardown").is_some() {
        let text = required_string(
            table.get("teardown"),
            "worktree.teardown",
            MAX_WORKTREE_SETUP_CHARS,
            false,
        )?;
        guard_credential_shape(text, "worktree.teardown")?;
        teardown = Some(text.to_string());
    }
    let setup_timeout_seconds = optional_integer(
        table.get("setup_timeout_seconds"),
        "worktree.setup_timeout_seconds",
        1,
        MAX_WORKTREE_SETUP_TIMEOUT_SECONDS,
        DEFAULT_WORKTREE_SETUP_TIMEOUT_SECONDS,
    )?;
    let teardown_timeout_seconds = if teardown.is_some() {
        Some(optional_integer(
            table.get("teardown_timeout_seconds"),
            "worktree.teardown_timeout_seconds",
            1,
            MAX_WORKTREE_TEARDOWN_TIMEOUT_SECONDS,
            DEFAULT_WORKTREE_TEARDOWN_TIMEOUT_SECONDS,
        )?)
    } else {
        None
    };
    Ok(Some(Worktree {
        setup,
        setup_timeout_seconds,
        teardown,
        teardown_timeout_seconds,
    }))
}

/// `[mcp_servers.<name>]`: a stdio MCP server a role may use.
fn resolve_mcp_servers(table: &Table, warnings: &mut Vec<String>) -> Result<Vec<McpServer>> {
    if table.len() > MAX_MCP_SERVERS {
        return invalid(format!("mcp_servers exceeds {MAX_MCP_SERVERS} servers"));
    }
    let mut servers = Vec::new();
    for (name, item) in table.entries() {
        if !mcp_server_name_ok(name) {
            return invalid(format!(
                "mcp_servers has a server name that does not match {MCP_SERVER_NAME_PATTERN}"
            ));
        }
        let at = format!("mcp_servers.{name}");
        let server = required_table(Some(item), &at)?;
        reject_unknown_keys(server, MCP_SERVER_KEYS, &at)?;
        let command = required_string(server.get("command"), &format!("{at}.command"), 200, false)?;
        if command.starts_with('-') {
            return invalid(format!("{at}.command must not start with a dash"));
        }
        if !printable_line(command) {
            return invalid(format!(
                "{at}.command must be one line of printable ASCII text"
            ));
        }
        guard_credential_shape(command, &format!("{at}.command"))?;
        let mut args: Vec<String> = Vec::new();
        if let Some(value) = server.get("args") {
            let Item::Array(entries) = value else {
                return invalid(format!("{at}.args must be an array of strings"));
            };
            if entries.len() > MAX_LIST_ENTRIES {
                return invalid(format!("{at}.args exceeds {MAX_LIST_ENTRIES} entries"));
            }
            for (index, entry) in entries.iter().enumerate() {
                let entry_at = format!("{at}.args[{index}]");
                let text = required_string(Some(entry), &entry_at, MAX_ENTRY_CHARS, false)?;
                guard_credential_shape(text, &entry_at)?;
                args.push(text.to_string());
            }
        }
        for arg in &args {
            if arg.ends_with("@latest") {
                warnings.push(format!(
                    "{at}.args names {arg}, an unpinned package; pin an exact version"
                ));
            }
        }
        // The first non-flag argument of a package runner is the package; it needs an @version.
        if is_package_runner(command) {
            if let Some(package) = args.iter().find(|arg| !arg.starts_with('-')) {
                if !package.ends_with("@latest") && !has_version(package) {
                    warnings.push(format!(
                        "{at}.args names {package}, a package with no @version; pin an exact version"
                    ));
                }
            }
        }
        servers.push(McpServer {
            name: name.to_string(),
            command: command.to_string(),
            args,
        });
    }
    Ok(servers)
}

/// `/^(?:.*\/)?(?:npx|bunx|pnpx)$/`.
fn is_package_runner(command: &str) -> bool {
    ["npx", "bunx", "pnpx"].iter().any(|runner| {
        command == *runner
            || command
                .strip_suffix(runner)
                .is_some_and(|head| head.ends_with('/'))
    })
}

/// `/.@[^@/]+$/`.
fn has_version(package: &str) -> bool {
    match package.rfind('@') {
        Some(at) if at > 0 => {
            let version = &package[at + 1..];
            !version.is_empty() && !version.contains('/')
        }
        _ => false,
    }
}

fn resolve_role_mcp(
    value: Option<&Item>,
    at: &str,
    servers: &[McpServer],
) -> Result<Vec<McpServer>> {
    let names = string_list(value, at, MAX_LIST_ENTRIES)?;
    let mut chosen: Vec<McpServer> = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let Some(server) = servers.iter().find(|candidate| candidate.name == *name) else {
            return invalid(format!(
                "{at}[{index}] does not name a configured [mcp_servers] table"
            ));
        };
        if chosen.iter().any(|seen| seen.name == *name) {
            return invalid(format!("{at}[{index}] repeats {name}"));
        }
        chosen.push(server.clone());
    }
    Ok(chosen)
}

fn resolve_hosts(table: &Table) -> Result<Vec<Host>> {
    let names = validated_names(table, "hosts", "host")?;
    if names.is_empty() {
        return invalid("hosts must define at least one host");
    }
    let mut hosts = Vec::new();
    for name in names {
        let at = format!("hosts.{name}");
        let host = required_table(table.get(name), &at)?;
        reject_unknown_keys(host, HOST_KEYS, &at)?;
        let kind = enum_value(host.get("kind"), &format!("{at}.kind"), &HOST_KINDS)?;
        let command =
            optional_string(host.get("command"), &format!("{at}.command"), 200)?.unwrap_or(kind);
        if !is_executable_path(command) {
            return invalid(format!("{at}.command must be an executable name or path"));
        }
        guard_credential_shape(command, &format!("{at}.command"))?;
        let shell_command_timeout_seconds = optional_integer(
            host.get("shell_command_timeout_seconds"),
            &format!("{at}.shell_command_timeout_seconds"),
            1,
            3600,
            120,
        )?;
        let wait_timeout_seconds = optional_integer(
            host.get("wait_timeout_seconds"),
            &format!("{at}.wait_timeout_seconds"),
            1,
            MAX_WAIT_TIMEOUT_SECONDS,
            DEFAULT_WAIT_TIMEOUT_SECONDS,
        )?;
        if wait_timeout_seconds >= shell_command_timeout_seconds {
            return invalid(format!(
                "{at}.wait_timeout_seconds ({wait_timeout_seconds}) must be below {at}.shell_command_timeout_seconds ({shell_command_timeout_seconds})"
            ));
        }
        let model = parse_model(host.get("model"), &format!("{at}.model"))?;
        let permission_mode = parse_permission_mode(
            host.get("permission_mode"),
            &format!("{at}.permission_mode"),
        )?;
        if let Some(mode) = permission_mode {
            if kind != "claude" && mode != "acceptEdits" && mode != "auto" {
                return invalid(format!(
                    "{at}.permission_mode must be acceptEdits or auto on a {kind} host, which runs unattended with full access"
                ));
            }
        }
        hosts.push(Host {
            name: name.to_string(),
            kind: kind.to_string(),
            command: command.to_string(),
            shell_command_timeout_seconds,
            wait_timeout_seconds,
            model,
            permission_mode: permission_mode.map(str::to_string),
        });
    }
    Ok(hosts)
}

/// Codex, OMP and pi run unattended with full access, so a role that needs a read-only PM or a mode this controller cannot enforce there is refused.
fn reject_unenforceable(at: &str, host: &Host, kind: &str, permission_mode: &str) -> Result<()> {
    if kind == "PM" || kind == "Supervisor" {
        return invalid(format!(
            "{at}: a {kind} role is read-only by design and only a claude host can enforce that; host {} is {}",
            host.name, host.kind
        ));
    }
    if permission_mode != "acceptEdits" && permission_mode != "auto" {
        return invalid(format!(
            "{at}.permission_mode must be acceptEdits or auto on host {} ({}), which runs unattended with full access (set it on the role or in [defaults])",
            host.name, host.kind
        ));
    }
    Ok(())
}

#[derive(Default, Clone)]
struct RoleDefaults {
    model: Option<String>,
    permission_mode: Option<&'static str>,
}

struct Defaults {
    all: RoleDefaults,
    by_kind: Vec<(&'static str, RoleDefaults)>,
}

fn parse_model(value: Option<&Item>, at: &str) -> Result<Option<String>> {
    let model = optional_string(value, at, 100)?;
    if let Some(model) = model {
        guard_credential_shape(model, at)?;
        if model.starts_with('-') {
            return invalid(format!("{at} must not start with a dash"));
        }
    }
    Ok(model.map(str::to_string))
}

fn parse_permission_mode(value: Option<&Item>, at: &str) -> Result<Option<&'static str>> {
    match value {
        None => Ok(None),
        some => enum_value(some, at, &PERMISSION_MODES).map(Some),
    }
}

/// `[defaults]` and `[defaults.<kind>]`: the model and permission mode a role takes when it sets none of its own.
fn resolve_defaults(table: &Table) -> Result<Defaults> {
    let mut allowed = DEFAULTS_ROLE_KEYS.to_vec();
    allowed.extend(ROLE_KINDS);
    reject_unknown_keys(table, &allowed, "defaults")?;
    let read = |source: &Table, at: &str| -> Result<RoleDefaults> {
        reject_unknown_keys(source, DEFAULTS_ROLE_KEYS, at)?;
        Ok(RoleDefaults {
            model: parse_model(source.get("model"), &format!("{at}.model"))?,
            permission_mode: parse_permission_mode(
                source.get("permission_mode"),
                &format!("{at}.permission_mode"),
            )?,
        })
    };
    let rest = Table::from_members(
        table
            .entries()
            .filter(|(key, _)| !ROLE_KINDS.contains(key))
            .map(|(key, item)| (key.to_string(), item.clone()))
            .collect(),
    );
    let all = read(&rest, "defaults")?;
    let mut by_kind = Vec::new();
    for kind in ROLE_KINDS {
        let at = format!("defaults.{kind}");
        by_kind.push((kind, read(optional_table(table.get(kind), &at)?, &at)?));
    }
    Ok(Defaults { all, by_kind })
}

fn resolve_roles(
    table: &Table,
    hosts: &[Host],
    project_root: &Path,
    defaults: &Defaults,
    mcp_servers: &[McpServer],
    design: &crate::design::Design,
    warnings: &mut Vec<String>,
) -> Result<Vec<Role>> {
    let names = validated_names(table, "roles", "role")?;
    if names.is_empty() {
        return invalid("roles must define at least one role");
    }
    let mut roles = Vec::new();
    for name in names {
        let at = format!("roles.{name}");
        let role = required_table(table.get(name), &at)?;
        reject_unknown_keys(role, ROLE_KEYS, &at)?;
        let kind = enum_value(role.get("kind"), &format!("{at}.kind"), &ROLE_KINDS)?;
        if len16(name) > MAX_ROLE_NAME_CHARS {
            return invalid(format!(
                "{at}: the role name is longer than {MAX_ROLE_NAME_CHARS} characters, so an agent name with the project prefix would not fit Herdr's 32"
            ));
        }
        let host_name = required_string(role.get("host"), &format!("{at}.host"), 32, false)?;
        let Some(host) = hosts.iter().find(|host| host.name == host_name) else {
            return invalid(format!("{at}.host does not name a configured host"));
        };
        let kind_defaults = &defaults
            .by_kind
            .iter()
            .find(|(candidate, _)| *candidate == kind)
            .expect("every kind has defaults")
            .1;
        let (model, model_source) = match role.get("model") {
            None => match (&host.model, &kind_defaults.model, &defaults.all.model) {
                (Some(model), _, _) => (Some(model.clone()), "host"),
                (None, Some(model), _) | (None, None, Some(model)) => {
                    (Some(model.clone()), "defaults")
                }
                (None, None, None) => (None, "none"),
            },
            some => (parse_model(some, &format!("{at}.model"))?, "role"),
        };
        let permission_mode: &str = match role.get("permission_mode") {
            None => host
                .permission_mode
                .as_deref()
                .or(kind_defaults.permission_mode)
                .or(defaults.all.permission_mode)
                .unwrap_or(if host.kind == "claude" {
                    "default"
                } else {
                    "auto"
                }),
            some => parse_permission_mode(some, &format!("{at}.permission_mode"))?
                .expect("a present value resolves to a mode"),
        };
        let allow = string_list(role.get("allow"), &format!("{at}.allow"), MAX_LIST_ENTRIES)?;
        let deny = match (role.get("deny"), kind) {
            (None, "PM") => PM_DEFAULT_DENY
                .iter()
                .map(|rule| rule.to_string())
                .collect(),
            (None, "Supervisor") => SUPERVISOR_DEFAULT_DENY
                .iter()
                .map(|rule| rule.to_string())
                .collect(),
            (value, _) => string_list(value, &format!("{at}.deny"), MAX_DENY_ENTRIES)?,
        };
        let hooks = match role.get("hooks") {
            None => "off",
            some => enum_value(some, &format!("{at}.hooks"), &["off", "inherit"])?,
        };
        let memory_wanted = match (role.get("memory"), kind) {
            (Some(_), kind) if kind != "PM" => {
                return invalid(format!("{at}.memory is only allowed on a PM role"))
            }
            (value, kind) => {
                kind == "PM" && optional_boolean(value, &format!("{at}.memory"), true)?
            }
        };
        // The allowance needs the guard hook and the absence of a Write/Edit deny: hooks = "off" written by the
        // project turns it off, and so does a deny of the project's own that lists Write or Edit (the user's deny
        // wins, also over an explicit memory = true).
        let memory = memory_wanted
            && host.kind == "claude"
            && role.get("hooks").is_none_or(|_| hooks != "off")
            && (role.get("deny").is_none()
                || !deny
                    .iter()
                    .any(|rule| PM_GUARDED_DENY.contains(&rule.as_str())));
        let mcp = resolve_role_mcp(role.get("mcp"), &format!("{at}.mcp"), mcp_servers)?;
        let max_active = match (role.get("max_active"), kind) {
            (Some(_), "PM") => {
                return invalid(format!(
                    "{at}.max_active is not allowed on a PM role: a project has exactly one PM"
                ))
            }
            (None, "Supervisor") => Some(1),
            (None, _) => None,
            (value, _) => Some(optional_integer(
                value,
                &format!("{at}.max_active"),
                1,
                MAX_MAX_ACTIVE,
                1,
            )?),
        };
        let (prompt, prompt_text, guidance) =
            resolve_prompt(role, &at, name, kind, project_root, design, warnings)?;
        let (mut allow, mut deny, mut mcp) = (allow, deny, mcp);
        if host.kind != "claude" {
            reject_unenforceable(&at, host, kind, permission_mode)?;
            let label = format!("host {} ({})", host.name, host.kind);
            if !allow.is_empty() || !deny.is_empty() {
                warnings.push(format!(
                    "{at}: allow and deny are Claude Code tool rules that {label} cannot enforce; they are ignored"
                ));
            }
            if !mcp.is_empty() {
                warnings.push(format!(
                    "{at}.mcp is a Claude Code option that {label} cannot use; it is ignored"
                ));
            }
            if hooks == "inherit" {
                warnings.push(format!(
                    "{at}.hooks = \"inherit\" is a Claude Code option that {label} cannot use; it is ignored"
                ));
            }
            allow = Vec::new();
            deny = Vec::new();
            mcp = Vec::new();
        }
        let mut resolved = Role {
            name: name.to_string(),
            kind: kind.to_string(),
            host: host.name.clone(),
            model,
            model_source,
            permission_mode: permission_mode.to_string(),
            allow,
            deny,
            hooks,
            memory,
            mcp,
            prompt,
            prompt_text,
            guidance,
            max_active,
            config_hash: String::new(),
        };
        resolved.config_hash = role_hash(&resolved, host);
        roles.push(resolved);
    }
    Ok(roles)
}

fn resolve_prompt(
    role: &Table,
    at: &str,
    name: &str,
    kind: &str,
    project_root: &Path,
    design: &crate::design::Design,
    warnings: &mut Vec<String>,
) -> Result<(PromptSource, Option<String>, Option<AppendedGuidance>)> {
    let designer = name == "designer" && kind == "Developer";
    if role.get("prompt").is_some() && role.get("prompt_file").is_some() {
        return invalid(format!("{at} sets both prompt and prompt_file"));
    }
    if role.get("prompt").is_some() {
        let text = required_string(
            role.get("prompt"),
            &format!("{at}.prompt"),
            MAX_PROMPT_CHARS,
            true,
        )?;
        guard_credential_shape(text, &format!("{at}.prompt"))?;
        let source = PromptSource {
            source: "inline",
            path: None,
            hash: Some(sha256(text)),
        };
        return Ok(own_prompt(
            designer,
            design,
            source,
            text.to_string(),
            warnings,
        ));
    }
    if role.get("prompt_file").is_none() {
        if designer {
            return Ok(builtin_designer_prompt(design));
        }
        return Ok((
            PromptSource {
                source: "none",
                path: None,
                hash: None,
            },
            None,
            None,
        ));
    }
    let relative = required_string(
        role.get("prompt_file"),
        &format!("{at}.prompt_file"),
        200,
        false,
    )?;
    if relative.starts_with('/') {
        return invalid(format!("{at}.prompt_file must be relative to the project"));
    }
    let resolved = fs::canonicalize(project_root)
        .and_then(|root| fs::canonicalize(resolve_path(&root, relative)).map(|file| (root, file)));
    let Ok((real_root, real_file)) = resolved else {
        // A stale prompt_file must not stop the controller when the role has a built-in prompt to fall back on.
        if designer {
            warnings.push(format!(
                "{at}.prompt_file {relative} does not exist; using the built-in {name} prompt (remove the prompt_file line)"
            ));
            return Ok(builtin_designer_prompt(design));
        }
        return invalid(format!("{at}.prompt_file does not exist"));
    };
    let (Some(root_text), Some(file_text)) = (real_root.to_str(), real_file.to_str()) else {
        return Err(ConfigError::Unreadable(
            "a project path that is not UTF-8".into(),
        ));
    };
    if !file_text.starts_with(&format!("{root_text}/")) {
        return invalid(format!("{at}.prompt_file must stay inside the project"));
    }
    let too_big = || {
        ConfigError::Invalid(format!(
            "{at}.prompt_file must be a regular file of at most {MAX_FILE_BYTES} bytes"
        ))
    };
    let unreadable = |_: std::io::Error| {
        ConfigError::Invalid(format!("{at}.prompt_file cannot be read as a regular file"))
    };
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&real_file)
        .map_err(unreadable)?;
    let metadata = file.metadata().map_err(unreadable)?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return Err(too_big());
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(unreadable)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(too_big());
    }
    let Ok(decoded) = std::str::from_utf8(&bytes) else {
        return invalid(format!("{at}.prompt_file is not valid UTF-8"));
    };
    let text = decoded
        .strip_prefix('\u{feff}')
        .unwrap_or(decoded)
        .replace("\r\n", "\n");
    if trim(&text).is_empty() {
        return invalid(format!("{at}.prompt_file must not be empty"));
    }
    assert_safe_text(&text, &format!("{at}.prompt_file"), true)?;
    guard_credential_shape(&text, &format!("{at}.prompt_file"))?;
    let source = PromptSource {
        source: "file",
        path: Some(file_text.to_string()),
        hash: Some(sha256(&text)),
    };
    Ok(own_prompt(designer, design, source, text, warnings))
}

type ResolvedPrompt = (PromptSource, Option<String>, Option<AppendedGuidance>);

/// What follows a designer's prompt text: the pieces in order, joined by "\n", and the names of the pieces.
fn appended(pieces: Vec<(&'static str, &str)>) -> (String, Option<AppendedGuidance>) {
    if pieces.is_empty() {
        return (String::new(), None);
    }
    let mut text = String::new();
    let mut parts = Vec::new();
    for (part, piece) in pieces {
        text.push('\n');
        text.push_str(piece);
        parts.push(part);
    }
    let hash = sha256(&text);
    (text, Some(AppendedGuidance { parts, hash }))
}

/// The built-in designer prompt: designer.md and the guidance, or designer.md alone with `guidance = "off"`, then
/// the block the keys ask for. `prompt.hash` covers the built-in text without the block.
fn builtin_designer_prompt(design: &crate::design::Design) -> ResolvedPrompt {
    let (base, part) = if design.guidance == GuidanceMode::Off {
        (crate::prompts::DESIGNER_PROMPT, "record")
    } else {
        (crate::prompts::DESIGNER_BUILTIN_PROMPT, "project")
    };
    let block = crate::design_guidance::project_block(design);
    let (tail, guidance) = appended(block.iter().map(|block| (part, block.as_str())).collect());
    let source = PromptSource {
        source: "builtin",
        path: None,
        hash: Some(sha256(base)),
    };
    (source, Some(format!("{base}{tail}")), guidance)
}

/// A prompt the role wrote itself. A designer gets the guidance with `guidance = "on"`, and the block its keys ask
/// for; `prompt.hash` stays the hash of its own text. Other roles are untouched.
fn own_prompt(
    designer: bool,
    design: &crate::design::Design,
    source: PromptSource,
    text: String,
    warnings: &mut Vec<String>,
) -> ResolvedPrompt {
    if !designer {
        return (source, Some(text), None);
    }
    let mut pieces: Vec<(&'static str, &str)> = Vec::new();
    let block;
    if design.guidance == GuidanceMode::On {
        pieces.push(("guidance", crate::prompts::DESIGN_GUIDANCE));
        block = crate::design_guidance::project_block(design);
        if let Some(block) = &block {
            pieces.push(("project", block));
        }
    } else {
        if design.guidance == GuidanceMode::Builtin {
            warn_ignored_keys(design, warnings);
        }
        block = crate::design_guidance::record_block(design);
        if let Some(block) = &block {
            pieces.push(("record", block));
        }
    }
    let (tail, guidance) = appended(pieces);
    (source, Some(format!("{text}{tail}")), guidance)
}

/// `guidance = "builtin"` leaves a designer with its own prompt without the guidance, so the keys that shape the
/// guidance change nothing.
fn warn_ignored_keys(design: &crate::design::Design, warnings: &mut Vec<String>) {
    let base = crate::design::Design::default();
    let keys: Vec<&str> = [
        (
            "directions_min",
            design.directions_min != base.directions_min,
        ),
        (
            "directions_max",
            design.directions_max != base.directions_max,
        ),
        (
            "guidance_rules",
            design.guidance_rules != base.guidance_rules,
        ),
        (
            "guidance_extra",
            design.guidance_extra != base.guidance_extra,
        ),
        (
            "reference_collection",
            design.reference_collection != base.reference_collection,
        ),
    ]
    .into_iter()
    .filter_map(|(key, set)| set.then_some(key))
    .collect();
    if !keys.is_empty() {
        warnings.push(format!(
            "design: guidance = \"builtin\" and roles.designer has its own prompt, so {} have no effect; set guidance = \"on\"",
            keys.join(", ")
        ));
    }
}

/// `path.resolve(base, relative)` for an absolute `base`: `.` and `..` are folded without touching the file system.
fn resolve_path(base: &Path, relative: &str) -> PathBuf {
    let mut parts: Vec<std::ffi::OsString> = base
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_os_string()),
            _ => None,
        })
        .collect();
    for part in relative.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other.into()),
        }
    }
    let mut path = PathBuf::from("/");
    path.extend(parts);
    path
}

fn resolve_nexora(table: &Table) -> Result<Nexora> {
    reject_unknown_keys(table, NEXORA_KEYS, "nexora")?;
    let track = match table.get("track") {
        None => "ask",
        some => enum_value(some, "nexora.track", &NEXORA_TRACK_MODES)?,
    };
    let default_action = match table.get("default_action") {
        None => "create",
        some => enum_value(some, "nexora.default_action", &NEXORA_DEFAULT_ACTIONS)?,
    };
    if track == "always" && default_action == "none" {
        return invalid(
            "nexora.track = \"always\" contradicts nexora.default_action = \"none\"; use track = \"never\" to turn tracking off",
        );
    }
    Ok(Nexora {
        track,
        default_action,
    })
}

fn resolve_architect(table: &Table, roles: &[Role], hosts: &[Host]) -> Result<Architect> {
    reject_unknown_keys(table, ARCHITECT_KEYS, "architect")?;
    let role = optional_string(table.get("role"), "architect.role", 32)?
        .unwrap_or(DEFAULT_ARCHITECT_ROLE)
        .to_string();
    if !name_ok(&role) {
        return invalid(format!("architect.role must match {NAME_PATTERN}"));
    }
    let reviewer_role = optional_string(table.get("reviewer_role"), "architect.reviewer_role", 32)?
        .map(str::to_string);
    if let Some(reviewer) = &reviewer_role {
        if !name_ok(reviewer) {
            return invalid(format!("architect.reviewer_role must match {NAME_PATTERN}"));
        }
    }
    let architect = Architect {
        enabled: optional_boolean(table.get("enabled"), "architect.enabled", false)?,
        role: role.clone(),
        plan_review: match table.get("plan_review") {
            None => "high_risk",
            some => enum_value(some, "architect.plan_review", &PLAN_REVIEW_MODES)?,
        }
        .to_string(),
        reviewer_role: reviewer_role.clone(),
        max_packages: optional_integer(
            table.get("max_packages"),
            "architect.max_packages",
            1,
            MAX_ARCHITECT_PACKAGES,
            DEFAULT_ARCHITECT_MAX_PACKAGES,
        )?,
        max_review_rounds: optional_integer(
            table.get("max_review_rounds"),
            "architect.max_review_rounds",
            1,
            MAX_ARCHITECT_REVIEW_ROUNDS,
            DEFAULT_ARCHITECT_MAX_REVIEW_ROUNDS,
        )?,
        plan_amend: optional_boolean(table.get("plan_amend"), "architect.plan_amend", true)?,
        count_toward_worker_limit: optional_boolean(
            table.get("count_toward_worker_limit"),
            "architect.count_toward_worker_limit",
            false,
        )?,
        high_risk_triggers: match table.get("high_risk_triggers") {
            None => DEFAULT_HIGH_RISK_TRIGGERS
                .iter()
                .map(|t| t.to_string())
                .collect(),
            some => string_list(some, "architect.high_risk_triggers", MAX_LIST_ENTRIES)?,
        },
    };
    if !architect.enabled {
        return Ok(architect);
    }
    let Some(architect_role) = roles.iter().find(|candidate| candidate.name == role) else {
        return invalid(format!(
            "architect.role \"{role}\" does not name a configured role"
        ));
    };
    if architect_role.kind != "Developer" {
        return invalid(format!(
            "architect.role \"{role}\" must be a Developer role; roles.{role}.kind is {}",
            architect_role.kind
        ));
    }
    let host = hosts.iter().find(|host| host.name == architect_role.host);
    if host.map(|h| h.kind.as_str()) != Some("claude") {
        return invalid(format!(
            "roles.{role}: the architect role needs a claude host so its deny rules are enforced; host {} is {}",
            architect_role.host,
            host.map_or("unknown", |h| h.kind.as_str())
        ));
    }
    if let Some(reviewer_name) = &reviewer_role {
        let Some(reviewer) = roles
            .iter()
            .find(|candidate| candidate.name == *reviewer_name)
        else {
            return invalid(format!(
                "architect.reviewer_role \"{reviewer_name}\" does not name a configured role"
            ));
        };
        if reviewer.kind != "Verifier" {
            return invalid(format!(
                "architect.reviewer_role \"{reviewer_name}\" must be a Verifier role; roles.{reviewer_name}.kind is {}",
                reviewer.kind
            ));
        }
    }
    Ok(architect)
}

fn output_dir_segment(part: &str) -> bool {
    !part.is_empty()
        && part
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// `OUTPUT_DIR_PATTERN` and the check for `.` and `..` segments.
fn output_dir_ok(dir: &str) -> bool {
    dir.split('/')
        .all(|part| output_dir_segment(part) && part != "." && part != "..")
}

fn resolve_researcher(
    table: &Table,
    configured: bool,
    roles: &[Role],
    hosts: &[Host],
    architect: &Architect,
    operator: &Operator,
) -> Result<Researcher> {
    reject_unknown_keys(table, RESEARCHER_KEYS, "researcher")?;
    let role = optional_string(table.get("role"), "researcher.role", 32)?
        .unwrap_or(DEFAULT_RESEARCHER_ROLE)
        .to_string();
    if !name_ok(&role) {
        return invalid(format!("researcher.role must match {NAME_PATTERN}"));
    }
    let output_dir = optional_string(table.get("output_dir"), "researcher.output_dir", 200)?
        .unwrap_or(DEFAULT_RESEARCHER_OUTPUT_DIR);
    if !output_dir_ok(output_dir) {
        return invalid(
            "researcher.output_dir must be a repo-relative directory with no '..', no leading '/' and only letters, digits, '.', '_', '-' and '/'",
        );
    }
    let user_agent = optional_string(table.get("user_agent"), "researcher.user_agent", 200)?
        .unwrap_or(DEFAULT_RESEARCHER_USER_AGENT);
    if !printable_line(user_agent) || user_agent.contains(['"', '\'', '`']) {
        return invalid(
            "researcher.user_agent must be one printable line with no quote characters",
        );
    }
    guard_credential_shape(user_agent, "researcher.user_agent")?;
    let researcher = Researcher {
        configured,
        enabled: optional_boolean(table.get("enabled"), "researcher.enabled", false)?,
        role: role.clone(),
        output_dir: output_dir.to_string(),
        user_agent: user_agent.to_string(),
    };
    if !researcher.enabled {
        return Ok(researcher);
    }
    if architect.enabled && role == architect.role {
        return invalid(format!(
            "researcher.role \"{role}\" must differ from architect.role"
        ));
    }
    if operator.configured && role == operator.role {
        return invalid(format!(
            "researcher.role \"{role}\" must differ from operator.role"
        ));
    }
    let Some(researcher_role) = roles.iter().find(|candidate| candidate.name == role) else {
        return invalid(format!(
            "researcher.role \"{role}\" does not name a configured role"
        ));
    };
    let host = hosts.iter().find(|host| host.name == researcher_role.host);
    if host.map(|h| h.kind.as_str()) != Some("claude") {
        return invalid(format!(
            "roles.{role}: the researcher role needs a claude host so its allow and deny rules are enforced; host {} is {}",
            researcher_role.host,
            host.map_or("unknown", |h| h.kind.as_str())
        ));
    }
    if let Some(problem) = researcher_rule_problems(researcher_role, &researcher)
        .into_iter()
        .next()
    {
        return invalid(problem);
    }
    Ok(researcher)
}

fn operator_rules(value: Option<&Item>, at: &str) -> Result<Vec<String>> {
    let rules = string_list(value, at, MAX_LIST_ENTRIES)?;
    for (index, rule) in rules.iter().enumerate() {
        if let Some(problem) = auto_approve_rule_problem(rule) {
            return invalid(format!("{at}[{index}] {problem}"));
        }
    }
    Ok(rules)
}

/// `operator.timed_safe_commands`: absent is the built-in list, `[]` disables timed grants.
fn timed_safe_commands(value: Option<&Item>) -> Result<Vec<String>> {
    let at = "operator.timed_safe_commands";
    if value.is_none() {
        return Ok(DEFAULT_TIMED_SAFE_COMMANDS
            .iter()
            .map(|entry| (*entry).to_string())
            .collect());
    }
    let entries = string_list(value, at, MAX_LIST_ENTRIES)?;
    for (index, entry) in entries.iter().enumerate() {
        if let Some(problem) = timed_safe_command_problem(entry) {
            return invalid(format!("{at}[{index}] {problem}"));
        }
    }
    Ok(entries)
}

/// `operator.approval_floor`: absent is the built-in floor; `[]` is accepted with a warning.
fn approval_floor(value: Option<&Item>, warnings: &mut Vec<String>) -> Result<Vec<String>> {
    let at = "operator.approval_floor";
    if value.is_none() {
        return Ok(DEFAULT_APPROVAL_FLOOR
            .iter()
            .map(|entry| (*entry).to_string())
            .collect());
    }
    let entries = string_list(value, at, MAX_LIST_ENTRIES)?;
    for (index, entry) in entries.iter().enumerate() {
        if let Some(problem) = approval_floor_entry_problem(entry) {
            return invalid(format!("{at}[{index}] {problem}"));
        }
    }
    if entries.is_empty() {
        warnings.push(format!(
            "{at} is empty: an auto-approval window will answer every file-tool prompt without a floor check"
        ));
    }
    Ok(entries)
}

fn resolve_prompt_relay(table: &Table, present: bool) -> Result<PromptRelay> {
    reject_unknown_keys(table, PROMPT_RELAY_KEYS, "prompt_relay")?;
    Ok(PromptRelay {
        present,
        enabled: optional_boolean(table.get("enabled"), "prompt_relay.enabled", false)?,
        capture_ttl_seconds: optional_integer(
            table.get("capture_ttl_seconds"),
            "prompt_relay.capture_ttl_seconds",
            60,
            3600,
            600,
        )?,
    })
}

/// `/^Bash\(cstan[ :][^()`$;&|<>\\\n]*\)$/`.
fn operator_allow_rule(rule: &str) -> bool {
    let Some(rest) = rule.strip_prefix("Bash(cstan") else {
        return false;
    };
    let Some(rest) = rest.strip_prefix([' ', ':']) else {
        return false;
    };
    rest.strip_suffix(')').is_some_and(|middle| {
        !middle.chars().any(|c| {
            matches!(
                c,
                '(' | ')' | '`' | '$' | ';' | '&' | '|' | '<' | '>' | '\\' | '\n'
            )
        })
    })
}

fn resolve_operator(
    table: &Table,
    configured: bool,
    roles: &[Role],
    hosts: &[Host],
    architect: &Architect,
    warnings: &mut Vec<String>,
) -> Result<Operator> {
    reject_unknown_keys(table, OPERATOR_KEYS, "operator")?;
    let role = optional_string(table.get("role"), "operator.role", 32)?
        .unwrap_or(DEFAULT_OPERATOR_ROLE)
        .to_string();
    if !name_ok(&role) {
        return invalid(format!("operator.role must match {NAME_PATTERN}"));
    }
    let int = |key: &str, min: i64, max: i64, fallback: i64| {
        optional_integer(
            table.get(key),
            &format!("operator.{key}"),
            min,
            max,
            fallback,
        )
    };
    let operator = Operator {
        configured,
        enabled: optional_boolean(table.get("enabled"), "operator.enabled", false)?,
        role: role.clone(),
        auto_approve: operator_rules(table.get("auto_approve"), "operator.auto_approve")?,
        auto_approve_prefix: operator_rules(
            table.get("auto_approve_prefix"),
            "operator.auto_approve_prefix",
        )?,
        timeout_seconds: int("timeout_seconds", 1, OPERATOR_HARD_MAX_TIMEOUT_SECONDS, 300)?,
        max_timeout_seconds: int(
            "max_timeout_seconds",
            1,
            OPERATOR_HARD_MAX_TIMEOUT_SECONDS,
            1800,
        )?,
        output_tail_bytes: int("output_tail_bytes", 1, MAX_OPERATOR_OUTPUT_TAIL_BYTES, 8192)?,
        proposal_ttl_minutes: int("proposal_ttl_minutes", 1, 1440, 60)?,
        approval_ttl_minutes: int("approval_ttl_minutes", 1, 120, 10)?,
        max_pending_proposals: int("max_pending_proposals", 1, 50, 5)?,
        count_toward_worker_limit: optional_boolean(
            table.get("count_toward_worker_limit"),
            "operator.count_toward_worker_limit",
            false,
        )?,
        restart_health_timeout_seconds: int("restart_health_timeout_seconds", 1, 600, 60)?,
        restart_idle_wait_seconds: int("restart_idle_wait_seconds", 0, 3600, 120)?,
        session_grant_max_minutes: int(
            "session_grant_max_minutes",
            1,
            OPERATOR_HARD_MAX_SESSION_MINUTES,
            60,
        )?,
        full_auto_default_minutes: int(
            "full_auto_default_minutes",
            1,
            OPERATOR_HARD_MAX_SESSION_MINUTES,
            30,
        )?,
        full_auto_max_minutes: int(
            "full_auto_max_minutes",
            1,
            OPERATOR_HARD_MAX_SESSION_MINUTES,
            120,
        )?,
        timed_grant_written: [
            "timed_grant_default_minutes",
            "timed_grant_max_minutes",
            "timed_safe_commands",
        ]
        .iter()
        .any(|key| table.get(key).is_some()),
        timed_grant_default_minutes: int(
            "timed_grant_default_minutes",
            1,
            OPERATOR_HARD_MAX_SESSION_MINUTES,
            60,
        )?,
        timed_grant_max_minutes: int(
            "timed_grant_max_minutes",
            1,
            OPERATOR_HARD_MAX_SESSION_MINUTES,
            240,
        )?,
        timed_safe_commands: timed_safe_commands(table.get("timed_safe_commands"))?,
        auto_window_written: [
            "approval_floor",
            "auto_window_max_minutes",
            "auto_window_tool_prompts",
        ]
        .iter()
        .any(|key| table.get(key).is_some()),
        approval_floor: approval_floor(table.get("approval_floor"), warnings)?,
        auto_window_max_minutes: match table.get("auto_window_max_minutes") {
            Some(_) => int(
                "auto_window_max_minutes",
                1,
                OPERATOR_HARD_MAX_SESSION_MINUTES,
                240,
            )?,
            None => int(
                "timed_grant_max_minutes",
                1,
                OPERATOR_HARD_MAX_SESSION_MINUTES,
                240,
            )?,
        },
        auto_window_tool_prompts: optional_boolean(
            table.get("auto_window_tool_prompts"),
            "operator.auto_window_tool_prompts",
            true,
        )?,
    };
    for key in [
        "timed_grant_default_minutes",
        "timed_grant_max_minutes",
        "timed_safe_commands",
    ] {
        if table.get(key).is_some() {
            warnings.push(format!("operator.{key} {TIMED_GRANT_REMOVED}"));
        }
    }
    if operator.full_auto_default_minutes > operator.full_auto_max_minutes {
        return invalid(format!(
            "operator.full_auto_default_minutes ({}) must not exceed operator.full_auto_max_minutes ({})",
            operator.full_auto_default_minutes, operator.full_auto_max_minutes
        ));
    }
    if operator.timeout_seconds > operator.max_timeout_seconds {
        return invalid(format!(
            "operator.timeout_seconds ({}) must not exceed operator.max_timeout_seconds ({})",
            operator.timeout_seconds, operator.max_timeout_seconds
        ));
    }
    if !operator.enabled {
        return Ok(operator);
    }
    if role == architect.role {
        return invalid(format!(
            "operator.role \"{role}\" must differ from architect.role"
        ));
    }
    let Some(operator_role) = roles.iter().find(|candidate| candidate.name == role) else {
        return invalid(format!(
            "operator.role \"{role}\" does not name a configured role"
        ));
    };
    if operator_role.kind != "Developer" {
        return invalid(format!(
            "operator.role \"{role}\" must be a Developer role; roles.{role}.kind is {}",
            operator_role.kind
        ));
    }
    let host = hosts.iter().find(|host| host.name == operator_role.host);
    if host.map(|h| h.kind.as_str()) != Some("claude") {
        return invalid(format!(
            "roles.{role}: the operator role needs a claude host so its deny rules are enforced; host {} is {}",
            operator_role.host,
            host.map_or("unknown", |h| h.kind.as_str())
        ));
    }
    for (index, rule) in operator_role.allow.iter().enumerate() {
        if !operator_allow_rule(rule) {
            return invalid(format!(
                "roles.{role}.allow[{index}] must be a Bash(cstan ...) rule; the operator role may not allow {rule}"
            ));
        }
    }
    for required in OPERATOR_REQUIRED_DENY {
        if !operator_role.deny.iter().any(|rule| rule == required) {
            return invalid(format!(
                "roles.{role}.deny must include {required}: the operator role may not read or edit project files or start subagents"
            ));
        }
    }
    Ok(operator)
}

/// The `[messages]` table.
fn resolve_messages(table: &Table) -> Result<Messages> {
    reject_unknown_keys(table, MESSAGES_KEYS, "messages")?;
    let wake = match table.get("wake") {
        None => WakePolicy::Action,
        some => match enum_value(some, "messages.wake", &["action", "all"])? {
            "all" => WakePolicy::All,
            _ => WakePolicy::Action,
        },
    };
    let send_default = match table.get("send_default") {
        None => MessageClassName::Action,
        some => class_name(enum_value(some, "messages.send_default", &CLASS_NAMES)?),
    };
    let info_wake_after_seconds = optional_integer(
        table.get("info_wake_after_seconds"),
        "messages.info_wake_after_seconds",
        0,
        MAX_INFO_WAKE_AFTER_SECONDS,
        DEFAULT_INFO_WAKE_AFTER_SECONDS,
    )?;
    let auto_ack_info =
        optional_boolean(table.get("auto_ack_info"), "messages.auto_ack_info", true)?;
    let mut notice_class = std::collections::BTreeMap::new();
    for (kind, value) in
        optional_table(table.get("notice_class"), "messages.notice_class")?.entries()
    {
        let at = format!("messages.notice_class.{kind}");
        let Some((_, overridable)) = MESSAGE_NOTICE_KINDS.iter().find(|(name, _)| *name == kind)
        else {
            return invalid(format!("{at} is not a known notice kind"));
        };
        if !overridable {
            return invalid(format!(
                "{at} cannot be changed: this notice kind is always an action message"
            ));
        }
        notice_class.insert(
            kind.to_string(),
            class_name(enum_value(Some(value), &at, &CLASS_NAMES)?),
        );
    }
    Ok(Messages {
        wake,
        send_default,
        info_wake_after_seconds,
        auto_ack_info,
        notice_class,
    })
}

const CLASS_NAMES: [&str; 2] = ["action", "info"];

fn class_name(text: &str) -> MessageClassName {
    if text == "info" {
        MessageClassName::Info
    } else {
        MessageClassName::Action
    }
}

#[cfg(test)]
mod trust_parity_tests {
    use super::*;
    use crate::trust::accepted_path_of;
    use crate::trust_classes::is_checked;

    #[test]
    fn local_only_list_matches_the_consts() {
        let mut want: Vec<String> = ["remote.merge", "remote.push", "ntfy.enabled", "env.pass"]
            .map(String::from)
            .into();
        want.extend(
            LOCAL_ONLY_REMOTE_VALUES
                .iter()
                .map(|k| format!("remote.{k}")),
        );
        want.extend(LOCAL_ONLY_NTFY_VALUES.iter().map(|k| format!("ntfy.{k}")));
        let mut have = local_only_paths();
        want.sort();
        have.sort();
        assert_eq!(have, want);
        assert_eq!(have.len(), 14);
    }

    #[test]
    fn every_local_only_path_set_in_the_tracked_file_is_refused() {
        for path in local_only_paths() {
            let (table, key) = path.split_once('.').unwrap();
            let value = if matches!(
                path.as_str(),
                "remote.merge" | "remote.push" | "ntfy.enabled"
            ) {
                "true"
            } else {
                "\"x\""
            };
            let text = format!("[{table}]\n{key} = {value}\n");
            let result = parse_layered_config(text.as_bytes(), None, Path::new("/tmp"), None);
            let error = result.expect_err(&path).to_string();
            assert!(error.contains("not honoured in"), "{path}: {error}");
            assert!(!is_checked(&path), "{path}");
        }
    }

    #[test]
    fn similar_names_are_still_checked() {
        for real in [
            "hosts.max_active.command",
            "roles.max_active.model",
            "roles.max_active",
        ] {
            // A real path no accepted path fits is checked as well (see `trust::build_subset`).
            let checked = accepted_path_of(real).is_none_or(|accepted| is_checked(&accepted));
            assert!(checked, "{real}");
        }
    }
}
