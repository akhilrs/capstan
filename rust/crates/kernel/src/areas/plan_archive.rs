//! Archiving plans: the operator's statement that plans are no longer followed. The rows are in `plan_archives`; a plan's
//! own state is untouched except that a plan in review goes back to draft. `refuse_if_archived` is the one check every
//! plan mutation makes first.

use crate::dispatch::{arg, call, Args};
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{execute, query_opt, safe_id, safe_text};
use crate::kernel::Kernel;
use crate::records::{MutationEvent, MutationOutput};
use crate::types::MutationContext;
use serde_json::{json, Value};
use std::collections::BTreeSet;

use super::{message_notices, plan_packages, plan_status, plans, warm_reviewers};

const MAX_PLANS: usize = 500;
const NOTICE_IDS: usize = 40;
const EVENT_IDS: usize = 100;

/// Whether the table exists. A read-only open of an old-schema ledger never migrates, so readers that touch a newer table
/// check first.
pub(crate) fn table_present(kernel: &Kernel, name: &str) -> KernelResult<bool> {
    Ok(query_opt(
        &kernel.database,
        "SELECT 1 AS present FROM sqlite_master WHERE type = 'table' AND name = ?",
        [name],
        |row| row.get::<_, i64>(0),
    )?
    .is_some())
}

/// Whether the plan has an archive row.
pub(crate) fn is_archived(kernel: &Kernel, plan_id: &str) -> KernelResult<bool> {
    Ok(query_opt(
        &kernel.database,
        "SELECT 1 AS present FROM plan_archives WHERE project_id = ? AND plan_id = ?",
        [&kernel.project_id, plan_id],
        |row| row.get::<_, i64>(0),
    )?
    .is_some())
}

/// Refuses with `plan_archived` when the plan is archived; a plan that does not exist passes (the caller refuses it).
pub(crate) fn refuse_if_archived(kernel: &Kernel, plan_id: &str) -> KernelResult<()> {
    if is_archived(kernel, plan_id)? {
        return Err(KernelError::controller(format!(
            "plan_archived: plan {plan_id} is archived; run cstan plan unarchive {plan_id} first"
        )));
    }
    Ok(())
}

fn operator_only(role: &str, what: &str) -> KernelResult<()> {
    if role != "operator" {
        return Err(KernelError::controller(format!(
            "only the operator {what} plans"
        )));
    }
    Ok(())
}

