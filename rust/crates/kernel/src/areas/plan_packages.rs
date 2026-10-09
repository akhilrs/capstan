//! The plan packages (src/controller/plan-packages.ts).

use crate::canonical::sha256;
use crate::dispatch::{arg, call, Args};
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{
    execute, plan_cancelled_notice, plan_signed_off_notice, query_all, query_opt, safe_id,
    safe_id_str, safe_text,
};
use crate::kernel::Kernel;
use crate::plan_body::{package_of_body, work_package_message};
use crate::records::{MutationEvent, MutationOutput, MAX_MESSAGE_BYTES};
use crate::types::MutationContext;
use serde_json::{json, Map, Value};

use super::{integrations, message_notices, messages, pauses, plans};

/// True for the report `r` (agent_reports) that counts for package `pp` (plan_packages): the assignee's accepted report of
/// its current generation made since the assignment, unless a later-assigned package of the same agent claims it.
const PACKAGE_REPORT_PREDICATE: &str = "pp.assignee_agent_id = r.agent_id AND r.state = 'accepted' AND pp.assigned_at <= r.created_at
  AND EXISTS (SELECT 1 FROM agents a WHERE a.project_id = r.project_id AND a.agent_id = r.agent_id AND a.generation = r.generation)
  AND NOT EXISTS (
    SELECT 1 FROM plan_packages other
    WHERE other.project_id = r.project_id AND other.assignee_agent_id = r.agent_id AND other.assigned_at <= r.created_at
      AND (other.assigned_at, other.plan_id, other.package_id) > (pp.assigned_at, pp.plan_id, pp.package_id))";

fn field<'a>(input: &'a Value, key: &str) -> &'a Value {
    input.get(key).unwrap_or(&Value::Null)
}

/// The columns of `plan_packages` the derived state reads.
#[derive(Clone, Debug)]
pub(crate) struct PackageRow {
    pub package_id: String,
    pub assignee_agent_id: Option<String>,
    pub assigned_at: Option<String>,
    pub cancelled_at: Option<String>,
}

/// `PackageFacts` of src/nexora.ts: the derived progress plus whether the package was cancelled or confirmed.
pub(crate) struct PackageFacts {
    pub progress: &'static str,
    pub cancelled: bool,
    pub confirmed: bool,
}

/// One line of derived state per package: the latest accepted report of the assignee's current generation decides. A report
/// counts only for the package assigned last at or before the report was accepted, so it never predates the assignment it
/// counts for and never counts for two packages (ties on assigned_at fall back to plan and package id).
pub(crate) fn package_report(
    kernel: &Kernel,
    plan_id: &str,
    package_id: &str,
    assignee_agent_id: Option<&str>,
    assigned_at: Option<&str>,
) -> KernelResult<Option<String>> {
    let (Some(assignee), Some(assigned_at)) = (assignee_agent_id, assigned_at) else {
        return Ok(None);
    };
    query_opt(
        &kernel.database,
        "SELECT r.report_id FROM agent_reports r
         JOIN agents a ON a.project_id = r.project_id AND a.agent_id = r.agent_id AND a.generation = r.generation
         WHERE r.project_id = ? AND r.agent_id = ? AND r.state = 'accepted' AND r.created_at >= ?
           AND NOT EXISTS (
             SELECT 1 FROM plan_packages other
             WHERE other.project_id = r.project_id AND other.assignee_agent_id = r.agent_id
               AND other.assigned_at <= r.created_at
               AND (other.assigned_at, other.plan_id, other.package_id) > (?, ?, ?))
         ORDER BY r.sequence DESC LIMIT 1",
        [
            kernel.project_id.as_str(),
            assignee,
            assigned_at,
            assigned_at,
            plan_id,
            package_id,
        ],
        |row| row.get::<_, String>(0),
    )
}

