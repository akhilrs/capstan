//! Resolves the parsed document into a `RoleConfig` (`parseCapstanConfig` and the `resolve-*.ts` modules). Every check
//! runs in the order the Node loader runs it, so the first refusal is the same text.
use std::fs;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

use crate::error::{invalid, ConfigError, Result};
use crate::json::{role_hash, sha256};
use crate::operator_policy::auto_approve_rule_problem;
use crate::primitives::*;
use crate::researcher_policy::researcher_rule_problems;
use crate::text::{len16, trim};
use crate::types::*;
use crate::value::{Item, Table};

/// `MAX_ROLE_NAME_CHARS` of `src/herdr/naming.ts`: 32 less the project slug, two dashes and four digits.
const MAX_ROLE_NAME_CHARS: usize = 32 - 10 - 1 - 1 - 4;
const MAX_MCP_SERVERS: usize = 16;

/// `[timers]`: key, default, minimum, maximum.
const TIMER_DEFAULTS: [(&str, i64, i64, i64); 10] = [
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

pub fn parse_config(bytes: &[u8], project_root: &Path) -> Result<RoleConfig> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return invalid(format!("{CONFIG_FILE_NAME} exceeds {MAX_FILE_BYTES} bytes"));
    }
    let Ok(decoded) = std::str::from_utf8(bytes) else {
        return invalid(format!("{CONFIG_FILE_NAME} is not valid UTF-8"));
    };
    let source = decoded.strip_prefix('\u{feff}').unwrap_or(decoded);
    let source = source.replace("\r\n", "\n");
    // The document is what `smol-toml` makes of the text, or the place it refuses it.
    let root = match crate::smol::parse(&source) {
        Ok(root) => root,
        Err(position) => {
            return invalid(format!(
                "{CONFIG_FILE_NAME} is not valid TOML at line {}, column {}",
                position.line, position.column
            ))
        }
    };
    resolve_document(&root, project_root)
}

fn resolve_document(root: &Table, project_root: &Path) -> Result<RoleConfig> {
    reject_unknown_keys(
        root,
        &[
            "schema_version",
            "project",
            "herdr_session",
            "notifications",
            "timers",
            "supervision",
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
        ],
        "top level",
    )?;
    if root.get("schema_version") != Some(&Item::Int(1)) {
        return invalid("schema_version must be the integer 1");
    }

    let project = optional_table(root.get("project"), "project")?;
    reject_unknown_keys(project, &["name"], "project")?;
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
    reject_unknown_keys(
        notification_table,
        &["herdr", "fallback", "pm_stale_minutes"],
        "notifications",
    )?;
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
    };
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
    reject_unknown_keys(
        supervision_table,
        &["enabled", "check_seconds"],
        "supervision",
    )?;
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

    let limit_table = optional_table(root.get("limits"), "limits")?;
    reject_unknown_keys(limit_table, &["max_workers"], "limits")?;
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
    reject_unknown_keys(ledger_table, &["keep_migration_backups"], "ledger")?;
    let ledger = Ledger {
        keep_migration_backups: optional_integer(
            ledger_table.get("keep_migration_backups"),
            "ledger.keep_migration_backups",
            1,
            MAX_KEEP_MIGRATION_BACKUPS,
            DEFAULT_KEEP_MIGRATION_BACKUPS,
        )?,
    };

    let layout_table = optional_table(root.get("layout"), "layout")?;
    reject_unknown_keys(
        layout_table,
        &[
            "spawn",
            "split",
            "pm_width_percent",
            "min_pane_columns",
            "min_pane_rows",
        ],
        "layout",
    )?;
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
    reject_unknown_keys(daemon_table, &["implementation"], "daemon")?;
    let daemon_implementation = if root.get("daemon").is_some() {
        Some(
            match daemon_table.get("implementation") {
                None => "node",
                value => enum_value(value, "daemon.implementation", &["node", "rust"])?,
            }
            .to_string(),
        )
    } else {
        None
    };

    let env_table = optional_table(root.get("env"), "env")?;
    reject_unknown_keys(env_table, &["pass"], "env")?;
    let env_pass = passed_environment_names(env_table.get("pass"))?;

    let hosts = resolve_hosts(required_table(root.get("hosts"), "hosts")?)?;
    let mcp_servers = resolve_mcp_servers(
        optional_table(root.get("mcp_servers"), "mcp_servers")?,
        &mut warnings,
    )?;
    let role_table = required_table(root.get("roles"), "roles")?;
    let defaults = resolve_defaults(optional_table(root.get("defaults"), "defaults")?)?;
    let roles = resolve_roles(role_table, &hosts, project_root, &defaults, &mcp_servers)?;
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

    Ok(RoleConfig {
        project_name: project_name.map(str::to_string),
        herdr_session: herdr_session.to_string(),
        notifications,
        timers,
        supervision,
        architect,
        operator,
        researcher,
        mcp_servers,
        prompt_relay,
        nexora,
        limits,
        ledger,
        layout,
        worktree,
        daemon_implementation,
        env_pass,
        hosts,
        roles,
        warnings,
    })
}

