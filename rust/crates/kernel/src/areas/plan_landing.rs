//! Where the content of a plan package landed: one reading of the ledger for package progress, plan cancel and sign-off,
//! plus the operator's record of a landing no integration holds (`package_landings`).

use crate::dispatch::{arg, call, Args};
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{execute, query_all, query_opt, safe_id, safe_text};
use crate::kernel::Kernel;
use crate::records::{MutationEvent, MutationOutput};
use crate::types::MutationContext;
use serde_json::{json, Value};

use super::{message_notices, plan_packages, plans};

/// True for the report `r` (agent_reports) that counts for package `pp` (plan_packages): the assignee's accepted report of
/// its current generation made since the assignment, unless a later-assigned package of the same agent claims it.
pub(crate) const PACKAGE_REPORT_PREDICATE: &str = "pp.assignee_agent_id = r.agent_id AND r.state = 'accepted' AND pp.assigned_at <= r.created_at
  AND EXISTS (SELECT 1 FROM agents a WHERE a.project_id = r.project_id AND a.agent_id = r.agent_id AND a.generation = r.generation)
  AND NOT EXISTS (
    SELECT 1 FROM plan_packages other
    WHERE other.project_id = r.project_id AND other.assignee_agent_id = r.agent_id AND other.assigned_at <= r.created_at
      AND (other.assigned_at, other.plan_id, other.package_id) > (pp.assigned_at, pp.plan_id, pp.package_id))";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LandingKind {
    Merged,
    Confirmed,
    Covered,
    Recorded,
}

impl LandingKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            LandingKind::Merged => "merged",
            LandingKind::Confirmed => "confirmed",
            LandingKind::Covered => "covered",
            LandingKind::Recorded => "recorded",
        }
    }
}

/// One way the content of a package is on HEAD.
#[derive(Clone, Debug)]
pub(crate) struct Landing {
    pub kind: LandingKind,
    pub integration_id: Option<String>,
    pub integration_sequence: Option<i64>,
    pub commit_sha: Option<String>,
}

impl Landing {
    /// True for every kind but a merge that is not yet confirmed.
    pub(crate) fn settled(&self) -> bool {
        self.kind != LandingKind::Merged
    }

    /// `<kind> <integration-id or sha12>` as the refusals name it.
    fn describe(&self) -> String {
        let what = match (&self.integration_id, &self.commit_sha) {
            (Some(id), _) => id.clone(),
            (None, Some(sha)) => sha.chars().take(12).collect(),
            (None, None) => String::new(),
        };
        format!("{} {what}", self.kind.as_str())
    }
}

/// Every landing of the package: its accepted reports (`PACKAGE_REPORT_PREDICATE`) in a merged or confirmed integration or
/// covered by a confirmed one, and the recorded row; by integration sequence, the recorded one last.
pub(crate) fn package_landings(
    kernel: &Kernel,
    plan_id: &str,
    package_id: &str,
) -> KernelResult<Vec<Landing>> {
    let sql = format!(
        "SELECT i.state, i.integration_id, i.sequence, NULL AS commit_sha
         FROM plan_packages pp
         JOIN agent_reports r ON r.project_id = pp.project_id
         JOIN integration_reports ir ON ir.project_id = r.project_id AND ir.report_id = r.report_id
         JOIN integrations i ON i.project_id = ir.project_id AND i.integration_id = ir.integration_id
         WHERE pp.project_id = ?1 AND pp.plan_id = ?2 AND pp.package_id = ?3 AND i.state IN ('merged', 'confirmed')
           AND {PACKAGE_REPORT_PREDICATE}
         UNION ALL
         SELECT 'covered', i.integration_id, i.sequence, NULL
         FROM plan_packages pp
         JOIN agent_reports r ON r.project_id = pp.project_id
         JOIN integration_covered_reports c ON c.project_id = r.project_id AND c.report_id = r.report_id
         JOIN integrations i ON i.project_id = c.project_id AND i.integration_id = c.integration_id
         WHERE pp.project_id = ?1 AND pp.plan_id = ?2 AND pp.package_id = ?3 AND i.state = 'confirmed'
           AND {PACKAGE_REPORT_PREDICATE}
         UNION ALL
         SELECT 'recorded', NULL, NULL, commit_sha FROM package_landings
         WHERE project_id = ?1 AND plan_id = ?2 AND package_id = ?3
"
    );
    let mut landings = query_all(
        &kernel.database,
        &sql,
        rusqlite::params![kernel.project_id, plan_id, package_id],
        |row| {
            let kind = match row.get::<_, String>(0)?.as_str() {
                "merged" => LandingKind::Merged,
                "confirmed" => LandingKind::Confirmed,
                "covered" => LandingKind::Covered,
                _ => LandingKind::Recorded,
            };
            Ok(Landing {
                kind,
                integration_id: row.get(1)?,
                integration_sequence: row.get(2)?,
                commit_sha: row.get(3)?,
            })
        },
    )?;
    landings.sort_by_key(|l| (l.integration_sequence.is_none(), l.integration_sequence));
    Ok(landings)
}

