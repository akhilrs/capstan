//! The messages (src/controller/messages.ts): the queue, delivery bookkeeping, acknowledgement and resolution.

use crate::auth::{authenticate_actor, require_capability, AuthenticatedActor};
use crate::canonical::sha256;
use crate::dispatch::{arg, call, Args};
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{
    cancelled_reason, execute, is_message_rejection, message_record, query_all, query_opt, safe_id,
    safe_id_str, safe_text,
};
use crate::kernel::Kernel;
use crate::records::{
    is_final_state, AgentRow, MessageRow, MutationEvent, MutationOutput, MAX_INPUT_CLEAR_BYTES,
    MAX_MESSAGE_BYTES,
};
use crate::types::MutationContext;
use rusqlite::types::Value as Sql;
use serde_json::{json, Value};

use super::message_notices;
use super::messaging::{
    is_legal_transition, queue_head, resolution_target, DEFERRAL_REASONS, RESOLUTION_DECISIONS,
};
use super::pauses;

/// The states that are not final, as an SQL list; a query on it reads `messages` through `INDEXED BY messages_by_state`.
pub const OPEN_MESSAGE_STATES_SQL: &str =
    "'queued', 'deferred', 'sent', 'unacked', 'expired', 'failed'";

fn record_value(row: &MessageRow) -> Value {
    serde_json::to_value(message_record(row)).expect("a message record serializes")
}

fn records_value(rows: &[MessageRow]) -> Value {
    Value::Array(rows.iter().map(record_value).collect())
}

fn field<'a>(input: &'a Value, key: &str) -> &'a Value {
    input.get(key).unwrap_or(&Value::Null)
}

/// What `reject` records about a refused message operation.
pub struct Rejection {
    pub message_id: Option<String>,
    pub code: String,
    pub message: String,
    pub from_state: Option<String>,
    pub attempted_state: Option<String>,
}

impl Rejection {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            message_id: None,
            code: code.into(),
            message: message.into(),
            from_state: None,
            attempted_state: None,
        }
    }

    pub fn message_id(mut self, id: &str) -> Self {
        self.message_id = Some(id.into());
        self
    }

    pub fn from_state(mut self, state: Option<&str>) -> Self {
        self.from_state = state.map(str::to_string);
        self
    }

    pub fn attempted(mut self, state: &str) -> Self {
        self.attempted_state = Some(state.into());
        self
    }
}

/// `enqueueMessage`.
pub fn enqueue_message(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let recipient_id = safe_id(field(input, "recipientAgentId"), "recipient agent id")?.to_string();
    safe_text(
        field(input, "body"),
        "message body",
        MAX_MESSAGE_BYTES,
        true,
    )?;
    let body = field(input, "body").as_str().unwrap_or("").to_string();
    let bytes = body.len();
    if bytes > MAX_MESSAGE_BYTES {
        return Err(KernelError::type_error(format!(
            "message body must be at most {MAX_MESSAGE_BYTES} bytes"
        )));
    }
    let body_hash = sha256(&body);
    let action_needed = field(input, "actionNeeded") == &Value::Bool(true);
    message_mutation(
        kernel,
        context,
        "message.enqueue",
        "message:send",
        &json!({"recipientAgentId": recipient_id, "bodyHash": body_hash, "bodyBytes": bytes}),
        |actor| {
            let agent = match kernel.agent_row(&recipient_id)? {
                Some(agent) if agent.state == "active" => agent,
                _ => {
                    return reject(
                        kernel,
                        actor,
                        "message.enqueue",
                        Rejection::new(
                            "unknown_recipient",
                            "message recipient is not an active agent",
                        ),
                    )
                }
            };
            let now = kernel.now();
            let message_id = insert_queued_message(
                kernel,
                &actor.actor_id,
                &agent,
                &body,
                &body_hash,
                &now,
                action_needed,
            )?;
            if let Some(sender) = kernel.agent_by_actor(&actor.actor_id)? {
                kernel.touch_agent(&sender.agent_id, &now)?;
            }
            Ok(MutationOutput::new(json!({"messageId": message_id}), {
                let mut event = MutationEvent::new("message", &message_id, 0).with_details(
                    json!({"recipientAgentId": agent.agent_id, "bodyHash": body_hash}),
                );
                event.to_state = Some("queued".into());
                event
            }))
        },
    )
}

/// `message`.
pub fn message(kernel: &Kernel, message_id: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    safe_id_str(message_id, "message id")?;
    Ok(message_row(kernel, message_id)?.map_or(Value::Null, |row| record_value(&row)))
}

/// `messagesFor`.
pub fn messages_for(kernel: &Kernel, agent_id: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    safe_id_str(agent_id, "agent id")?;
    Ok(records_value(&message_rows_for(kernel, agent_id)?))
}

/// `openMessagesFor`: the agent's messages that are not final, oldest first.
pub fn open_messages_for(kernel: &Kernel, agent_id: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    safe_id_str(agent_id, "agent id")?;
    Ok(records_value(&open_message_rows_for(kernel, agent_id)?))
}

fn check_limit(limit: i64) -> KernelResult<()> {
    if !(1..=1000).contains(&limit) {
        return Err(KernelError::type_error(
            "limit must be an integer from 1 to 1000",
        ));
    }
    Ok(())
}

