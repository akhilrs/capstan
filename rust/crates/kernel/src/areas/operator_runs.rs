//! The operator runs (src/controller/operator-runs.ts).

use crate::canonical::sha256;
use crate::dispatch::{arg, call, Args, RestartFilter};
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{execute, query_all, query_opt, safe_id};
use crate::kernel::Kernel;
use crate::records::{MutationEvent, MutationOutput, MAX_OPERATOR_TAIL_BYTES};
use crate::types::MutationContext;
use serde_json::{json, Value};

use super::messaging::parse_iso_ms;
use super::operator_proposals::{
    self, operator_event, operator_is_stale, operator_record, operator_row, operator_running,
    run_result_notice, set_operator_state, Limits, FULL_AUTO_RULE,
};
use super::{message_notices, pauses};

/// Abandoned runs whose process group has not been confirmed gone.
pub(crate) fn orphans(kernel: &Kernel) -> KernelResult<Vec<Value>> {
    kernel.assert_open()?;
    query_all(
        &kernel.database,
        "SELECT proposal_id, pgid, leader_start FROM operator_runs WHERE project_id = ? AND status = 'abandoned' AND pgid IS NOT NULL AND orphan_cleared_at IS NULL ORDER BY started_at",
        [&kernel.project_id],
        |row| {
            Ok(json!({
                "proposalId": row.get::<_, String>(0)?,
                "pgid": row.get::<_, i64>(1)?,
                "leaderStart": row.get::<_, Option<String>>(2)?,
            }))
        },
    )
}

/// `unclearedOperatorOrphans`.
pub fn uncleared_operator_orphans(kernel: &Kernel) -> KernelResult<Value> {
    orphans(kernel).map(Value::Array)
}

fn claim_refusal(proposal_id: &str, reason: &str) -> MutationOutput {
    MutationOutput::new(
        json!({"claimed": false, "reason": reason}),
        MutationEvent::new("operator_proposal", proposal_id, 0)
            .with_details(json!({"claimed": false, "reason": reason})),
    )
}

/// `claimOperatorRun`: the only way a proposal starts to run: approved -> running and the run row in one
/// transaction, so one approval executes at most once.
pub fn claim_operator_run(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let proposal_id = safe_id(
        input.get("proposalId").unwrap_or(&Value::Null),
        "proposal id",
    )?
    .to_string();
    let limits = Limits::of(input);
    kernel.mutate(
        context,
        "operator.claim",
        "controller:reconcile",
        input,
        |_| {
            let row = match operator_row(kernel, &proposal_id)? {
                Some(row) if row.state == "approved" => row,
                _ => return Ok(claim_refusal(&proposal_id, "not_approved")),
            };
            let now = kernel.now();
            // A one-off decision of the PM still runs while the run is paused; a run approved by an auto rule, a
            // session-grant match or full auto does not, and the refusal is recorded.
            let paused = pauses::pause_state_of(kernel)?.run;
            if let (Some(paused), Some(_)) = (&paused, &row.auto_rule) {
                let detail = format!(
                    "run_paused: running {} is refused while the run is paused: {}",
                    row.proposal_id, paused.reason
                );
                set_operator_state(kernel, &row, "cancelled", &now)?;
                message_notices::notice_to_pm(
                    kernel,
                    &format!("Operator proposal {} was approved automatically but not run: the run is paused ({}). It is cancelled; the Operator may propose it again after resume.", row.proposal_id, paused.reason),
                    &now,
                    false,
                )?;
                return Ok(MutationOutput::new(
                    json!({"claimed": false, "reason": "run_paused", "detail": detail}),
                    operator_event(
                        &row.proposal_id,
                        Some("approved"),
                        "cancelled",
                        json!({"claimed": false, "reason": "run_paused", "error": detail}),
                    ),
                ));
            }
            if operator_is_stale(&row, &limits, &now) {
                operator_proposals::expire_operator_row(kernel, &row, &now)?;
                return Ok(claim_refusal(&proposal_id, "expired"));
            }
            if operator_running(kernel)?.is_some() {
                return Ok(claim_refusal(&proposal_id, "busy"));
            }
            set_operator_state(kernel, &row, "running", &now)?;
            execute(
                &kernel.database,
                "INSERT INTO operator_runs(project_id, proposal_id, started_at, status, full_auto) VALUES (?, ?, ?, 'running', ?)",
                rusqlite::params![
                    kernel.project_id,
                    row.proposal_id,
                    now,
                    i64::from(row.auto_rule.as_deref() == Some(FULL_AUTO_RULE))
                ],
            )?;
            let after = operator_row(kernel, &row.proposal_id)?
                .ok_or_else(|| KernelError::controller("the proposal vanished"))?;
            let details = if row.auto_rule.as_deref() == Some(FULL_AUTO_RULE) {
                json!({"fullAuto": true})
            } else {
                json!({})
            };
            Ok(MutationOutput::new(
                json!({"claimed": true, "proposal": operator_record(kernel, &after)?}),
                operator_event(&row.proposal_id, Some("approved"), "running", details),
            ))
        },
    )
}

