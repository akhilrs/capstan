//! The message notices and waits (src/controller/message-notices.ts): controller notices to the PM, the wait
//! bookkeeping of long polls, and the timer pass over the messages.

use crate::auth::AuthenticatedActor;
use crate::canonical::{js_number, sha256};
use crate::dispatch::{arg, call, Args};
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{
    advance, agent_stuck_notice, assert_timers, delivery_problem_notice, execute, exists,
    message_record, one_line_text, query_all, query_opt, safe_id, safe_id_str,
};
use crate::kernel::Kernel;
use crate::records::{
    is_final_state, AgentRow, MessageRow, MutationEvent, MutationOutput, SUPERVISION_CHECK_TEXT,
};
use crate::types::MutationContext;
use capstan_ledger::iso_from_millis;
use rusqlite::types::Value as Sql;
use serde_json::{json, Value};
use std::collections::HashMap;

use super::messages::{
    append_message_event, insert_queued_message, message_mutation, message_row, message_rows_for,
    reject, update_message, Rejection, OPEN_MESSAGE_STATES_SQL,
};
use super::messaging::{
    evaluate_messaging, parse_iso_ms, queue_head, AgentFacts, MessageFacts, MessagingEvaluation,
    MessagingTimers, StateObservation, WaitInterval,
};
use super::{pauses, prompt_relay};

/// Thrown inside the `message.advance` mutation when nothing is due, so the transaction rolls back (`NoMessageTransitionDue`).
const NO_TRANSITION_DUE: &str = "no message transition is due";

fn record_value(row: &MessageRow) -> Value {
    serde_json::to_value(message_record(row)).expect("a message record serializes")
}

fn notice_event(kernel: &Kernel, details: Value) -> MutationEvent {
    MutationEvent::new("pm_notice", &kernel.project_id, 0).with_details(details)
}

/// The controller's own actor and the one active PM (`noticeParties`).
pub struct NoticeParties {
    pub controller_actor_id: String,
    pub pm: AgentRow,
}

/// The parties of a controller notice, or `None` when either is missing.
pub fn notice_parties(kernel: &Kernel) -> KernelResult<Option<NoticeParties>> {
    let mut pms = query_all(
        &kernel.database,
        "SELECT * FROM agents WHERE project_id = ? AND kind = 'PM' AND state = 'active'",
        [&kernel.project_id],
        AgentRow::from_row,
    )?;
    let controller = query_opt(
        &kernel.database,
        "SELECT actor_id FROM actors WHERE project_id = ? AND is_internal = 1 AND role = 'controller' AND active = 1 AND revoked_at IS NULL",
        [&kernel.project_id],
        |row| row.get::<_, String>(0),
    )?;
    match (pms.len(), controller) {
        (1, Some(controller_actor_id)) => Ok(Some(NoticeParties {
            controller_actor_id,
            pm: pms.remove(0),
        })),
        _ => Ok(None),
    }
}

/// `noticeToPm`: queues a controller notice to the one active PM; false when there is none. The caller owns the transaction.
pub fn notice_to_pm(
    kernel: &Kernel,
    body: &str,
    now: &str,
    action_needed: bool,
) -> KernelResult<bool> {
    let Some(parties) = notice_parties(kernel)? else {
        return Ok(false);
    };
    insert_queued_message(
        kernel,
        &parties.controller_actor_id,
        &parties.pm,
        body,
        &sha256(body),
        now,
        action_needed,
    )?;
    Ok(true)
}

/// `noticeToAgent`: a controller message to one active agent; none when the agent has ended. The caller owns the transaction.
pub fn notice_to_agent(
    kernel: &Kernel,
    agent_id: &str,
    body: &str,
    now: &str,
    action_needed: bool,
) -> KernelResult<Option<String>> {
    match kernel.agent_row(agent_id)? {
        Some(agent) if agent.state == "active" => Ok(Some(insert_queued_message(
            kernel,
            &kernel.internal_actor_id,
            &agent,
            body,
            &sha256(body),
            now,
            action_needed,
        )?)),
        _ => Ok(None),
    }
}

/// `openWaits`: wait rows that are still open; only a controller may list them.
pub fn open_waits(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    kernel.authorize(credential, "controller:reconcile")?;
    let rows = query_all(
        &kernel.database,
        "SELECT wait_id, agent_id FROM agent_waits WHERE project_id = ? AND ended_at IS NULL ORDER BY started_at, wait_id",
        [&kernel.project_id],
        |row| {
            Ok(json!({
                "waitId": row.get::<_, String>(0)?,
                "agentId": row.get::<_, String>(1)?,
            }))
        },
    )?;
    Ok(Value::Array(rows))
}

