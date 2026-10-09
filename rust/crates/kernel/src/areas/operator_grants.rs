//! The operator grants (src/controller/operator-grants.ts).

use crate::dispatch::{arg, call, Args};
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{execute, query_all, query_opt, safe_id, safe_text};
use crate::kernel::Kernel;
use crate::records::{AgentRow, MutationEvent, MutationOutput};
use crate::types::MutationContext;
use serde_json::{json, Value};

use super::message_notices;
use super::messaging::parse_iso_ms;
use super::operator_proposals::{
    self, ended_without_run_notice, full_auto_notice, grant_ended_notice, FULL_AUTO_RULE,
};
use rusqlite::Row;

struct GrantRow {
    grant_id: String,
    sequence: i64,
    kind: String,
    text: String,
    command_sha: String,
    created_by: String,
    source_proposal_id: String,
    created_at: String,
    expires_at: String,
    revoked_at: Option<String>,
    ended_reason: Option<String>,
}

impl GrantRow {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            grant_id: row.get("grant_id")?,
            sequence: row.get("sequence")?,
            kind: row.get("kind")?,
            text: row.get("text")?,
            command_sha: row.get("command_sha")?,
            created_by: row.get("created_by")?,
            source_proposal_id: row.get("source_proposal_id")?,
            created_at: row.get("created_at")?,
            expires_at: row.get("expires_at")?,
            revoked_at: row.get("revoked_at")?,
            ended_reason: row.get("ended_reason")?,
        })
    }

    fn record(&self) -> Value {
        json!({
            "grantId": self.grant_id,
            "sequence": self.sequence,
            "kind": self.kind,
            "text": self.text,
            "commandSha": self.command_sha,
            "createdBy": self.created_by,
            "sourceProposalId": self.source_proposal_id,
            "createdAt": self.created_at,
            "expiresAt": self.expires_at,
            "revokedAt": self.revoked_at,
            "endedReason": self.ended_reason,
        })
    }
}

fn grant_row(kernel: &Kernel, grant_id: &str) -> KernelResult<Option<GrantRow>> {
    query_opt(
        &kernel.database,
        "SELECT * FROM operator_grants WHERE project_id = ? AND grant_id = ?",
        [&kernel.project_id, grant_id],
        GrantRow::from_row,
    )
}

fn open_grant_rows(kernel: &Kernel) -> KernelResult<Vec<GrantRow>> {
    query_all(
        &kernel.database,
        "SELECT * FROM operator_grants WHERE project_id = ? AND ended_reason IS NULL ORDER BY sequence",
        [&kernel.project_id],
        GrantRow::from_row,
    )
}

/// `grantOfProposal`: the session grant a proposal's approval created, or null.
pub(crate) fn grant_of_proposal(kernel: &Kernel, proposal_id: &str) -> KernelResult<Value> {
    Ok(query_opt(
        &kernel.database,
        "SELECT * FROM operator_grants WHERE project_id = ? AND source_proposal_id = ?",
        [&kernel.project_id, proposal_id],
        GrantRow::from_row,
    )?
    .map_or(Value::Null, |row| row.record()))
}

/// The record of one grant, if it exists.
pub(crate) fn grant_record_of(kernel: &Kernel, grant_id: &str) -> KernelResult<Option<Value>> {
    Ok(grant_row(kernel, grant_id)?.map(|row| row.record()))
}

/// `activeOperatorGrants`, as a list: grants that have not ended and have not passed their time cap.
pub(crate) fn active_grant_records(kernel: &Kernel) -> KernelResult<Vec<Value>> {
    kernel.assert_open()?;
    let now = kernel.now();
    Ok(open_grant_rows(kernel)?
        .into_iter()
        .filter(|row| parse_iso_ms(&row.expires_at) > parse_iso_ms(&now))
        .map(|row| row.record())
        .collect())
}

/// `activeOperatorGrants`.
pub fn active_operator_grants(kernel: &Kernel) -> KernelResult<Value> {
    active_grant_records(kernel).map(Value::Array)
}

/// `listOperatorGrants`: newest first, ended ones included.
pub fn list_operator_grants(kernel: &Kernel, limit: Option<i64>) -> KernelResult<Value> {
    kernel.assert_open()?;
    let rows = query_all(
        &kernel.database,
        "SELECT * FROM operator_grants WHERE project_id = ? ORDER BY sequence DESC LIMIT ?",
        rusqlite::params![kernel.project_id, limit.unwrap_or(50)],
        GrantRow::from_row,
    )?;
    Ok(Value::Array(rows.iter().map(GrantRow::record).collect()))
}