fn resolve_worktree(table: &Table) -> Result<Option<Worktree>> {
    reject_unknown_keys(
        table,
        &[
            "setup",
            "setup_timeout_seconds",
            "teardown",
            "teardown_timeout_seconds",
        ],
        "worktree",
    )?;
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
        reject_unknown_keys(server, &["command", "args"], &at)?;
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
        reject_unknown_keys(
            host,
            &[
                "kind",
                "command",
                "shell_command_timeout_seconds",
                "wait_timeout_seconds",
            ],
            &at,
        )?;
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
        hosts.push(Host {
            name: name.to_string(),
            kind: kind.to_string(),
            command: command.to_string(),
            shell_command_timeout_seconds,
            wait_timeout_seconds,
        });
    }
    Ok(hosts)
}

/// Codex and OMP run unattended with full access, so a rule this controller cannot enforce there is refused, not ignored.
fn reject_unenforceable(
    at: &str,
    host: &Host,
    kind: &str,
    permission_mode: &str,
    allow: &[String],
    deny: &[String],
    mcp: &[McpServer],
) -> Result<()> {
    if kind == "PM" || kind == "Supervisor" {
        return invalid(format!(
            "{at}: a {kind} role is read-only by design and only a claude host can enforce that; host {} is {}",
            host.name, host.kind
        ));
    }
    if !mcp.is_empty() {
        return invalid(format!(
            "{at}.mcp is a Claude Code option and host {} ({}) cannot enforce it",
            host.name, host.kind
        ));
    }
    if !allow.is_empty() || !deny.is_empty() {
        return invalid(format!(
            "{at}: allow and deny are Claude Code tool rules and host {} ({}) cannot enforce them",
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
    let mut allowed = vec!["model", "permission_mode"];
    allowed.extend(ROLE_KINDS);
    reject_unknown_keys(table, &allowed, "defaults")?;
    let read = |source: &Table, at: &str| -> Result<RoleDefaults> {
        reject_unknown_keys(source, &["model", "permission_mode"], at)?;
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
) -> Result<Vec<Role>> {
    let names = validated_names(table, "roles", "role")?;
    if names.is_empty() {
        return invalid("roles must define at least one role");
    }
    let mut roles = Vec::new();
    for name in names {
        let at = format!("roles.{name}");
        let role = required_table(table.get(name), &at)?;
        reject_unknown_keys(
            role,
            &[
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
            ],
            &at,
        )?;
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
        let model = match role.get("model") {
            None => kind_defaults
                .model
                .clone()
                .or_else(|| defaults.all.model.clone()),
            some => parse_model(some, &format!("{at}.model"))?,
        };
        let permission_mode: &str = match role.get("permission_mode") {
            None => kind_defaults
                .permission_mode
                .or(defaults.all.permission_mode)
                .unwrap_or("default"),
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
        let mcp = resolve_role_mcp(role.get("mcp"), &format!("{at}.mcp"), mcp_servers)?;
        let (prompt, prompt_text) = resolve_prompt(role, &at, project_root)?;
        if host.kind != "claude" {
            reject_unenforceable(&at, host, kind, permission_mode, &allow, &deny, &mcp)?;
        }
        let mut resolved = Role {
            name: name.to_string(),
            kind: kind.to_string(),
            host: host.name.clone(),
            model,
            permission_mode: permission_mode.to_string(),
            allow,
            deny,
            hooks,
            mcp,
            prompt,
            prompt_text,
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
    project_root: &Path,
) -> Result<(PromptSource, Option<String>)> {
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
        return Ok((
            PromptSource {
                source: "inline",
                path: None,
                hash: Some(sha256(text)),
            },
            Some(text.to_string()),
        ));
    }
    if role.get("prompt_file").is_none() {
        return Ok((
            PromptSource {
                source: "none",
                path: None,
                hash: None,
            },
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
    let missing = || ConfigError::Invalid(format!("{at}.prompt_file does not exist"));
    let real_root = fs::canonicalize(project_root).map_err(|_| missing())?;
    let real_file = fs::canonicalize(resolve_path(&real_root, relative)).map_err(|_| missing())?;
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
    Ok((
        PromptSource {
            source: "file",
            path: Some(file_text.to_string()),
            hash: Some(sha256(&text)),
        },
        Some(text),
    ))
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
    reject_unknown_keys(table, &["track", "default_action"], "nexora")?;
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
    reject_unknown_keys(
        table,
        &[
            "enabled",
            "role",
            "plan_review",
            "reviewer_role",
            "max_packages",
            "count_toward_worker_limit",
            "high_risk_triggers",
        ],
        "architect",
    )?;
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
    reject_unknown_keys(
        table,
        &["enabled", "role", "output_dir", "user_agent"],
        "researcher",
    )?;
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

fn resolve_prompt_relay(table: &Table, present: bool) -> Result<PromptRelay> {
    reject_unknown_keys(table, &["enabled", "capture_ttl_seconds"], "prompt_relay")?;
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
) -> Result<Operator> {
    reject_unknown_keys(
        table,
        &[
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
        ],
        "operator",
    )?;
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
    };
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