/// `recordOperatorRunProcess`: the process group of a run that started, so a later start can find a stray one.
pub fn record_operator_run_process(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let proposal_id = safe_id(
        input.get("proposalId").unwrap_or(&Value::Null),
        "proposal id",
    )?
    .to_string();
    let pgid = input.get("pgid").and_then(Value::as_f64);
    let pgid = match pgid {
        Some(value) if value.is_finite() && value.fract() == 0.0 && value > 1.0 => value as i64,
        _ => {
            return Err(KernelError::type_error(
                "the process group must be an integer above 1",
            ))
        }
    };
    let leader_start = input.get("leaderStart").and_then(Value::as_str);
    kernel.mutate(
        context,
        "operator.process",
        "controller:reconcile",
        input,
        |_| {
            let changed = execute(
                &kernel.database,
                "UPDATE operator_runs SET pgid = ?, leader_start = ? WHERE project_id = ? AND proposal_id = ? AND status = 'running' AND pgid IS NULL",
                rusqlite::params![pgid, leader_start, kernel.project_id, proposal_id],
            )?;
            if changed != 1 {
                return Err(KernelError::controller(format!(
                    "no running run of {proposal_id} can record a process"
                )));
            }
            Ok(MutationOutput::new(
                Value::Null,
                MutationEvent::new("operator_proposal", &proposal_id, 0)
                    .with_details(json!({"pgid": pgid})),
            ))
        },
    )
}

/// `Math.round`: halves go up.
fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

/// `finishOperatorRun`: ends a run with its result, tells the Operator (and the PM for an auto-approved command) and
/// moves the proposal to its final state.
pub fn finish_operator_run(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let proposal_id = safe_id(
        input.get("proposalId").unwrap_or(&Value::Null),
        "proposal id",
    )?
    .to_string();
    let output_tail = input
        .get("outputTail")
        .and_then(Value::as_str)
        .ok_or_else(|| KernelError::type_error("The \"string\" argument must be of type string"))?;
    if output_tail.len() > MAX_OPERATOR_TAIL_BYTES {
        return Err(KernelError::type_error(format!(
            "the output tail must be at most {MAX_OPERATOR_TAIL_BYTES} bytes"
        )));
    }
    let status = input
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let exit_code = input.get("exitCode").cloned().unwrap_or(Value::Null);
    let duration_ms = input
        .get("durationMs")
        .and_then(Value::as_f64)
        .unwrap_or(f64::NAN);
    let truncated = input.get("truncated").and_then(Value::as_bool) == Some(true);
    kernel.mutate(
        context,
        "operator.finish",
        "controller:reconcile",
        &json!({
            "proposalId": proposal_id,
            "status": status,
            "exitCode": exit_code,
            "durationMs": input.get("durationMs").cloned().unwrap_or(Value::Null),
            "tailSha": sha256(output_tail),
        }),
        |_| {
            let row = match operator_row(kernel, &proposal_id)? {
                Some(row) if row.state == "running" => row,
                _ => {
                    return Err(KernelError::controller(format!(
                        "proposal {proposal_id} is not running"
                    )))
                }
            };
            let now = kernel.now();
            execute(
                &kernel.database,
                "UPDATE operator_runs SET status = ?, finished_at = ?, exit_code = ?, duration_ms = ?, output_tail = ?, output_truncated = ?
                 WHERE project_id = ? AND proposal_id = ? AND status = 'running'",
                rusqlite::params![
                    status,
                    now,
                    exit_code.as_i64(),
                    js_round(duration_ms).max(0.0) as i64,
                    output_tail,
                    i64::from(truncated),
                    kernel.project_id,
                    proposal_id
                ],
            )?;
            let state = match status.as_str() {
                "ok" => "finished",
                "timeout" => "timeout",
                _ => "failed",
            };
            set_operator_state(kernel, &row, state, &now)?;
            let record = record_after(kernel, &row.proposal_id)?;
            announce_run(kernel, &record, &now)?;
            let mut details = json!({"status": status, "exitCode": exit_code});
            if row.auto_rule.as_deref() == Some(FULL_AUTO_RULE) {
                details["fullAuto"] = json!(true);
            }
            Ok(MutationOutput::new(
                record_after(kernel, &row.proposal_id)?,
                operator_event(&row.proposal_id, Some("running"), state, details),
            ))
        },
    )
}

fn record_after(kernel: &Kernel, proposal_id: &str) -> KernelResult<Value> {
    let row = operator_row(kernel, proposal_id)?
        .ok_or_else(|| KernelError::controller("the proposal vanished"))?;
    operator_record(kernel, &row)
}

/// `announceOperatorRun`: sends the result of an ended run once. The caller owns the transaction.
fn announce_run(kernel: &Kernel, record: &Value, now: &str) -> KernelResult<()> {
    if record["run"].is_null() {
        return Ok(());
    }
    let body = run_result_notice(record, &record["run"])?;
    let proposer = record["proposerAgentId"].as_str().unwrap_or("");
    let message_id = message_notices::notice_to_agent(kernel, proposer, &body, now, false)?;
    if !record["autoRule"].is_null() {
        message_notices::notice_to_pm(kernel, &body, now, false)?;
    }
    if let Some(message_id) = message_id {
        execute(
            &kernel.database,
            "UPDATE operator_runs SET notified_message_id = ? WHERE project_id = ? AND proposal_id = ? AND notified_message_id IS NULL",
            [
                &message_id,
                &kernel.project_id,
                record["proposalId"].as_str().unwrap_or(""),
            ],
        )?;
    }
    Ok(())
}