/// `unresolvedMessages`: unresolved messages of every agent, notified ones first, then oldest first by sequence.
pub fn unresolved_messages(kernel: &Kernel, credential: &str, limit: i64) -> KernelResult<Value> {
    kernel.authorize(credential, "controller:reconcile")?;
    check_limit(limit)?;
    let rows = query_all(
        &kernel.database,
        &format!("SELECT * FROM messages INDEXED BY messages_by_state WHERE project_id = ? AND state IN ({OPEN_MESSAGE_STATES_SQL}) ORDER BY (last_notified_at IS NULL), sequence LIMIT ?"),
        rusqlite::params![kernel.project_id, limit + 1],
        MessageRow::from_row,
    )?;
    let shown = &rows[..rows.len().min(limit as usize)];
    Ok(json!({
        "messages": shown.iter().map(record_value).collect::<Vec<_>>(),
        "truncated": rows.len() > limit as usize,
    }))
}

/// `inputClears`: the newest input clears, without their text.
pub fn input_clears(kernel: &Kernel, credential: &str, limit: i64) -> KernelResult<Value> {
    kernel.authorize(credential, "controller:reconcile")?;
    check_limit(limit)?;
    let rows = query_all(
        &kernel.database,
        "SELECT clear_id, message_id, created_at FROM message_input_clears WHERE project_id = ? ORDER BY created_at DESC, clear_id LIMIT ?",
        rusqlite::params![kernel.project_id, limit],
        |row| {
            Ok(json!({
                "clearId": row.get::<_, String>(0)?,
                "messageId": row.get::<_, String>(1)?,
                "recordedAt": row.get::<_, String>(2)?,
            }))
        },
    )?;
    Ok(Value::Array(rows))
}

/// `senderOf`: who an actor is, for showing a message's sender.
pub fn sender_of(kernel: &Kernel, actor_id: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    let row = query_opt(
        &kernel.database,
        "SELECT a.role AS kind, g.agent_id AS agent_id, g.role_name AS role_name
         FROM actors a LEFT JOIN agents g ON g.project_id = a.project_id AND g.actor_id = a.actor_id
         WHERE a.project_id = ? AND a.actor_id = ?",
        [&kernel.project_id, actor_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        },
    )?;
    Ok(match row {
        None => json!({"role": "unknown", "agentId": null}),
        Some((kind, _, _)) if kind == "operator" => json!({"role": "operator", "agentId": null}),
        Some((kind, agent_id, role_name)) => {
            json!({"role": role_name.unwrap_or(kind), "agentId": agent_id})
        }
    })
}

/// `messageRejections`.
pub fn message_rejections(kernel: &Kernel) -> KernelResult<Value> {
    kernel.assert_open()?;
    let rows = query_all(
        &kernel.database,
        "SELECT rejection_id, message_id, action, code, from_state, attempted_state, actor_id, reason FROM message_rejections WHERE project_id = ? ORDER BY sequence",
        [&kernel.project_id],
        |row| {
            Ok(json!({
                "rejectionId": row.get::<_, String>(0)?,
                "messageId": row.get::<_, Option<String>>(1)?,
                "action": row.get::<_, String>(2)?,
                "code": row.get::<_, String>(3)?,
                "fromState": row.get::<_, Option<String>>(4)?,
                "attemptedState": row.get::<_, Option<String>>(5)?,
                "actorId": row.get::<_, String>(6)?,
                "reason": row.get::<_, String>(7)?,
            }))
        },
    )?;
    Ok(Value::Array(rows))
}

/// `agentInbox`.
pub fn agent_inbox(
    kernel: &Kernel,
    credential: &str,
    agent_id: Option<&str>,
) -> KernelResult<Value> {
    kernel.assert_open()?;
    if let Some(id) = agent_id {
        safe_id_str(id, "agent id")?;
    }
    let actor = authenticate_actor(&kernel.database, &kernel.project_id, credential)?;
    let target: Option<String> = if actor.role == "operator" {
        agent_id.map(str::to_string)
    } else {
        require_capability(&actor, "message:receive")?;
        let target = kernel.agent_by_actor(&actor.actor_id)?.map(|a| a.agent_id);
        if let Some(id) = agent_id {
            if Some(id) != target.as_deref() {
                return Err(KernelError::Authorization(
                    "an agent reads only its own inbox".into(),
                ));
            }
        }
        target
    };
    let Some(target) = target else {
        return Ok(json!([]));
    };
    let rows = message_rows_for(kernel, &target)?;
    let head_id = queue_head(&rows).map(|h| h.message_id.clone());
    let shown: Vec<Value> = rows
        .iter()
        .filter(|r| {
            Some(&r.message_id) == head_id.as_ref() || r.state == "sent" || r.state == "unacked"
        })
        .map(record_value)
        .collect();
    Ok(Value::Array(shown))
}