/// Whether sign-off against the integration with `target_sequence` counts the package as landed: any settled landing, or a
/// merge in that integration or an earlier one.
pub(crate) fn counts_for_signoff(landings: &[Landing], target_sequence: i64) -> bool {
    landings
        .iter()
        .any(|l| l.settled() || l.integration_sequence.is_some_and(|s| s <= target_sequence))
}

fn field<'a>(input: &'a Value, key: &str) -> &'a Value {
    input.get(key).unwrap_or(&Value::Null)
}

fn sha12(sha: &str) -> String {
    sha.chars().take(12).collect()
}

fn signed_off(kernel: &Kernel, plan_id: &str) -> KernelResult<bool> {
    Ok(query_opt(
        &kernel.database,
        "SELECT 1 AS present FROM plan_signoffs WHERE project_id = ? AND plan_id = ? LIMIT 1",
        [&kernel.project_id, plan_id],
        |_| Ok(()),
    )?
    .is_some())
}

/// Whether the report itself is in a merged or confirmed integration or covered by a confirmed one.
fn report_landed(kernel: &Kernel, report_id: &str) -> KernelResult<bool> {
    Ok(query_opt(
        &kernel.database,
        "SELECT 1 AS present FROM integration_reports ir
         JOIN integrations i ON i.project_id = ir.project_id AND i.integration_id = ir.integration_id
         WHERE ir.project_id = ?1 AND ir.report_id = ?2 AND i.state IN ('merged', 'confirmed')
         UNION ALL
         SELECT 1 FROM integration_covered_reports c
         JOIN integrations i ON i.project_id = c.project_id AND i.integration_id = c.integration_id
         WHERE c.project_id = ?1 AND c.report_id = ?2 AND i.state = 'confirmed'",
        [&kernel.project_id, report_id],
        |_| Ok(()),
    )?
    .is_some())
}

