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
use crate::file_handoff;
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
        .take_while(|arg| matches!(arg.as_str(), "--action" | "--info" | "--answered"))
        .count();
    let args: Vec<String> = call.args[flags..].to_vec();
    let has = |flag: &str| call.args[..flags].iter().any(|arg| arg == flag);
    let (action_flag, info_flag) = (has("--action"), has("--info"));
    if has("--answered") {
        if flags != 1 || args.len() != 2 || args[0] != "@user" {
            return Some(fail(
                ErrorCode::InvalidRequest,
                "send --answered takes @user and the number of your question, and no other flag",
            ));
        }
        return Some(close_user_question(call, env, &args[1]));
    }
    if action_flag && info_flag {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "send takes --action or --info, not both",
        ));
    }
    if args.get(1).map(String::as_str) == Some("--file") {
        return Some(send_file(env, call, &args, action_flag, info_flag));
    }
    // `@user <text> --option <a> --option <b>`: a question's choices follow its text.
    let mut options: Vec<String> = Vec::new();
    let args: Vec<String> =
        if args.first().map(String::as_str) == Some("@user") && args.len() > 2 {
            let mut rest = args[2..].iter();
            while let Some(word) = rest.next() {
                match (word.as_str(), rest.next()) {
                    ("--option", Some(value)) => options.push(value.clone()),
                    _ => return Some(fail(
                        ErrorCode::InvalidRequest,
                        "after the text, send @user takes only --option <choice>, once per choice",
                    )),
                }
            }
            args[..2].to_vec()
        } else {
            args
        };
    if flags > 1 || args.len() != 2 {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "send needs a recipient and a text, after an optional --action or --info",
        ));
    }
    let (target, body) = (args[0].clone(), args[1].clone());
    let (limit, from_config) = body_limit(env, call);
    if body.len() > limit {
        return Some(fail(
            ErrorCode::BodyTooLarge,
            if !from_config {
                format!("a message body may be at most {MAX_SEND_BODY_BYTES} bytes")
            } else {
                format!(
                    "a message body may be at most {limit} bytes ([messages] max_body_bytes); write the text to a file on your branch, commit it and send cstan send --file <path> --summary <text>"
                )
            },
        ));
    }
    if imitates_frame(&body) {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "a message body must not contain a line that looks like a Capstan message frame",
        ));
    }
    deliver(env, call, target, body, action_flag, info_flag, options)
}