/// `abandonRunningOperatorRuns`: at startup, a run that was running when the controller stopped is abandoned and
/// never run again. Returns the abandoned runs.
pub fn abandon_running_operator_runs(
    kernel: &Kernel,
    context: &MutationContext,
    skip_restarts_with_plan: Option<RestartFilter<'_>>,
) -> KernelResult<Value> {
    kernel.mutate(
        context,
        "operator.abandon",
        "controller:reconcile",
        &json!({}),
        |_| {
            let now = kernel.now();
            let mut abandoned: Vec<Value> = Vec::new();
            let rows = query_all(
                &kernel.database,
                "SELECT * FROM operator_proposals WHERE project_id = ? AND state = 'running' ORDER BY sequence",
                [&kernel.project_id],
                |row| {
                    Ok((
                        row.get::<_, String>("proposal_id")?,
                        row.get::<_, String>("kind")?,
                    ))
                },
            )?;
            for (proposal_id, kind) in rows {
                if kind == "restart" && skip_restarts_with_plan.is_some_and(|skip| skip(&proposal_id)) {
                    continue;
                }
                let run = query_opt(
                    &kernel.database,
                    "SELECT pgid, leader_start, started_at FROM operator_runs WHERE project_id = ? AND proposal_id = ?",
                    [&kernel.project_id, &proposal_id],
                    |row| {
                        Ok((
                            row.get::<_, Option<i64>>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )?;
                let started = run.as_ref().map_or(now.as_str(), |r| r.2.as_str());
                execute(
                    &kernel.database,
                    "UPDATE operator_runs SET status = 'abandoned', finished_at = ?, duration_ms = ?
                     WHERE project_id = ? AND proposal_id = ? AND status = 'running'",
                    rusqlite::params![
                        now,
                        (parse_iso_ms(&now) - parse_iso_ms(started)).max(0),
                        kernel.project_id,
                        proposal_id
                    ],
                )?;
                let row = operator_row(kernel, &proposal_id)?
                    .ok_or_else(|| KernelError::controller("the proposal vanished"))?;
                set_operator_state(kernel, &row, "abandoned", &now)?;
                announce_run(kernel, &record_after(kernel, &proposal_id)?, &now)?;
                abandoned.push(json!({
                    "proposalId": proposal_id,
                    "pgid": run.as_ref().and_then(|r| r.0),
                    "leaderStart": run.as_ref().and_then(|r| r.1.clone()),
                }));
            }
            let ids: Vec<Value> = abandoned.iter().map(|a| a["proposalId"].clone()).collect();
            Ok(MutationOutput::new(
                Value::Array(abandoned.clone()),
                MutationEvent::new(
                    "operator_proposal",
                    abandoned
                        .first()
                        .and_then(|a| a["proposalId"].as_str())
                        .unwrap_or("none"),
                    0,
                )
                .with_details(json!({"abandoned": ids})),
            ))
        },
    )
}

/// `clearOperatorOrphan`: records that the process group of an abandoned run is gone (or was never ours to signal).
pub fn clear_operator_orphan(
    kernel: &Kernel,
    context: &MutationContext,
    proposal_id: &str,
) -> KernelResult<Value> {
    safe_id(&json!(proposal_id), "proposal id")?;
    kernel.mutate(
        context,
        "operator.orphan_clear",
        "controller:reconcile",
        &json!({"proposalId": proposal_id}),
        |_| {
            execute(
                &kernel.database,
                "UPDATE operator_runs SET orphan_cleared_at = ? WHERE project_id = ? AND proposal_id = ? AND status = 'abandoned' AND orphan_cleared_at IS NULL",
                [&kernel.now(), &kernel.project_id, proposal_id],
            )?;
            Ok(MutationOutput::new(
                Value::Null,
                MutationEvent::new("operator_proposal", proposal_id, 0)
                    .with_details(json!({"orphanCleared": true})),
            ))
        },
    )
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "unclearedOperatorOrphans" => Some(call(uncleared_operator_orphans(kernel))),
        "claimOperatorRun" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(claim_operator_run(kernel, &context, input)))
        }
        "recordOperatorRunProcess" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(record_operator_run_process(kernel, &context, input)))
        }
        "finishOperatorRun" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(finish_operator_run(kernel, &context, input)))
        }
        "abandonRunningOperatorRuns" => {
            let context = arg!(a.ctx(0));
            Some(call(abandon_running_operator_runs(kernel, &context, None)))
        }
        "clearOperatorOrphan" => {
            let context = arg!(a.ctx(0));
            let proposal_id = arg!(a.str(1, "proposalId"));
            Some(call(clear_operator_orphan(kernel, &context, proposal_id)))
        }
        _ => None,
    }
}
