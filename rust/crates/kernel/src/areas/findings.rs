//! The supervisor findings (src/controller/findings.ts).

use crate::auth::AuthenticatedActor;
use crate::canonical::sha256;
use crate::dispatch::{arg, call, Args, ErrorHandler};
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{
    execute, finding_notice_body, finding_task, finding_text, query_all, query_opt, safe_id,
};
use crate::kernel::Kernel;
use crate::records::{
    is_final_state, AgentFindingRow, AgentRow, MutationEvent, MutationOutput,
    FINDING_INTERVENTIONS, FINDING_SEVERITIES, MAX_FINDING_CONDITION_BYTES,
    MAX_FINDING_CORRECTION_BYTES, MAX_FINDING_EVIDENCE_BYTES,
};
use crate::types::MutationContext;
use rusqlite::types::Value as Sql;
use serde_json::{json, Value};

use super::{message_notices, messages, messaging, pauses};

fn finding_row(kernel: &Kernel, finding_id: &str) -> KernelResult<AgentFindingRow> {
    query_opt(
        &kernel.database,
        "SELECT * FROM agent_findings WHERE project_id = ? AND finding_id = ?",
        [&kernel.project_id, finding_id],
        AgentFindingRow::from_row,
    )?
    .ok_or_else(|| KernelError::controller("the finding does not exist"))
}

fn finding_record(kernel: &Kernel, row: &AgentFindingRow) -> KernelResult<Value> {
    let deliveries = query_all(
        &kernel.database,
        "SELECT d.attempt, d.message_id, m.state AS message_state FROM agent_finding_deliveries d
         LEFT JOIN messages m ON m.project_id = d.project_id AND m.message_id = d.message_id
         WHERE d.project_id = ? AND d.finding_id = ? ORDER BY d.attempt",
        [&kernel.project_id, &row.finding_id],
        |r| {
            Ok(json!({
                "attempt": r.get::<_, i64>(0)?,
                "messageId": r.get::<_, String>(1)?,
                "messageState": r.get::<_, Option<String>>(2)?,
            }))
        },
    )?;
    let checks = query_all(
        &kernel.database,
        "SELECT after_intervention, result, evidence_text, created_at FROM agent_finding_checks WHERE project_id = ? AND finding_id = ? ORDER BY after_intervention",
        [&kernel.project_id, &row.finding_id],
        |r| {
            Ok(json!({
                "afterIntervention": r.get::<_, i64>(0)?,
                "result": r.get::<_, String>(1)?,
                "evidence": r.get::<_, String>(2)?,
                "createdAt": r.get::<_, String>(3)?,
            }))
        },
    )?;
    Ok(json!({
        "findingId": row.finding_id,
        "sequence": row.sequence,
        "targetAgentId": row.target_agent_id,
        "raisedByAgentId": row.raised_by_agent_id,
        "severity": row.severity,
        "evidence": row.evidence_text,
        "requestedCorrection": row.requested_correction,
        "resolutionCondition": row.resolution_condition,
        "state": row.state,
        "interventions": row.interventions,
        "stateReason": row.state_reason,
        "createdAt": row.created_at,
        "closedAt": row.closed_at,
        "deliveries": deliveries,
        "checks": checks,
    }))
}

/// Writes one intervention: the delivery row and the message from the controller to the target.
fn deliver_finding(
    kernel: &Kernel,
    finding: &AgentFindingRow,
    target: &AgentRow,
    attempt: i64,
    evidence: &str,
    now: &str,
) -> KernelResult<String> {
    let body = finding_task(finding, attempt, evidence);
    let message_id = messages::insert_queued_message(
        kernel,
        &kernel.controller_actor_id()?,
        target,
        &body,
        &sha256(&body),
        now,
        true,
    )?;
    execute(
        &kernel.database,
        "INSERT INTO agent_finding_deliveries(project_id, finding_id, attempt, message_id, evidence_text, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        rusqlite::params![
            kernel.project_id,
            finding.finding_id,
            attempt,
            message_id,
            evidence,
            now
        ],
    )?;
    Ok(message_id)
}