/// `recordNotification`.
pub fn record_notification(
    kernel: &Kernel,
    context: &MutationContext,
    message_id: &str,
) -> KernelResult<Value> {
    safe_id_str(message_id, "message id")?;
    message_mutation(
        kernel,
        context,
        "message.notified",
        "controller:reconcile",
        &json!({"messageId": message_id}),
        |actor| {
            let row = message_row(kernel, message_id)?;
            let head_id = match &row {
                Some(row) => {
                    let rows = message_rows_for(kernel, &row.recipient_agent_id)?;
                    queue_head(&rows).map(|h| h.message_id.clone())
                }
                None => None,
            };
            let row = match row {
                Some(row)
                    if head_id.as_deref() == Some(row.message_id.as_str())
                        && !is_final_state(&row.state) =>
                {
                    row
                }
                other => {
                    return reject(
                        kernel,
                        actor,
                        "message.notified",
                        Rejection::new(
                            "not_notifiable",
                            "only the head of a queue that is not final is notified",
                        )
                        .message_id(message_id)
                        .from_state(other.as_ref().map(|r| r.state.as_str())),
                    )
                }
            };
            let now = kernel.now();
            execute(
                &kernel.database,
                "UPDATE messages SET notified_at = COALESCE(notified_at, ?), last_notified_at = ?, updated_at = ? WHERE project_id = ? AND message_id = ?",
                rusqlite::params![now, now, now, kernel.project_id, message_id],
            )?;
            let updated = message_row(kernel, message_id)?.expect("the message exists");
            Ok(MutationOutput::new(
                record_value(&updated),
                MutationEvent::new("message_note", message_id, row.state_version)
                    .transition(&row.state, &row.state)
                    .with_details(json!({"notified": true})),
            ))
        },
    )
}

/// `beginWait`.
pub fn begin_wait(kernel: &Kernel, context: &MutationContext) -> KernelResult<Value> {
    message_mutation(
        kernel,
        context,
        "wait.begin",
        "message:receive",
        &json!({}),
        |actor| {
            let Some(agent) = kernel.agent_by_actor(&actor.actor_id)? else {
                return reject(
                    kernel,
                    actor,
                    "wait.begin",
                    Rejection::new(
                        "not_an_agent",
                        "the caller is not the current actor of an active agent",
                    ),
                );
            };
            let wait_id = kernel.env.uuid();
            let now = kernel.now();
            execute(
                &kernel.database,
                "INSERT INTO agent_waits(project_id, wait_id, agent_id, started_at) VALUES (?, ?, ?, ?)",
                rusqlite::params![kernel.project_id, wait_id, agent.agent_id, now],
            )?;
            kernel.touch_agent(&agent.agent_id, &now)?;
            let mut event = MutationEvent::new("agent_wait", &wait_id, 0)
                .with_details(json!({"agentId": agent.agent_id}));
            event.to_state = Some("open".into());
            Ok(MutationOutput::new(json!({"waitId": wait_id}), event))
        },
    )
}