/// The most bytes a body may have and whether it comes from the config: `messages.max_body_bytes` for an agent other
/// than the PM, else the daemon's limit.
fn body_limit(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> (usize, bool) {
    match (&call.identity.agent, env.deps.options.capstan.as_deref()) {
        (Some(agent), Some(config)) if agent.kind != "PM" => (
            usize::try_from(config.messages.max_body_bytes)
                .map_or(MAX_SEND_BODY_BYTES, |n| n.min(MAX_SEND_BODY_BYTES)),
            true,
        ),
        _ => (MAX_SEND_BODY_BYTES, false),
    }
}

/// `send [--info|--action] <recipient> --file <path> [--summary <text>]`: hands over a file committed on the caller's
/// branch by reference.
fn send_file(
    env: &CommandEnv<'_>,
    call: &CommandCall<'_>,
    args: &[String],
    action_flag: bool,
    info_flag: bool,
) -> CommandResponse {
    let usage = "send --file takes a recipient, --file <path> and an optional --summary <text>";
    if args.len() != 3 && !(args.len() == 5 && args[3] == "--summary") {
        return fail(ErrorCode::InvalidRequest, usage);
    }
    let (target, path) = (args[0].clone(), args[2].clone());
    let summary = args.get(4).cloned();
    if target != "@user" && target != "@pm" && !is_safe_agent_id(&target) {
        return fail(ErrorCode::InvalidRequest, "the recipient id is not valid");
    }
    let Some(caller) = call.identity.agent.clone() else {
        return fail(ErrorCode::InvalidRequest, NEEDS_BRANCH);
    };
    if !file_handoff::valid_path(&path) {
        return fail(ErrorCode::InvalidRequest, file_handoff::PATH_RULE);
    }
    if let Some(summary) = &summary {
        if summary.len() > file_handoff::MAX_SUMMARY_BYTES {
            return fail(
                ErrorCode::InvalidRequest,
                "the summary may be at most 1000 bytes",
            );
        }
        if imitates_frame(summary) {
            return fail(
                ErrorCode::InvalidRequest,
                "a message body must not contain a line that looks like a Capstan message frame",
            );
        }
    }
    let controller = env.deps.credential().to_string();
    let agent_id = caller.agent_id.clone();
    let row = env.deps.kernel.run(move |core| {
        let panes = core.agent_panes(&controller)?;
        Ok(panes
            .as_array()
            .into_iter()
            .flatten()
            .find(|row| row["agentId"] == agent_id.as_str())
            .map(|row| {
                (
                    row["branch"].as_str().map(str::to_string),
                    row["worktreePath"].as_str().map(str::to_string),
                )
            }))
    });
    let (branch, worktree) = match row {
        Ok(Some((Some(branch), Some(worktree)))) => (branch, worktree),
        Ok(_) => return fail(ErrorCode::InvalidRequest, NEEDS_BRANCH),
        Err(error) => return map_kernel_error(&error),
    };
    let found =
        match file_handoff::locate(&env.deps.options.workspace_root, &branch, &worktree, &path) {
            Ok(found) => found,
            Err(message) => return fail(ErrorCode::InvalidRequest, message),
        };
    let body = file_handoff::body(&path, &branch, &found, summary.as_deref());
    let answer = deliver(env, call, target, body, action_flag, info_flag, Vec::new())
        .unwrap_or_else(|| fail(ErrorCode::InvalidRequest, "the controller is not answering"));
    match answer {
        CommandResponse::Ok(mut result) => {
            if let Some(map) = result.as_object_mut() {
                map.insert(
                    "file".into(),
                    json!({"path": path, "branch": branch, "commit": found.commit, "size": found.size}),
                );
                if found.uncommitted {
                    map.insert("uncommitted".into(), json!(true));
                    map.insert(
                        "warning".into(),
                        json!(format!(
                            "{path} has uncommitted changes; the message names the committed version"
                        )),
                    );
                }
            }
            ok(result)
        }
        other => other,
    }
}

const NEEDS_BRANCH: &str =
    "send --file needs your own branch: only an agent with a worktree can hand over a file";

/// Queues `body` for `target` (an agent id, `@pm` or `@user`).
fn deliver(
    env: &CommandEnv<'_>,
    call: &CommandCall<'_>,
    target: String,
    body: String,
    action_flag: bool,
    info_flag: bool,
    options: Vec<String>,
) -> Option<CommandResponse> {
    if target == "@user" {
        return Some(send_to_user(env, call, body, action_flag, options));
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

/// `send [--action|--info] @user <text>`: an agent sends the user a message for the remote clients. `--action` asks the
/// user something (it waits in the waiting list until answered); otherwise it is info.
fn send_to_user(
    env: &CommandEnv<'_>,
    call: &CommandCall<'_>,
    body: String,
    action: bool,
    options: Vec<String>,
) -> CommandResponse {
    if call.identity.agent.is_none() {
        return fail(
            ErrorCode::InvalidRequest,
            "only an agent sends the user a message",
        );
    }
    let credential = call.credential.to_string();
    on_kernel(env, move |core| {
        let context = new_context(core, &credential)?;
        let sent = core.send_to_user(
            &context,
            &json!({"body": body, "class": if action { "action" } else { "info" }, "options": options}),
        )?;
        Ok(ok(json!({"messageId": sent["messageId"], "sequence": sent["sequence"]})))
    })
    .unwrap_or_else(|| fail(ErrorCode::InvalidRequest, "the controller is not answering"))
}

/// `send --answered @user <n>`: the agent's question `n` was answered in its terminal, so the remote clients stop
/// showing it as waiting.
fn close_user_question(
    call: &CommandCall<'_>,
    env: &CommandEnv<'_>,
    number: &str,
) -> CommandResponse {
    let Ok(sequence) = number.parse::<i64>() else {
        return fail(
            ErrorCode::InvalidRequest,
            "the question number must be a positive integer",
        );
    };
    if call.identity.agent.is_none() {
        return fail(
            ErrorCode::InvalidRequest,
            "only an agent closes its question",
        );
    }
    let credential = call.credential.to_string();
    on_kernel(env, move |core| {
        let context = new_context(core, &credential)?;
        let closed = core.close_user_question(&context, &json!({"sequence": sequence}))?;
        Ok(ok(
            json!({"messageId": closed["messageId"], "sequence": closed["sequence"]}),
        ))
    })
    .unwrap_or_else(|| fail(ErrorCode::InvalidRequest, "the controller is not answering"))
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
