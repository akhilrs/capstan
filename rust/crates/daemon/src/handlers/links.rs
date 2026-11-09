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
    if call.args.first().map(String::as_str) == Some("model") {
        return Some(link_model(env, call));
    }
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
    let mut plain: Vec<String> = Vec::new();
    let mut model: Option<String> = None;
    let mut index = 0;
    while index < rest.len() {
        if rest[index] == "--model" {
            match rest.get(index + 1) {
                Some(value) if model.is_none() => model = Some(value.clone()),
                _ => return Some(link_usage()),
            }
            index += 2;
        } else {
            plain.push(rest[index].clone());
            index += 1;
        }
    }
    let rest = plain.as_slice();
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
    // The model is checked (value and fit) before the link is written: a refusal creates no link and no override.
    let mut warnings: Vec<String> = Vec::new();
    if let Some(model) = &model {
        if first != "requirement" {
            return Some(fail(
                ErrorCode::InvalidRequest,
                "--model applies to a requirement link only",
            ));
        }
        match super::plans::check_fit(env, None, model) {
            Ok(found) => warnings = found,
            Err(refusal) => return Some(refusal),
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
    let set_ref = ref_id.clone();
    let linked = env.deps.kernel.run(move |core| {
        let context = new_context(core, &credential)?;
        let link = core.link_external(&context, &Value::Object(input))?;
        let set = match &model {
            Some(model) => {
                let context = new_context(core, &credential)?;
                Some(core.set_model_override(
                    &context,
                    &json!({"scopeType": "requirement", "scopeId": set_ref, "roleKind": null, "model": model}),
                )?)
            }
            None => None,
        };
        Ok((link, set, model))
    });
    Some(match linked {
        Ok((link, set, model)) => {
            let field = |key: &str| link.get(key).cloned().unwrap_or(Value::Null);
            let mut result = Map::new();
            for key in [
                "refKind",
                "refId",
                "externalId",
                "syncedState",
                "wanted",
                "drift",
            ] {
                result.insert(key.into(), field(key));
            }
            if let (Some(_), Some(model)) = (set, model) {
                result.insert(
                    "modelOverrides".into(),
                    json!([{"roleKind": null, "model": model}]),
                );
            }
            if !warnings.is_empty() {
                result.insert("warnings".into(), json!(warnings));
            }
            ok(Value::Object(result))
        }
        Err(error) => refuse(&error),
    })
}

/// `link model <requirement-ref-id> [<Kind>] [<id> | --clear]`.
fn link_model(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> CommandResponse {
    let rest = &call.args[1..];
    let Some(ref_id) = rest.first().filter(|id| is_safe_agent_id(id)) else {
        return fail(
            ErrorCode::InvalidRequest,
            "link model needs a requirement ref id, then optionally a kind and a model id or --clear",
        );
    };
    match super::plans::model_command(env, call, "requirement", ref_id, &rest[1..]) {
        Ok(response) => response,
        Err(error) => refuse(&error),
    }
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
