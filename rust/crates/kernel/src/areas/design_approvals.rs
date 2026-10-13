//! The design approvals (the design_approvals table): the user's decision on the report of a design package (plan body key
//! `design: true`). A decision is tied to the report it is about; the latest row per package is the current decision.
//! `assign_package` asks `gate_state` before it hands out a package that depends on a design package.
//!
//! Operations by `ControllerCore` method name (`dispatch`): `recordDesignDecision` takes the mutation context and one
//! input object; `designApprovals` takes (planId, packageId|null).

use crate::dispatch::{arg, call, Args};
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{execute, query_all, query_opt, safe_id, safe_text};
use crate::kernel::Kernel;
use crate::plan_body::{dependents_of_package, package_is_design};
use crate::records::{MutationEvent, MutationOutput};
use crate::types::MutationContext;
use serde_json::{json, Value};

use super::{message_notices, plan_packages, plans};

/// The longest link or user text.
const MAX_BYTES: usize = 1024;

fn field<'a>(input: &'a Value, key: &str) -> &'a Value {
    input.get(key).unwrap_or(&Value::Null)
}

/// One recorded decision.
#[derive(Clone, Debug)]
pub(crate) struct Decision {
    pub decision: String,
    pub link: Option<String>,
    pub report_id: String,
    pub asked_user: String,
    pub decided_by: String,
    pub decided_at: String,
}

impl Decision {
    fn value(&self) -> Value {
        json!({
            "decision": self.decision,
            "link": self.link,
            "reportId": self.report_id,
            "askedUser": self.asked_user,
            "decidedBy": self.decided_by,
            "decidedAt": self.decided_at,
        })
    }

    /// The short form `plan_packages` carries.
    pub(crate) fn summary(&self) -> Value {
        json!({
            "decision": self.decision,
            "link": self.link,
            "reportId": self.report_id,
            "decidedAt": self.decided_at,
        })
    }
}

/// The current decision of a package: its latest row.
pub(crate) fn current_decision(
    kernel: &Kernel,
    plan_id: &str,
    package_id: &str,
) -> KernelResult<Option<Decision>> {
    query_opt(
        &kernel.database,
        "SELECT decision, link, report_id, asked_user, decided_by, decided_at FROM design_approvals
         WHERE project_id = ? AND plan_id = ? AND package_id = ? ORDER BY sequence DESC LIMIT 1",
        [&kernel.project_id, plan_id, package_id],
        |row| {
            Ok(Decision {
                decision: row.get(0)?,
                link: row.get(1)?,
                report_id: row.get(2)?,
                asked_user: row.get(3)?,
                decided_by: row.get(4)?,
                decided_at: row.get(5)?,
            })
        },
    )
}

/// Why the design package `package_id` does not unlock its dependents, or `None` when its latest report is approved.
pub(crate) fn gate_state(
    kernel: &Kernel,
    plan_id: &str,
    package_id: &str,
) -> KernelResult<Option<&'static str>> {
    let Some(decision) = current_decision(kernel, plan_id, package_id)? else {
        return Ok(Some("no decision"));
    };
    if decision.decision != "approved" {
        return Ok(Some("changes requested"));
    }
    let latest = latest_report(kernel, plan_id, package_id)?;
    Ok(if latest.as_deref() == Some(decision.report_id.as_str()) {
        None
    } else {
        Some("approval is for an earlier report")
    })
}

/// The latest accepted report the package's assignee made for it.
fn latest_report(kernel: &Kernel, plan_id: &str, package_id: &str) -> KernelResult<Option<String>> {
    let row = query_opt(
        &kernel.database,
        "SELECT assignee_agent_id, assigned_at FROM plan_packages WHERE project_id = ? AND plan_id = ? AND package_id = ?",
        [&kernel.project_id, plan_id, package_id],
        |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<String>>(1)?)),
    )?;
    let Some((assignee, assigned_at)) = row else {
        return Ok(None);
    };
    plan_packages::package_report(
        kernel,
        plan_id,
        package_id,
        assignee.as_deref(),
        assigned_at.as_deref(),
    )
}