/// `pullMessage`.
pub fn pull_message(kernel: &Kernel, context: &MutationContext) -> KernelResult<Value> {
    let actor = kernel.authorize(&context.credential, "message:receive")?;
    let agent = kernel.agent_by_actor(&actor.actor_id)?;
    if let Some(agent) = &agent {
        if agent.kind == "PM" && !kernel.has_stored_request(context)? {
            let rows = message_rows_for(kernel, &agent.agent_id)?;
            if !matches!(queue_head(&rows), Some(h) if h.state == "queued") {
                return Ok(json!({"message": null}));
            }
            if pauses::delivery_paused(kernel, &agent.agent_id)? {
                return Ok(json!({"message": null}));
            }
        }
    }
    message_mutation(
        kernel,
        context,
        "message.pull",
        "message:receive",
        &json!({}),
        |caller| {
            let Some(recipient) = kernel.agent_by_actor(&caller.actor_id)? else {
                return reject(
                    kernel,
                    caller,
                    "message.pull",
                    Rejection::new(
                        "not_an_agent",
                        "the caller is not the current actor of an active agent",
                    ),
                );
            };
            if recipient.kind != "PM" {
                return reject(
                    kernel,
                    caller,
                    "message.pull",
                    Rejection::new(
                        "pull_not_allowed",
                        "only the PM pulls messages; workers receive pushes",
                    ),
                );
            }
            let rows = message_rows_for(kernel, &recipient.agent_id)?;
            let head = match queue_head(&rows) {
                Some(head)
                    if head.state == "queued"
                        && !pauses::delivery_paused(kernel, &recipient.agent_id)? =>
                {
                    head
                }
                _ => {
                    return reject(
                        kernel,
                        caller,
                        "message.pull",
                        Rejection::new(
                            "nothing_to_pull",
                            "the queue head is not waiting to be sent",
                        ),
                    )
                }
            };
            let now = kernel.now();
            let version = update_message(
                kernel,
                head,
                "sent",
                &[
                    ("sent_at", Sql::Text(now.clone())),
                    ("send_attempts", Sql::Integer(head.send_attempts + 1)),
                ],
                &now,
            )?;
            kernel.touch_agent(&recipient.agent_id, &now)?;
            let updated = message_row(kernel, &head.message_id)?.expect("the message exists");
            Ok(MutationOutput::new(
                json!({"message": record_value(&updated)}),
                MutationEvent::new("message", &head.message_id, version)
                    .transition("queued", "sent"),
            ))
        },
    )
}

/// `pullPending`: every queued or deferred message of the caller becomes sent, in sequence order, one ledger event each.
pub fn pull_pending(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    kernel.authorize(credential, "message:receive")?;
    let actor = authenticate_actor(&kernel.database, &kernel.project_id, credential)?;
    let Some(agent) = kernel.agent_by_actor(&actor.actor_id)? else {
        return Ok(json!([]));
    };
    if pauses::delivery_paused(kernel, &agent.agent_id)? {
        return Ok(json!([]));
    }
    let mut pulled: Vec<Value> = Vec::new();
    for row in message_rows_for(kernel, &agent.agent_id)? {
        if is_final_state(&row.state) || row.state == "sent" {
            continue;
        }
        if row.state == "unacked" {
            continue;
        }
        if row.state != "queued" && row.state != "deferred" {
            break;
        }
        let id = kernel.env.uuid();
        let context = MutationContext {
            credential: credential.to_string(),
            request_id: format!("req-{id}"),
            idempotency_key: format!("idem-{id}"),
            expected_version: kernel.state_version()?,
            input_revision: kernel.input_revision()?,
        };
        let first = pulled.is_empty();
        let record = message_mutation(
            kernel,
            &context,
            "message.pull",
            "message:receive",
            &json!({"messageId": row.message_id}),
            |caller| {
                let current = match message_row(kernel, &row.message_id)? {
                    Some(current) if current.state == "queued" || current.state == "deferred" => {
                        current
                    }
                    other => {
                        return reject(
                            kernel,
                            caller,
                            "message.pull",
                            Rejection::new(
                                "nothing_to_pull",
                                "the message is no longer waiting to be sent",
                            )
                            .message_id(&row.message_id)
                            .from_state(other.as_ref().map(|c| c.state.as_str())),
                        )
                    }
                };
                let now = kernel.now();
                let version = update_message(
                    kernel,
                    &current,
                    "sent",
                    &[
                        ("sent_at", Sql::Text(now.clone())),
                        ("send_attempts", Sql::Integer(current.send_attempts + 1)),
                    ],
                    &now,
                )?;
                if first {
                    kernel.touch_agent(&agent.agent_id, &now)?;
                }
                let updated = message_row(kernel, &row.message_id)?.expect("the message exists");
                Ok(MutationOutput::new(
                    record_value(&updated),
                    MutationEvent::new("message", &row.message_id, version)
                        .transition(&current.state, "sent"),
                ))
            },
        )?;
        pulled.push(record);
    }
    Ok(Value::Array(pulled))
}