/// `raiseFinding`: the finding, the first delivery and the PM notice, in one transaction.
pub fn raise_finding(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let field = |name: &str| input.get(name).unwrap_or(&Value::Null).clone();
    let target_agent_id = safe_id(&field("targetAgentId"), "target agent id")?.to_string();
    let severity = field("severity");
    let severity = match severity.as_str() {
        Some(text) if FINDING_SEVERITIES.contains(&text) => text.to_string(),
        _ => {
            return Err(KernelError::type_error(format!(
                "severity must be one of {}",
                FINDING_SEVERITIES.join(", ")
            )))
        }
    };
    let evidence = finding_text(&field("evidence"), "evidence", MAX_FINDING_EVIDENCE_BYTES)?;
    let correction = finding_text(
        &field("correction"),
        "correction",
        MAX_FINDING_CORRECTION_BYTES,
    )?;
    let done_when = finding_text(
        &field("doneWhen"),
        "done-when condition",
        MAX_FINDING_CONDITION_BYTES,
    )?;
    kernel.mutate(
        context,
        "finding.raise",
        "finding:raise",
        &json!({
            "targetAgentId": target_agent_id,
            "severity": severity,
            "evidenceHash": sha256(&evidence),
            "correctionHash": sha256(&correction),
            "doneWhenHash": sha256(&done_when),
        }),
        |actor| {
            let caller = match kernel.agent_by_actor(&actor.actor_id)? {
                Some(caller) if caller.kind == "Supervisor" && caller.state == "active" => caller,
                _ => {
                    return Err(KernelError::controller(
                        "only an active Supervisor can raise a finding",
                    ))
                }
            };
            let target = match kernel.agent_row(&target_agent_id)? {
                Some(target) if target.state == "active" => target,
                _ => return Err(KernelError::controller("the target agent is not active")),
            };
            if target.kind != "Developer" && target.kind != "Verifier" {
                return Err(KernelError::controller(
                    "a finding can only be raised about a Developer or Verifier agent",
                ));
            }
            if pauses::delivery_paused(kernel, &target.agent_id)? {
                return Err(KernelError::RunPaused(format!(
                    "target is paused: {} is held, so a finding cannot be raised about it",
                    target.agent_id
                )));
            }
            if target.agent_id == caller.agent_id {
                return Err(KernelError::controller(
                    "a Supervisor cannot raise a finding about itself",
                ));
            }
            let open = query_opt(
                &kernel.database,
                "SELECT finding_id FROM agent_findings WHERE project_id = ? AND target_agent_id = ? AND state = 'open'",
                [&kernel.project_id, &target.agent_id],
                |row| row.get::<_, String>(0),
            )?;
            if let Some(open) = open {
                return Err(KernelError::controller(format!(
                    "finding {open} is already open for this agent"
                )));
            }
            let now = kernel.now();
            let finding_id = kernel.env.uuid();
            let sequence = next_sequence(kernel, "agent_findings")?;
            execute(
                &kernel.database,
                "INSERT INTO agent_findings(project_id, finding_id, sequence, target_agent_id, raised_by_agent_id, raised_by_actor_id, severity,
                   evidence_text, requested_correction, resolution_condition, state, interventions, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'open', 1, ?)",
                rusqlite::params![
                    kernel.project_id,
                    finding_id,
                    sequence,
                    target.agent_id,
                    caller.agent_id,
                    actor.actor_id,
                    severity,
                    evidence,
                    correction,
                    done_when,
                    now
                ],
            )?;
            let row = finding_row(kernel, &finding_id)?;
            deliver_finding(kernel, &row, &target, 1, &evidence, &now)?;
            queue_finding_notice(kernel, &finding_id, "raised", &now)?;
            Ok(MutationOutput::new(
                finding_record(kernel, &finding_row(kernel, &finding_id)?)?,
                MutationEvent {
                    to_state: Some("open".into()),
                    ..MutationEvent::new("agent_finding", &finding_id, 0)
                }
                .with_details(json!({
                    "targetAgentId": target.agent_id,
                    "raisedByAgentId": caller.agent_id,
                    "severity": severity,
                })),
            ))
        },
    )
}

fn next_sequence(kernel: &Kernel, table: &str) -> KernelResult<i64> {
    Ok(query_opt(
        &kernel.database,
        &format!("SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM {table} WHERE project_id = ?"),
        [&kernel.project_id],
        |row| row.get::<_, i64>(0),
    )?
    .unwrap_or(1))
}

