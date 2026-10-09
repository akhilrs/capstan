//! The message commands (src/commands/messages.ts: inbox, ack, send, cancel, resolve).
//!
//! Node runs the steps of one command with no await between them, so nothing else touches the ledger meanwhile. Each
//! handler here does the same: all its ledger steps are one closure on the kernel thread, which returns the answer.

use super::env::{
    agent_from_value, delivered_messages, describe_message, is_architect_of, pull_for,
};
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
            })))
        });
    }
    if !args.is_empty() {
        return Some(fail(ErrorCode::InvalidRequest, "inbox takes no arguments"));
    }
    if !is_agent {
        return Some(fail(ErrorCode::Forbidden, "the caller is not an agent"));
    }
    on_kernel(env, move |core| {
        pull_for(core, &credential)?;
        let messages = delivered_messages(core, &credential)?;
        let action_needed = messages
            .iter()
            .filter(|message| message["actionNeeded"].as_bool().unwrap_or(false))
            .count();
        Ok(ok(json!({
            "messages": messages,
            "count": messages.len(),
            "actionNeededCount": action_needed,
        })))
    })
}

fn ack(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    if call.args.len() != 1 {
        return Some(fail(ErrorCode::InvalidRequest, "ack needs one message id"));
    }
    let (credential, message_id) = (call.credential.to_string(), call.args[0].clone());
    on_kernel(env, move |core| {
        let context = new_context(core, &credential)?;
        let record = core.ack_message(&context, &message_id)?;
        Ok(ok(
            json!({"messageId": record["messageId"], "state": record["state"]}),
        ))
    })
}

fn send(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let action_needed = call.args.first().is_some_and(|arg| arg == "--action");
    let args: Vec<String> = if action_needed {
        call.args[1..].to_vec()
    } else {
        call.args.to_vec()
    };
    if args.len() != 2 {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "send needs a recipient and a text, after an optional --action",
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