/// `unreadSummary`: what waits for the caller. Read-only; nothing is written.
pub fn unread_summary(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    let actor = authenticate_actor(&kernel.database, &kernel.project_id, credential)?;
    let agent = if actor.role == "operator" {
        None
    } else {
        kernel.agent_by_actor(&actor.actor_id)?
    };
    let waiting: Vec<MessageRow> = match &agent {
        None => Vec::new(),
        Some(agent) => message_rows_for(kernel, &agent.agent_id)?
            .into_iter()
            .filter(|r| matches!(r.state.as_str(), "queued" | "deferred" | "sent" | "unacked"))
            .collect(),
    };
    let mut oldest: Option<&str> = None;
    for row in &waiting {
        if oldest.is_none_or(|o| row.queued_at.as_str() < o) {
            oldest = Some(&row.queued_at);
        }
    }
    Ok(json!({
        "count": waiting.len(),
        "oldestQueuedAt": oldest,
        "messageIds": waiting.iter().map(|r| r.message_id.clone()).collect::<Vec<_>>(),
        "actionNeeded": waiting.iter().filter(|r| r.action_needed == 1).count(),
    }))
}

/// `recordDeferral`.
pub fn record_deferral(
    kernel: &Kernel,
    context: &MutationContext,
    message_id: &str,
    reason: &Value,
) -> KernelResult<Value> {
    safe_id_str(message_id, "message id")?;
    let reason = match reason.as_str() {
        Some(text) if DEFERRAL_REASONS.contains(&text) => text.to_string(),
        _ => return Err(KernelError::type_error("unknown deferral reason")),
    };
    kernel.authorize(&context.credential, "controller:reconcile")?;
    let current = message_row(kernel, message_id)?;
    if let Some(current) = &current {
        if current.state == "deferred"
            && current.deferred_reason.as_deref() == Some(reason.as_str())
            && !kernel.has_stored_request(context)?
        {
            return Ok(record_value(current));
        }
    }
    message_mutation(
        kernel,
        context,
        "message.defer",
        "controller:reconcile",
        &json!({"messageId": message_id, "reason": reason}),
        |actor| {
            let row = message_row(kernel, message_id)?;
            if let Some(refused) =
                refuse_delivery(kernel, actor, "message.defer", row.as_ref(), "deferred")?
            {
                return Ok(refused);
            }
            let row = row.expect("refuse_delivery checked the row exists");
            let now = kernel.now();
            let from = row.state.clone();
            let version = if from == "queued" {
                update_message(
                    kernel,
                    &row,
                    "deferred",
                    &[
                        ("deferred_at", Sql::Text(now.clone())),
                        ("deferred_reason", Sql::Text(reason.clone())),
                        ("deferral_count", Sql::Integer(row.deferral_count + 1)),
                    ],
                    &now,
                )?
            } else {
                // The clock restarts with the reason, so the maximum deferral counts from when the input line was first seen non-empty.
                let mut extra: Vec<(&str, Sql)> = Vec::new();
                if row.deferred_reason.as_deref() != Some(reason.as_str()) {
                    extra.push(("deferred_at", Sql::Text(now.clone())));
                }
                extra.push(("deferred_reason", Sql::Text(reason.clone())));
                extra.push((
                    "deferral_count",
                    Sql::Integer(if reason == "input_not_empty" {
                        row.deferral_count + 1
                    } else {
                        row.deferral_count
                    }),
                ));
                update_message(kernel, &row, "deferred", &extra, &now)?
            };
            let updated = message_row(kernel, message_id)?.expect("the message exists");
            Ok(MutationOutput::new(
                record_value(&updated),
                MutationEvent::new(
                    if from == "queued" {
                        "message"
                    } else {
                        "message_note"
                    },
                    message_id,
                    version,
                )
                .transition(&from, "deferred")
                .with_details(json!({"reason": reason})),
            ))
        },
    )
}

/// `recordInputClear`.
pub fn record_input_clear(
    kernel: &Kernel,
    context: &MutationContext,
    message_id: &str,
    text: &str,
) -> KernelResult<Value> {
    safe_id_str(message_id, "message id")?;
    if text.is_empty() || text.len() > MAX_INPUT_CLEAR_BYTES {
        return Err(KernelError::type_error(format!(
            "input text must be non-empty, well-formed and at most {MAX_INPUT_CLEAR_BYTES} bytes"
        )));
    }
    let text_hash = sha256(text);
    message_mutation(
        kernel,
        context,
        "message.input_clear",
        "controller:reconcile",
        &json!({"messageId": message_id, "textHash": text_hash}),
        |actor| {
            let row = match message_row(kernel, message_id)? {
                Some(row)
                    if row.state == "deferred"
                        && row.deferred_reason.as_deref() == Some("input_not_empty") =>
                {
                    row
                }
                other => {
                    return reject(
                        kernel,
                        actor,
                        "message.input_clear",
                        Rejection::new(
                            "not_input_deferred",
                            "input text is recorded only for a message deferred because the input line is not empty",
                        )
                        .message_id(message_id)
                        .from_state(other.as_ref().map(|r| r.state.as_str())),
                    )
                }
            };
            let clear_id = kernel.env.uuid();
            execute(
                &kernel.database,
                "INSERT INTO message_input_clears(project_id, clear_id, message_id, deferral_count, text, text_hash, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    kernel.project_id,
                    clear_id,
                    message_id,
                    row.deferral_count,
                    text,
                    text_hash,
                    kernel.now()
                ],
            )?;
            Ok(MutationOutput::new(
                json!({"clearId": clear_id}),
                MutationEvent::new("message_note", message_id, row.state_version)
                    .transition("deferred", "deferred")
                    .with_details(json!({"inputCleared": true, "textHash": text_hash})),
            ))
        },
    )
}