/// The state of the message that delivered an intervention.
fn delivery_message_state(
    kernel: &Kernel,
    finding_id: &str,
    attempt: i64,
) -> KernelResult<Option<String>> {
    query_opt(
        &kernel.database,
        "SELECT m.state FROM agent_finding_deliveries d
         JOIN messages m ON m.project_id = d.project_id AND m.message_id = d.message_id
         WHERE d.project_id = ? AND d.finding_id = ? AND d.attempt = ?",
        rusqlite::params![kernel.project_id, finding_id, attempt],
        |row| row.get::<_, String>(0),
    )
}

#[allow(clippy::too_many_arguments)]
fn insert_check(
    kernel: &Kernel,
    finding_id: &str,
    after_intervention: i64,
    result: &str,
    evidence: &str,
    actor_id: &str,
    now: &str,
) -> KernelResult<()> {
    let check_id = kernel.env.uuid();
    execute(
        &kernel.database,
        "INSERT INTO agent_finding_checks(project_id, finding_id, check_id, after_intervention, result, evidence_text, checked_by_actor_id, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        rusqlite::params![
            kernel.project_id,
            finding_id,
            check_id,
            after_intervention,
            result,
            evidence,
            actor_id,
            now
        ],
    )?;
    Ok(())
}

/// `checkFinding`: the raising Supervisor's resolution check.
pub fn check_finding(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let field = |name: &str| input.get(name).unwrap_or(&Value::Null).clone();
    let finding_id = safe_id(&field("findingId"), "finding id")?.to_string();
    let result = match field("result").as_str() {
        Some(text @ ("resolved" | "unresolved")) => text.to_string(),
        _ => {
            return Err(KernelError::type_error(
                "the result must be resolved or unresolved",
            ))
        }
    };
    let evidence = finding_text(&field("evidence"), "evidence", MAX_FINDING_EVIDENCE_BYTES)?;
    kernel.mutate(
        context,
        "finding.check",
        "finding:check",
        &json!({
            "findingId": finding_id,
            "result": result,
            "evidenceHash": sha256(&evidence),
        }),
        |actor| {
            let caller = kernel.agent_by_actor(&actor.actor_id)?;
            let finding = finding_row(kernel, &finding_id)?;
            match &caller {
                Some(caller)
                    if caller.kind == "Supervisor"
                        && caller.state == "active"
                        && caller.agent_id == finding.raised_by_agent_id => {}
                _ => {
                    return Err(KernelError::controller(
                        "only the Supervisor that raised the finding can check it",
                    ))
                }
            }
            if finding.state != "open" {
                return Err(KernelError::controller(format!(
                    "the finding is already {}",
                    finding.state
                )));
            }
            let message_state = delivery_message_state(kernel, &finding.finding_id, finding.interventions)?;
            if !matches!(message_state.as_deref(), Some("acked" | "acked_late")) {
                return Err(KernelError::controller(
                    "the target has not acknowledged the latest correction yet",
                ));
            }
            let now = kernel.now();
            insert_check(
                kernel,
                &finding.finding_id,
                finding.interventions,
                &result,
                &evidence,
                &actor.actor_id,
                &now,
            )?;
            let mut to_state = "open";
            if result == "resolved" {
                close_finding(kernel, actor, context, &finding, "resolved", None, &now)?;
                to_state = "resolved";
            } else if finding.interventions < FINDING_INTERVENTIONS {
                let target = match kernel.agent_row(&finding.target_agent_id)? {
                    Some(target) if target.state == "active" => target,
                    _ => return Err(KernelError::controller("the target agent is not active")),
                };
                let moved = execute(
                    &kernel.database,
                    "UPDATE agent_findings SET interventions = 2 WHERE project_id = ? AND finding_id = ? AND state = 'open' AND interventions = 1",
                    [&kernel.project_id, &finding.finding_id],
                )?;
                if moved != 1 {
                    return Err(KernelError::controller(
                        "the finding changed during the check",
                    ));
                }
                deliver_finding(
                    kernel,
                    &finding_row(kernel, &finding.finding_id)?,
                    &target,
                    2,
                    &evidence,
                    &now,
                )?;
            } else {
                close_finding(
                    kernel,
                    actor,
                    context,
                    &finding,
                    "escalated",
                    Some("second_unresolved"),
                    &now,
                )?;
                to_state = "escalated";
            }
            Ok(MutationOutput::new(
                finding_record(kernel, &finding_row(kernel, &finding.finding_id)?)?,
                MutationEvent::new("agent_finding", &finding.finding_id, 0)
                    .transition("open", to_state)
                    .with_details(json!({
                        "result": result,
                        "afterIntervention": finding.interventions,
                    })),
            ))
        },
    )
}

