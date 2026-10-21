//! The `[verify]` section: the project's own gate commands, which the daemon runs in a scratch worktree so a report can
//! be checked against a result instead of an agent's word. This module holds the shape and resolves and validates the
//! section; nothing here runs a command or touches the filesystem.
use capstan_wire::js::Value;

use crate::error::{invalid, Result};
use crate::json::{number, object, optional, sha256, strings, text};
use crate::keys::VERIFY_KEYS;
use crate::primitives::*;
use crate::types::{RESERVED_ENV_NAMES, RESERVED_ENV_PREFIXES};
use crate::value::Item;

pub const MAX_VERIFY_COMMANDS: usize = 16;
pub const MAX_VERIFY_COMMAND_CHARS: usize = 1000;
const MAX_ENV_ENTRIES: usize = 32;
const MAX_ENV_VALUE_CHARS: usize = 1024;

/// The resolved `[verify]` section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verify {
    pub commands: Vec<String>,
    pub env: Vec<(String, String)>,
    pub timeout_seconds: i64,
    pub max_parallel: i64,
    pub auto_report: bool,
    pub auto_integration: bool,
    pub require_review: bool,
    pub require_integrate: bool,
    pub output_tail_bytes: i64,
    pub max_log_bytes: i64,
    pub keep_logs: i64,
    pub use_worktree_hooks: bool,
    pub reap: bool,
    /// Absolute or `~/...`; `None` means the daemon picks `<cache root>/projects/<project id>/verify`.
    pub scratch_dir: Option<String>,
}

impl Verify {
    /// Lowercase hex SHA-256 of the canonical JSON array of the commands: the key a passed run must match.
    pub fn commands_sha(&self) -> String {
        let json = serde_json::to_string(&self.commands).unwrap_or_default();
        sha256(&json)
    }

    pub(crate) fn to_value(&self) -> Value {
        object(vec![
            ("commands", strings(&self.commands)),
            (
                "env",
                object(
                    self.env
                        .iter()
                        .map(|(name, value)| (name.as_str(), text(value)))
                        .collect(),
                ),
            ),
            ("timeoutSeconds", number(self.timeout_seconds)),
            ("maxParallel", number(self.max_parallel)),
            ("autoReport", Value::Bool(self.auto_report)),
            ("autoIntegration", Value::Bool(self.auto_integration)),
            ("requireReview", Value::Bool(self.require_review)),
            ("requireIntegrate", Value::Bool(self.require_integrate)),
            ("outputTailBytes", number(self.output_tail_bytes)),
            ("maxLogBytes", number(self.max_log_bytes)),
            ("keepLogs", number(self.keep_logs)),
            ("useWorktreeHooks", Value::Bool(self.use_worktree_hooks)),
            ("reap", Value::Bool(self.reap)),
            ("scratchDir", optional(&self.scratch_dir)),
        ])
    }
}

/// A subset of `allowed` written as an array of strings; a repeat or an unknown value is refused.
fn choice_set(value: Option<&Item>, at: &str, allowed: &[&str]) -> Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let Item::Array(entries) = value else {
        return invalid(format!("{at} must be an array of strings"));
    };
    let mut chosen: Vec<String> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let at = format!("{at}[{index}]");
        let found = enum_value(Some(entry), &at, allowed)?;
        if chosen.iter().any(|seen| seen == found) {
            return invalid(format!("{at} repeats {found}"));
        }
        chosen.push(found.to_string());
    }
    Ok(chosen)
}

fn env_name_problem(name: &str) -> Option<String> {
    if !env_name_ok(name) {
        return Some("must be an upper-case variable name (letters, digits and underscore, at most 64 characters)".into());
    }
    if name.starts_with("CAPSTAN_") {
        return Some("must not start with CAPSTAN_: those variables belong to Capstan".into());
    }
    if RESERVED_ENV_NAMES.contains(&name)
        || RESERVED_ENV_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
    {
        return Some(format!(
            "must not be {name}: Capstan already sets it or it changes how a shell, loader or git behaves"
        ));
    }
    None
}

fn scratch_dir(value: Option<&Item>) -> Result<Option<String>> {
    let Some(path) = optional_string(value, "verify.scratch_dir", 1024)? else {
        return Ok(None);
    };
    guard_credential_shape(path, "verify.scratch_dir")?;
    let home_relative = path.starts_with("~/");
    if !path.starts_with('/') && !home_relative {
        return invalid("verify.scratch_dir must be an absolute path or start with ~/");
    }
    let tail = if home_relative { &path[2..] } else { path };
    if tail.split('/').any(|part| part == "..") {
        return invalid("verify.scratch_dir must not contain ..");
    }
    if tail.trim_matches('/').is_empty() {
        return invalid("verify.scratch_dir must not be the filesystem root or the home directory");
    }
    Ok(Some(path.trim_end_matches('/').to_string()))
}

