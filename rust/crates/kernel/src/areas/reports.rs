//! The worker reports (src/controller/reports.ts).

use crate::canonical::sha256;
use crate::dispatch::{arg, call, Args};
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{
    execute, js_trim, query_all, query_opt, report_notice, report_record, safe_id_str,
};
use crate::json::stringify;
use crate::kernel::Kernel;
use crate::records::{
    AgentReportRow, AgentRow, MutationEvent, MutationOutput, MAX_REJECTED_REPORTS,
    MAX_REPORT_SUMMARY_BYTES,
};
use crate::types::MutationContext;
use regex::Regex;
use serde_json::{json, Value};
use std::sync::LazyLock;

use super::messages;

static SHA1: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9a-f]{40}$").expect("the pattern is valid"));

/// `UNSAFE_TEXT` of src/controller/records.ts.
static UNSAFE_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[\p{Cc}\p{Cf}\p{Zl}\p{Zp}\p{Noncharacter_Code_Point}]")
        .expect("the pattern is valid")
});

fn field<'a>(input: &'a Value, key: &str) -> &'a Value {
    input.get(key).unwrap_or(&Value::Null)
}

/// `Number.isInteger`.
fn is_integer(value: &Value) -> bool {
    value.as_i64().is_some() || value.as_f64().is_some_and(|f| f.fract() == 0.0)
}

/// `value === null || /^[0-9a-f]{40}$/.test(value)`, as the evidence check reads a sha field.
fn sha_or_null(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Null) => true,
        Some(Value::String(text)) => SHA1.is_match(text),
        _ => false,
    }
}

fn report_by_id(kernel: &Kernel, report_id: &str) -> KernelResult<Option<AgentReportRow>> {
    query_opt(
        &kernel.database,
        "SELECT * FROM agent_reports WHERE project_id = ? AND report_id = ?",
        [&kernel.project_id, report_id],
        AgentReportRow::from_row,
    )
}

