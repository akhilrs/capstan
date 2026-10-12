//! The message commands (src/commands/messages.ts: inbox, ack, send, cancel, resolve).
//!
//! Node runs the steps of one command with no await between them, so nothing else touches the ledger meanwhile. Each
//! handler here does the same: all its ledger steps are one closure on the kernel thread, which returns the answer.

use super::env::{agent_from_value, describe_message, inbox_reply, is_architect_of};
use super::shared::{
    fail, imitates_frame, is_safe_agent_id, map_kernel_error, ok, CommandCall, CommandEnv,
    CommandResponse, ErrorCode, MAX_SEND_BODY_BYTES,
};
use super::HandlerMap;
use crate::deps::new_context;
use capstan_herdr::api::is_agent_name;
use capstan_kernel::areas::messaging::RESOLUTION_DECISIONS;
use capstan_kernel::types::AgentRecord;
use capstan_kernel::{Core, KernelResult};
use serde_json::{json, Map, Value};

/// Registers the handlers of the routes this module serves (inbox, ack, send, cancel, resolve).
pub fn register(map: &mut HandlerMap) {
    map.insert("inbox", inbox);
    map.insert("ack", ack);
    map.insert("send", send);
    map.insert("resolve", resolve);
    map.insert("cancel", cancel);
}

/// Runs `step` on the kernel thread; a failure of the ledger is `mapError`'s answer.
fn on_kernel<F>(env: &CommandEnv<'_>, step: F) -> Option<CommandResponse>
where
    F: FnOnce(&Core) -> KernelResult<CommandResponse> + Send + 'static,
{
    Some(
        env.deps
            .kernel
            .run(step)
            .unwrap_or_else(|error| map_kernel_error(&error)),
    )
}

fn inbox(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let credential = call.credential.to_string();
    let args = call.args.to_vec();
    if call.identity.role == "operator" {
        if args.len() != 1 {
            return Some(fail(
                ErrorCode::InvalidRequest,
                "inbox needs exactly one agent id",
            ));
        }
        let agent_id = args[0].clone();
        if !is_safe_agent_id(&agent_id) {
            return Some(fail(ErrorCode::InvalidRequest, "the agent id is not valid"));
        }
        return on_kernel(env, move |core| {
            if core.agent_record(&agent_id)?.is_null() {
                return Ok(fail(ErrorCode::UnknownAgent, "no agent has this id"));
            }
            let mut messages = Vec::new();
            for row in core
                .agent_inbox(&credential, Some(&agent_id))?
                .as_array()
                .into_iter()
                .flatten()
            {
                messages.push(describe_message(core, row)?);
            }
            Ok(ok(json!({ "messages": messages })))
        });
    }
    let is_agent = call.identity.agent.is_some();
    if args.len() == 1 && args[0] == "--hook" {
        // Read-only: the hook runs after every tool call and never pulls or prints a body.
        if !is_agent {
            return Some(fail(ErrorCode::Forbidden, "the caller is not an agent"));
        }
        return on_kernel(env, move |core| {
            let summary = core.unread_summary(&credential)?;
            Ok(ok(json!({
                "count": summary["count"],
                "oldestQueuedAt": summary["oldestQueuedAt"],
                "messageIds": summary["messageIds"],
                "actionNeeded": summary["actionNeeded"],
                "wakeCount": summary["wakeCount"],
                "infoCount": summary["infoCount"],
            })))
        });
    }
    // `--digest` (the current client) asks for info mail apart and acknowledged; no argument is the earlier shape.
    let digest = args.len() == 1 && args[0] == "--digest";
    if !args.is_empty() && !digest {
        return Some(fail(ErrorCode::InvalidRequest, "inbox takes no arguments"));
    }
    if !is_agent {
        return Some(fail(ErrorCode::Forbidden, "the caller is not an agent"));
    }
    let auto_ack_info = env
        .config()
        .is_none_or(|config| config.messages.auto_ack_info);
    on_kernel(env, move |core| {
        let (_, reply) = inbox_reply(core, &credential, digest, auto_ack_info)?;
        Ok(ok(Value::Object(reply)))
    })
}

fn ack(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let credential = call.credential.to_string();
    let args = call.args.to_vec();
    if args.len() == 1 && args[0] == "--all-read" {
        return on_kernel(env, move |core| {
            let context = new_context(core, &credential)?;
            let ids = core.ack_all_read(&context)?;
            let acked: Vec<Value> = ids
                .as_array()
                .into_iter()
                .flatten()
                .map(|id| json!({"messageId": id, "state": "acked"}))
                .collect();
            Ok(ok(json!({"acked": acked, "count": acked.len()})))
        });
    }
    if args.is_empty() || args.iter().any(|arg| arg.starts_with("--")) {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "ack needs one or more message ids, or --all-read alone",
        ));
    }
    on_kernel(env, move |core| {
        let context = new_context(core, &credential)?;
        // One id takes the single ack, which draws no more ids than it always did; several are validated together.
        let records = match args.as_slice() {
            [only] => json!([core.ack_message(&context, only)?]),
            _ => core.ack_messages(&context, &args)?,
        };
        let acked: Vec<Value> = records
            .as_array()
            .into_iter()
            .flatten()
            .map(|record| json!({"messageId": record["messageId"], "state": record["state"]}))
            .collect();
        let mut reply = Map::new();
        // One id keeps the earlier answer's members.
        if let [only] = acked.as_slice() {
            reply.insert("messageId".into(), only["messageId"].clone());
            reply.insert("state".into(), only["state"].clone());
        }
        reply.insert("count".into(), json!(acked.len()));
        reply.insert("acked".into(), Value::Array(acked));
        Ok(ok(Value::Object(reply)))
    })
}

