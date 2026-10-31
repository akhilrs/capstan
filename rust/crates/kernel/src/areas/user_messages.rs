//! Messages from an agent to the user (`cstan send @user`), the other half of the remote conversation with the PM.
//!
//! An agent writes one with `user_message.send`; an agent whose question the user answered in the terminal closes it
//! with `user_message.close` (`cstan send --answered @user <n>`). An `action` message asks the user something: it is `open` and waits in
//! the snapshot's waiting list (kind `message`) until the user answers it with the `message reply` decision, which sends
//! the answer to the agent as an action message and marks the row `answered`. An `info` message only tells the user
//! something (state `info`). Every message is listed in the snapshot's `messages` section, latest
//! [`LISTED_MESSAGES`] only.
//!
//! The body is the agent's text: clients show it as plain text, never as a link or rich text, and never in a
//! notification.

use crate::auth::AuthenticatedActor;
use crate::canonical::sha256;
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{execute, query_opt, safe_text};
use crate::kernel::Kernel;
use crate::records::{MutationEvent, MutationOutput};
use crate::types::MutationContext;
use serde_json::{json, Value};

/// The most bytes of a message body.
pub const MAX_USER_MESSAGE_BYTES: usize = 4096;
/// The most messages the agents of a project may send the user in an hour.
pub const MESSAGES_PER_HOUR: i64 = 60;
/// How many of the latest messages the snapshot lists.
pub const LISTED_MESSAGES: i64 = 200;

/// A stored message to the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserMessageRow {
    pub message_id: String,
    pub sequence: i64,
    pub sender_agent_id: String,
    pub class: String,
    pub body: String,
    pub state: String,
    pub reply_message_id: Option<String>,
    pub created_at: String,
    pub answered_at: Option<String>,
}

const COLUMNS: &str =
    "message_id, sequence, sender_agent_id, class, body, state, reply_message_id, created_at, answered_at";

fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<UserMessageRow> {
    Ok(UserMessageRow {
        message_id: row.get(0)?,
        sequence: row.get(1)?,
        sender_agent_id: row.get(2)?,
        class: row.get(3)?,
        body: row.get(4)?,
        state: row.get(5)?,
        reply_message_id: row.get(6)?,
        created_at: row.get(7)?,
        answered_at: row.get(8)?,
    })
}

fn refused(code: &str, message: &str) -> KernelError {
    KernelError::MessageTransition {
        code: code.into(),
        message: message.into(),
    }
}

/// `user_message.send`: the calling agent sends the user a message. Input `{body, class}`, class `action` or `info`
/// (default `info`). Refused for a caller that is not an active agent (`not_an_agent`) and past
/// [`MESSAGES_PER_HOUR`] messages of the project in the last hour (`limit_reached`). Returns `{messageId, sequence}`.
pub fn send_to_user(kernel: &Kernel, context: &MutationContext, input: &Value) -> KernelResult<Value> {
    let body = safe_text(
        input.get("body").unwrap_or(&Value::Null),
        "message body",
        MAX_USER_MESSAGE_BYTES,
        true,
    )?;
    if body.len() > MAX_USER_MESSAGE_BYTES {
        return Err(KernelError::type_error(format!(
            "message body must be at most {MAX_USER_MESSAGE_BYTES} bytes"
        )));
    }
    let class = match input.get("class").and_then(Value::as_str).unwrap_or("info") {
        "action" => "action",
        "info" => "info",
        other => return Err(KernelError::type_error(format!("unknown message class {other}"))),
    };
    let body_hash = sha256(&body);
    kernel.mutate(
        context,
        "user_message.send",
        "message:send",
        &json!({"class": class, "bodyHash": body_hash, "bodyBytes": body.len()}),
        |actor| apply_send(kernel, actor, &body, &body_hash, class),
    )
}