/// `recordSent`.
pub fn record_sent(
    kernel: &Kernel,
    context: &MutationContext,
    message_id: &str,
) -> KernelResult<Value> {
    safe_id_str(message_id, "message id")?;
    message_mutation(
        kernel,
        context,
        "message.sent",
        "controller:reconcile",
        &json!({"messageId": message_id}),
        |actor| {
            let row = message_row(kernel, message_id)?;
            if let Some(refused) =
                refuse_delivery(kernel, actor, "message.sent", row.as_ref(), "sent")?
            {
                return Ok(refused);
            }
            let row = row.expect("refuse_delivery checked the row exists");
            let now = kernel.now();
            let from = row.state.clone();
            let version = update_message(
                kernel,
                &row,
                "sent",
                &[
                    ("sent_at", Sql::Text(now.clone())),
                    ("send_attempts", Sql::Integer(row.send_attempts + 1)),
                ],
                &now,
            )?;
            let updated = message_row(kernel, message_id)?.expect("the message exists");
            Ok(MutationOutput::new(
                record_value(&updated),
                MutationEvent::new("message", message_id, version).transition(&from, "sent"),
            ))
        },
    )
}

/// `recordFailure`.
pub fn record_failure(
    kernel: &Kernel,
    context: &MutationContext,
    message_id: &str,
    reason: &str,
) -> KernelResult<Value> {
    safe_id_str(message_id, "message id")?;
    safe_text(
        &Value::String(reason.to_string()),
        "failure reason",
        500,
        false,
    )?;
    message_mutation(
        kernel,
        context,
        "message.fail",
        "controller:reconcile",
        &json!({"messageId": message_id, "reason": reason}),
        |actor| {
            let row = message_row(kernel, message_id)?;
            if let Some(refused) =
                refuse_delivery(kernel, actor, "message.fail", row.as_ref(), "failed")?
            {
                return Ok(refused);
            }
            let row = row.expect("refuse_delivery checked the row exists");
            let from = row.state.clone();
            let failed_at = kernel.now();
            let version = update_message(
                kernel,
                &row,
                "failed",
                &[("state_reason", Sql::Text(reason.to_string()))],
                &failed_at,
            )?;
            message_notices::queue_delivery_notice(
                kernel,
                &row,
                "failed",
                &failed_at,
                Some(reason),
            )?;
            let updated = message_row(kernel, message_id)?.expect("the message exists");
            Ok(MutationOutput::new(
                record_value(&updated),
                MutationEvent::new("message", message_id, version)
                    .transition(&from, "failed")
                    .with_details(json!({"reason": reason})),
            ))
        },
    )
}

/// `ackMessage`.
pub fn ack_message(
    kernel: &Kernel,
    context: &MutationContext,
    message_id: &str,
) -> KernelResult<Value> {
    safe_id_str(message_id, "message id")?;
    message_mutation(
        kernel,
        context,
        "message.ack",
        "message:receive",
        &json!({"messageId": message_id}),
        |actor| {
            let agent = kernel.agent_by_actor(&actor.actor_id)?;
            let row = message_row(kernel, message_id)?;
            let Some(agent) = agent else {
                return reject(
                    kernel,
                    actor,
                    "message.ack",
                    Rejection::new(
                        "not_an_agent",
                        "the caller is not the current actor of an active agent",
                    )
                    .message_id(message_id)
                    .from_state(row.as_ref().map(|r| r.state.as_str())),
                );
            };
            let Some(row) = row else {
                return reject(
                    kernel,
                    actor,
                    "message.ack",
                    Rejection::new("unknown_message", "message does not exist")
                        .message_id(message_id),
                );
            };
            if row.recipient_agent_id != agent.agent_id {
                return reject(
                    kernel,
                    actor,
                    "message.ack",
                    Rejection::new(
                        "not_recipient",
                        "only the recipient agent may ack a message",
                    )
                    .message_id(message_id)
                    .from_state(Some(&row.state)),
                );
            }
            let to = if row.state == "unacked" {
                "acked_late"
            } else {
                "acked"
            };
            if !is_legal_transition(&row.state, to) {
                return reject(
                    kernel,
                    actor,
                    "message.ack",
                    Rejection::new(
                        "illegal_transition",
                        format!("a message in state {} cannot be acked", row.state),
                    )
                    .message_id(message_id)
                    .from_state(Some(&row.state))
                    .attempted(to),
                );
            }
            let now = kernel.now();
            let version = update_message(
                kernel,
                &row,
                to,
                &[("acked_at", Sql::Text(now.clone()))],
                &now,
            )?;
            kernel.touch_agent(&agent.agent_id, &now)?;
            let updated = message_row(kernel, message_id)?.expect("the message exists");
            Ok(MutationOutput::new(
                record_value(&updated),
                MutationEvent::new("message", message_id, version).transition(&row.state, to),
            ))
        },
    )
}

