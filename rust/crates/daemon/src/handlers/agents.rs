//! The commands that manage agents (src/commands/agents.ts: pause, resume, launch, spawn, replace, release, pm-restart),
//! `observe` and `peek` (src/commands/relay.ts, served by this module's routes), and `shutdown` (src/daemon.ts).
//!
//! The launcher and herdr calls block their connection's thread for as long as they take and never run inside a kernel
//! closure. With launching off (`CAPSTAN_LAUNCH=off`, or no `capstan.toml`) `deps.launcher` is `None` and each route
//! answers as Node does: `not_configured` with the same text.

use super::shared::{
    fail, map_error, map_kernel_error, ok, CommandCall, CommandEnv, CommandResponse, ErrorCode,
    HandlerError,
};
use super::HandlerMap;
use capstan_kernel::helpers::{is_js_space, js_trim};
use capstan_launcher::api::SpawnOptions;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::sync::LazyLock;

/// `BRANCH_TYPES` of src/conventions.ts.
pub const BRANCH_TYPES: [&str; 9] = [
    "feat", "fix", "docs", "refactor", "perf", "test", "build", "ci", "chore",
];

const MAX_TASK_TITLE: usize = 200;

static NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(capstan_config::primitives::NAME_PATTERN).expect("the name pattern is valid")
});
static TASK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}(/[A-Za-z0-9][A-Za-z0-9._:-]{0,127})?$")
        .expect("the task pattern is valid")
});
static CONTROL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]").expect("the pattern is valid"));

/// Registers the handlers of the routes this module serves.
pub fn register(map: &mut HandlerMap) {
    map.insert("pause", pause);
    map.insert("resume", resume);
    map.insert("launch", launch);
    map.insert("spawn", spawn);
    map.insert("replace", replace);
    map.insert("release", release);
    map.insert("pm-restart", pm_restart);
    map.insert("observe", observe);
    map.insert("peek", peek);
    map.insert("shutdown", shutdown);
}

/// `normalizeTaskTitle`: runs of tab, CR and LF folded to one space, other control characters refused, whitespace folded,
/// cut to 200 characters. The error is the text after `the title `.
pub fn normalize_task_title(raw: &str) -> Result<String, &'static str> {
    let mut folded = String::with_capacity(raw.len());
    let mut in_run = false;
    for c in raw.chars() {
        if matches!(c, '\t' | '\r' | '\n') {
            if !in_run {
                folded.push(' ');
            }
            in_run = true;
        } else {
            folded.push(c);
            in_run = false;
        }
    }
    if CONTROL.is_match(&folded) {
        return Err("must not contain control characters");
    }
    let mut collapsed = String::with_capacity(folded.len());
    let mut in_space = false;
    for c in folded.chars() {
        if is_js_space(c) {
            if !in_space {
                collapsed.push(' ');
            }
            in_space = true;
        } else {
            collapsed.push(c);
            in_space = false;
        }
    }
    let cut: String = js_trim(&collapsed).chars().take(MAX_TASK_TITLE).collect();
    let title = js_trim(&cut);
    if title.is_empty() {
        return Err("must not be empty");
    }
    Ok(title.to_string())
}

fn pause(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    Some(env.change_pause(call, true))
}

fn resume(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    Some(env.change_pause(call, false))
}

/// `core.assertRunNotPaused(action)` as the handlers call it.
fn assert_not_paused(env: &CommandEnv<'_>, action: &'static str) -> Result<(), CommandResponse> {
    env.deps
        .kernel
        .run(move |core| core.assert_run_not_paused(action))
        .map(|_| ())
        .map_err(|error| map_kernel_error(&error))
}

fn launch(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    if let Err(refusal) = assert_not_paused(env, "launch") {
        return Some(refusal);
    }
    let Some(launcher) = &env.deps.launcher else {
        return Some(fail(
            ErrorCode::NotConfigured,
            "launching agents needs capstan.toml and Herdr",
        ));
    };
    if !call.args.is_empty() {
        return Some(fail(ErrorCode::InvalidRequest, "launch takes no arguments"));
    }
    Some(match launcher.launch_pm() {
        Ok(result) => ok(result.to_value()),
        Err(error) => map_error(&HandlerError::from(error)),
    })
}