fn wait_row(kernel: &Kernel, wait_id: &str) -> KernelResult<Option<(String, Option<String>)>> {
    query_opt(
        &kernel.database,
        "SELECT agent_id, ended_at FROM agent_waits WHERE project_id = ? AND wait_id = ?",
        [&kernel.project_id, wait_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
    )
}

/// `#waitAlreadyClosedFor`.
fn wait_already_closed_for(
    kernel: &Kernel,
    actor: &AuthenticatedActor,
    wait_id: &str,
) -> KernelResult<bool> {
    let Some((wait_agent, ended_at)) = wait_row(kernel, wait_id)? else {
        return Ok(false);
    };
    if ended_at.is_none() {
        return Ok(false);
    }
    Ok(actor.capabilities.contains("controller:reconcile")
        || kernel
            .agent_by_actor(&actor.actor_id)?
            .is_some_and(|a| a.agent_id == wait_agent))
}

/// `#endWait`.
fn end_wait_in(
    kernel: &Kernel,
    actor: &AuthenticatedActor,
    action: &str,
    wait_id: &str,
) -> KernelResult<MutationOutput> {
    let wait = wait_row(kernel, wait_id)?;
    let caller = kernel.agent_by_actor(&actor.actor_id)?;
    let allowed = actor.capabilities.contains("controller:reconcile")
        || matches!((&caller, &wait), (Some(c), Some((agent, _))) if &c.agent_id == agent);
    let Some((wait_agent, ended_at)) = wait.filter(|_| allowed) else {
        let unknown = wait_row(kernel, wait_id)?.is_none();
        return reject(
            kernel,
            actor,
            action,
            Rejection::new(
                if unknown {
                    "unknown_wait"
                } else {
                    "not_wait_owner"
                },
                "the wait does not exist or belongs to another agent",
            ),
        );
    };
    let now = kernel.now();
    if ended_at.is_none() {
        execute(
            &kernel.database,
            "UPDATE agent_waits SET ended_at = MAX(?, started_at) WHERE project_id = ? AND wait_id = ?",
            [&now, &kernel.project_id, wait_id],
        )?;
    }
    if let Some(caller) = &caller {
        if caller.agent_id == wait_agent {
            kernel.touch_agent(&caller.agent_id, &now)?;
        }
    }
    Ok(MutationOutput::new(
        json!({"ended": ended_at.is_none()}),
        MutationEvent::new("agent_wait", wait_id, 1).transition("open", "closed"),
    ))
}

/// `endWait`.
pub fn end_wait(kernel: &Kernel, context: &MutationContext, wait_id: &str) -> KernelResult<Value> {
    safe_id_str(wait_id, "wait id")?;
    let caller = kernel.authorize(&context.credential, "message:receive")?;
    if !kernel.has_stored_request(context)? && wait_already_closed_for(kernel, &caller, wait_id)? {
        return Ok(json!({"ended": false}));
    }
    message_mutation(
        kernel,
        context,
        "wait.end",
        "message:receive",
        &json!({"waitId": wait_id}),
        |actor| end_wait_in(kernel, actor, "wait.end", wait_id),
    )
}

/// `endWaitAsController`.
pub fn end_wait_as_controller(
    kernel: &Kernel,
    context: &MutationContext,
    wait_id: &str,
) -> KernelResult<Value> {
    safe_id_str(wait_id, "wait id")?;
    let caller = kernel.authorize(&context.credential, "controller:reconcile")?;
    if !kernel.has_stored_request(context)? && wait_already_closed_for(kernel, &caller, wait_id)? {
        return Ok(json!({"ended": false}));
    }
    message_mutation(
        kernel,
        context,
        "wait.end_controller",
        "controller:reconcile",
        &json!({"waitId": wait_id}),
        |actor| end_wait_in(kernel, actor, "wait.end_controller", wait_id),
    )
}

/// `closeWaits`: ends every open wait of the agent.
pub fn close_waits(kernel: &Kernel, agent_id: &str, now: &str) -> KernelResult<()> {
    execute(
        &kernel.database,
        "UPDATE agent_waits SET ended_at = MAX(?, started_at) WHERE project_id = ? AND agent_id = ? AND ended_at IS NULL",
        [now, &kernel.project_id, agent_id],
    )?;
    Ok(())
}

/// `advanceMessaging`.
pub fn advance_messaging(
    kernel: &Kernel,
    context: &MutationContext,
    timers: &Value,
) -> KernelResult<Value> {
    assert_timers(timers)?;
    kernel.authorize(&context.credential, "controller:reconcile")?;
    let parsed = MessagingTimers::from_value(timers);
    let first = evaluate(kernel, &parsed)?;
    if first.transitions.is_empty() && !kernel.has_stored_request(context)? {
        return Ok(advance(&first.to_value(), &[]));
    }
    let outcome = kernel.mutate(
        context,
        "message.advance",
        "controller:reconcile",
        &json!({"timers": timers}),
        |actor| {
            let evaluation = evaluate(kernel, &parsed)?;
            if evaluation.transitions.is_empty() {
                return Err(KernelError::Other(NO_TRANSITION_DUE.into()));
            }
            let now = kernel.now();
            let mut applied: Vec<String> = Vec::new();
            for transition in &evaluation.transitions {
                let Some(row) = message_row(kernel, &transition.message_id)? else {
                    continue;
                };
                let from = if transition.to == "unacked" {
                    "sent"
                } else {
                    "deferred"
                };
                if row.state != from {
                    continue;
                }
                let version = update_message(kernel, &row, transition.to, &[], &now)?;
                append_message_event(
                    kernel,
                    actor,
                    context,
                    &row.message_id,
                    from,
                    transition.to,
                    version,
                    &json!({"timer": true}),
                )?;
                queue_delivery_notice(kernel, &row, transition.to, &now, None)?;
                applied.push(row.message_id.clone());
            }
            Ok(MutationOutput::new(
                advance(&evaluation.to_value(), &applied),
                MutationEvent::new("message_timer", &kernel.project_id, 0).with_details(json!({
                    "transitions": applied,
                    "actions": evaluation.actions.len(),
                })),
            ))
        },
    );
    match outcome {
        Err(KernelError::Other(message)) if message == NO_TRANSITION_DUE => {
            Ok(advance(&first.to_value(), &[]))
        }
        other => other,
    }
}

/// `#queuePmNotice`: one controller message to the PM, once per (kind, subject, episode). The caller owns the transaction.
fn queue_pm_notice(
    kernel: &Kernel,
    kind: &str,
    subject: &str,
    episode: &str,
    body: &str,
    now: &str,
    action_needed: bool,
) -> KernelResult<bool> {
    let Some(parties) = notice_parties(kernel)? else {
        return Ok(false);
    };
    if exists(
        &kernel.database,
        "SELECT 1 AS present FROM pm_notices WHERE project_id = ? AND kind = ? AND subject = ? AND episode = ?",
        [&kernel.project_id, kind, subject, episode],
    )? {
        return Ok(false);
    }
    let message_id = insert_queued_message(
        kernel,
        &parties.controller_actor_id,
        &parties.pm,
        body,
        &sha256(body),
        now,
        action_needed,
    )?;
    execute(
        &kernel.database,
        "INSERT INTO pm_notices(project_id, notice_id, kind, subject, episode, message_id, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        rusqlite::params![
            kernel.project_id,
            kernel.env.uuid(),
            kind,
            subject,
            episode,
            message_id,
            now
        ],
    )?;
    Ok(true)
}