fn approved_body(kernel: &Kernel, plan_id: &str) -> KernelResult<Option<String>> {
    let Some(plan) = plans::plan_row(kernel, plan_id)? else {
        return Ok(None);
    };
    query_opt(
        &kernel.database,
        "SELECT body_json FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
        rusqlite::params![kernel.project_id, plan_id, plan.approved_revision],
        |row| row.get::<_, String>(0),
    )
}

/// `recordDesignDecision`: the PM or the operator records the user's `approved` or `changes` decision on a design
/// package's latest report.
pub fn record_design_decision(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let plan_id = safe_id(field(input, "planId"), "plan id")?;
    let package_id = safe_id(field(input, "packageId"), "package id")?;
    let decision = match field(input, "decision").as_str() {
        Some(d @ ("approved" | "changes")) => d,
        _ => {
            return Err(KernelError::type_error(
                "the decision must be approved or changes",
            ))
        }
    };
    let asked_user = safe_text(field(input, "askedUser"), "--asked-user", MAX_BYTES, false)?;
    if asked_user.len() > MAX_BYTES {
        return Err(KernelError::type_error(format!(
            "--asked-user must be at most {MAX_BYTES} bytes"
        )));
    }
    let link = match input.get("link") {
        None | Some(Value::Null) => None,
        Some(value) => Some(safe_text(value, "--link", MAX_BYTES, false)?),
    };
    if link.as_ref().is_some_and(|l| l.len() > MAX_BYTES) {
        return Err(KernelError::type_error(format!(
            "--link must be at most {MAX_BYTES} bytes"
        )));
    }
    if decision == "approved" && link.is_none() {
        return Err(KernelError::type_error(
            "--link is required: the design the user approved (a URL or repository path)",
        ));
    }
    kernel.mutate(context, "design.decide", "plan:write", input, |actor| {
        if actor.role != "PM" && actor.role != "operator" {
            return Err(KernelError::controller(
                "only the PM or the operator records a design decision",
            ));
        }
        if plans::plan_row(kernel, plan_id)?.is_none() {
            return Err(KernelError::controller(format!("plan {plan_id} does not exist")));
        }
        let package = query_opt(
            &kernel.database,
            "SELECT assignee_agent_id, cancelled_at FROM plan_packages WHERE project_id = ? AND plan_id = ? AND package_id = ?",
            [&kernel.project_id, plan_id, package_id],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<String>>(1)?)),
        )?;
        let Some((assignee, cancelled_at)) = package else {
            return Err(KernelError::controller(format!(
                "plan {plan_id} has no package {package_id}"
            )));
        };
        if cancelled_at.is_some() {
            return Err(KernelError::controller(format!(
                "package_cancelled: package {package_id} of plan {plan_id} was cancelled"
            )));
        }
        let body = approved_body(kernel, plan_id)?;
        if !body.as_deref().is_some_and(|b| package_is_design(b, package_id)) {
            return Err(KernelError::controller(format!(
                "package {package_id} of plan {plan_id} is not a design package (design: true in the plan)"
            )));
        }
        let Some(report_id) = latest_report(kernel, plan_id, package_id)? else {
            return Err(KernelError::controller(format!(
                "design package {package_id} of plan {plan_id} has no report yet; decide once its designer has reported"
            )));
        };
        let now = kernel.now();
        let sequence = query_opt(
            &kernel.database,
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM design_approvals WHERE project_id = ?",
            [&kernel.project_id],
            |row| row.get::<_, i64>(0),
        )?
        .unwrap_or(1);
        execute(
            &kernel.database,
            "INSERT INTO design_approvals(project_id, decision_id, sequence, plan_id, package_id, decision, report_id, link, asked_user, decided_by, decided_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                kernel.project_id,
                kernel.env.uuid(),
                sequence,
                plan_id,
                package_id,
                decision,
                report_id,
                link,
                asked_user,
                actor.actor_id,
                now
            ],
        )?;
        let dependents: Vec<String> = body
            .as_deref()
            .map(|b| dependents_of_package(b, package_id))
            .unwrap_or_default();
        let mut assigned_dependents = Vec::new();
        for dependent in &dependents {
            let held = query_opt(
                &kernel.database,
                "SELECT assignee_agent_id FROM plan_packages WHERE project_id = ? AND plan_id = ? AND package_id = ? AND cancelled_at IS NULL",
                [&kernel.project_id, plan_id, dependent],
                |row| row.get::<_, Option<String>>(0),
            )?
            .flatten();
            if let Some(agent) = held {
                assigned_dependents.push(json!({"packageId": dependent, "agentId": agent}));
            }
        }
        let mut message_id = Value::Null;
        if decision == "changes" {
            if let Some(designer) = assignee {
                let text = format!(
                    "Design changes requested for package {package_id} of plan {plan_id}.\nDesign: {}\nThe user said: {asked_user}\nRevise the design, commit it (amend your commit) and report again with cstan report; the next decision is about your new report.{}",
                    link.as_deref().unwrap_or("(no link given)"),
                    if assigned_dependents.is_empty() {
                        String::new()
                    } else {
                        "\nPackages that depend on this design are already assigned; the PM will tell their developers what changes.".to_string()
                    }
                );
                if let Some(id) =
                    message_notices::notice_to_agent(kernel, "work_package", &designer, &text, &now, true)?
                {
                    message_id = json!(id);
                }
            }
        }
        let record = json!({
            "planId": plan_id,
            "packageId": package_id,
            "decision": decision,
            "reportId": report_id,
            "link": link,
            "askedUser": asked_user,
            "decidedAt": now,
            "messageId": message_id,
            "assignedDependents": assigned_dependents,
        });
        Ok(MutationOutput::new(
            record,
            MutationEvent::new("plan", plan_id, 0).with_details(json!({
                "packageId": package_id, "designDecision": decision, "reportId": report_id,
            })),
        ))
    })
}