/// `resolveMessage`.
pub fn resolve_message(
    kernel: &Kernel,
    context: &MutationContext,
    message_id: &str,
    decision: &Value,
    note: Option<&str>,
) -> KernelResult<Value> {
    safe_id_str(message_id, "message id")?;
    let decision = match decision.as_str() {
        Some(text) if RESOLUTION_DECISIONS.contains(&text) => text.to_string(),
        _ => return Err(KernelError::type_error("unknown resolution decision")),
    };
    if let Some(note) = note {
        safe_text(
            &Value::String(note.to_string()),
            "resolution note",
            1000,
            true,
        )?;
    }
    message_mutation(
        kernel,
        context,
        "message.resolve",
        "message:resolve",
        &json!({"messageId": message_id, "decision": decision, "note": note}),
        |actor| {
            let Some(row) = message_row(kernel, message_id)? else {
                return reject(
                    kernel,
                    actor,
                    "message.resolve",
                    Rejection::new("unknown_message", "message does not exist")
                        .message_id(message_id),
                );
            };
            let recipient = kernel.agent_row(&row.recipient_agent_id)?;
            if matches!(&recipient, Some(r) if r.kind == "PM") && actor.role != "operator" {
                return reject(
                    kernel,
                    actor,
                    "message.resolve",
                    Rejection::new(
                        "operator_only",
                        "a message addressed to the PM is resolved only by the operator",
                    )
                    .message_id(message_id)
                    .from_state(Some(&row.state)),
                );
            }
            if decision == "retry" && !matches!(&recipient, Some(r) if r.state == "active") {
                return reject(
                    kernel,
                    actor,
                    "message.resolve",
                    Rejection::new(
                        "recipient_not_active",
                        "the recipient agent has ended, so the message cannot be sent again",
                    )
                    .message_id(message_id)
                    .from_state(Some(&row.state)),
                );
            }
            let Some(to) = resolution_target(&decision, &row.state) else {
                return reject(
                    kernel,
                    actor,
                    "message.resolve",
                    Rejection::new(
                        "illegal_resolution",
                        format!(
                            "{decision} does not apply to a message in state {}",
                            row.state
                        ),
                    )
                    .message_id(message_id)
                    .from_state(Some(&row.state)),
                );
            };
            let now = kernel.now();
            execute(
                &kernel.database,
                "INSERT INTO message_resolutions(project_id, resolution_id, message_id, decision, decided_by, note, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    kernel.project_id,
                    kernel.env.uuid(),
                    message_id,
                    decision,
                    actor.actor_id,
                    note,
                    now
                ],
            )?;
            if let Some(resolver) = kernel.agent_by_actor(&actor.actor_id)? {
                kernel.touch_agent(&resolver.agent_id, &now)?;
            }
            let version = if decision == "retry" {
                update_message(
                    kernel,
                    &row,
                    "queued",
                    &[
                        ("queued_at", Sql::Text(now.clone())),
                        ("deferred_at", Sql::Null),
                        ("deferred_reason", Sql::Null),
                        ("notified_at", Sql::Null),
                        ("last_notified_at", Sql::Null),
                        ("state_reason", Sql::Null),
                    ],
                    &now,
                )?
            } else {
                update_message(
                    kernel,
                    &row,
                    "cancelled",
                    &[(
                        "state_reason",
                        Sql::Text(cancelled_reason(
                            &format!("resolution_{decision}"),
                            &row.state,
                            row.state_reason.as_deref(),
                        )),
                    )],
                    &now,
                )?
            };
            let updated = message_row(kernel, message_id)?.expect("the message exists");
            Ok(MutationOutput::new(
                record_value(&updated),
                MutationEvent::new("message", message_id, version)
                    .transition(&row.state, to)
                    .with_details(json!({"decision": decision})),
            ))
        },
    )
}

/// `messageRow`.
pub fn message_row(kernel: &Kernel, message_id: &str) -> KernelResult<Option<MessageRow>> {
    query_opt(
        &kernel.database,
        "SELECT * FROM messages WHERE project_id = ? AND message_id = ?",
        [&kernel.project_id, message_id],
        MessageRow::from_row,
    )
}

/// The agent's messages, oldest first (`messageRowsFor`).
pub fn message_rows_for(kernel: &Kernel, agent_id: &str) -> KernelResult<Vec<MessageRow>> {
    query_all(
        &kernel.database,
        "SELECT * FROM messages WHERE project_id = ? AND recipient_agent_id = ? ORDER BY sequence",
        [&kernel.project_id, agent_id],
        MessageRow::from_row,
    )
}

/// `openMessageRowsFor`.
pub fn open_message_rows_for(kernel: &Kernel, agent_id: &str) -> KernelResult<Vec<MessageRow>> {
    query_all(
        &kernel.database,
        &format!("SELECT * FROM messages INDEXED BY messages_by_state WHERE project_id = ? AND recipient_agent_id = ? AND state IN ({OPEN_MESSAGE_STATES_SQL}) ORDER BY sequence"),
        [&kernel.project_id, agent_id],
        MessageRow::from_row,
    )
}