/// `queueDeliveryNotice`: tells the PM that a message to a worker is stuck. Routine Supervisor checks and messages to the PM are not told.
pub fn queue_delivery_notice(
    kernel: &Kernel,
    row: &MessageRow,
    state: &str,
    now: &str,
    reason: Option<&str>,
) -> KernelResult<()> {
    let Some(recipient) = kernel.agent_row(&row.recipient_agent_id)? else {
        return Ok(());
    };
    if recipient.kind == "PM" {
        return Ok(());
    }
    if exists(
        &kernel.database,
        "SELECT 1 AS present FROM supervision_checks WHERE project_id = ? AND message_id = ?",
        [&kernel.project_id, &row.message_id],
    )? {
        return Ok(());
    }
    let first = one_line_text(&row.body, 80);
    // One notice per delivery attempt: a message retried that goes wrong again is told again.
    queue_pm_notice(
        kernel,
        "delivery",
        &row.message_id,
        &format!("{state}#{}", row.send_attempts),
        &delivery_problem_notice(&row.message_id, &recipient.agent_id, state, reason, &first),
        now,
        true,
    )?;
    Ok(())
}

/// `queueMissingDeliveryNotices`.
pub fn queue_missing_delivery_notices(
    kernel: &Kernel,
    context: &MutationContext,
) -> KernelResult<Value> {
    kernel.authorize(&context.credential, "controller:reconcile")?;
    let missing = || -> KernelResult<Vec<MessageRow>> {
        let mut rows = query_all(
            &kernel.database,
            "SELECT m.* FROM messages m INDEXED BY messages_by_state JOIN agents a ON a.project_id = m.project_id AND a.agent_id = m.recipient_agent_id
             WHERE m.project_id = ? AND a.state = 'active' AND a.kind <> 'PM'
               AND m.state IN ('unacked', 'expired', 'failed')
               AND NOT EXISTS (SELECT 1 FROM pm_notices n WHERE n.project_id = m.project_id AND n.kind = 'delivery'
                               AND n.subject = m.message_id AND n.episode = m.state || '#' || m.send_attempts)
               AND NOT EXISTS (SELECT 1 FROM supervision_checks c WHERE c.project_id = m.project_id AND c.message_id = m.message_id)
             ORDER BY m.sequence",
            [&kernel.project_id],
            MessageRow::from_row,
        )?;
        rows.truncate(20);
        Ok(rows)
    };
    if notice_parties(kernel)?.is_none() || missing()?.is_empty() {
        return Ok(json!({"queued": 0}));
    }
    let count = || -> KernelResult<i64> {
        Ok(query_opt(
            &kernel.database,
            "SELECT COUNT(*) AS n FROM pm_notices WHERE project_id = ?",
            [&kernel.project_id],
            |row| row.get::<_, i64>(0),
        )?
        .unwrap_or(0))
    };
    kernel.mutate(
        context,
        "pm.delivery_notices",
        "controller:reconcile",
        &json!({}),
        |_| {
            let now = kernel.now();
            let mut queued = 0;
            for row in missing()? {
                let before = count()?;
                queue_delivery_notice(
                    kernel,
                    &row,
                    &row.state,
                    &now,
                    if row.state == "failed" {
                        row.state_reason.as_deref()
                    } else {
                        None
                    },
                )?;
                if count()? > before {
                    queued += 1;
                }
            }
            Ok(MutationOutput::new(
                json!({"queued": queued}),
                notice_event(kernel, json!({"queued": queued, "backfill": true})),
            ))
        },
    )
}