fn apply_send(
    kernel: &Kernel,
    actor: &AuthenticatedActor,
    body: &str,
    body_hash: &str,
    class: &str,
) -> KernelResult<MutationOutput> {
    let Some(sender) = kernel.agent_by_actor(&actor.actor_id)? else {
        return Err(refused("not_an_agent", "only an active agent sends the user a message"));
    };
    let now = kernel.now();
    let recent = query_opt(
        &kernel.database,
        "SELECT COUNT(*) FROM user_messages
          WHERE project_id = ? AND julianday(created_at) > julianday(?) - 1.0 / 24.0",
        rusqlite::params![kernel.project_id, now],
        |row| row.get::<_, i64>(0),
    )?
    .unwrap_or(0);
    if recent >= MESSAGES_PER_HOUR {
        return Err(refused(
            "limit_reached",
            "the agents of this project sent the user 60 messages in the last hour; wait",
        ));
    }
    let message_id = kernel.env.uuid();
    let sequence = query_opt(
        &kernel.database,
        "SELECT COALESCE(MAX(sequence), 0) + 1 FROM user_messages WHERE project_id = ?",
        [&kernel.project_id],
        |row| row.get::<_, i64>(0),
    )?
    .unwrap_or(1);
    let state = if class == "action" { "open" } else { "info" };
    execute(
        &kernel.database,
        "INSERT INTO user_messages(project_id, message_id, sequence, sender_agent_id, sender_actor_id, class, body,
           body_hash, state, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        rusqlite::params![
            kernel.project_id,
            message_id,
            sequence,
            sender.agent_id,
            actor.actor_id,
            class,
            body,
            body_hash,
            state,
            now
        ],
    )?;
    kernel.touch_agent(&sender.agent_id, &now)?;
    let mut event = MutationEvent::new("user_message", &message_id, 0).with_details(json!({
        "senderAgentId": sender.agent_id,
        "class": class,
        "bodyHash": body_hash,
        "sequence": sequence,
    }));
    event.to_state = Some(state.into());
    Ok(MutationOutput::new(
        json!({"messageId": message_id, "sequence": sequence}),
        event,
    ))
}

/// `user_message.close`: the agent that asked question `sequence` says the user answered it in the terminal, so it
/// stops waiting on the remote clients. Input `{sequence}`. Refused when the question is not the caller's
/// (`not_yours`) or not open (`not_open`). Returns `{messageId, sequence}`.
pub fn close_question(kernel: &Kernel, context: &MutationContext, input: &Value) -> KernelResult<Value> {
    let sequence = input
        .get("sequence")
        .and_then(Value::as_i64)
        .filter(|n| *n > 0)
        .ok_or_else(|| KernelError::type_error("the question number must be a positive integer"))?;
    kernel.mutate(
        context,
        "user_message.close",
        "message:send",
        &json!({"sequence": sequence}),
        |actor| {
            let Some(caller) = kernel.agent_by_actor(&actor.actor_id)? else {
                return Err(refused("not_an_agent", "only an active agent closes its question"));
            };
            let row = query_opt(
                &kernel.database,
                &format!("SELECT {COLUMNS} FROM user_messages WHERE project_id = ? AND sequence = ?"),
                rusqlite::params![kernel.project_id, sequence],
                from_row,
            )?
            .ok_or_else(|| refused("not_open", "there is no such question"))?;
            if row.sender_agent_id != caller.agent_id {
                return Err(refused("not_yours", "only the agent that asked closes a question"));
            }
            if row.state != "open" {
                return Err(refused("not_open", "the question is not waiting for an answer"));
            }
            execute(
                &kernel.database,
                "UPDATE user_messages SET state = 'answered', answered_at = ?
                  WHERE project_id = ? AND message_id = ? AND state = 'open'",
                rusqlite::params![kernel.now(), kernel.project_id, row.message_id],
            )?;
            let event = MutationEvent::new("user_message", &row.message_id, 1)
                .transition("open", "answered")
                .with_details(json!({"sequence": sequence, "answeredIn": "terminal"}));
            Ok(MutationOutput::new(
                json!({"messageId": row.message_id, "sequence": sequence}),
                event,
            ))
        },
    )
}

/// The message with this id, if any.
pub fn user_message_row(kernel: &Kernel, message_id: &str) -> KernelResult<Option<UserMessageRow>> {
    query_opt(
        &kernel.database,
        &format!("SELECT {COLUMNS} FROM user_messages WHERE project_id = ? AND message_id = ?"),
        rusqlite::params![kernel.project_id, message_id],
        from_row,
    )
}

/// Marks an open message answered by the message `reply_message_id` to its sender. Run inside the decision's
/// transaction. False when the message was not open.
pub fn mark_answered(kernel: &Kernel, message_id: &str, reply_message_id: &str) -> KernelResult<bool> {
    let now = kernel.now();
    let changed = execute(
        &kernel.database,
        "UPDATE user_messages SET state = 'answered', reply_message_id = ?, answered_at = ?
          WHERE project_id = ? AND message_id = ? AND state = 'open'",
        rusqlite::params![reply_message_id, now, kernel.project_id, message_id],
    )?;
    Ok(changed == 1)
}
