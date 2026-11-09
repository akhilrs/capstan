//! The keep command (`cstan keep <agent-id> [<hours>]`): keeps a worker from automatic release for some hours, or clears
//! the keep with 0. The PM and the operator are the only callers. The record is an `agent_keep` ledger event the
//! idle-release loop and `cstan status` read; the newest record of an agent wins and survives a restart.

use super::shared::{
    fail, map_kernel_error, ok, CommandCall, CommandEnv, CommandResponse, ErrorCode,
};
use super::HandlerMap;
use crate::deps::new_context;
use serde_json::{json, Value};

/// The hours a keep lasts when none are given.
pub const DEFAULT_HOURS: i64 = 24;
/// The most hours a keep may last (one week).
pub const MAX_HOURS: i64 = 168;

/// Registers the handler of the route this module serves (keep).
pub fn register(map: &mut HandlerMap) {
    map.insert("keep", keep);
}

/// The hours of a keep from the second argument: 0 or 1..=168, whole numbers only.
fn parse_hours(text: &str) -> Option<i64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse::<i64>()
        .ok()
        .filter(|h| (0..=MAX_HOURS).contains(h))
}

fn keep(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let Some(by) = env.worker_manager(call.identity) else {
        return Some(fail(
            ErrorCode::Forbidden,
            "not_allowed: only the PM or the operator may keep an agent",
        ));
    };
    let args = call.args;
    if args.is_empty() || args.len() > 2 {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "keep takes an agent id and optionally the hours: keep <agent-id> [<hours>]",
        ));
    }
    let hours = match args.get(1) {
        None => DEFAULT_HOURS,
        Some(text) => match parse_hours(text) {
            Some(hours) => hours,
            None => {
                return Some(fail(
                    ErrorCode::InvalidRequest,
                    format!("bad_hours: hours must be a whole number from 1 to {MAX_HOURS}, or 0 to clear the keep"),
                ))
            }
        },
    };
    let agent_id = args[0].clone();
    let credential = env.deps.credential().to_string();
    let done = env.deps.kernel.run(move |core| {
        let record = core.agent_record(&agent_id)?;
        if record.is_null() {
            return Ok(Err(("unknown", agent_id)));
        }
        if record["state"] != "active" {
            return Ok(Err(("unknown", agent_id)));
        }
        if !["Developer", "Verifier"].contains(&record["kind"].as_str().unwrap_or("")) {
            return Ok(Err(("not_a_worker", agent_id)));
        }
        let context = new_context(core, &credential)?;
        let result = core.record_keep(
            &context,
            &json!({"agentId": agent_id, "hours": hours, "by": by}),
        )?;
        Ok(Ok((agent_id, result["until"].clone())))
    });
    Some(match done {
        Err(error) => map_kernel_error(&error),
        Ok(Err(("unknown", agent_id))) => fail(
            ErrorCode::UnknownAgent,
            format!("unknown_agent: {agent_id} is not an active agent"),
        ),
        Ok(Err((_, agent_id))) => fail(
            ErrorCode::Rejected,
            format!(
                "not_a_worker: {agent_id} is not a worker; only workers are released automatically"
            ),
        ),
        Ok(Ok((agent_id, until))) => ok(match until {
            Value::Null => json!({"agentId": agent_id, "kept": false, "until": Value::Null}),
            until => json!({"agentId": agent_id, "kept": true, "until": until}),
        }),
    })
}