/// `#hasOpenUnreadableNotice`: true while a notice for an unreadable input of this agent names a head message that is still deferred or queued.
fn has_open_unreadable_notice(kernel: &Kernel, agent_id: &str) -> KernelResult<bool> {
    exists(
        &kernel.database,
        "SELECT 1 AS present FROM pm_notices n JOIN messages m ON m.project_id = n.project_id AND m.message_id = substr(n.episode, 12)
         WHERE n.project_id = ? AND n.kind = 'blocked' AND n.subject = ? AND n.episode LIKE 'unreadable:%'
           AND m.state IN ('deferred', 'queued') LIMIT 1",
        [&kernel.project_id, agent_id],
    )
}

/// `queueInputBlockedNotice`.
pub fn queue_input_blocked_notice(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    kernel.authorize(&context.credential, "controller:reconcile")?;
    let null = Value::Null;
    let agent_id = safe_id(input.get("agentId").unwrap_or(&null), "agent id")?.to_string();
    let message_id = safe_id(input.get("messageId").unwrap_or(&null), "message id")?.to_string();
    let blocker = input.get("blocker").and_then(Value::as_str).unwrap_or("");
    let episode = format!("unreadable:{message_id}");
    let known = exists(
        &kernel.database,
        "SELECT 1 AS present FROM pm_notices WHERE project_id = ? AND kind = 'blocked' AND subject = ? AND episode = ?",
        [&kernel.project_id, &agent_id, &episode],
    )?;
    if known || notice_parties(kernel)?.is_none() {
        return Ok(json!({"queued": false}));
    }
    let relay = prompt_relay::enabled(kernel)?;
    let advice = if relay && blocker == "dialog" {
        format!("Run cstan prompt show {agent_id}: it can relay this dialog with Esc only.")
    } else if relay && blocker == "permission_prompt" {
        format!("Run cstan prompt show {agent_id}: it can relay this permission prompt.")
    } else {
        format!("cstan prompt show cannot relay it; look with cstan observe {agent_id} and tell the operator.")
    };
    let body = format!(
        "Agent blocked: {agent_id} cannot take messages because a dialog covers its input box; messages to it wait until it is cleared.\n{advice}"
    );
    kernel.mutate(
        context,
        "pm.input_blocked",
        "controller:reconcile",
        input,
        |_| {
            let queued = queue_pm_notice(
                kernel,
                "blocked",
                &agent_id,
                &episode,
                &body,
                &kernel.now(),
                true,
            )?;
            Ok(MutationOutput::new(
                json!({"queued": queued}),
                notice_event(kernel, json!({"queued": queued, "blocker": blocker})),
            ))
        },
    )
}

fn episode_text(episode: &Value) -> String {
    match episode.get("episodeMs") {
        Some(Value::Number(n)) => js_number(n),
        Some(Value::String(s)) => s.clone(),
        _ => "undefined".into(),
    }
}

/// `queueAttentionNotices`.
pub fn queue_attention_notices(
    kernel: &Kernel,
    context: &MutationContext,
    episodes: &Value,
) -> KernelResult<Value> {
    kernel.authorize(&context.credential, "controller:reconcile")?;
    let empty = Vec::new();
    let mut fresh: Vec<&Value> = Vec::new();
    for episode in episodes.as_array().unwrap_or(&empty) {
        let kind = episode.get("kind").and_then(Value::as_str).unwrap_or("");
        let agent = episode.get("agentId").and_then(Value::as_str).unwrap_or("");
        if kind == "blocked" && has_open_unreadable_notice(kernel, agent)? {
            continue;
        }
        let known = exists(
            &kernel.database,
            "SELECT 1 AS present FROM pm_notices WHERE project_id = ? AND kind = ? AND subject = ? AND episode = ?",
            [&kernel.project_id, kind, agent, &episode_text(episode)],
        )?;
        if !known {
            fresh.push(episode);
        }
    }
    if fresh.is_empty() {
        return Ok(json!({"queued": 0}));
    }
    kernel.mutate(
        context,
        "pm.attention",
        "controller:reconcile",
        &json!({"episodes": fresh}),
        |_| {
            let now = kernel.now();
            let mut queued = 0;
            for episode in &fresh {
                let kind = episode.get("kind").and_then(Value::as_str).unwrap_or("");
                let agent = episode.get("agentId").and_then(Value::as_str).unwrap_or("");
                let body = agent_stuck_notice(kind, agent, prompt_relay::enabled(kernel)?);
                if queue_pm_notice(
                    kernel,
                    kind,
                    agent,
                    &episode_text(episode),
                    &body,
                    &now,
                    true,
                )? {
                    queued += 1;
                }
            }
            Ok(MutationOutput::new(
                json!({"queued": queued}),
                notice_event(kernel, json!({"queued": queued})),
            ))
        },
    )
}