/// `recordPackageLanding`: the operator states that the content of a package is on HEAD at `commitSha`.
pub fn record_package_landing(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let plan_id = safe_id(field(input, "planId"), "plan id")?;
    let package_id = safe_id(field(input, "packageId"), "package id")?;
    let commit_sha = match field(input, "commitSha").as_str() {
        Some(sha)
            if sha.len() == 40 && sha.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) =>
        {
            sha
        }
        _ => {
            return Err(KernelError::type_error(
                "commitSha must be 40 lowercase hexadecimal characters",
            ))
        }
    };
    let note = safe_text(field(input, "note"), "landing note", 500, false)?;
    kernel.mutate(context, "plan.landed", "plan:write", input, |actor| {
        if actor.role != "operator" {
            return Err(KernelError::controller(
                "only the operator records a landed package",
            ));
        }
        let Some(plan) = plans::plan_row(kernel, plan_id)? else {
            return Err(KernelError::controller(format!(
                "plan {plan_id} does not exist"
            )));
        };
        if plan.cancelled_at.is_some() {
            return Err(KernelError::controller(format!(
                "plan_cancelled: plan {plan_id} was cancelled"
            )));
        }
        if plan.state != "approved" {
            return Err(KernelError::controller(format!(
                "plan_not_approved: plan {plan_id} is {}",
                plan.state
            )));
        }
        if signed_off(kernel, plan_id)? {
            return Err(KernelError::controller(format!(
                "plan_signed_off: plan {plan_id} was signed off; its landings are final"
            )));
        }
        let row = plan_packages::plan_package_rows(kernel, plan_id)?
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
        let latest = plan_packages::package_report(
            kernel,
            plan_id,
            package_id,
            row.assignee_agent_id.as_deref(),
            row.assigned_at.as_deref(),
        )?;
        let Some(latest) = latest else {
            return Err(KernelError::controller(format!(
                "no_report: package {package_id} has no accepted report; cancel it instead"
            )));
        };
        if let Some(settled) = package_landings(kernel, plan_id, package_id)?
            .into_iter()
            .find(Landing::settled)
        {
            return Err(KernelError::controller(format!(
                "already_landed: package {package_id} already landed ({})",
                settled.describe()
            )));
        }
        let now = kernel.now();
        execute(
            &kernel.database,
            "INSERT INTO package_landings(project_id, plan_id, package_id, commit_sha, recorded_by, recorded_at, note)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            [
                kernel.project_id.as_str(),
                plan_id,
                package_id,
                commit_sha,
                actor.actor_id.as_str(),
                now.as_str(),
                note.as_str(),
            ],
        )?;
        let not_landed = !report_landed(kernel, &latest)?;
        let mut body = format!(
            "Package {plan_id}/{package_id} was recorded as landed at {} by the operator: {note}. It counts as integrated for sign-off.",
            sha12(commit_sha)
        );
        if not_landed {
            body.push_str(&format!(" Its latest report {latest} is not integrated."));
        }
        message_notices::notice_to_pm(kernel, "plan_notice", &body, &now, true)?;
        let mut value = json!({
            "planId": plan_id,
            "packageId": package_id,
            "commitSha": commit_sha,
            "recordedAt": now,
        });
        if not_landed {
            value["latestReportNotLanded"] = json!(latest);
        }
        Ok(MutationOutput::new(
            value,
            MutationEvent::new("plan", plan_id, 0)
                .with_details(json!({"packageId": package_id, "commitSha": commit_sha})),
        ))
    })
}

/// `clearPackageLanding`: the operator withdraws a recorded landing.
pub fn clear_package_landing(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let plan_id = safe_id(field(input, "planId"), "plan id")?;
    let package_id = safe_id(field(input, "packageId"), "package id")?;
    kernel.mutate(
        context,
        "plan.landing_cleared",
        "plan:write",
        input,
        |actor| {
            if actor.role != "operator" {
                return Err(KernelError::controller(
                    "only the operator clears a recorded landing",
                ));
            }
            if plans::plan_row(kernel, plan_id)?.is_none() {
                return Err(KernelError::controller(format!(
                    "plan {plan_id} does not exist"
                )));
            }
            if !plan_packages::plan_package_rows(kernel, plan_id)?
                .iter()
                .any(|r| r.package_id == package_id)
            {
                return Err(KernelError::controller(format!(
                    "plan {plan_id} has no package {package_id}"
                )));
            }
            let recorded = query_opt(
                &kernel.database,
                "SELECT 1 AS present FROM package_landings WHERE project_id = ? AND plan_id = ? AND package_id = ?",
                [&kernel.project_id, plan_id, package_id],
                |_| Ok(()),
            )?;
            if recorded.is_none() {
                return Err(KernelError::controller(format!(
                    "no_landing: package {package_id} has no recorded landing"
                )));
            }
            if signed_off(kernel, plan_id)? {
                return Err(KernelError::controller(format!(
                    "plan_signed_off: plan {plan_id} was signed off; its landings are final"
                )));
            }
            execute(
                &kernel.database,
                "DELETE FROM package_landings WHERE project_id = ? AND plan_id = ? AND package_id = ?",
                [&kernel.project_id, plan_id, package_id],
            )?;
            let now = kernel.now();
            message_notices::notice_to_pm(
                kernel,
                "plan_notice",
                &format!("The recorded landing of {plan_id}/{package_id} was cleared by the operator."),
                &now,
                true,
            )?;
            Ok(MutationOutput::new(
                json!({"planId": plan_id, "packageId": package_id}),
                MutationEvent::new("plan", plan_id, 0)
                    .with_details(json!({"packageId": package_id})),
            ))
        },
    )
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "recordPackageLanding" => {
            let context = arg!(a.ctx(0));
            Some(call(record_package_landing(kernel, &context, a.value(1))))
        }
        "clearPackageLanding" => {
            let context = arg!(a.ctx(0));
            Some(call(clear_package_landing(kernel, &context, a.value(1))))
        }
        _ => None,
    }
}