/// `closeFinding`: closes an open finding: the state change is conditional, a delivery message that is not final is
/// cancelled, and the PM notice is queued. The caller owns the transaction.
pub(crate) fn close_finding(
    kernel: &Kernel,
    actor: &AuthenticatedActor,
    context: &MutationContext,
    finding: &AgentFindingRow,
    state: &str,
    reason: Option<&str>,
    now: &str,
) -> KernelResult<()> {
    let closed = execute(
        &kernel.database,
        "UPDATE agent_findings SET state = ?, state_reason = ?, closed_at = ? WHERE project_id = ? AND finding_id = ? AND state = 'open'",
        rusqlite::params![state, reason, now, kernel.project_id, finding.finding_id],
    )?;
    if closed != 1 {
        return Err(KernelError::controller("the finding is no longer open"));
    }
    let deliveries = query_all(
        &kernel.database,
        "SELECT message_id FROM agent_finding_deliveries WHERE project_id = ? AND finding_id = ?",
        [&kernel.project_id, &finding.finding_id],
        |row| row.get::<_, String>(0),
    )?;
    for message_id in deliveries {
        let Some(message) = messages::message_row(kernel, &message_id)? else {
            continue;
        };
        if is_final_state(&message.state) {
            continue;
        }
        let version = messages::update_message(
            kernel,
            &message,
            "cancelled",
            &[("state_reason", Sql::Text("finding_closed".into()))],
            now,
        )?;
        messages::append_message_event(
            kernel,
            actor,
            context,
            &message.message_id,
            &message.state,
            "cancelled",
            version,
            &json!({"reason": "finding_closed"}),
        )?;
    }
    queue_finding_notice(kernel, &finding.finding_id, state, now)
}

/// Inserts the PM notice row for an event and announces it at once when it can be (exactly one active PM and no
/// earlier notice of this finding waiting).
fn queue_finding_notice(
    kernel: &Kernel,
    finding_id: &str,
    event: &str,
    now: &str,
) -> KernelResult<()> {
    let notice_id = kernel.env.uuid();
    let sequence = next_sequence(kernel, "agent_finding_notices")?;
    execute(
        &kernel.database,
        "INSERT INTO agent_finding_notices(project_id, notice_id, sequence, finding_id, event, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        rusqlite::params![kernel.project_id, notice_id, sequence, finding_id, event, now],
    )?;
    announce_notice(kernel, &notice_id, now)?;
    Ok(())
}

/// Queues the PM message for a waiting notice; false when it must wait. The caller owns the transaction.
fn announce_notice(kernel: &Kernel, notice_id: &str, now: &str) -> KernelResult<bool> {
    let notice = query_opt(
        &kernel.database,
        "SELECT sequence, finding_id, event FROM agent_finding_notices WHERE project_id = ? AND notice_id = ? AND message_id IS NULL",
        [&kernel.project_id, notice_id],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    )?;
    let Some((sequence, finding_id, event)) = notice else {
        return Ok(false);
    };
    let earlier = query_opt(
        &kernel.database,
        "SELECT 1 AS present FROM agent_finding_notices WHERE project_id = ? AND finding_id = ? AND message_id IS NULL AND sequence < ?",
        rusqlite::params![kernel.project_id, finding_id, sequence],
        |_| Ok(()),
    )?;
    if earlier.is_some() {
        return Ok(false);
    }
    let Some(parties) = message_notices::notice_parties(kernel)? else {
        return Ok(false);
    };
    let finding = finding_row(kernel, &finding_id)?;
    let last = query_opt(
        &kernel.database,
        "SELECT evidence_text FROM agent_finding_checks WHERE project_id = ? AND finding_id = ? ORDER BY after_intervention DESC LIMIT 1",
        [&kernel.project_id, &finding_id],
        |row| row.get::<_, String>(0),
    )?;
    let body = finding_notice_body(&finding, &event, last.as_deref());
    let message_id = messages::insert_queued_message(
        kernel,
        &parties.controller_actor_id,
        &parties.pm,
        &body,
        &sha256(&body),
        now,
        true,
    )?;
    execute(
        &kernel.database,
        "UPDATE agent_finding_notices SET message_id = ? WHERE project_id = ? AND notice_id = ? AND message_id IS NULL",
        [&message_id, &kernel.project_id, notice_id],
    )?;
    Ok(true)
}

