//! The command-line arguments of the Codex and OMP hosts (src/herdr/hosts.ts).

use crate::adapter::validate::invalid;
use crate::api::AdapterResult;
use std::path::Path;

/// One argument may not pass about 128 KiB on Linux; leave room for the other arguments.
const MAX_ARGUMENT_BYTES: usize = 120 * 1024;

/// The settings of a resolved role that the host arguments use (`HostRoleSettings`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostRoleSettings {
    pub model: Option<String>,
}

/// A TOML basic string: backslash, quote and every control character escaped, so the value holds no newline.
pub fn toml_string(value: &str) -> String {
    let mut out = String::from("\"");
    for character in value.chars() {
        let code = character as u32;
        if character == '\\' {
            out.push_str("\\\\");
        } else if character == '"' {
            out.push_str("\\\"");
        } else if code < 0x20 || (0x7f..=0x9f).contains(&code) {
            out.push_str(&format!("\\u{code:04x}"));
        } else {
            out.push(character);
        }
    }
    out.push('"');
    out
}

fn require_no_dash(model: &Option<String>) -> AdapterResult<()> {
    match model {
        Some(model) if model.starts_with('-') => {
            Err(invalid("a model value must not start with a dash"))
        }
        _ => Ok(()),
    }
}

/// Codex runs with full access and no approval prompt: its sandbox blocks the daemon's Unix socket in every restricted
/// mode, and a prompt would leave the agent blocked. The worktree is pre-trusted with a run-time override, which does
/// not write the operator's own configuration file.
pub fn codex_arguments(
    role: &HostRoleSettings,
    prompt_text: &str,
    worktree_path: Option<&Path>,
) -> AdapterResult<Vec<String>> {
    require_no_dash(&role.model)?;
    let mut args: Vec<String> = [
        "--sandbox",
        "danger-full-access",
        "--ask-for-approval",
        "never",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Some(model) = &role.model {
        args.push("--model".into());
        args.push(model.clone());
    }
    args.push("-c".into());
    args.push("check_for_update_on_startup=false".into());
    if let Some(worktree) = worktree_path {
        let real = std::fs::canonicalize(worktree)
            .map_err(|error| invalid(format!("the worktree path cannot be resolved: {error}")))?;
        let real = real.to_string_lossy();
        if real.contains('\u{fffd}') {
            return Err(invalid(
                "the worktree path is not valid UTF-8, so Codex cannot be told to trust it",
            ));
        }
        args.push("-c".into());
        args.push(format!(
            "projects.{}.trust_level=\"trusted\"",
            toml_string(&real)
        ));
    }
    let instructions = format!("developer_instructions={}", toml_string(prompt_text));
    if instructions.len() > MAX_ARGUMENT_BYTES {
        return Err(invalid(
            "the role prompt is too large to pass to Codex as one argument",
        ));
    }
    args.push("-c".into());
    args.push(instructions);
    Ok(args)
}

/// OMP runs with every tool call approved: an approval prompt would leave the agent blocked and delivery deferred.
pub fn omp_arguments(role: &HostRoleSettings, prompt_file: &str) -> AdapterResult<Vec<String>> {
    require_no_dash(&role.model)?;
    let mut args: Vec<String> = vec!["--approval-mode".into(), "yolo".into()];
    if let Some(model) = &role.model {
        args.push("--model".into());
        args.push(model.clone());
    }
    args.push("--append-system-prompt".into());
    args.push(prompt_file.to_string());
    Ok(args)
}