/// Queues a message from `sender_actor_id` to `recipient` and returns its id (`insertQueuedMessage`). The caller owns the transaction.
pub fn insert_queued_message(
    kernel: &Kernel,
    sender_actor_id: &str,
    recipient: &AgentRow,
    body: &str,
    body_hash: &str,
    now: &str,
    action_needed: bool,
) -> KernelResult<String> {
    let message_id = kernel.env.uuid();
    let sequence = query_opt(
        &kernel.database,
        "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM messages WHERE project_id = ?",
        [&kernel.project_id],
        |row| row.get::<_, i64>(0),
    )?
    .unwrap_or(1);
    execute(
        &kernel.database,
        "INSERT INTO messages(project_id, message_id, sequence, recipient_agent_id, recipient_generation, sender_actor_id,
          body, body_hash, state, state_version, queued_at, deferral_count, send_attempts, created_at, updated_at, action_needed)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'queued', 0, ?, 0, 0, ?, ?, ?)",
        rusqlite::params![
            kernel.project_id,
            message_id,
            sequence,
            recipient.agent_id,
            recipient.generation,
            sender_actor_id,
            body,
            body_hash,
            now,
            now,
            now,
            i64::from(action_needed)
        ],
    )?;
    Ok(message_id)
}

/// `updateMessage`: moves `row` to `to` with the extra columns, and returns the message's new state version.
pub fn update_message(
    kernel: &Kernel,
    row: &MessageRow,
    to: &str,
    extra: &[(&str, Sql)],
    now: &str,
) -> KernelResult<i64> {
    let columns: String = extra
        .iter()
        .map(|(name, _)| format!(", {name} = ?"))
        .collect();
    let mut params: Vec<Sql> = vec![Sql::Text(to.into()), Sql::Text(now.into())];
    params.extend(extra.iter().map(|(_, value)| value.clone()));
    params.push(Sql::Text(kernel.project_id.clone()));
    params.push(Sql::Text(row.message_id.clone()));
    params.push(Sql::Text(row.state.clone()));
    let changes = execute(
        &kernel.database,
        &format!("UPDATE messages SET state = ?, state_version = state_version + 1, updated_at = ?{columns} WHERE project_id = ? AND message_id = ? AND state = ?"),
        rusqlite::params_from_iter(params),
    )?;
    if changes != 1 {
        return Err(KernelError::controller(
            "message state changed during the mutation",
        ));
    }
    Ok(row.state_version + 1)
}

/// `reject`: records a refusal and returns the result and event that ledger it.
pub fn reject(
    kernel: &Kernel,
    actor: &AuthenticatedActor,
    action: &str,
    rejection: Rejection,
) -> KernelResult<MutationOutput> {
    let rejection_id = kernel.env.uuid();
    execute(
        &kernel.database,
        "INSERT INTO message_rejections(project_id, rejection_id, sequence, message_id, action, code, from_state, attempted_state, actor_id, reason, created_at) VALUES (?, ?, (SELECT COALESCE(MAX(sequence), 0) + 1 FROM message_rejections WHERE project_id = ?), ?, ?, ?, ?, ?, ?, ?, ?)",
        rusqlite::params![
            kernel.project_id,
            rejection_id,
            kernel.project_id,
            rejection.message_id,
            action,
            rejection.code,
            rejection.from_state,
            rejection.attempted_state,
            actor.actor_id,
            rejection.message,
            kernel.now()
        ],
    )?;
    let mut event = MutationEvent::new("message_rejection", &rejection_id, 0);
    event.from_state = rejection.from_state.clone();
    event.details = Some(json!({
        "action": action,
        "code": rejection.code,
        "messageId": rejection.message_id,
        "attemptedState": rejection.attempted_state,
    }));
    Ok(MutationOutput::new(
        json!({"rejected": true, "code": rejection.code, "message": rejection.message}),
        event,
    ))
}

/// `messageMutation`: a mutation whose refusal is a recorded rejection that surfaces as a `MessageTransitionError`.
pub fn message_mutation<F>(
    kernel: &Kernel,
    context: &MutationContext,
    action: &str,
    capability: &str,
    payload: &Value,
    apply: F,
) -> KernelResult<Value>
where
    F: FnOnce(&AuthenticatedActor) -> KernelResult<MutationOutput>,
{
    let result = kernel.mutate(context, action, capability, payload, apply)?;
    if is_message_rejection(&result) {
        return Err(KernelError::MessageTransition {
            code: result
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            message: result
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        });
    }
    Ok(result)
}

/// `#refuseDelivery`: the rejection for a delivery step that cannot happen, or none.
fn refuse_delivery(
    kernel: &Kernel,
    actor: &AuthenticatedActor,
    action: &str,
    row: Option<&MessageRow>,
    attempted: &str,
) -> KernelResult<Option<MutationOutput>> {
    let Some(row) = row else {
        return reject(
            kernel,
            actor,
            action,
            Rejection::new("unknown_message", "message does not exist").attempted(attempted),
        )
        .map(Some);
    };
    if !is_legal_transition(&row.state, attempted)
        && !(row.state == "deferred" && attempted == "deferred")
    {
        return reject(
            kernel,
            actor,
            action,
            Rejection::new(
                "illegal_transition",
                format!("a message in state {} cannot become {attempted}", row.state),
            )
            .message_id(&row.message_id)
            .from_state(Some(&row.state))
            .attempted(attempted),
        )
        .map(Some);
    }
    let rows = message_rows_for(kernel, &row.recipient_agent_id)?;
    if queue_head(&rows).map(|h| h.message_id.as_str()) != Some(row.message_id.as_str()) {
        return reject(
            kernel,
            actor,
            action,
            Rejection::new(
                "not_head",
                "an earlier message to the same recipient is unresolved",
            )
            .message_id(&row.message_id)
            .from_state(Some(&row.state))
            .attempted(attempted),
        )
        .map(Some);
    }
    Ok(None)
}