/// `unannouncedFindingNotices`: finding notices that are still waiting for a PM, oldest first.
pub fn unannounced_finding_notices(
    kernel: &Kernel,
    credential: &str,
    limit: Option<i64>,
) -> KernelResult<Value> {
    kernel.authorize(credential, "controller:reconcile")?;
    let rows = query_all(
        &kernel.database,
        "SELECT notice_id, sequence, finding_id, event, message_id FROM agent_finding_notices WHERE project_id = ? AND message_id IS NULL ORDER BY sequence LIMIT ?",
        rusqlite::params![kernel.project_id, limit.unwrap_or(50)],
        |row| {
            Ok(json!({
                "noticeId": row.get::<_, String>(0)?,
                "sequence": row.get::<_, i64>(1)?,
                "findingId": row.get::<_, String>(2)?,
                "event": row.get::<_, String>(3)?,
                "messageId": row.get::<_, Option<String>>(4)?,
            }))
        },
    )?;
    Ok(Value::Array(rows))
}

/// `announceFindingNotice`.
pub fn announce_finding_notice(
    kernel: &Kernel,
    context: &MutationContext,
    notice_id: &str,
) -> KernelResult<Value> {
    safe_id(&json!(notice_id), "notice id")?;
    kernel.mutate(
        context,
        "finding.announce",
        "controller:reconcile",
        &json!({"noticeId": notice_id}),
        |_| {
            let announced = announce_notice(kernel, notice_id, &kernel.now())?;
            Ok(MutationOutput::new(
                json!({"announced": announced}),
                MutationEvent::new("agent_finding", notice_id, 0)
                    .with_details(json!({"announced": announced})),
            ))
        },
    )
}

/// `assertCanObserve`: refuses a caller whose actor lacks the capability to observe agents.
pub fn assert_can_observe(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    kernel.authorize(credential, "agent:observe")?;
    Ok(Value::Null)
}

/// `findings`: newest first, with their deliveries and checks.
pub fn findings(kernel: &Kernel, credential: &str, limit: Option<i64>) -> KernelResult<Value> {
    kernel.authorize(credential, "controller:reconcile")?;
    let rows = query_all(
        &kernel.database,
        "SELECT * FROM agent_findings WHERE project_id = ? ORDER BY sequence DESC LIMIT ?",
        rusqlite::params![kernel.project_id, limit.unwrap_or(20)],
        AgentFindingRow::from_row,
    )?;
    let records = rows
        .iter()
        .map(|row| finding_record(kernel, row))
        .collect::<KernelResult<Vec<_>>>()?;
    Ok(Value::Array(records))
}

/// `sweepFindings`: ends every open finding whose latest delivery has seen no change for `deadline_seconds` as
/// escalated, with a check by the controller's own actor. Returns the ids it escalated.
pub fn sweep_findings(
    kernel: &Kernel,
    new_context: &dyn Fn() -> MutationContext,
    deadline_seconds: f64,
    on_error: Option<ErrorHandler<'_>>,
) -> KernelResult<Value> {
    if !deadline_seconds.is_finite() || deadline_seconds.fract() != 0.0 || deadline_seconds < 1.0 {
        return Err(KernelError::type_error(
            "the deadline must be a positive number of seconds",
        ));
    }
    let deadline = deadline_seconds as i64;
    let first = new_context();
    kernel.authorize(&first.credential, "controller:reconcile")?;
    let due = due_finding_ids(kernel, deadline)?;
    let mut escalated = Vec::new();
    let mut first = Some(first);
    for finding_id in due {
        let context = first.take().unwrap_or_else(new_context);
        match sweep_one(kernel, &context, &finding_id, deadline) {
            Ok(true) => escalated.push(Value::String(finding_id)),
            Ok(false) => {}
            Err(error) => match on_error {
                Some(handler) => handler(&finding_id, &error),
                None => return Err(error),
            },
        }
    }
    Ok(Value::Array(escalated))
}