/// `recordPmWake`: records a wake line before it is typed, so a crash after the record never types it twice.
pub fn record_pm_wake(
    kernel: &Kernel,
    context: &MutationContext,
    message_id: &str,
) -> KernelResult<Value> {
    safe_id_str(message_id, "message id")?;
    kernel.mutate(
        context,
        "pm.wake",
        "controller:reconcile",
        &json!({"messageId": message_id}),
        |_| {
            let row = message_row(kernel, message_id)?;
            let recipient = match &row {
                Some(row) => kernel.agent_row(&row.recipient_agent_id)?,
                None => None,
            };
            if !matches!(&row, Some(r) if r.state == "queued")
                || !matches!(&recipient, Some(r) if r.kind == "PM")
            {
                return Err(KernelError::controller(
                    "the message is no longer waiting for the PM",
                ));
            }
            let now = kernel.now();
            execute(
                &kernel.database,
                "INSERT INTO pm_wakes(project_id, wake_id, message_id, sent_at) VALUES (?, ?, ?, ?)",
                rusqlite::params![kernel.project_id, kernel.env.uuid(), message_id, now],
            )?;
            let wakes = query_opt(
                &kernel.database,
                "SELECT COUNT(*) AS n FROM pm_wakes WHERE project_id = ? AND message_id = ?",
                [&kernel.project_id, message_id],
                |row| row.get::<_, i64>(0),
            )?
            .unwrap_or(0);
            Ok(MutationOutput::new(
                json!({"wakes": wakes}),
                MutationEvent::new("message_note", message_id, 0).with_details(json!({"wake": wakes})),
            ))
        },
    )
}