/// `endOperatorGrantsOf`: the session grants approved for an agent's proposals end when the agent is released or
/// replaced. The caller owns the transaction.
pub(crate) fn end_operator_grants_of(kernel: &Kernel, agent_id: &str) -> KernelResult<()> {
    execute(
        &kernel.database,
        "UPDATE operator_grants SET ended_reason = 'released'
         WHERE project_id = ? AND ended_reason IS NULL AND source_proposal_id IN
           (SELECT proposal_id FROM operator_proposals WHERE project_id = ? AND proposer_agent_id = ?)",
        [&kernel.project_id, &kernel.project_id, agent_id],
    )?;
    Ok(())
}

fn due_ids(kernel: &Kernel, also_due: Option<&[String]>) -> KernelResult<Vec<String>> {
    kernel.assert_open()?;
    let now = parse_iso_ms(&kernel.now());
    Ok(open_grant_rows(kernel)?
        .into_iter()
        .filter(|row| {
            parse_iso_ms(&row.expires_at) <= now
                || also_due.is_some_and(|also| also.contains(&row.grant_id))
        })
        .map(|row| row.grant_id)
        .collect())
}

/// `dueOperatorGrantExpiries`: open grants whose time cap has passed. Read-only.
pub fn due_operator_grant_expiries(
    kernel: &Kernel,
    also_due: Option<&[String]>,
) -> KernelResult<Value> {
    Ok(json!(due_ids(kernel, also_due)?))
}

fn end_grant_row(
    kernel: &Kernel,
    row: &GrantRow,
    reason: &str,
    now: &str,
) -> KernelResult<MutationEvent> {
    execute(
        &kernel.database,
        "UPDATE operator_grants SET ended_reason = ?, revoked_at = ? WHERE project_id = ? AND grant_id = ? AND ended_reason IS NULL",
        rusqlite::params![
            reason,
            if reason == "revoked" { Some(now) } else { None },
            kernel.project_id,
            row.grant_id
        ],
    )?;
    Ok(MutationEvent {
        from_state: Some("active".into()),
        to_state: Some(
            if reason == "revoked" {
                "revoked"
            } else {
                "ended"
            }
            .into(),
        ),
        details: Some(json!({
            "event": match reason {
                "revoked" => "operator.grant_revoked",
                "expired" => "operator.grant_expired",
                _ => "operator.grant_ended",
            },
            "reason": reason,
        })),
        ..MutationEvent::new("operator_grant", &row.grant_id, 0)
    })
}

fn notice_grant_ended(
    kernel: &Kernel,
    row: &GrantRow,
    reason: &str,
    now: &str,
) -> KernelResult<()> {
    let Some(source) = operator_proposals::operator_row(kernel, &row.source_proposal_id)? else {
        return Ok(());
    };
    message_notices::notice_to_agent(
        kernel,
        &source.proposer_agent_id,
        &grant_ended_notice(&row.record(), reason)?,
        now,
        false,
    )?;
    Ok(())
}

/// Events of several grants as one mutation output: the first is the event, the rest are extra.
fn many_events(
    value: usize,
    mut events: Vec<MutationEvent>,
    none: MutationEvent,
) -> MutationOutput {
    let first = if events.is_empty() {
        none
    } else {
        events.remove(0)
    };
    let mut output = MutationOutput::new(json!(value), first);
    output.extra_events = events;
    output
}

/// `expireOperatorGrants`: ends every grant that is past its cap; the operator tick calls it.
pub fn expire_operator_grants(
    kernel: &Kernel,
    context: &MutationContext,
    also_due: Option<&[String]>,
) -> KernelResult<Value> {
    kernel.mutate(
        context,
        "operator.grant_expire",
        "controller:reconcile",
        &json!({}),
        |_| {
            let now = kernel.now();
            let due = due_ids(kernel, also_due)?;
            let mut events = Vec::new();
            for row in open_grant_rows(kernel)? {
                if !due.contains(&row.grant_id) {
                    continue;
                }
                events.push(end_grant_row(kernel, &row, "expired", &now)?);
                notice_grant_ended(kernel, &row, "expired", &now)?;
            }
            let count = events.len();
            Ok(many_events(
                count,
                events,
                MutationEvent::new("operator_grant", "none", 0).with_details(json!({"expired": 0})),
            ))
        },
    )
}