fn send(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let flags = call
        .args
        .iter()
        .take_while(|arg| matches!(arg.as_str(), "--action" | "--info"))
        .count();
    let args: Vec<String> = call.args[flags..].to_vec();
    let has = |flag: &str| call.args[..flags].iter().any(|arg| arg == flag);
    let (action_flag, info_flag) = (has("--action"), has("--info"));
    if action_flag && info_flag {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "send takes --action or --info, not both",
        ));
    }
    if flags > 1 || args.len() != 2 {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "send needs a recipient and a text, after an optional --action or --info",
        ));
    }
    let (target, body) = (args[0].clone(), args[1].clone());
    if body.len() > MAX_SEND_BODY_BYTES {
        return Some(fail(
            ErrorCode::BodyTooLarge,
            format!("a message body may be at most {MAX_SEND_BODY_BYTES} bytes"),
        ));
    }
    if imitates_frame(&body) {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "a message body must not contain a line that looks like a Capstan message frame",
        ));
    }
    if target != "@pm" && !is_safe_agent_id(&target) {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "the recipient id is not valid",
        ));
    }
    let caller: Option<AgentRecord> = call.identity.agent.clone();
    let config = env.deps.options.capstan.clone();
    // Without a flag the class is the config's `send_default` (action when there is no config).
    let info = info_flag
        || (!action_flag
            && config
                .as_deref()
                .is_some_and(|config| config.messages.send_default.as_str() == "info"));
    let action_needed = action_flag;
    let credential = call.credential.to_string();
    on_kernel(env, move |core| {
        let recipient: Option<AgentRecord> = if target == "@pm" {
            let pms: Vec<AgentRecord> = core
                .active_agents()?
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(agent_from_value)
                .filter(|agent| agent.kind == "PM")
                .collect();
            if pms.len() > 1 {
                return Ok(fail(
                    ErrorCode::AmbiguousRecipient,
                    "more than one PM is active; name the agent id",
                ));
            }
            pms.into_iter().next()
        } else {
            agent_from_value(&core.agent_record(&target)?)
        };
        let Some(recipient) = recipient.filter(|recipient| recipient.state == "active") else {
            return Ok(fail(
                ErrorCode::UnknownRecipient,
                "the recipient is not an active agent",
            ));
        };
        if let (Some(caller), Some(config)) = (&caller, config.as_deref()) {
            if caller.kind != "PM"
                && config.operator.enabled
                && recipient.kind == "Developer"
                && recipient.role_name == config.operator.role
            {
                return Ok(fail(
                    ErrorCode::RecipientNotAllowed,
                    "only the PM sends to the operator agent; the controller tells it what it needs",
                ));
            }
        }
        if let Some(caller) = &caller {
            if caller.agent_id == recipient.agent_id {
                return Ok(fail(ErrorCode::SelfSend, "an agent cannot send to itself"));
            }
            let architect = |agent: &AgentRecord| is_architect_of(config.as_deref(), Some(agent));
            if caller.kind != "PM"
                && recipient.kind != "PM"
                && !(caller.kind == "Developer" && architect(&recipient))
                && !(architect(caller) && recipient.kind == "Developer")
            {
                return Ok(fail(
                    ErrorCode::RecipientNotAllowed,
                    "an agent other than the PM may send only to the PM, a developer to the architect, or the architect to a developer",
                ));
            }
        }
        if recipient.kind != "PM" && !is_agent_name(&recipient.agent_id) {
            return Ok(fail(
                ErrorCode::RecipientNotDeliverable,
                "this agent id cannot be used as a Herdr agent name",
            ));
        }
        let context = new_context(core, &credential)?;
        let queued = core.enqueue_message(
            &context,
            &json!({
                "recipientAgentId": recipient.agent_id,
                "body": body,
                "actionNeeded": action_needed,
                "class": if info { "info" } else { "action" },
            }),
        )?;
        Ok(ok(json!({"messageId": queued["messageId"]})))
    })
}

fn resolve(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    if call.args.len() < 2 || call.args.len() > 3 {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "resolve needs a message id, a decision and an optional note",
        ));
    }
    let (message_id, decision) = (call.args[0].clone(), call.args[1].clone());
    let note = call.args.get(2).cloned();
    if !RESOLUTION_DECISIONS.contains(&decision.as_str()) {
        return Some(fail(
            ErrorCode::InvalidRequest,
            format!(
                "the decision must be one of {}",
                RESOLUTION_DECISIONS.join(", ")
            ),
        ));
    }
    let credential = call.credential.to_string();
    on_kernel(env, move |core| {
        let before = core.message(&message_id)?["state"]
            .as_str()
            .map(str::to_string);
        let context = new_context(core, &credential)?;
        let record =
            core.resolve_message(&context, &message_id, &json!(decision), note.as_deref())?;
        let warn = decision == "retry" && matches!(before.as_deref(), Some("sent" | "unacked"));
        let mut result = Map::new();
        result.insert("messageId".into(), record["messageId"].clone());
        result.insert("state".into(), record["state"].clone());
        if warn {
            result.insert(
                "warning".into(),
                json!("the recipient may already have received this message"),
            );
        }
        Ok(ok(Value::Object(result)))
    })
}

fn cancel(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    if call.args.len() != 1 {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "cancel needs one message id",
        ));
    }
    let (credential, message_id) = (call.credential.to_string(), call.args[0].clone());
    on_kernel(env, move |core| {
        let context = new_context(core, &credential)?;
        let record = core.resolve_message(&context, &message_id, &json!("cancel"), None)?;
        Ok(ok(
            json!({"messageId": record["messageId"], "state": record["state"]}),
        ))
    })
}