/// `#supervisionCheckDue`.
fn supervision_check_due(kernel: &Kernel, interval_seconds: i64) -> KernelResult<bool> {
    let active = query_all(
        &kernel.database,
        "SELECT agent_id, kind FROM agents WHERE project_id = ? AND state = 'active' AND kind IN ('Supervisor', 'Developer', 'Verifier')",
        [&kernel.project_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    let supervisors: Vec<&(String, String)> =
        active.iter().filter(|a| a.1 == "Supervisor").collect();
    if supervisors.len() != 1 || active.len() == 1 || notice_parties(kernel)?.is_none() {
        return Ok(false);
    }
    let last = query_opt(
        &kernel.database,
        "SELECT MAX(c.queued_at) AS at FROM supervision_checks c JOIN messages m ON m.project_id = c.project_id AND m.message_id = c.message_id
         WHERE c.project_id = ? AND m.recipient_agent_id = ?",
        [&kernel.project_id, &supervisors[0].0],
        |row| row.get::<_, Option<String>>(0),
    )?
    .flatten();
    Ok(match last {
        None => true,
        Some(at) => kernel.env.now() - parse_iso_ms(&at) >= interval_seconds * 1000,
    })
}

/// `queueSupervisionCheck`: queues the routine check for the one active Supervisor when a worker is active and the last
/// check is at least `interval_seconds` old; a check still open when the next is due is cancelled and replaced.
pub fn queue_supervision_check(
    kernel: &Kernel,
    context: &MutationContext,
    interval_seconds: f64,
) -> KernelResult<Value> {
    kernel.authorize(&context.credential, "controller:reconcile")?;
    if interval_seconds.fract() != 0.0
        || interval_seconds.abs() > 9_007_199_254_740_991.0
        || interval_seconds < 1.0
    {
        return Err(KernelError::type_error(
            "the check interval must be a positive integer",
        ));
    }
    let interval = interval_seconds as i64;
    // Nothing is written, not even an event, while no check is due: the tick asks every few seconds.
    if !supervision_check_due(kernel, interval)? {
        return Ok(json!({"queued": false, "cancelled": 0}));
    }
    kernel.mutate(
        context,
        "supervision.check",
        "controller:reconcile",
        &json!({"intervalSeconds": interval}),
        |actor| {
            let none = || {
                MutationOutput::new(
                    json!({"queued": false, "cancelled": 0}),
                    MutationEvent::new("supervision", &kernel.project_id, 0)
                        .with_details(json!({"queued": false})),
                )
            };
            let active = query_all(
                &kernel.database,
                "SELECT * FROM agents WHERE project_id = ? AND state = 'active' AND kind IN ('Supervisor', 'Developer', 'Verifier')",
                [&kernel.project_id],
                AgentRow::from_row,
            )?;
            let supervisors: Vec<&AgentRow> = active.iter().filter(|a| a.kind == "Supervisor").collect();
            let workers = active.iter().filter(|a| a.kind != "Supervisor").count();
            let parties = notice_parties(kernel)?;
            let (Some(parties), 1, true) = (parties, supervisors.len(), workers > 0) else {
                return Ok(none());
            };
            let supervisor = supervisors[0];
            let checks = query_all(
                &kernel.database,
                "SELECT m.* FROM messages m JOIN supervision_checks c ON c.project_id = m.project_id AND c.message_id = m.message_id
                 WHERE m.project_id = ? AND m.recipient_agent_id = ? ORDER BY m.sequence DESC",
                [&kernel.project_id, &supervisor.agent_id],
                MessageRow::from_row,
            )?;
            let now_ms = kernel.env.now();
            if let Some(last) = checks.first() {
                if now_ms - parse_iso_ms(&last.queued_at) < interval * 1000 {
                    return Ok(none());
                }
            }
            let now = kernel.now();
            let mut cancelled = 0;
            for open in &checks {
                if is_final_state(&open.state) {
                    continue;
                }
                let version = update_message(
                    kernel,
                    open,
                    "cancelled",
                    &[("state_reason", Sql::Text("superseded_check".into()))],
                    &now,
                )?;
                append_message_event(
                    kernel,
                    actor,
                    context,
                    &open.message_id,
                    &open.state,
                    "cancelled",
                    version,
                    &json!({"reason": "superseded_check"}),
                )?;
                cancelled += 1;
            }
            let body = SUPERVISION_CHECK_TEXT;
            let message_id = insert_queued_message(
                kernel,
                &parties.controller_actor_id,
                supervisor,
                body,
                &sha256(body),
                &now,
                false,
            )?;
            execute(
                &kernel.database,
                "INSERT INTO supervision_checks(project_id, message_id, queued_at) VALUES (?, ?, ?)",
                [&kernel.project_id, &message_id, &now],
            )?;
            Ok(MutationOutput::new(
                json!({"queued": true, "cancelled": cancelled}),
                MutationEvent::new("supervision", &kernel.project_id, 0)
                    .with_details(json!({"queued": true, "cancelled": cancelled})),
            ))
        },
    )
}

/// `#evaluateMessaging`: reads the facts and runs the pure evaluation.
fn evaluate(kernel: &Kernel, timers: &MessagingTimers) -> KernelResult<MessagingEvaluation> {
    let now_ms = kernel.env.now();
    let wakes: HashMap<String, (i64, i64)> = query_all(
        &kernel.database,
        "SELECT message_id, COUNT(*) AS n, MAX(sent_at) AS last FROM pm_wakes WHERE project_id = ? GROUP BY message_id",
        [&kernel.project_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    )?
    .into_iter()
    .map(|(id, n, last)| (id, (n, parse_iso_ms(&last))))
    .collect();
    let rows = query_all(
        &kernel.database,
        &format!("SELECT m.*, EXISTS (SELECT 1 FROM message_input_clears c WHERE c.project_id = m.project_id AND c.message_id = m.message_id AND c.deferral_count = m.deferral_count) AS input_clear_recorded
           FROM messages m INDEXED BY messages_by_state JOIN agents a ON a.project_id = m.project_id AND a.agent_id = m.recipient_agent_id
           WHERE m.project_id = ? AND a.state = 'active' AND m.state IN ({OPEN_MESSAGE_STATES_SQL})
           ORDER BY m.sequence"),
        [&kernel.project_id],
        |row| Ok((MessageRow::from_row(row)?, row.get::<_, i64>("input_clear_recorded")?)),
    )?;
    let mut messages: Vec<MessageFacts> = Vec::new();
    for (row, input_clear_recorded) in &rows {
        let resumed = pauses::resumed_ms_for(kernel, &row.recipient_agent_id)?;
        let wake = wakes.get(&row.message_id);
        messages.push(MessageFacts {
            wake_count: wake.map_or(0, |w| w.0),
            last_wake_ms: wake.map(|w| w.1),
            message_id: row.message_id.clone(),
            recipient_agent_id: row.recipient_agent_id.clone(),
            state: row.state.clone(),
            sequence: row.sequence,
            queued_ms: parse_iso_ms(&row.queued_at).max(resumed),
            sent_ms: row.sent_at.as_deref().map(parse_iso_ms),
            deferred_ms: row
                .deferred_at
                .as_deref()
                .map(|at| parse_iso_ms(at).max(resumed)),
            deferred_reason: row.deferred_reason.clone(),
            input_clear_recorded: *input_clear_recorded == 1,
            last_notified_ms: row.last_notified_at.as_deref().map(parse_iso_ms),
        });
    }
    let agent_rows = query_all(
        &kernel.database,
        "SELECT agent_id, kind, last_activity_at FROM agents WHERE project_id = ? AND state = 'active'",
        [&kernel.project_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    )?;
    let pause_state = pauses::pause_state_of(kernel)?;
    let mut agents: Vec<AgentFacts> = Vec::new();
    for (agent_id, kind, last_activity_at) in &agent_rows {
        // A pause covers the time up to its resume: timers start again from there.
        let resumed_ms = pauses::resumed_ms_for(kernel, agent_id)?;
        let last_activity_ms = parse_iso_ms(last_activity_at).max(resumed_ms);
        // No timer looks further back than the agent's last activity or its oldest open message.
        let cutoff_ms = messages
            .iter()
            .filter(|m| &m.recipient_agent_id == agent_id)
            .map(|m| m.queued_ms)
            .fold(last_activity_ms, i64::min);
        let cutoff = iso_from_millis(cutoff_ms);
        let observation = |row: &rusqlite::Row<'_>| -> rusqlite::Result<StateObservation> {
            Ok(StateObservation {
                state: row.get(0)?,
                at_ms: parse_iso_ms(&row.get::<_, String>(1)?),
            })
        };
        let entries = query_all(
            &kernel.database,
            "SELECT herdr_state, observed_at FROM agent_state_history
             WHERE project_id = ? AND agent_id = ? AND observed_at >= ? ORDER BY sequence",
            [&kernel.project_id, agent_id, &cutoff],
            observation,
        )?;
        let before = query_opt(
            &kernel.database,
            "SELECT herdr_state, observed_at FROM agent_state_history
             WHERE project_id = ? AND agent_id = ? AND observed_at < ? ORDER BY sequence DESC LIMIT 1",
            [&kernel.project_id, agent_id, &cutoff],
            observation,
        )?;
        let waits = query_all(
            &kernel.database,
            "SELECT started_at, ended_at FROM agent_waits
             WHERE project_id = ? AND agent_id = ? AND (ended_at IS NULL OR ended_at >= ?)",
            [&kernel.project_id, agent_id, &cutoff],
            |row| {
                Ok(WaitInterval {
                    start_ms: parse_iso_ms(&row.get::<_, String>(0)?),
                    end_ms: row
                        .get::<_, Option<String>>(1)?
                        .as_deref()
                        .map(parse_iso_ms),
                })
            },
        )?;
        let mut observations: Vec<StateObservation> = Vec::new();
        observations.extend(before);
        observations.extend(entries);
        agents.push(AgentFacts {
            agent_id: agent_id.clone(),
            kind: kind.clone(),
            paused: pause_state
                .agents
                .iter()
                .any(|p| p.agent_id.as_deref() == Some(agent_id.as_str()))
                || (pause_state.run.is_some() && kind != "PM"),
            last_activity_ms,
            observations,
            waits,
        });
    }
    Ok(evaluate_messaging(&agents, &messages, now_ms, timers))
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "openWaits" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(open_waits(kernel, credential)))
        }
        "recordNotification" => {
            let context = arg!(a.ctx(0));
            let message_id = arg!(a.str(1, "messageId"));
            Some(call(record_notification(kernel, &context, message_id)))
        }
        "beginWait" => {
            let context = arg!(a.ctx(0));
            Some(call(begin_wait(kernel, &context)))
        }
        "endWait" => {
            let context = arg!(a.ctx(0));
            let wait_id = arg!(a.str(1, "waitId"));
            Some(call(end_wait(kernel, &context, wait_id)))
        }
        "endWaitAsController" => {
            let context = arg!(a.ctx(0));
            let wait_id = arg!(a.str(1, "waitId"));
            Some(call(end_wait_as_controller(kernel, &context, wait_id)))
        }
        "advanceMessaging" => {
            let context = arg!(a.ctx(0));
            let timers = a.value(1);
            Some(call(advance_messaging(kernel, &context, timers)))
        }
        "queueMissingDeliveryNotices" => {
            let context = arg!(a.ctx(0));
            Some(call(queue_missing_delivery_notices(kernel, &context)))
        }
        "queueInputBlockedNotice" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(queue_input_blocked_notice(kernel, &context, input)))
        }
        "queueAttentionNotices" => {
            let context = arg!(a.ctx(0));
            let episodes = a.value(1);
            Some(call(queue_attention_notices(kernel, &context, episodes)))
        }
        "recordPmWake" => {
            let context = arg!(a.ctx(0));
            let message_id = arg!(a.str(1, "messageId"));
            Some(call(record_pm_wake(kernel, &context, message_id)))
        }
        "queueSupervisionCheck" => {
            let context = arg!(a.ctx(0));
            let interval_seconds = arg!(a.f64(1, "intervalSeconds"));
            Some(call(queue_supervision_check(
                kernel,
                &context,
                interval_seconds,
            )))
        }
        _ => None,
    }
}