fn sweep_one(
    kernel: &Kernel,
    context: &MutationContext,
    finding_id: &str,
    deadline: i64,
) -> KernelResult<bool> {
    let value = kernel.mutate(
        context,
        "finding.deadline",
        "controller:reconcile",
        &json!({"findingId": finding_id, "deadlineSeconds": deadline}),
        |actor| {
            let finding = finding_row(kernel, finding_id)?;
            if finding.state != "open"
                || !due_finding_ids(kernel, deadline)?
                    .iter()
                    .any(|id| id == finding_id)
            {
                return Ok(MutationOutput::new(
                    json!(false),
                    MutationEvent::new("agent_finding", finding_id, 0)
                        .with_details(json!({"skipped": true})),
                ));
            }
            let now = kernel.now();
            let latest = delivery_message_state(kernel, finding_id, finding.interventions)?;
            let acknowledged = matches!(latest.as_deref(), Some("acked" | "acked_late"));
            let evidence = if acknowledged {
                format!("no check by the supervisor within {deadline} seconds of the latest change to the delivery (the correction was acknowledged)")
            } else {
                format!(
                    "the latest correction was not acknowledged and no check was recorded within {deadline} seconds of its latest change (message state {})",
                    latest.as_deref().unwrap_or("unknown")
                )
            };
            insert_check(
                kernel,
                finding_id,
                finding.interventions,
                "timed_out",
                &evidence,
                &actor.actor_id,
                &now,
            )?;
            close_finding(kernel, actor, context, &finding, "escalated", Some("timed_out"), &now)?;
            Ok(MutationOutput::new(
                json!(true),
                MutationEvent::new("agent_finding", finding_id, 0)
                    .transition("open", "escalated")
                    .with_details(json!({"reason": "timed_out", "deadlineSeconds": deadline})),
            ))
        },
    )?;
    Ok(value.as_bool().unwrap_or(false))
}

fn due_finding_ids(kernel: &Kernel, deadline_seconds: i64) -> KernelResult<Vec<String>> {
    type Due = (
        String,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
    );
    let rows = query_all(
        &kernel.database,
        "SELECT f.finding_id, f.target_agent_id, d.created_at AS delivered_at, m.queued_at, m.sent_at, m.acked_at
         FROM agent_findings f
         JOIN agent_finding_deliveries d ON d.project_id = f.project_id AND d.finding_id = f.finding_id AND d.attempt = f.interventions
         JOIN messages m ON m.project_id = d.project_id AND m.message_id = d.message_id
         WHERE f.project_id = ? AND f.state = 'open' ORDER BY f.sequence",
        [&kernel.project_id],
        |row| -> rusqlite::Result<Due> {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        },
    )?;
    let now_ms = messaging::parse_iso_ms(&kernel.now());
    let mut due = Vec::new();
    for (finding_id, target, delivered, queued, sent, acked) in rows {
        if pauses::delivery_paused(kernel, &target)? {
            continue;
        }
        let anchor = [Some(delivered), Some(queued), sent, acked]
            .iter()
            .flatten()
            .map(|value| messaging::parse_iso_ms(value))
            .chain(std::iter::once(pauses::resumed_ms_for(kernel, &target)?))
            .max()
            .unwrap_or(0);
        if now_ms - anchor >= deadline_seconds * 1000 {
            due.push(finding_id);
        }
    }
    Ok(due)
}

/// `supervisionReason`: why supervision is degraded: the reason of the newest degraded event, or null.
pub fn supervision_reason(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    kernel.authorize(credential, "controller:reconcile")?;
    let reason = query_opt(
        &kernel.database,
        "SELECT json_extract(e.payload_json, '$.details.reason') AS reason
         FROM supervision_control c
         LEFT JOIN controller_events e ON e.project_id = c.project_id
           AND e.sequence = (SELECT MAX(sequence) FROM controller_events
             WHERE project_id = c.project_id AND entity_type = 'run_control' AND to_state = 'degraded')
         WHERE c.project_id = ? AND c.health = 'degraded'",
        [&kernel.project_id],
        |row| row.get::<_, Sql>(0),
    )?;
    Ok(match reason {
        Some(Sql::Text(text)) => Value::String(text),
        _ => Value::Null,
    })
}