/// Resolves `[verify]`; an absent table is `None`, which changes nothing for the project.
pub fn resolve_verify(value: Option<&Item>) -> Result<Option<Verify>> {
    if value.is_none() {
        return Ok(None);
    }
    let table = required_table(value, "verify")?;
    reject_unknown_keys(table, VERIFY_KEYS, "verify")?;
    let Some(Item::Array(entries)) = table.get("commands") else {
        return invalid("verify.commands must be an array of 1 to 16 command strings");
    };
    if entries.is_empty() || entries.len() > MAX_VERIFY_COMMANDS {
        return invalid(format!(
            "verify.commands must have between 1 and {MAX_VERIFY_COMMANDS} entries"
        ));
    }
    let mut commands = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let at = format!("verify.commands[{index}]");
        commands
            .push(required_string(Some(entry), &at, MAX_VERIFY_COMMAND_CHARS, false)?.to_string());
    }

    let env_table = optional_table(table.get("env"), "verify.env")?;
    if env_table.len() > MAX_ENV_ENTRIES {
        return invalid(format!("verify.env exceeds {MAX_ENV_ENTRIES} entries"));
    }
    let mut env = Vec::new();
    for (name, item) in env_table.entries() {
        let at = format!("verify.env.{name}");
        if let Some(problem) = env_name_problem(name) {
            return invalid(format!("{at} {problem}"));
        }
        let value = required_string(Some(item), &at, MAX_ENV_VALUE_CHARS, false)?;
        guard_credential_shape(value, &at)?;
        env.push((name.to_string(), value.to_string()));
    }

    let auto = choice_set(table.get("auto"), "verify.auto", &["report", "integration"])?;
    let require = choice_set(
        table.get("require"),
        "verify.require",
        &["review", "integrate"],
    )?;

    Ok(Some(Verify {
        commands,
        env,
        timeout_seconds: optional_integer(
            table.get("timeout_seconds"),
            "verify.timeout_seconds",
            60,
            14_400,
            3600,
        )?,
        max_parallel: optional_integer(table.get("max_parallel"), "verify.max_parallel", 1, 8, 1)?,
        auto_report: auto.iter().any(|name| name == "report"),
        auto_integration: auto.iter().any(|name| name == "integration"),
        require_review: require.iter().any(|name| name == "review"),
        require_integrate: require.iter().any(|name| name == "integrate"),
        output_tail_bytes: optional_integer(
            table.get("output_tail_bytes"),
            "verify.output_tail_bytes",
            1024,
            12_288,
            8192,
        )?,
        max_log_bytes: optional_integer(
            table.get("max_log_bytes"),
            "verify.max_log_bytes",
            65_536,
            67_108_864,
            4_194_304,
        )?,
        keep_logs: optional_integer(table.get("keep_logs"), "verify.keep_logs", 1, 500, 20)?,
        use_worktree_hooks: optional_boolean(
            table.get("worktree_hooks"),
            "verify.worktree_hooks",
            true,
        )?,
        reap: optional_boolean(table.get("reap"), "verify.reap", true)?,
        scratch_dir: scratch_dir(table.get("scratch_dir"))?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(commands: &[&str]) -> Verify {
        Verify {
            commands: commands.iter().map(|c| c.to_string()).collect(),
            env: vec![],
            timeout_seconds: 3600,
            max_parallel: 1,
            auto_report: false,
            auto_integration: false,
            require_review: false,
            require_integrate: false,
            output_tail_bytes: 8192,
            max_log_bytes: 4_194_304,
            keep_logs: 20,
            use_worktree_hooks: true,
            reap: true,
            scratch_dir: None,
        }
    }

    #[test]
    fn commands_sha_is_stable_and_order_sensitive() {
        let a = sample(&["a", "b"]);
        assert_eq!(a.commands_sha(), sample(&["a", "b"]).commands_sha());
        assert_eq!(a.commands_sha().len(), 64);
        assert_ne!(a.commands_sha(), sample(&["b", "a"]).commands_sha());
        assert_ne!(a.commands_sha(), sample(&["a", "c"]).commands_sha());
        assert_eq!(sample(&["a"]).commands_sha(), sha256("[\"a\"]"));
    }
}