fn package_progress(
    kernel: &Kernel,
    plan_id: &str,
    package_id: &str,
    assignee_agent_id: Option<&str>,
    assigned_at: Option<&str>,
) -> KernelResult<&'static str> {
    if assignee_agent_id.is_none() || assigned_at.is_none() {
        return Ok("unassigned");
    }
    let Some(report) = package_report(kernel, plan_id, package_id, assignee_agent_id, assigned_at)?
    else {
        return Ok("assigned");
    };
    let integrated = query_opt(
        &kernel.database,
        "SELECT 1 AS present FROM integration_reports ir
         JOIN integrations i ON i.project_id = ir.project_id AND i.integration_id = ir.integration_id
         WHERE ir.project_id = ? AND ir.report_id = ? AND i.state IN ('merged', 'confirmed')",
        [&kernel.project_id, &report],
        |_| Ok(()),
    )?;
    if integrated.is_some() {
        return Ok("integrated");
    }
    let latest = query_opt(
        &kernel.database,
        "SELECT state FROM reviews WHERE project_id = ? AND subject_report_id = ? AND state IN ('passed', 'findings')
         ORDER BY sequence DESC LIMIT 1",
        [&kernel.project_id, &report],
        |row| row.get::<_, String>(0),
    )?;
    Ok(match latest.as_deref() {
        None => "reported",
        Some("passed") => "reviewed",
        Some(_) => "findings",
    })
}

/// `planPackages`: the packages of a plan with their derived progress.
pub(crate) fn plan_packages(kernel: &Kernel, plan_id: &str) -> KernelResult<Vec<Value>> {
    let rows = query_all(
        &kernel.database,
        "SELECT * FROM plan_packages WHERE project_id = ? AND plan_id = ? ORDER BY package_id",
        [&kernel.project_id, plan_id],
        |row| {
            Ok((
                row.get::<_, String>("package_id")?,
                row.get::<_, Option<String>>("assignee_agent_id")?,
                row.get::<_, Option<String>>("assigned_at")?,
                row.get::<_, Option<String>>("assignment_message_id")?,
                row.get::<_, Option<String>>("cancelled_at")?,
            ))
        },
    )?;
    let mut out = Vec::new();
    for (package_id, assignee, assigned_at, message_id, cancelled_at) in rows {
        let progress = package_progress(
            kernel,
            plan_id,
            &package_id,
            assignee.as_deref(),
            assigned_at.as_deref(),
        )?;
        out.push(json!({
            "packageId": package_id,
            "assigneeAgentId": assignee,
            "assignedAt": assigned_at,
            "assignmentMessageId": message_id,
            "cancelledAt": cancelled_at,
            "progress": progress,
        }));
    }
    Ok(out)
}