/// `designApprovals`: the decisions of one design package, or of every design package of the plan, oldest first, each
/// with the current decision and the latest report.
pub fn design_approvals(
    kernel: &Kernel,
    plan_id: &str,
    package_id: Option<&str>,
) -> KernelResult<Value> {
    let rows = query_all(
        &kernel.database,
        "SELECT package_id, decision, link, report_id, asked_user, decided_by, decided_at FROM design_approvals
         WHERE project_id = ? AND plan_id = ? AND (? IS NULL OR package_id = ?) ORDER BY sequence",
        rusqlite::params![kernel.project_id, plan_id, package_id, package_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                Decision {
                    decision: row.get(1)?,
                    link: row.get(2)?,
                    report_id: row.get(3)?,
                    asked_user: row.get(4)?,
                    decided_by: row.get(5)?,
                    decided_at: row.get(6)?,
                },
            ))
        },
    )?;
    let body = approved_body(kernel, plan_id)?;
    let mut ids: Vec<String> = match package_id {
        Some(id) => vec![id.to_string()],
        None => plan_packages::plan_packages(kernel, plan_id)?
            .iter()
            .filter_map(|p| p["packageId"].as_str().map(str::to_string))
            .filter(|id| body.as_deref().is_some_and(|b| package_is_design(b, id)))
            .collect(),
    };
    ids.sort();
    let mut packages = Vec::new();
    for id in ids {
        let history: Vec<Value> = rows
            .iter()
            .filter(|(package, _)| *package == id)
            .map(|(_, decision)| decision.value())
            .collect();
        packages.push(json!({
            "packageId": id,
            "design": body.as_deref().is_some_and(|b| package_is_design(b, &id)),
            "latestReportId": latest_report(kernel, plan_id, &id)?,
            "gate": gate_state(kernel, plan_id, &id)?.unwrap_or("approved"),
            "decisions": history,
        }));
    }
    Ok(json!({"planId": plan_id, "packages": packages}))
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "recordDesignDecision" => {
            let context = arg!(a.ctx(0));
            Some(call(record_design_decision(kernel, &context, a.value(1))))
        }
        "designApprovals" => {
            let plan_id = arg!(a.str(0, "planId"));
            let package_id = arg!(a.opt_str(1, "packageId"));
            Some(call(design_approvals(kernel, plan_id, package_id)))
        }
        _ => None,
    }
}