/// `supervisionActivity`: what the live supervision loop is doing now.
pub fn supervision_activity(kernel: &Kernel) -> KernelResult<Value> {
    kernel.assert_open()?;
    let supervisor = query_opt(
        &kernel.database,
        "SELECT agent_id, state FROM agents WHERE project_id = ? AND kind = 'Supervisor' AND state = 'active' ORDER BY created_at DESC LIMIT 1",
        [&kernel.project_id],
        |row| Ok(json!({"agentId": row.get::<_, String>(0)?, "state": row.get::<_, String>(1)?})),
    )?;
    let any_check = query_opt(
        &kernel.database,
        "SELECT 1 AS present FROM supervision_checks WHERE project_id = ? LIMIT 1",
        [&kernel.project_id],
        |_| Ok(()),
    )?;
    let check = if any_check.is_none() {
        None
    } else {
        query_opt(
            &kernel.database,
            "SELECT m.message_id, m.state, m.queued_at, m.acked_at
             FROM supervision_checks c JOIN messages m ON m.project_id = c.project_id AND m.message_id = c.message_id
             WHERE c.project_id = ? ORDER BY m.sequence DESC LIMIT 1",
            [&kernel.project_id],
            |row| {
                Ok(json!({
                    "messageId": row.get::<_, String>(0)?,
                    "state": row.get::<_, String>(1)?,
                    "queuedAt": row.get::<_, String>(2)?,
                    "ackedAt": row.get::<_, Option<String>>(3)?,
                }))
            },
        )?
    };
    let open = query_opt(
        &kernel.database,
        "SELECT COUNT(*) AS n FROM agent_findings WHERE project_id = ? AND state = 'open'",
        [&kernel.project_id],
        |row| row.get::<_, i64>(0),
    )?
    .unwrap_or(0);
    Ok(json!({
        "supervisor": supervisor.unwrap_or(Value::Null),
        "lastCheck": check.unwrap_or(Value::Null),
        "openFindings": open,
    }))
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "raiseFinding" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(raise_finding(kernel, &context, input)))
        }
        "checkFinding" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(check_finding(kernel, &context, input)))
        }
        "unannouncedFindingNotices" => {
            let credential = arg!(a.str(0, "credential"));
            let limit = arg!(a.opt_i64(1, "limit"));
            Some(call(unannounced_finding_notices(kernel, credential, limit)))
        }
        "announceFindingNotice" => {
            let context = arg!(a.ctx(0));
            let notice_id = arg!(a.str(1, "noticeId"));
            Some(call(announce_finding_notice(kernel, &context, notice_id)))
        }
        "assertCanObserve" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(assert_can_observe(kernel, credential)))
        }
        "findings" => {
            let credential = arg!(a.str(0, "credential"));
            let limit = arg!(a.opt_i64(1, "limit"));
            Some(call(findings(kernel, credential, limit)))
        }
        "supervisionReason" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(supervision_reason(kernel, credential)))
        }
        "supervisionActivity" => Some(call(supervision_activity(kernel))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::areas::status::legacy_rows::{context, project};
    use crate::dispatch::dispatch;
    use crate::env::{Env, SeededEnv};
    use crate::kernel::KernelOptions;
    use std::cell::Cell;
    use std::rc::Rc;

    /// The seeded clock and randomness, with a clock that can also be moved forward by hand.
    struct SteppedEnv {
        inner: SeededEnv,
        skipped: Rc<Cell<i64>>,
    }

    impl Env for SteppedEnv {
        fn now(&self) -> i64 {
            self.inner.now() + self.skipped.get()
        }
        fn uuid(&self) -> String {
            self.inner.uuid()
        }
        fn random_bytes(&self, n: usize) -> Vec<u8> {
            self.inner.random_bytes(n)
        }
    }

    fn call(kernel: &Kernel, credential: &str, label: &str, op: &str, args: &[Value]) -> Value {
        let mut all = vec![serde_json::to_value(context(kernel, credential, label)).unwrap()];
        all.extend_from_slice(args);
        dispatch(kernel, op, &all).unwrap_or_else(|e| panic!("{op}: {e}"))
    }

    /// `sweepFindings` has no parity sequence (its first argument is a function); this exercises it on a clock moved by hand.
    #[test]
    fn sweeping_escalates_a_finding_nobody_checked_within_the_deadline() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(
            directory.path(),
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .unwrap();
        let skipped = Rc::new(Cell::new(0));
        let info = project("psweepfindings", "owner-sweep-credential-0123456789");
        let kernel = Kernel::open(
            directory.path(),
            &info,
            &KernelOptions::default(),
            Box::new(SteppedEnv {
                inner: SeededEnv::new("sweep"),
                skipped: Rc::clone(&skipped),
            }),
        )
        .unwrap();
        let owner = info.owner_credential.clone();
        call(
            &kernel,
            &owner,
            "roles",
            "syncRoleDefinitions",
            &[json!([
                {"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)},
                {"name": "supervisor", "kind": "Supervisor", "host": "claude", "configHash": "b".repeat(64)},
            ])],
        );
        let mut credentials = Vec::new();
        for (label, role, role_name, agent_id) in [
            ("dev", "Developer", "developer", "dev-1"),
            ("sup", "Supervisor", "supervisor", "sup-1"),
        ] {
            let seat = format!("seat-{label}");
            call(
                &kernel,
                &owner,
                &format!("seat-{label}"),
                "createSeat",
                &[json!({"seatId": seat, "name": seat, "role": role})],
            );
            let actor = call(
                &kernel,
                &owner,
                &format!("actor-{label}"),
                "createActor",
                &[json!({"displayName": label, "role": role, "seatId": seat})],
            );
            call(
                &kernel,
                &owner,
                &format!("agent-{label}"),
                "registerAgent",
                &[json!({
                    "agentId": agent_id, "roleName": role_name, "seatId": seat, "actorId": actor["actorId"],
                })],
            );
            credentials.push(actor["credential"].as_str().unwrap().to_string());
        }
        let raised = call(
            &kernel,
            &credentials[1],
            "raise",
            "raiseFinding",
            &[json!({
                "targetAgentId": "dev-1", "severity": "high", "evidence": "the same command fails",
                "correction": "try another approach", "doneWhen": "the command passes",
            })],
        );
        let finding_id = raised["findingId"].as_str().unwrap().to_string();
        let counter = Cell::new(0);
        let new_context = || {
            counter.set(counter.get() + 1);
            context(&kernel, &owner, &format!("sweep-{}", counter.get()))
        };
        assert!(sweep_findings(&kernel, &new_context, 0.5, None).is_err());
        assert!(sweep_findings(&kernel, &new_context, 0.0, None).is_err());
        assert_eq!(
            sweep_findings(&kernel, &new_context, 60.0, None).unwrap(),
            json!([])
        );
        skipped.set(61_000);
        let failures = std::cell::RefCell::new(Vec::new());
        let bad_context = || {
            let mut stale = new_context();
            stale.expected_version = 1;
            stale
        };
        let record = |id: &str, error: &KernelError| {
            failures.borrow_mut().push((id.to_string(), error.name()))
        };
        assert_eq!(
            sweep_findings(&kernel, &bad_context, 60.0, Some(&record)).unwrap(),
            json!([])
        );
        assert!(sweep_findings(&kernel, &bad_context, 60.0, None).is_err());
        assert_eq!(
            failures.into_inner(),
            [(finding_id.clone(), "StateVersionConflictError")]
        );
        assert_eq!(
            sweep_findings(&kernel, &new_context, 60.0, None).unwrap(),
            json!([finding_id])
        );
        assert_eq!(
            sweep_findings(&kernel, &new_context, 60.0, None).unwrap(),
            json!([])
        );
        let after = findings(&kernel, &owner, None).unwrap();
        assert_eq!(after[0]["state"], "escalated");
        assert_eq!(after[0]["stateReason"], "timed_out");
        assert_eq!(after[0]["checks"][0]["result"], "timed_out");
        assert_eq!(
            after[0]["checks"][0]["evidence"],
            "the latest correction was not acknowledged and no check was recorded within 60 seconds of its latest change (message state queued)"
        );
        assert_eq!(after[0]["deliveries"][0]["messageState"], "cancelled");
    }
}