/// `recordAgentReport`.
pub fn record_agent_report(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let commit_sha = match field(input, "commitSha").as_str() {
        Some(sha) if SHA1.is_match(sha) => sha,
        _ => {
            return Err(KernelError::type_error(
                "commit sha must be 40 lowercase hex characters",
            ))
        }
    };
    let summary = match field(input, "summary").as_str() {
        Some(text)
            if !js_trim(text).is_empty()
                && !UNSAFE_TEXT.is_match(text)
                && text.len() <= MAX_REPORT_SUMMARY_BYTES =>
        {
            text
        }
        _ => {
            return Err(KernelError::type_error(format!(
                "report summary must be one line of printable text of at most {MAX_REPORT_SUMMARY_BYTES} bytes"
            )))
        }
    };
    let evidence = field(input, "evidence");
    match input.get("evidence") {
        None => {
            return Err(KernelError::type_error(
                "Cannot read properties of undefined (reading 'generation')",
            ))
        }
        Some(Value::Null) => {
            return Err(KernelError::type_error(
                "Cannot read properties of null (reading 'generation')",
            ))
        }
        Some(_) => {}
    }
    let well_formed = is_integer(field(evidence, "generation"))
        && field(evidence, "branch").is_string()
        && field(evidence, "commitExists").is_boolean()
        && field(evidence, "isAncestorOfTip").is_boolean()
        && field(evidence, "isAncestorOfBase").is_boolean()
        && field(evidence, "checkedAt").is_string()
        && sha_or_null(evidence.get("baseSha"))
        && sha_or_null(evidence.get("branchTip"));
    if !well_formed {
        return Err(KernelError::type_error(
            "report evidence is not well formed",
        ));
    }
    let evidence_generation = field(evidence, "generation").as_i64().unwrap_or(0);
    kernel.mutate(
        context,
        "report.record",
        "report:submit",
        &json!({"commitSha": commit_sha, "summaryHash": sha256(summary), "evidence": evidence}),
        |actor| {
            let Some(agent) = kernel.agent_by_actor(&actor.actor_id)? else {
                return Err(KernelError::controller("only an active agent can report"));
            };
            let existing = query_opt(
                &kernel.database,
                "SELECT * FROM agent_reports WHERE project_id = ? AND agent_id = ? AND generation = ? AND commit_sha = ? AND state = 'accepted'",
                rusqlite::params![kernel.project_id, agent.agent_id, agent.generation, commit_sha],
                AgentReportRow::from_row,
            )?;
            if let Some(existing) = existing {
                return Ok(MutationOutput::new(
                    json!({"record": report_record(&existing)?, "duplicate": true}),
                    MutationEvent::new("agent_report", &existing.report_id, 0)
                        .with_details(json!({"duplicate": true})),
                ));
            }
            let pane = query_opt(
                &kernel.database,
                "SELECT branch, base_sha FROM agent_panes WHERE project_id = ? AND agent_id = ?",
                [&kernel.project_id, &agent.agent_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                    ))
                },
            )?;
            let pane_branch = pane.as_ref().and_then(|(branch, _)| branch.clone());
            let base_sha = pane
                .as_ref()
                .and_then(|(_, base)| base.as_ref())
                .map(|base| base.to_lowercase());
            let evidence_branch = field(evidence, "branch").as_str();
            let evidence_base = field(evidence, "baseSha").as_str();
            let reason: Option<&str> = match (&pane_branch, &base_sha) {
                (None, _) => Some("no_branch"),
                (_, None) => Some("no_base"),
                (Some(branch), Some(base)) => {
                    if evidence_generation != agent.generation
                        || evidence_branch != Some(branch.as_str())
                        || evidence_base != Some(base.as_str())
                    {
                        Some("agent_changed")
                    } else if !field(evidence, "commitExists").as_bool().unwrap_or(false) {
                        Some("commit_missing")
                    } else if commit_sha == base
                        || field(evidence, "isAncestorOfBase")
                            .as_bool()
                            .unwrap_or(false)
                    {
                        Some("not_new_on_branch")
                    } else if !field(evidence, "isAncestorOfTip").as_bool().unwrap_or(false) {
                        Some("not_on_branch")
                    } else {
                        None
                    }
                }
            };
            if reason.is_some() {
                let rejected = query_opt(
                    &kernel.database,
                    "SELECT COUNT(*) AS n FROM agent_reports WHERE project_id = ? AND agent_id = ? AND generation = ? AND state = 'rejected'",
                    rusqlite::params![kernel.project_id, agent.agent_id, agent.generation],
                    |row| row.get::<_, i64>(0),
                )?
                .unwrap_or(0);
                // Only a report that would be rejected is refused at the cap: a correct report always gets through.
                if rejected >= MAX_REJECTED_REPORTS as i64 {
                    return Err(KernelError::controller(
                        "report limit reached for this agent generation",
                    ));
                }
            }
            let now = kernel.now();
            let report_id = kernel.env.uuid();
            let sequence = query_opt(
                &kernel.database,
                "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM agent_reports WHERE project_id = ?",
                [&kernel.project_id],
                |row| row.get::<_, i64>(0),
            )?
            .unwrap_or(1);
            execute(
                &kernel.database,
                "INSERT INTO agent_reports(project_id, report_id, sequence, agent_id, generation, actor_id, commit_sha, branch, summary, state, reason, evidence_json, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    kernel.project_id,
                    report_id,
                    sequence,
                    agent.agent_id,
                    agent.generation,
                    actor.actor_id,
                    commit_sha,
                    pane_branch,
                    summary,
                    if reason.is_none() { "accepted" } else { "rejected" },
                    reason,
                    stringify(evidence),
                    now
                ],
            )?;
            kernel.touch_agent(&agent.agent_id, &now)?;
            if reason.is_none() {
                announce(kernel, &report_id, &agent.role_name, &now)?;
            }
            let row = report_by_id(kernel, &report_id)?
                .ok_or_else(|| KernelError::controller("the report row is missing"))?;
            let mut event = MutationEvent::new("agent_report", &report_id, 0).with_details(
                json!({"agentId": agent.agent_id, "generation": agent.generation, "reason": reason}),
            );
            event.to_state = Some(if reason.is_none() { "accepted" } else { "rejected" }.into());
            Ok(MutationOutput::new(
                json!({"record": report_record(&row)?, "duplicate": false}),
                event,
            ))
        },
    )
}