/// `appendMessageEvent`.
#[allow(clippy::too_many_arguments)]
pub fn append_message_event(
    kernel: &Kernel,
    actor: &AuthenticatedActor,
    context: &MutationContext,
    message_id: &str,
    from: &str,
    to: &str,
    state_version: i64,
    details: &Value,
) -> KernelResult<()> {
    kernel.append_event(
        actor,
        context,
        "message",
        message_id,
        from,
        to,
        "message.transition",
        state_version,
        details,
    )
}

/// `cancelMessagesOf`: cancels what is not final. `keep_failed` leaves a failed message failed.
pub fn cancel_messages_of(
    kernel: &Kernel,
    actor: &AuthenticatedActor,
    context: &MutationContext,
    agent_id: &str,
    reason: &str,
    now: &str,
    keep_failed: bool,
) -> KernelResult<Vec<String>> {
    let mut cancelled = Vec::new();
    for row in message_rows_for(kernel, agent_id)? {
        if is_final_state(&row.state) {
            continue;
        }
        if keep_failed && row.state == "failed" {
            continue;
        }
        let version = update_message(
            kernel,
            &row,
            "cancelled",
            &[(
                "state_reason",
                Sql::Text(cancelled_reason(
                    reason,
                    &row.state,
                    row.state_reason.as_deref(),
                )),
            )],
            now,
        )?;
        append_message_event(
            kernel,
            actor,
            context,
            &row.message_id,
            &row.state,
            "cancelled",
            version,
            &json!({"reason": reason}),
        )?;
        cancelled.push(row.message_id.clone());
    }
    Ok(cancelled)
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "enqueueMessage" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(enqueue_message(kernel, &context, input)))
        }
        "message" => {
            let message_id = arg!(a.str(0, "messageId"));
            Some(call(message(kernel, message_id)))
        }
        "messagesFor" => {
            let agent_id = arg!(a.str(0, "agentId"));
            Some(call(messages_for(kernel, agent_id)))
        }
        "openMessagesFor" => {
            let agent_id = arg!(a.str(0, "agentId"));
            Some(call(open_messages_for(kernel, agent_id)))
        }
        "unresolvedMessages" => {
            let credential = arg!(a.str(0, "credential"));
            let limit = arg!(a.i64(1, "limit"));
            Some(call(unresolved_messages(kernel, credential, limit)))
        }
        "inputClears" => {
            let credential = arg!(a.str(0, "credential"));
            let limit = arg!(a.i64(1, "limit"));
            Some(call(input_clears(kernel, credential, limit)))
        }
        "senderOf" => {
            let actor_id = arg!(a.str(0, "actorId"));
            Some(call(sender_of(kernel, actor_id)))
        }
        "messageRejections" => Some(call(message_rejections(kernel))),
        "agentInbox" => {
            let credential = arg!(a.str(0, "credential"));
            let agent_id = arg!(a.opt_str(1, "agentId"));
            Some(call(agent_inbox(kernel, credential, agent_id)))
        }
        "pullMessage" => {
            let context = arg!(a.ctx(0));
            Some(call(pull_message(kernel, &context)))
        }
        "pullPending" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(pull_pending(kernel, credential)))
        }
        "unreadSummary" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(unread_summary(kernel, credential)))
        }
        "recordDeferral" => {
            let context = arg!(a.ctx(0));
            let message_id = arg!(a.str(1, "messageId"));
            let reason = a.value(2);
            Some(call(record_deferral(kernel, &context, message_id, reason)))
        }
        "recordInputClear" => {
            let context = arg!(a.ctx(0));
            let message_id = arg!(a.str(1, "messageId"));
            let text = arg!(a.str(2, "text"));
            Some(call(record_input_clear(kernel, &context, message_id, text)))
        }
        "recordSent" => {
            let context = arg!(a.ctx(0));
            let message_id = arg!(a.str(1, "messageId"));
            Some(call(record_sent(kernel, &context, message_id)))
        }
        "recordFailure" => {
            let context = arg!(a.ctx(0));
            let message_id = arg!(a.str(1, "messageId"));
            let reason = arg!(a.str(2, "reason"));
            Some(call(record_failure(kernel, &context, message_id, reason)))
        }
        "ackMessage" => {
            let context = arg!(a.ctx(0));
            let message_id = arg!(a.str(1, "messageId"));
            Some(call(ack_message(kernel, &context, message_id)))
        }
        "resolveMessage" => {
            let context = arg!(a.ctx(0));
            let message_id = arg!(a.str(1, "messageId"));
            let decision = a.value(2);
            let note = arg!(a.opt_str(3, "note"));
            Some(call(resolve_message(
                kernel, &context, message_id, decision, note,
            )))
        }
        _ => None,
    }
}