/// `assignPackage`.
pub fn assign_package(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let plan_id = safe_id(field(input, "planId"), "plan id")?;
    let package_id = safe_id(field(input, "packageId"), "package id")?;
    let agent_id = safe_id(field(input, "agentId"), "agent id")?;
    let early: Option<String> = match input.get("early") {
        None => None,
        Some(value) => Some(safe_text(value, "early reason", 500, false)?),
    };
    kernel.mutate(context, "plan.assign", "plan:write", input, |actor| {
        pauses::assert_run_not_paused(kernel, "plan assign")?;
        if actor.role != "PM" && actor.role != "operator" {
            return Err(KernelError::controller(
                "only the PM or the operator assigns a package",
            ));
        }
        let paused = pauses::pause_state_of(kernel)?
            .agents
            .into_iter()
            .find(|pause| pause.agent_id.as_deref() == Some(agent_id));
        if let Some(pause) = paused {
            return Err(KernelError::RunPaused(format!(
                "agent_paused: {agent_id} is paused and cannot be assigned a package: {}",
                pause.reason
            )));
        }
        let Some(plan) = plans::plan_row(kernel, plan_id)? else {
            return Err(KernelError::controller(format!("plan {plan_id} does not exist")));
        };
        if plan.cancelled_at.is_some() {
            return Err(KernelError::controller(format!(
                "plan_cancelled: plan {plan_id} was cancelled"
            )));
        }
        if plan.state != "approved" {
            return Err(KernelError::controller(format!(
                "plan {plan_id} is {}; packages are assigned once it is approved",
                plan.state
            )));
        }
        let pkg = query_opt(
            &kernel.database,
            "SELECT assignee_agent_id, cancelled_at FROM plan_packages WHERE project_id = ? AND plan_id = ? AND package_id = ?",
            [&kernel.project_id, plan_id, package_id],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<String>>(1)?)),
        )?;
        let Some((current_assignee, pkg_cancelled)) = pkg else {
            return Err(KernelError::controller(format!(
                "plan {plan_id} has no package {package_id}"
            )));
        };
        if pkg_cancelled.is_some() {
            return Err(KernelError::controller(format!(
                "package_cancelled: package {package_id} of plan {plan_id} was cancelled"
            )));
        }
        let agent = match kernel.agent_row(agent_id)? {
            Some(agent) if agent.state == "active" && agent.kind == "Developer" => agent,
            _ => {
                return Err(KernelError::controller(format!(
                    "{agent_id} is not an active developer-kind agent"
                )))
            }
        };
        if plan.architect_agent_id.as_deref() == Some(agent.agent_id.as_str()) {
            return Err(KernelError::controller(
                "the architect cannot hold a package",
            ));
        }
        if let Some(current_id) = &current_assignee {
            if kernel
                .agent_row(current_id)?
                .is_some_and(|current| current.state == "active")
            {
                return Err(KernelError::controller(format!(
                    "package {package_id} is already assigned to {current_id}"
                )));
            }
        }
        let holding = query_opt(
            &kernel.database,
            "SELECT package_id FROM plan_packages WHERE project_id = ? AND plan_id = ? AND assignee_agent_id = ? AND package_id <> ? AND cancelled_at IS NULL",
            [&kernel.project_id, plan_id, &agent.agent_id, package_id],
            |row| row.get::<_, String>(0),
        )?;
        if let Some(held) = holding {
            return Err(KernelError::controller(format!(
                "{} already holds package {held} of this plan",
                agent.agent_id
            )));
        }
        let revision = query_opt(
            &kernel.database,
            "SELECT body_json FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
            rusqlite::params![kernel.project_id, plan_id, plan.approved_revision],
            |row| row.get::<_, String>(0),
        )?;
        let view = revision.and_then(|body| package_of_body(&body, package_id));
        let (Some(view), Some(architect)) = (view, plan.architect_agent_id.as_deref()) else {
            return Err(KernelError::controller(format!(
                "plan {plan_id} has no readable text for package {package_id}"
            )));
        };
        // The gate asks whether the interface to build against will exist: a cancelled dependency stays unmet, so the PM
        // decides with --early. (Integration order asks whether anything must merge first; nothing will for a cancelled one.)
        let mut unmet: Vec<String> = Vec::new();
        for dep in &view.depends_on {
            let row = query_opt(
                &kernel.database,
                "SELECT assignee_agent_id, assigned_at, cancelled_at FROM plan_packages WHERE project_id = ? AND plan_id = ? AND package_id = ?",
                [&kernel.project_id, plan_id, dep],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )?;
            let Some((assignee, assigned_at, cancelled_at)) = row else {
                unmet.push(format!("{dep} (unknown)"));
                continue;
            };
            if cancelled_at.is_some() {
                unmet.push(format!("{dep} (cancelled)"));
                continue;
            }
            let state =
                package_progress(kernel, plan_id, dep, assignee.as_deref(), assigned_at.as_deref())?;
            if state != "reviewed" && state != "integrated" {
                unmet.push(format!("{dep} ({state})"));
            }
        }
        if !unmet.is_empty() && early.is_none() {
            return Err(KernelError::controller(format!(
                "dependencies_unmet: package {package_id} depends on {}; assign them first, or pass --early \"<reason>\" to assign anyway",
                unmet.join(", ")
            )));
        }
        let task = work_package_message(plan_id, package_id, architect, &view, &unmet);
        if task.len() > MAX_MESSAGE_BYTES {
            return Err(KernelError::controller(format!(
                "package {package_id} is too large to send as one message"
            )));
        }
        let now = kernel.now();
        let message_id = messages::insert_queued_message(
            kernel,
            &kernel.controller_actor_id()?,
            &agent,
            &task,
            &sha256(&task),
            &now,
            false,
        )?;
        execute(
            &kernel.database,
            "UPDATE plan_packages SET assignee_agent_id = ?, assigned_at = ?, assignment_message_id = ? WHERE project_id = ? AND plan_id = ? AND package_id = ?",
            [
                &agent.agent_id,
                &now,
                &message_id,
                &kernel.project_id,
                &plan_id.to_string(),
                &package_id.to_string(),
            ],
        )?;
        execute(
            &kernel.database,
            "UPDATE plans SET updated_at = ? WHERE project_id = ? AND plan_id = ?",
            [now.as_str(), kernel.project_id.as_str(), plan_id],
        )?;
        let mut record = plan_packages(kernel, plan_id)?
            .into_iter()
            .find(|p| p["packageId"] == package_id)
            .and_then(|p| p.as_object().cloned())
            .unwrap_or_default();
        record.insert(
            "unmet".into(),
            if early.is_none() { json!([]) } else { json!(unmet) },
        );
        let mut details = Map::new();
        details.insert("packageId".into(), json!(package_id));
        details.insert("agentId".into(), json!(agent.agent_id));
        if let (Some(early), false) = (&early, unmet.is_empty()) {
            details.insert("early".into(), json!(early));
            details.insert("unmet".into(), json!(unmet));
        }
        Ok(MutationOutput::new(
            Value::Object(record),
            MutationEvent::new("plan", plan_id, 0).with_details(Value::Object(details)),
        ))
    })
}