/// `endOperatorGrantsForRestart`: at startup no grant survives.
pub fn end_operator_grants_for_restart(
    kernel: &Kernel,
    context: &MutationContext,
) -> KernelResult<Value> {
    kernel.mutate(
        context,
        "operator.grant_restart",
        "controller:reconcile",
        &json!({}),
        |_| {
            let now = kernel.now();
            let mut events = Vec::new();
            for row in open_grant_rows(kernel)? {
                events.push(end_grant_row(kernel, &row, "restart", &now)?);
            }
            let count = events.len();
            Ok(many_events(
                count,
                events,
                MutationEvent::new("operator_grant", "none", 0)
                    .with_details(json!({"ended": 0, "reason": "restart"})),
            ))
        },
    )
}

/// `revokeOperatorGrant`: the PM or the operator CLI ends one grant at once.
pub fn revoke_operator_grant(
    kernel: &Kernel,
    context: &MutationContext,
    grant_id: &str,
) -> KernelResult<Value> {
    safe_id(&json!(grant_id), "grant id")?;
    kernel.mutate(
        context,
        "operator.grant_revoke",
        "operator:decide",
        &json!({"grantId": grant_id}),
        |actor| {
            let agent = kernel.agent_by_actor(&actor.actor_id)?;
            if actor.role != "operator" && !matches!(&agent, Some(a) if a.kind == "PM") {
                return Err(KernelError::controller(
                    "only the PM or the operator revokes a grant",
                ));
            }
            let Some(row) = grant_row(kernel, grant_id)? else {
                return Err(KernelError::controller(format!(
                    "unknown_grant: grant {grant_id} does not exist"
                )));
            };
            if row.ended_reason.is_some()
                || parse_iso_ms(&row.expires_at) <= parse_iso_ms(&kernel.now())
            {
                return Err(KernelError::controller(format!(
                    "grant_not_active: grant {grant_id} has already ended"
                )));
            }
            let now = kernel.now();
            let event = end_grant_row(kernel, &row, "revoked", &now)?;
            notice_grant_ended(kernel, &row, "revoked", &now)?;
            let after = grant_row(kernel, grant_id)?
                .ok_or_else(|| KernelError::controller("the grant vanished"))?;
            Ok(MutationOutput::new(after.record(), event))
        },
    )
}

/// `recordOperatorFullAuto`: audits a full-auto change and tells the PM and the Operator agent.
pub fn record_operator_full_auto(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let change = input
        .get("change")
        .and_then(Value::as_str)
        .unwrap_or("undefined")
        .to_string();
    let by_controller = change == "expired" || change == "startup";
    let asked_user: Option<String> = match input.get("askedUser") {
        None => None,
        Some(value) => Some(safe_text(value, "asked-user text", 1024, false)?),
    };
    let minutes = input.get("minutes").cloned().unwrap_or(Value::Null);
    let operator_role = input
        .get("operatorRole")
        .and_then(Value::as_str)
        .unwrap_or("");
    kernel.mutate(
        context,
        &format!("operator.full_auto_{change}"),
        if by_controller { "controller:reconcile" } else { "operator:decide" },
        &json!({"change": change, "minutes": minutes, "askedUser": asked_user}),
        |actor| {
            let agent = kernel.agent_by_actor(&actor.actor_id)?;
            let is_pm = matches!(&agent, Some(a) if a.kind == "PM");
            if change == "on" {
                if !is_pm {
                    return Err(KernelError::controller(
                        "only an active PM switches full auto on, and only after asking the user",
                    ));
                }
                if asked_user.is_none() {
                    return Err(KernelError::controller(
                        "asked_user_missing: full auto on needs the text of what the user said",
                    ));
                }
            } else if change == "off" && actor.role != "operator" && !is_pm {
                return Err(KernelError::controller(
                    "only the PM or the operator switches full auto off",
                ));
            }
            let now = kernel.now();
            if change != "startup" {
                let body = full_auto_notice(&change, &minutes)?;
                message_notices::notice_to_pm(kernel, &body, &now, false)?;
                let operators = query_all(
                    &kernel.database,
                    "SELECT * FROM agents WHERE project_id = ? AND role_name = ? AND state = 'active'",
                    [&kernel.project_id, operator_role],
                    AgentRow::from_row,
                )?;
                for operator in operators {
                    message_notices::notice_to_agent(kernel, &operator.agent_id, &body, &now, false)?;
                }
            }
            let mut details = serde_json::Map::new();
            details.insert(
                "event".into(),
                json!(if change == "startup" {
                    "operator.full_auto_off".to_string()
                } else {
                    format!("operator.full_auto_{change}")
                }),
            );
            if change == "startup" {
                details.insert("reason".into(), json!("startup"));
            }
            details.insert("minutes".into(), minutes.clone());
            details.insert("askedUser".into(), json!(asked_user));
            details.insert("by".into(), json!(actor.actor_id));
            Ok(MutationOutput::new(
                Value::Null,
                MutationEvent {
                    to_state: Some(change.clone()),
                    details: Some(Value::Object(details)),
                    ..MutationEvent::new("operator_full_auto", "full-auto", 0)
                },
            ))
        },
    )
}