fn spawn(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let Some(requested_by) = env.worker_manager(call.identity) else {
        return Some(fail(
            ErrorCode::Forbidden,
            "only the PM or the operator may spawn workers",
        ));
    };
    let usage = "spawn needs a role name and takes --task <plan-id>/<package-id> or <requirement-ref-id>, --type <type> and --title <text>";
    let Some((role_name, flags)) = call.args.split_first() else {
        return Some(fail(ErrorCode::InvalidRequest, usage));
    };
    if !NAME.is_match(role_name) {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "the role name is not valid",
        ));
    }
    // In the order the flags were given, as the members of Node's `named` object.
    let mut named: Vec<(&'static str, String)> = Vec::new();
    let mut index = 0;
    while index < flags.len() {
        let key = match flags[index].as_str() {
            "--task" => "task",
            "--type" => "type",
            "--title" => "title",
            _ => return Some(fail(ErrorCode::InvalidRequest, usage)),
        };
        let Some(value) = flags.get(index + 1) else {
            return Some(fail(ErrorCode::InvalidRequest, usage));
        };
        if named.iter().any(|(existing, _)| *existing == key) {
            return Some(fail(ErrorCode::InvalidRequest, usage));
        }
        named.push((key, value.clone()));
        index += 2;
    }
    let get = |named: &[(&str, String)], key: &str| -> Option<String> {
        named
            .iter()
            .find(|(existing, _)| *existing == key)
            .map(|(_, value)| value.clone())
    };
    if get(&named, "task").is_some_and(|task| !TASK.is_match(&task)) {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "--task must be <plan-id>/<package-id> or a requirement ref id",
        ));
    }
    if get(&named, "type").is_some_and(|kind| !BRANCH_TYPES.contains(&kind.as_str())) {
        return Some(fail(
            ErrorCode::InvalidRequest,
            format!("--type must be one of {}", BRANCH_TYPES.join(", ")),
        ));
    }
    if let Some(title) = get(&named, "title") {
        match normalize_task_title(&title) {
            Ok(clean) => {
                if let Some(slot) = named.iter_mut().find(|(key, _)| *key == "title") {
                    slot.1 = clean;
                }
            }
            Err(why) => return Some(fail(ErrorCode::InvalidRequest, format!("--title {why}"))),
        }
    }
    if let Err(refusal) = assert_not_paused(env, "spawn") {
        return Some(refusal);
    }
    let Some(launcher) = &env.deps.launcher else {
        return Some(fail(
            ErrorCode::NotConfigured,
            "spawning agents needs capstan.toml and Herdr",
        ));
    };
    let mut detail = Map::new();
    detail.insert("requestedBy".into(), json!(requested_by));
    detail.insert("role".into(), json!(role_name));
    for (key, value) in &named {
        detail.insert((*key).to_string(), json!(value));
    }
    env.log("spawn_requested", Value::Object(detail));
    let options = SpawnOptions {
        task: get(&named, "task"),
        type_: get(&named, "type"),
        title: get(&named, "title"),
        ..SpawnOptions::default()
    };
    Some(match launcher.spawn(role_name, &options) {
        Ok(result) => ok(result.to_value()),
        Err(error) => map_error(&HandlerError::from(error)),
    })
}

/// The agent id a replace or release names, with the refusals before the launcher check.
fn worker_target(
    env: &CommandEnv<'_>,
    call: &CommandCall<'_>,
    verb: &str,
    plural_need: &str,
) -> Result<(String, String), CommandResponse> {
    let Some(requested_by) = env.worker_manager(call.identity) else {
        return Err(fail(
            ErrorCode::Forbidden,
            format!("only the PM or the operator may {verb} workers"),
        ));
    };
    if call.args.len() != 1 {
        return Err(fail(
            ErrorCode::InvalidRequest,
            format!("{plural_need} needs one agent id"),
        ));
    }
    let agent_id = call.args[0].clone();
    if !super::shared::is_safe_agent_id(&agent_id) {
        return Err(fail(ErrorCode::InvalidRequest, "the agent id is not valid"));
    }
    Ok((requested_by, agent_id))
}

