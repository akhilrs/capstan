//! The link command (src/commands/links.ts): external references on a task.

use super::shared::{
    fail, is_safe_agent_id, map_kernel_error, ok, CommandCall, CommandEnv, CommandResponse,
    ErrorCode,
};
use super::HandlerMap;
use crate::deps::new_context;
use capstan_kernel::KernelError;
use serde_json::{json, Map, Value};

/// `NEXORA_STATES` of src/nexora.ts.
const NEXORA_STATES: [&str; 6] = [
    "backlog",
    "todo",
    "in_progress",
    "in_review",
    "completed",
    "wont_do",
];

/// `EXTERNAL_REF_KINDS` of src/nexora.ts.
const EXTERNAL_REF_KINDS: [&str; 3] = ["requirement", "plan", "package"];

/// Registers the handlers of the routes this module serves (link).
pub fn register(map: &mut HandlerMap) {
    map.insert("link", link);
}

/// What the `catch` of the Node handler answers.
fn refuse(error: &KernelError) -> CommandResponse {
    if error.is_controller_error() {
        return fail(
            ErrorCode::Rejected,
            format!("link_refused: {}", error.message()),
        );
    }
    if let KernelError::Type(message) = error {
        return fail(ErrorCode::InvalidRequest, message.clone());
    }
    map_kernel_error(error)
}

fn link(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let Some(requested_by) = env.worker_manager(call.identity) else {
        return Some(fail(
            ErrorCode::Forbidden,
            "only the PM or the operator may link",
        ));
    };
    let first = call.args.first().map(String::as_str);
    let rest: &[String] = call.args.get(1..).unwrap_or(&[]);
    let credential = call.credential.to_string();
    if first == Some("bind") {
        let (ref_id, agent_id) = (rest.first(), rest.get(1));
        let (Some(ref_id), Some(agent_id)) = (ref_id, agent_id) else {
            return Some(bind_usage());
        };
        if rest.len() > 2 || !is_safe_agent_id(ref_id) || !is_safe_agent_id(agent_id) {
            return Some(bind_usage());
        }
        env.log(
            "link_bind_requested",
            json!({"requestedBy": requested_by, "refId": ref_id, "agentId": agent_id}),
        );
        let input = json!({"refId": ref_id, "agentId": agent_id});
        let bound = env.deps.kernel.run(move |core| {
            let context = new_context(core, &credential)?;
            core.bind_requirement(&context, &input)
        });
        let link = match bound {
            Ok(link) => link,
            Err(error) => return Some(refuse(&error)),
        };
        let mut result = Map::new();
        for key in ["refKind", "refId", "boundAgentId", "boundAt"] {
            result.insert(key.into(), link.get(key).cloned().unwrap_or(Value::Null));
        }
        result.extend(env.rename_branch(agent_id, ref_id));
        return Some(ok(Value::Object(result)));
    }
    let (ref_id, external_id, synced_state) = (rest.first(), rest.get(1), rest.get(2));
    let (Some(first), Some(ref_id), Some(external_id)) = (first, ref_id, external_id) else {
        return Some(link_usage());
    };
    if !EXTERNAL_REF_KINDS.contains(&first) || rest.len() > 3 {
        return Some(link_usage());
    }
    if let Some(state) = synced_state {
        if !NEXORA_STATES.contains(&state.as_str()) {
            return Some(fail(
                ErrorCode::InvalidRequest,
                format!("the state must be one of {}", NEXORA_STATES.join(", ")),
            ));
        }
    }
    env.log(
        "link_requested",
        json!({"requestedBy": requested_by, "kind": first, "refId": ref_id}),
    );
    let mut input = Map::new();
    input.insert("refKind".into(), json!(first));
    input.insert("refId".into(), json!(ref_id));
    input.insert("externalId".into(), json!(external_id));
    if let Some(state) = synced_state {
        input.insert("syncedState".into(), json!(state));
    }
    let linked = env.deps.kernel.run(move |core| {
        let context = new_context(core, &credential)?;
        core.link_external(&context, &Value::Object(input))
    });
    Some(match linked {
        Ok(link) => {
            let field = |key: &str| link.get(key).cloned().unwrap_or(Value::Null);
            ok(json!({
                "refKind": field("refKind"),
                "refId": field("refId"),
                "externalId": field("externalId"),
                "syncedState": field("syncedState"),
                "wanted": field("wanted"),
                "drift": field("drift"),
            }))
        }
        Err(error) => refuse(&error),
    })
}

fn bind_usage() -> CommandResponse {
    fail(
        ErrorCode::InvalidRequest,
        "link bind needs a requirement ref id and an agent id",
    )
}

fn link_usage() -> CommandResponse {
    fail(
        ErrorCode::InvalidRequest,
        "link needs requirement|plan|package, a ref id, a Nexora id and optionally a state, or link bind <requirement-ref-id> <agent-id>",
    )
}