struct PackageRoute {
    architect: AgentRow,
    plan_id: String,
    package_id: String,
}

/// The package a worker's reports belong to (its latest assignment in an approved plan) and that plan's architect while the
/// architect is active; `None` when there is none, so the notice falls back to the PM.
fn package_route(kernel: &Kernel, agent_id: &str) -> KernelResult<Option<PackageRoute>> {
    let row = query_opt(
        &kernel.database,
        "SELECT p.plan_id, p.package_id, pl.architect_agent_id FROM plan_packages p
         JOIN plans pl ON pl.project_id = p.project_id AND pl.plan_id = p.plan_id
         WHERE p.project_id = ? AND p.assignee_agent_id = ? AND pl.state = 'approved' AND pl.architect_agent_id IS NOT NULL
         ORDER BY p.assigned_at DESC, pl.sequence DESC LIMIT 1",
        [&kernel.project_id, agent_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    )?;
    let Some((plan_id, package_id, architect_id)) = row else {
        return Ok(None);
    };
    Ok(kernel
        .agent_row(&architect_id)?
        .filter(|architect| architect.state == "active")
        .map(|architect| PackageRoute {
            architect,
            plan_id,
            package_id,
        }))
}

/// Queues the notice for an accepted report that has none yet, to the plan's architect for a package report and to the PM
/// otherwise; false when no recipient is active or there is no controller actor. The caller owns the transaction.
fn announce(kernel: &Kernel, report_id: &str, role_name: &str, now: &str) -> KernelResult<bool> {
    let row = query_opt(
        &kernel.database,
        "SELECT * FROM agent_reports WHERE project_id = ? AND report_id = ? AND state = 'accepted' AND notified_message_id IS NULL",
        [&kernel.project_id, report_id],
        AgentReportRow::from_row,
    )?;
    let Some(row) = row else {
        return Ok(false);
    };
    let controller = query_opt(
        &kernel.database,
        "SELECT actor_id FROM actors WHERE project_id = ? AND is_internal = 1 AND role = 'controller' AND active = 1 AND revoked_at IS NULL",
        [&kernel.project_id],
        |row| row.get::<_, String>(0),
    )?;
    let Some(controller) = controller else {
        return Ok(false);
    };
    let route = package_route(kernel, &row.agent_id)?;
    let recipient = match &route {
        Some(route) => route.architect.clone(),
        None => {
            let mut pms = query_all(
                &kernel.database,
                "SELECT * FROM agents WHERE project_id = ? AND kind = 'PM' AND state = 'active'",
                [&kernel.project_id],
                AgentRow::from_row,
            )?;
            if pms.len() != 1 {
                return Ok(false);
            }
            pms.remove(0)
        }
    };
    let body = match &route {
        Some(route) => format!(
            "{}\nWork package: {}/{}",
            report_notice(&row, role_name),
            route.plan_id,
            route.package_id
        ),
        None => report_notice(&row, role_name),
    };
    let message_id = messages::insert_queued_message(
        kernel,
        &controller,
        &recipient,
        &body,
        &sha256(&body),
        now,
        false,
    )?;
    execute(
        &kernel.database,
        "UPDATE agent_reports SET notified_message_id = ? WHERE project_id = ? AND report_id = ? AND notified_message_id IS NULL",
        [&message_id, &kernel.project_id, report_id],
    )?;
    Ok(true)
}

fn records(rows: &[AgentReportRow]) -> KernelResult<Value> {
    Ok(Value::Array(
        rows.iter()
            .map(|row| Ok(serde_json::to_value(report_record(row)?)?))
            .collect::<KernelResult<Vec<_>>>()?,
    ))
}

/// `acceptedReportFor`: the accepted report of this exact commit by this agent generation, or null.
pub fn accepted_report_for(
    kernel: &Kernel,
    agent_id: &str,
    generation: i64,
    commit_sha: &str,
) -> KernelResult<Value> {
    kernel.assert_open()?;
    let row = query_opt(
        &kernel.database,
        "SELECT * FROM agent_reports WHERE project_id = ? AND agent_id = ? AND generation = ? AND commit_sha = ? AND state = 'accepted'",
        rusqlite::params![kernel.project_id, agent_id, generation, commit_sha],
        AgentReportRow::from_row,
    )?;
    match row {
        Some(row) => Ok(serde_json::to_value(report_record(&row)?)?),
        None => Ok(Value::Null),
    }
}

/// `unannouncedReports`.
pub fn unannounced_reports(
    kernel: &Kernel,
    credential: &str,
    limit: Option<i64>,
) -> KernelResult<Value> {
    kernel.authorize(credential, "controller:reconcile")?;
    let rows = query_all(
        &kernel.database,
        "SELECT * FROM agent_reports WHERE project_id = ? AND state = 'accepted' AND notified_message_id IS NULL ORDER BY sequence LIMIT ?",
        rusqlite::params![kernel.project_id, limit.unwrap_or(50)],
        AgentReportRow::from_row,
    )?;
    records(&rows)
}

/// `announceReport`.
pub fn announce_report(
    kernel: &Kernel,
    context: &MutationContext,
    report_id: &str,
) -> KernelResult<Value> {
    safe_id_str(report_id, "report id")?;
    kernel.mutate(
        context,
        "report.announce",
        "controller:reconcile",
        &json!({"reportId": report_id}),
        |_| {
            let role = query_opt(
                &kernel.database,
                "SELECT a.role_name AS role_name FROM agent_reports r JOIN agents a ON a.project_id = r.project_id AND a.agent_id = r.agent_id WHERE r.project_id = ? AND r.report_id = ?",
                [&kernel.project_id, report_id],
                |row| row.get::<_, String>(0),
            )?;
            let announced = match role {
                Some(role) => announce(kernel, report_id, &role, &kernel.now())?,
                None => false,
            };
            Ok(MutationOutput::new(
                json!({"announced": announced}),
                MutationEvent::new("agent_report", report_id, 0)
                    .with_details(json!({"announced": announced})),
            ))
        },
    )
}

/// `agentReports`.
pub fn agent_reports(kernel: &Kernel, credential: &str, limit: Option<i64>) -> KernelResult<Value> {
    kernel.authorize(credential, "controller:reconcile")?;
    let rows = query_all(
        &kernel.database,
        "SELECT * FROM agent_reports WHERE project_id = ? ORDER BY sequence DESC LIMIT ?",
        rusqlite::params![kernel.project_id, limit.unwrap_or(20)],
        AgentReportRow::from_row,
    )?;
    records(&rows)
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "recordAgentReport" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(record_agent_report(kernel, &context, input)))
        }
        "acceptedReportFor" => {
            let agent_id = arg!(a.str(0, "agentId"));
            let generation = arg!(a.i64(1, "generation"));
            let commit_sha = arg!(a.str(2, "commitSha"));
            Some(call(accepted_report_for(
                kernel, agent_id, generation, commit_sha,
            )))
        }
        "unannouncedReports" => {
            let credential = arg!(a.str(0, "credential"));
            let limit = arg!(a.opt_i64(1, "limit"));
            Some(call(unannounced_reports(kernel, credential, limit)))
        }
        "announceReport" => {
            let context = arg!(a.ctx(0));
            let report_id = arg!(a.str(1, "reportId"));
            Some(call(announce_report(kernel, &context, report_id)))
        }
        "agentReports" => {
            let credential = arg!(a.str(0, "credential"));
            let limit = arg!(a.opt_i64(1, "limit"));
            Some(call(agent_reports(kernel, credential, limit)))
        }
        _ => None,
    }
}