/// Reports of the integration that are not reports of any package of the plan, in integration order.
pub(crate) fn signoff_extra_reports(
    kernel: &Kernel,
    plan_id: &str,
    integration_id: &str,
) -> KernelResult<Vec<String>> {
    query_all(
        &kernel.database,
        &format!(
            "SELECT ir.report_id FROM integration_reports ir
             JOIN agent_reports r ON r.project_id = ir.project_id AND r.report_id = ir.report_id
             WHERE ir.project_id = ? AND ir.integration_id = ? AND NOT EXISTS (
               SELECT 1 FROM plan_packages pp WHERE pp.project_id = ir.project_id AND pp.plan_id = ?
                 AND {PACKAGE_REPORT_PREDICATE})
             ORDER BY ir.position"
        ),
        [&kernel.project_id, integration_id, plan_id],
        |row| row.get::<_, String>(0),
    )
}

/// `recordSignoff`.
pub fn record_signoff(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let plan_id = safe_id(field(input, "planId"), "plan id")?;
    let integration_id = safe_id(field(input, "integrationId"), "integration id")?;
    let summary = safe_text(field(input, "summary"), "sign-off summary", 1000, true)?;
    let mut payload = input.as_object().cloned().unwrap_or_default();
    payload.insert("summary".into(), json!(summary));
    kernel.mutate(
        context,
        "plan.signoff",
        "plan:write",
        &Value::Object(payload),
        |actor| {
            let agent = kernel.agent_by_actor(&actor.actor_id)?;
            let Some(plan) = plans::plan_row(kernel, plan_id)? else {
                return Err(KernelError::controller(format!("plan {plan_id} does not exist")));
            };
            let agent = match agent {
                Some(agent) if plan.architect_agent_id.as_deref() == Some(agent.agent_id.as_str()) => agent,
                _ => {
                    return Err(KernelError::controller(
                        "only the plan's architect signs it off",
                    ))
                }
            };
            if plan.state != "approved" {
                return Err(KernelError::controller(format!(
                    "plan {plan_id} is {}, not approved",
                    plan.state
                )));
            }
            let integration = query_opt(
                &kernel.database,
                "SELECT state, branch, head_sha FROM integrations WHERE project_id = ? AND integration_id = ?",
                [&kernel.project_id, integration_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )?;
            let Some((state, branch, head_sha)) = integration else {
                return Err(KernelError::controller(format!(
                    "integration {integration_id} does not exist"
                )));
            };
            if plan.cancelled_at.is_some() {
                return Err(KernelError::controller(format!("plan {plan_id} is cancelled")));
            }
            if state != "merged" && state != "confirmed" {
                return Err(KernelError::controller(format!(
                    "integration {integration_id} is {state}, not merged or confirmed; only an integration that merged cleanly can be signed off"
                )));
            }
            let report_count = query_opt(
                &kernel.database,
                "SELECT COUNT(*) AS n FROM integration_reports WHERE project_id = ? AND integration_id = ?",
                [&kernel.project_id, integration_id],
                |row| row.get::<_, i64>(0),
            )?
            .unwrap_or(0);
            if report_count == 0 {
                return Err(KernelError::controller(format!(
                    "integration {integration_id} has no reports"
                )));
            }
            // Every live package must be in this integration or an earlier one that merged; reports outside the plan are allowed.
            let not_integrated = query_all(
                &kernel.database,
                &format!(
                    "SELECT pp.package_id FROM plan_packages pp
                     WHERE pp.project_id = ? AND pp.plan_id = ? AND pp.cancelled_at IS NULL AND NOT EXISTS (
                       SELECT 1 FROM integration_reports ir
                       JOIN integrations i ON i.project_id = ir.project_id AND i.integration_id = ir.integration_id
                       JOIN integrations target ON target.project_id = i.project_id AND target.integration_id = ?
                       JOIN agent_reports r ON r.project_id = ir.project_id AND r.report_id = ir.report_id
                       WHERE ir.project_id = pp.project_id AND i.state IN ('merged', 'confirmed')
                         AND (i.integration_id = target.integration_id OR i.sequence < target.sequence)
                         AND {PACKAGE_REPORT_PREDICATE})
                     ORDER BY pp.package_id"
                ),
                [&kernel.project_id, plan_id, integration_id],
                |row| row.get::<_, String>(0),
            )?;
            if !not_integrated.is_empty() {
                let one = not_integrated.len() == 1;
                return Err(KernelError::controller(format!(
                    "package{} {} of plan {plan_id} {} not integrated in integration {integration_id} or an earlier merged integration",
                    if one { "" } else { "s" },
                    not_integrated.join(", "),
                    if one { "is" } else { "are" }
                )));
            }
            let latest = query_opt(
                &kernel.database,
                "SELECT state FROM reviews WHERE project_id = ? AND subject_integration_id = ? AND state IN ('passed', 'findings')
                 ORDER BY sequence DESC LIMIT 1",
                [&kernel.project_id, integration_id],
                |row| row.get::<_, String>(0),
            )?;
            if latest.as_deref() != Some("passed") {
                return Err(KernelError::controller(format!(
                    "integration {integration_id} has no passed review as its latest verdict"
                )));
            }
            let signed = query_opt(
                &kernel.database,
                "SELECT 1 AS present FROM plan_signoffs WHERE project_id = ? AND plan_id = ? AND integration_id = ?",
                [&kernel.project_id, plan_id, integration_id],
                |_| Ok(()),
            )?;
            if signed.is_some() {
                return Err(KernelError::controller(format!(
                    "plan {plan_id} is already signed off for integration {integration_id}"
                )));
            }
            let now = kernel.now();
            execute(
                &kernel.database,
                "INSERT INTO plan_signoffs(project_id, plan_id, integration_id, architect_agent_id, summary, created_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
                [
                    kernel.project_id.as_str(),
                    plan_id,
                    integration_id,
                    agent.agent_id.as_str(),
                    summary.as_str(),
                    now.as_str(),
                ],
            )?;
            let extra = signoff_extra_reports(kernel, plan_id, integration_id)?;
            message_notices::notice_to_pm(
                kernel,
                &plan_signed_off_notice(
                    plan_id,
                    integration_id,
                    &branch,
                    head_sha.as_deref(),
                    &summary,
                    state == "confirmed",
                    &extra,
                ),
                &now,
                true,
            )?;
            Ok(MutationOutput::new(
                json!({
                    "integrationId": integration_id,
                    "architectAgentId": agent.agent_id,
                    "summary": summary,
                    "createdAt": now,
                    "extraReports": signoff_extra_reports(kernel, plan_id, integration_id)?,
                }),
                MutationEvent::new("plan", plan_id, 0)
                    .with_details(json!({"integrationId": integration_id})),
            ))
        },
    )
}

// ---------------------------------------------------------------------------------------- Nexora links and cancellation

/// The facts `wantedPackageState` needs for one package row; `plan_cancelled` is the plan's flag.
pub(crate) fn package_facts(
    kernel: &Kernel,
    plan_id: &str,
    row: &PackageRow,
    plan_cancelled: bool,
) -> KernelResult<PackageFacts> {
    let report = package_report(
        kernel,
        plan_id,
        &row.package_id,
        row.assignee_agent_id.as_deref(),
        row.assigned_at.as_deref(),
    )?;
    let confirmed = match &report {
        Some(report_id) => integrations::report_in_confirmed_integration(kernel, report_id)?,
        None => false,
    };
    Ok(PackageFacts {
        progress: package_progress(
            kernel,
            plan_id,
            &row.package_id,
            row.assignee_agent_id.as_deref(),
            row.assigned_at.as_deref(),
        )?,
        cancelled: plan_cancelled || row.cancelled_at.is_some(),
        confirmed,
    })
}

pub(crate) fn plan_package_rows(kernel: &Kernel, plan_id: &str) -> KernelResult<Vec<PackageRow>> {
    query_all(
        &kernel.database,
        "SELECT package_id, assignee_agent_id, assigned_at, cancelled_at FROM plan_packages WHERE project_id = ? AND plan_id = ? ORDER BY package_id",
        [&kernel.project_id, plan_id],
        |row| {
            Ok(PackageRow {
                package_id: row.get(0)?,
                assignee_agent_id: row.get(1)?,
                assigned_at: row.get(2)?,
                cancelled_at: row.get(3)?,
            })
        },
    )
}

/// Splits a package ref `<plan-id>/<package-id>`; the ids themselves are checked against the ledger by the caller.
pub(crate) fn package_ref(ref_id: &str) -> KernelResult<(String, String)> {
    let parts: Vec<&str> = ref_id.split('/').collect();
    if parts.len() != 2 {
        return Err(KernelError::type_error(
            "a package ref is <plan-id>/<package-id>",
        ));
    }
    Ok((
        safe_id_str(parts[0], "plan id")?.to_string(),
        safe_id_str(parts[1], "package id")?.to_string(),
    ))
}

/// `cancelPlan`: records the operator's decision to cancel a plan or one package of an approved plan.
pub fn cancel_plan(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let plan_id = safe_id(field(input, "planId"), "plan id")?;
    let package_id: Option<&str> = match input.get("packageId") {
        None => None,
        Some(value) => Some(safe_id(value, "package id")?),
    };
    kernel.mutate(context, "plan.cancel", "plan:write", input, |actor| {
        if actor.role != "operator" {
            return Err(KernelError::controller("only the operator cancels a plan"));
        }
        let Some(plan) = plans::plan_row(kernel, plan_id)? else {
            return Err(KernelError::controller(format!("plan {plan_id} does not exist")));
        };
        if plan.cancelled_at.is_some() {
            return Err(KernelError::controller(format!(
                "{}: plan {plan_id} was cancelled",
                if package_id.is_none() { "already_cancelled" } else { "plan_cancelled" }
            )));
        }
        let now = kernel.now();
        let cancelled_packages: Vec<String>;
        let mut review_id: Option<String> = None;
        let mut reviewer_agent_id: Option<String> = None;
        if let Some(package_id) = package_id {
            if plan.state != "approved" {
                return Err(KernelError::controller(format!(
                    "plan_not_approved: plan {plan_id} is {}",
                    plan.state
                )));
            }
            let row = plan_package_rows(kernel, plan_id)?
                .into_iter()
                .find(|r| r.package_id == package_id);
            let Some(row) = row else {
                return Err(KernelError::controller(format!(
                    "plan {plan_id} has no package {package_id}"
                )));
            };
            if row.cancelled_at.is_some() {
                return Err(KernelError::controller(format!(
                    "already_cancelled: package {package_id} was cancelled"
                )));
            }
            if package_facts(kernel, plan_id, &row, false)?.confirmed {
                return Err(KernelError::controller(format!(
                    "package_confirmed: package {package_id} is already integrated and confirmed"
                )));
            }
            execute(
                &kernel.database,
                "UPDATE plan_packages SET cancelled_at = ? WHERE project_id = ? AND plan_id = ? AND package_id = ? AND cancelled_at IS NULL",
                [now.as_str(), kernel.project_id.as_str(), plan_id, package_id],
            )?;
            cancelled_packages = vec![package_id.to_string()];
        } else {
            if plan.state == "superseded" {
                return Err(KernelError::controller(format!(
                    "plan_superseded: plan {plan_id} was superseded"
                )));
            }
            if plan.state == "in_review" {
                let open = query_opt(
                    &kernel.database,
                    "SELECT review_id, reviewer_agent_id FROM reviews WHERE project_id = ? AND subject_plan_id = ? AND state = 'started'",
                    [&kernel.project_id, plan_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )?;
                if let Some((open_review, open_reviewer)) = open {
                    execute(
                        &kernel.database,
                        "UPDATE reviews SET state = 'cancelled', failure_reason = 'plan cancelled', completed_at = ? WHERE project_id = ? AND review_id = ? AND state = 'started'",
                        [now.as_str(), kernel.project_id.as_str(), open_review.as_str()],
                    )?;
                    review_id = Some(open_review);
                    reviewer_agent_id = Some(open_reviewer);
                }
                execute(
                    &kernel.database,
                    "UPDATE plans SET state = 'draft', updated_at = ? WHERE project_id = ? AND plan_id = ?",
                    [now.as_str(), kernel.project_id.as_str(), plan_id],
                )?;
            }
            let mut live = Vec::new();
            for row in plan_package_rows(kernel, plan_id)? {
                if row.cancelled_at.is_none() && !package_facts(kernel, plan_id, &row, false)?.confirmed {
                    live.push(row.package_id);
                }
            }
            for package in &live {
                execute(
                    &kernel.database,
                    "UPDATE plan_packages SET cancelled_at = ? WHERE project_id = ? AND plan_id = ? AND package_id = ? AND cancelled_at IS NULL",
                    [now.as_str(), kernel.project_id.as_str(), plan_id, package.as_str()],
                )?;
            }
            cancelled_packages = live;
            execute(
                &kernel.database,
                "UPDATE plans SET cancelled_at = ?, updated_at = ? WHERE project_id = ? AND plan_id = ? AND cancelled_at IS NULL",
                [now.as_str(), now.as_str(), kernel.project_id.as_str(), plan_id],
            )?;
        }
        let mut notified = plans::queue_plan_notice(
            kernel,
            &plan,
            &plan_cancelled_notice(plan_id, package_id),
            &now,
            true,
        )?;
        let held: Vec<PackageRow> = plan_package_rows(kernel, plan_id)?
            .into_iter()
            .filter(|row| cancelled_packages.contains(&row.package_id))
            .collect();
        for row in held {
            let developer = match &row.assignee_agent_id {
                Some(id) => kernel.agent_row(id)?,
                None => None,
            };
            let Some(developer) = developer.filter(|d| d.state == "active") else {
                continue;
            };
            let body = format!(
                "Stop work on package {} of plan {plan_id}: it was cancelled by the operator. Do not report it.",
                row.package_id
            );
            messages::insert_queued_message(
                kernel,
                &kernel.controller_actor_id()?,
                &developer,
                &body,
                &sha256(&body),
                &now,
                false,
            )?;
            if !notified.contains(&developer.agent_id) {
                notified.push(developer.agent_id.clone());
            }
        }
        Ok(MutationOutput::new(
            json!({
                "planId": plan_id,
                "packageId": package_id,
                "cancelledPackages": cancelled_packages,
                "reviewId": review_id,
                "reviewerAgentId": reviewer_agent_id,
                "notified": notified,
            }),
            MutationEvent::new("plan", plan_id, 0)
                .with_details(json!({"packageId": package_id, "cancelledPackages": cancelled_packages})),
        ))
    })
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "assignPackage" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(assign_package(kernel, &context, input)))
        }
        "recordSignoff" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(record_signoff(kernel, &context, input)))
        }
        "cancelPlan" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(cancel_plan(kernel, &context, input)))
        }
        _ => None,
    }
}
