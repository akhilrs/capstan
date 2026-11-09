//! The environment and command-line arguments an agent gets (src/herdr/claude-args.ts).

use crate::adapter::validate::{has_control, has_unsafe, invalid, ENVIRONMENT_KEY};
use crate::api::AdapterResult;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};

const ALLOWLISTED_BASE: [&str; 8] = [
    "PATH", "HOME", "USER", "LOGNAME", "LANG", "LC_ALL", "TERM", "TMPDIR",
];

/// Copies only the allowlisted names from `base`, then the names the operator listed in `pass`, then applies `extras`;
/// CAPSTAN_ variables never come from `base`. A passed value that is not acceptable fails and names the variable, never
/// the value. The result is ordered by name; the checks run in the order Node builds the object in.
pub fn build_agent_environment(
    base: &HashMap<String, String>,
    extras: &[(String, String)],
    pass: &[String],
) -> AdapterResult<BTreeMap<String, String>> {
    let mut environment: Vec<(String, String)> = Vec::new();
    fn set(environment: &mut Vec<(String, String)>, name: &str, value: &str) {
        match environment.iter_mut().find(|(n, _)| n == name) {
            Some(slot) => slot.1 = value.to_string(),
            None => environment.push((name.to_string(), value.to_string())),
        }
    }
    let names = ALLOWLISTED_BASE
        .iter()
        .copied()
        .chain(pass.iter().map(String::as_str));
    for name in names {
        if name.starts_with("CAPSTAN_") || !ENVIRONMENT_KEY.is_match(name) {
            return Err(invalid(format!(
                "environment name {name} is not acceptable"
            )));
        }
        // An empty value of a listed name counts as unset: the launcher reports it, and nothing is passed.
        if let Some(value) = base.get(name) {
            if !value.is_empty() || !pass.iter().any(|p| p == name) {
                set(&mut environment, name, value);
            }
        }
    }
    for (name, value) in extras {
        set(&mut environment, name, value);
    }
    for (name, value) in &environment {
        if !ENVIRONMENT_KEY.is_match(name) {
            return Err(invalid(format!(
                "environment name {name} is not acceptable"
            )));
        }
        if has_unsafe(value) {
            return Err(invalid(format!(
                "environment value for {name} is not acceptable (one line of printable text is allowed; a trailing carriage return from a CRLF file is not)"
            )));
        }
    }
    Ok(environment.into_iter().collect())
}

/// Read-only and silent when nothing waits; Claude Code runs it after every tool call.
pub const INBOX_HOOK_COMMAND: &str = "cstan inbox --hook";

/// One MCP server of a role.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
}

/// The settings of a resolved role that the Claude arguments use (`ClaudeRoleSettings`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaudeRoleSettings {
    pub model: Option<String>,
    pub permission_mode: String,
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    /// `"off"` turns every hook off; anything else installs the inbox hook.
    pub hooks: String,
    pub mcp: Vec<McpServer>,
    /// The folder a PM may write to (absolute). When set, the Write/Edit guard hook is installed and the folder is
    /// allowed; the caller has already left bare `Write` and `Edit` out of `deny`.
    pub write_guard: Option<String>,
    /// The folders a developer-kind role may write to (absolute). When not empty, the same guard hook is installed
    /// with `--dirs`, each folder is allowed, and the caller has already left bare `Write` and `Edit` out of `deny`.
    pub write_dirs: Vec<String>,
}

/// The hook matcher of the guard: every tool that edits a file except NotebookEdit, which the PM keeps denied.
pub const WRITE_GUARD_MATCHER: &str = "Write|Edit|MultiEdit";

/// The shell command of the Write/Edit guard hook. `|| exit 2` makes a guard that cannot run (no `cstan`, a crash)
/// refuse the write: Claude Code treats any other non-zero exit as a non-blocking error.
pub fn write_guard_command(memory_dir: &str) -> String {
    format!("cstan guard-write '{memory_dir}' || exit 2")
}

/// The shell command of the guard hook for a role with write folders; same fail-closed `|| exit 2`.
pub fn write_dirs_guard_command(dirs: &[String]) -> String {
    let quoted: Vec<String> = dirs.iter().map(|d| format!("'{d}'")).collect();
    format!("cstan guard-write --dirs {} || exit 2", quoted.join(" "))
}

/// The folder is put inside single quotes and a `//` absolute-path rule, so it must not hold a quote, a glob
/// character or anything unprintable.
pub fn guard_path_is_usable(memory_dir: &str) -> bool {
    memory_dir.starts_with('/')
        && !memory_dir.contains("..")
        && !has_control(memory_dir)
        && !memory_dir.chars().any(|c| {
            matches!(
                c,
                '\'' | '"' | '\\' | '*' | '?' | '[' | ']' | '{' | '}' | '!' | '$' | '`' | '#'
            )
        })
}