fn replace(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let (requested_by, agent_id) = match worker_target(env, call, "replace", "replace") {
        Ok(target) => target,
        Err(refusal) => return Some(refusal),
    };
    let Some(launcher) = &env.deps.launcher else {
        return Some(fail(
            ErrorCode::NotConfigured,
            "replacing agents needs capstan.toml and Herdr",
        ));
    };
    env.log(
        "replace_requested",
        json!({"requestedBy": requested_by, "agentId": agent_id}),
    );
    Some(match launcher.replace(&agent_id) {
        Ok(result) => ok(result.to_value()),
        Err(error) => map_error(&HandlerError::from(error)),
    })
}

fn release(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let (requested_by, agent_id) = match worker_target(env, call, "release", "release") {
        Ok(target) => target,
        Err(refusal) => return Some(refusal),
    };
    let Some(launcher) = &env.deps.launcher else {
        return Some(fail(
            ErrorCode::NotConfigured,
            "releasing agents needs capstan.toml and Herdr",
        ));
    };
    env.log(
        "release_requested",
        json!({"requestedBy": requested_by, "agentId": agent_id}),
    );
    Some(match launcher.release(&agent_id) {
        Ok(result) => ok(result.to_value()),
        Err(error) => map_error(&HandlerError::from(error)),
    })
}

fn pm_restart(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let Some(launcher) = &env.deps.launcher else {
        return Some(fail(
            ErrorCode::NotConfigured,
            "restarting the PM needs capstan.toml and Herdr",
        ));
    };
    if !call.args.is_empty() {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "pm restart takes no arguments",
        ));
    }
    Some(match launcher.restart_pm() {
        Ok(result) => ok(result.to_value()),
        Err(error) => map_error(&HandlerError::from(error)),
    })
}

fn observe(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let caller = call.identity.agent.as_ref().filter(|agent| {
        (agent.kind == "Supervisor" || agent.kind == "PM") && agent.state == "active"
    });
    let Some(caller) = caller else {
        return Some(fail(
            ErrorCode::Forbidden,
            "only the PM or a Supervisor can observe an agent",
        ));
    };
    let credential = call.credential.to_string();
    Some(env.observe_agent(call.args, &caller.agent_id, move |core| {
        core.assert_can_observe(&credential)
    }))
}

/// The operator's read of an agent's screen; the same pipeline and limits as observe.
fn peek(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    Some(env.observe_agent(call.args, "operator", |_| Ok(Value::Null)))
}

fn shutdown(env: &CommandEnv<'_>, _call: &CommandCall<'_>) -> Option<CommandResponse> {
    // The connection writes this answer before the stop takes the server down: the drain waits for running connections.
    env.deps.shutdown.request();
    Some(ok(json!({"stopping": true})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_folded_and_cut_as_node_does() {
        assert_eq!(
            normalize_task_title("  Add\tthe\r\n thing  ").unwrap(),
            "Add the thing"
        );
        assert_eq!(normalize_task_title("a   b").unwrap(), "a b");
        assert_eq!(
            normalize_task_title("\u{a0}x\u{2003}y\u{3000}").unwrap(),
            "x y"
        );
        assert_eq!(
            normalize_task_title(&"é".repeat(300))
                .unwrap()
                .chars()
                .count(),
            200
        );
        assert_eq!(
            normalize_task_title("a\u{1}b"),
            Err("must not contain control characters")
        );
        assert_eq!(
            normalize_task_title("a\u{200b}b"),
            Err("must not contain control characters")
        );
        // U+FEFF is a format character: refused, though JavaScript counts it as white space.
        assert_eq!(
            normalize_task_title("a\u{feff}b"),
            Err("must not contain control characters")
        );
        assert_eq!(normalize_task_title(" \t\n "), Err("must not be empty"));
    }

    #[test]
    fn the_patterns_are_the_ones_of_node() {
        assert!(NAME.is_match("developer"));
        assert!(NAME.is_match("a-1"));
        assert!(!NAME.is_match("Developer"));
        assert!(!NAME.is_match("1a"));
        assert!(!NAME.is_match(&format!("a{}", "b".repeat(32))));
        assert!(TASK.is_match("plan-1/pkg-2"));
        assert!(TASK.is_match("req.1:x"));
        assert!(!TASK.is_match("a/b/c"));
        assert!(!TASK.is_match("-a"));
        assert!(!TASK.is_match("a\n"));
    }
}