/// `endFullAutoProposal`: an approved proposal that has not started is ended because full auto went off before it began.
pub fn end_full_auto_proposal(
    kernel: &Kernel,
    context: &MutationContext,
    proposal_id: &str,
) -> KernelResult<Value> {
    safe_id(&json!(proposal_id), "proposal id")?;
    kernel.mutate(
        context,
        "operator.full_auto_refuse",
        "controller:reconcile",
        &json!({"proposalId": proposal_id}),
        |_| {
            let row = match operator_proposals::operator_row(kernel, proposal_id)? {
                Some(row) if row.state == "approved" && row.auto_rule.as_deref() == Some(FULL_AUTO_RULE) => row,
                _ => {
                    return Err(KernelError::controller(format!(
                        "proposal {proposal_id} is not waiting under full auto"
                    )))
                }
            };
            let now = kernel.now();
            operator_proposals::set_operator_state(kernel, &row, "cancelled", &now)?;
            let after = operator_proposals::operator_row(kernel, proposal_id)?
                .ok_or_else(|| KernelError::controller("the proposal vanished"))?;
            let record = operator_proposals::operator_record(kernel, &after)?;
            let body = ended_without_run_notice(
                &record,
                "was not run: full auto ended before it started. Propose it again if it is still needed.",
            )?;
            message_notices::notice_to_agent(kernel, &row.proposer_agent_id, &body, &now, false)?;
            message_notices::notice_to_pm(kernel, &body, &now, false)?;
            Ok(MutationOutput::new(
                record,
                operator_proposals::operator_event(
                    proposal_id,
                    Some("approved"),
                    "cancelled",
                    json!({"reason": "full_auto_ended"}),
                ),
            ))
        },
    )
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "activeOperatorGrants" => Some(call(active_operator_grants(kernel))),
        "listOperatorGrants" => {
            let limit = arg!(a.opt_i64(0, "limit"));
            Some(call(list_operator_grants(kernel, limit)))
        }
        "dueOperatorGrantExpiries" => {
            let also = arg!(a.opt_str_list(0));
            Some(call(due_operator_grant_expiries(kernel, also.as_deref())))
        }
        "expireOperatorGrants" => {
            let context = arg!(a.ctx(0));
            let also = arg!(a.opt_str_list(1));
            Some(call(expire_operator_grants(
                kernel,
                &context,
                also.as_deref(),
            )))
        }
        "endOperatorGrantsForRestart" => {
            let context = arg!(a.ctx(0));
            Some(call(end_operator_grants_for_restart(kernel, &context)))
        }
        "revokeOperatorGrant" => {
            let context = arg!(a.ctx(0));
            let grant_id = arg!(a.str(1, "grantId"));
            Some(call(revoke_operator_grant(kernel, &context, grant_id)))
        }
        "recordOperatorFullAuto" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(record_operator_full_auto(kernel, &context, input)))
        }
        "endFullAutoProposal" => {
            let context = arg!(a.ctx(0));
            let proposal_id = arg!(a.str(1, "proposalId"));
            Some(call(end_full_auto_proposal(kernel, &context, proposal_id)))
        }
        _ => None,
    }
}