/// The arguments Claude Code gets for a role. Herdr quotes each argument safely, so none is quoted here; a newline is
/// refused because Herdr refuses it.
pub fn claude_arguments(
    role: &ClaudeRoleSettings,
    prompt_file: Option<&str>,
) -> AdapterResult<Vec<String>> {
    let mut args: Vec<String> = Vec::new();
    if let Some(model) = &role.model {
        args.push("--model".into());
        args.push(model.clone());
    }
    args.push("--permission-mode".into());
    args.push(role.permission_mode.clone());
    let mut allow = role.allow.clone();
    if role.write_guard.is_some() && !role.write_dirs.is_empty() {
        return Err(invalid(
            "a role has either the memory guard or write folders",
        ));
    }
    for dir in &role.write_dirs {
        if !guard_path_is_usable(dir) {
            return Err(invalid("a write folder is not a usable path"));
        }
        allow.push(format!("Edit(/{}/**)", dir.trim_end_matches('/')));
    }
    if let Some(dir) = &role.write_guard {
        if !guard_path_is_usable(dir) {
            return Err(invalid("the write guard folder is not a usable path"));
        }
        allow.push(format!("Edit(/{}/**)", dir.trim_end_matches('/')));
    }
    if !allow.is_empty() {
        args.push("--allowedTools".into());
        args.extend(allow);
    }
    if !role.deny.is_empty() {
        args.push("--disallowedTools".into());
        args.extend(role.deny.iter().cloned());
    }
    // JSON escapes control characters, so the check below cannot see them inside the config.
    for server in &role.mcp {
        for text in std::iter::once(&server.name)
            .chain(std::iter::once(&server.command))
            .chain(server.args.iter())
        {
            if has_control(text) {
                return Err(invalid(
                    "an mcp server name, command or argument has control characters",
                ));
            }
        }
    }
    if !role.mcp.is_empty() {
        let mut servers = Map::new();
        for server in &role.mcp {
            servers.insert(
                server.name.clone(),
                json!({"type": "stdio", "command": server.command, "args": server.args}),
            );
        }
        args.push("--mcp-config".into());
        args.push(json!({ "mcpServers": servers }).to_string());
        args.push("--strict-mcp-config".into());
    }
    // Both keys: Claude Code has renamed the attribution setting before.
    let attribution = |mut base: Map<String, Value>| {
        base.insert("includeCoAuthoredBy".into(), json!(false));
        base.insert("attribution".into(), json!({"commit": "", "pr": ""}));
        base
    };
    args.push("--settings".into());
    let guard_command = match &role.write_guard {
        Some(dir) => Some(write_guard_command(dir)),
        None if !role.write_dirs.is_empty() => Some(write_dirs_guard_command(&role.write_dirs)),
        None => None,
    };
    if let Some(command) = guard_command {
        // Only the guard: hooks = "off" still installs no inbox hook. disableAllHooks cannot stay, because it would
        // switch the guard off too, so hooks from the user's or project's own Claude settings run as well.
        let mut settings = attribution(Map::new());
        settings.insert("disableAllHooks".into(), json!(false));
        let mut hooks = Vec::new();
        hooks.push(json!({"matcher": WRITE_GUARD_MATCHER, "hooks": [{"type": "command", "command": command, "timeout": 10}]}));
        let mut events = Map::new();
        events.insert("PreToolUse".into(), Value::Array(hooks));
        if role.hooks != "off" {
            events.insert(
                "PostToolUse".into(),
                json!([{
                    "matcher": "*",
                    "hooks": [{"type": "command", "command": INBOX_HOOK_COMMAND, "timeout": 5}],
                }]),
            );
        }
        settings.insert("hooks".into(), Value::Object(events));
        args.push(Value::Object(settings).to_string());
    } else if role.hooks == "off" {
        let mut base = Map::new();
        base.insert("disableAllHooks".into(), json!(true));
        args.push(Value::Object(attribution(base)).to_string());
    } else {
        let mut settings = attribution(Map::new());
        settings.insert(
            "hooks".into(),
            json!({"PostToolUse": [{
                "matcher": "*",
                "hooks": [{"type": "command", "command": INBOX_HOOK_COMMAND, "timeout": 5}],
            }]}),
        );
        args.push(Value::Object(settings).to_string());
    }
    if let Some(file) = prompt_file {
        args.push("--append-system-prompt-file".into());
        args.push(file.to_string());
    }
    for arg in &args {
        if arg.is_empty() || has_control(arg) {
            return Err(invalid(
                "an agent argument is empty or has control characters",
            ));
        }
    }
    let values = role
        .model
        .iter()
        .chain(role.allow.iter())
        .chain(role.deny.iter())
        .chain(role.mcp.iter().map(|server| &server.command));
    if values.into_iter().any(|value| value.starts_with('-')) {
        return Err(invalid(
            "a model, allow, deny or mcp command value must not start with a dash",
        ));
    }
    Ok(args)
}