/// `archivePlans`: all or nothing; every guard of every id is checked before any write.
pub fn archive_plans(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let ids_value = input
        .get("planIds")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            KernelError::type_error(format!("planIds must be a list of 1-{MAX_PLANS} plan ids"))
        })?;
    if ids_value.is_empty() || ids_value.len() > MAX_PLANS {
        return Err(KernelError::type_error(format!(
            "planIds must be a list of 1-{MAX_PLANS} plan ids"
        )));
    }
    let mut ids: Vec<String> = Vec::new();
    let mut seen = BTreeSet::new();
    for value in ids_value {
        let id = safe_id(value, "plan id")?;
        if !seen.insert(id.to_string()) {
            return Err(KernelError::type_error(format!(
                "planIds lists {id} more than once"
            )));
        }
        ids.push(id.to_string());
    }
    let reason = safe_text(
        input.get("reason").unwrap_or(&Value::Null),
        "reason",
        500,
        false,
    )?;
    kernel.mutate(context, "plan.archive", "plan:write", input, |actor| {
        operator_only(&actor.role, "archives")?;
        let mut refused: Vec<String> = Vec::new();
        let mut candidates: Vec<String> = Vec::new();
        for id in &ids {
            if plans::plan_row(kernel, id)?.is_none() {
                refused.push(format!("{id}: does not exist"));
            } else if is_archived(kernel, id)? {
                refused.push(format!("{id}: already_archived"));
            } else {
                candidates.push(id.clone());
            }
        }
        let busy = plan_status::plan_guards(kernel, &candidates)?;
        let mut why: std::collections::BTreeMap<&str, String> = std::collections::BTreeMap::new();
        for id in &candidates {
            if let Some(guard) = busy.get(id) {
                why.insert(id.as_str(), format!("{id}: plan_busy: {guard}"));
            }
        }
        // Report the refusals in the order the ids were given.
        let mut lines: Vec<String> = Vec::new();
        for id in &ids {
            if let Some(line) = refused.iter().find(|l| l.starts_with(&format!("{id}: "))) {
                lines.push(line.clone());
            } else if let Some(line) = why.get(id.as_str()) {
                lines.push(line.clone());
            }
        }
        if !lines.is_empty() {
            return Err(KernelError::controller(lines.join("; ")));
        }
        let now = kernel.now();
        let mut released: Vec<String> = Vec::new();
        for id in &ids {
            let plan = plans::plan_row(kernel, id)?.expect("checked above");
            execute(
                &kernel.database,
                "INSERT INTO plan_archives (project_id, plan_id, archived_at, archived_by, reason) VALUES (?, ?, ?, ?, ?)",
                [
                    kernel.project_id.as_str(),
                    id.as_str(),
                    now.as_str(),
                    actor.actor_id.as_str(),
                    reason.as_str(),
                ],
            )?;
            if plan.state == "in_review" {
                plan_packages::reset_to_draft(kernel, id, &now)?;
            }
            // `release_reason` has no value for an archive (the column's CHECK is fixed); an archive closes the plan by
            // the operator like a cancel, so it releases with `cancel`.
            if let Some(agent) = warm_reviewers::release_plan(kernel, id, "cancel", &now)? {
                released.push(agent);
            }
        }
        let shown: Vec<&str> = ids.iter().take(NOTICE_IDS).map(String::as_str).collect();
        let mut listed = shown.join(", ");
        if ids.len() > NOTICE_IDS {
            listed.push_str(&format!(", and {} more", ids.len() - NOTICE_IDS));
        }
        message_notices::notice_to_pm(
            kernel,
            "plan_notice",
            &format!(
                "Plans archived by the operator ({}): {listed}. Reason: {reason}. Their history and Nexora links are kept; cstan plan show --archived lists them; cstan plan unarchive <plan-id> restores one.",
                ids.len()
            ),
            &now,
            false,
        )?;
        let mut details = json!({
            "planIds": ids.iter().take(EVENT_IDS).collect::<Vec<_>>(),
            "reason": reason,
        });
        if ids.len() > EVENT_IDS {
            details["more"] = json!(ids.len() - EVENT_IDS);
        }
        let mut value = json!({"archived": ids});
        if !released.is_empty() {
            value["releasedReviewerAgentIds"] = json!(released);
        }
        Ok(MutationOutput::new(
            value,
            MutationEvent::new("plan", &ids[0], 0).with_details(details),
        ))
    })
}

/// `unarchivePlan`: deletes the archive row; the plan stays as archive left it.
pub fn unarchive_plan(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let plan_id = safe_id(input.get("planId").unwrap_or(&Value::Null), "plan id")?;
    kernel.mutate(context, "plan.unarchive", "plan:write", input, |actor| {
        operator_only(&actor.role, "unarchives")?;
        if plans::plan_row(kernel, plan_id)?.is_none() {
            return Err(KernelError::controller(format!(
                "plan {plan_id} does not exist"
            )));
        }
        if !is_archived(kernel, plan_id)? {
            return Err(KernelError::controller(format!(
                "not_archived: plan {plan_id} is not archived"
            )));
        }
        execute(
            &kernel.database,
            "DELETE FROM plan_archives WHERE project_id = ? AND plan_id = ?",
            [kernel.project_id.as_str(), plan_id],
        )?;
        message_notices::notice_to_pm(
            kernel,
            "plan_notice",
            &format!("Plan {plan_id} was unarchived by the operator."),
            &kernel.now(),
            false,
        )?;
        Ok(MutationOutput::new(
            json!({"planId": plan_id}),
            MutationEvent::new("plan", plan_id, 0),
        ))
    })
}

/// The operations of this area, by the `ControllerCore` method name.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "archivePlans" => {
            let context = arg!(a.ctx(0));
            Some(call(archive_plans(kernel, &context, a.value(1))))
        }
        "unarchivePlan" => {
            let context = arg!(a.ctx(0));
            Some(call(unarchive_plan(kernel, &context, a.value(1))))
        }
        _ => None,
    }
}
