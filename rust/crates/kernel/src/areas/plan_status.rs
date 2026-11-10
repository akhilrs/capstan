//! The status of every plan and why a plan is stale, read in batches: one statement for the statuses, a fixed number for
//! all candidate plans at once, then a few per approved or draft candidate. `plan_guards` is the one answer to "is this
//! plan busy", shared with the archive operations.

use crate::dispatch::{arg, call, Args};
use crate::errors::{KernelError, KernelResult};
use crate::helpers::query_all;
use crate::kernel::Kernel;
use crate::records::PlanRow;
use serde_json::{json, Value};
use std::collections::BTreeMap;

use super::plan_landing::{package_landings, Landing, PACKAGE_REPORT_PREDICATE};
use super::{plan_packages, plans};
use capstan_ledger::iso_from_millis;

const MAX_STALE_DAYS: i64 = 3650;
const DAY_MS: i64 = 86_400_000;

/// Every plan in sequence order with its sign-off and archive rows (`p.*` for the plan row, then the extras).
pub const PLAN_STATUSES_SQL: &str = "SELECT p.*,
       EXISTS (SELECT 1 FROM plan_signoffs s WHERE s.project_id = p.project_id AND s.plan_id = p.plan_id) AS signed_off,
       a.archived_at, a.archived_by, a.reason AS archive_reason
FROM plans p LEFT JOIN plan_archives a ON a.project_id = p.project_id AND a.plan_id = p.plan_id
WHERE p.project_id = ? ORDER BY p.sequence";

/// The latest activity per plan among `json_each(?2)`: the plan, its revisions, the reviews of it, and for an approved plan
/// its packages (assignment, cancellation) and their reports.
pub const LAST_ACTIVITY_SQL: &str = "SELECT plan_id, MAX(at) FROM (
  SELECT p.plan_id AS plan_id, p.updated_at AS at FROM plans p
    WHERE p.project_id = ?1 AND p.plan_id IN (SELECT value FROM json_each(?2))
  UNION ALL
  SELECT plan_id, created_at FROM plan_revisions
    WHERE project_id = ?1 AND plan_id IN (SELECT value FROM json_each(?2))
  UNION ALL
  SELECT subject_plan_id, created_at FROM reviews
    WHERE project_id = ?1 AND subject_plan_id IN (SELECT value FROM json_each(?2))
  UNION ALL
  SELECT subject_plan_id, completed_at FROM reviews
    WHERE project_id = ?1 AND completed_at IS NOT NULL AND subject_plan_id IN (SELECT value FROM json_each(?2))
  UNION ALL
  SELECT pp.plan_id, pp.assigned_at FROM plan_packages pp JOIN plans p ON p.project_id = pp.project_id AND p.plan_id = pp.plan_id
    WHERE pp.project_id = ?1 AND p.state = 'approved' AND pp.assigned_at IS NOT NULL AND pp.plan_id IN (SELECT value FROM json_each(?2))
  UNION ALL
  SELECT pp.plan_id, pp.cancelled_at FROM plan_packages pp JOIN plans p ON p.project_id = pp.project_id AND p.plan_id = pp.plan_id
    WHERE pp.project_id = ?1 AND p.state = 'approved' AND pp.cancelled_at IS NOT NULL AND pp.plan_id IN (SELECT value FROM json_each(?2))
  UNION ALL
  SELECT pp.plan_id, r.created_at FROM plan_packages pp
    JOIN plans p ON p.project_id = pp.project_id AND p.plan_id = pp.plan_id
    JOIN agent_reports r ON r.project_id = pp.project_id
    WHERE pp.project_id = ?1 AND p.state = 'approved' AND pp.plan_id IN (SELECT value FROM json_each(?2))
      AND PACKAGE_REPORT_PREDICATE
) WHERE at IS NOT NULL GROUP BY plan_id";

/// The earliest started review of each plan among `json_each(?2)`.
pub const STARTED_REVIEWS_SQL: &str = "SELECT subject_plan_id, review_id FROM reviews
WHERE project_id = ?1 AND state = 'started' AND subject_plan_id IN (SELECT value FROM json_each(?2))
ORDER BY sequence";

/// The earliest running or merged integration holding an accepted report of a package of each plan among `json_each(?2)`.
pub const INTEGRATION_GUARD_SQL: &str = "SELECT pp.plan_id, i.integration_id FROM plan_packages pp
JOIN agent_reports r ON r.project_id = pp.project_id
JOIN integration_reports ir ON ir.project_id = r.project_id AND ir.report_id = r.report_id
JOIN integrations i ON i.project_id = ir.project_id AND i.integration_id = ir.integration_id
WHERE pp.project_id = ?1 AND pp.plan_id IN (SELECT value FROM json_each(?2)) AND i.state IN ('running', 'merged')
  AND PACKAGE_REPORT_PREDICATE
ORDER BY i.sequence";

/// A live plan (not archived, cancelled or superseded) naming each plan among `json_each(?2)` as the one it supersedes.
pub const SUPERSEDING_SQL: &str = "SELECT s.supersedes_plan_id, s.plan_id FROM plans s
WHERE s.project_id = ?1 AND s.supersedes_plan_id IN (SELECT value FROM json_each(?2))
  AND s.cancelled_at IS NULL AND s.state <> 'superseded'
  AND NOT EXISTS (SELECT 1 FROM plan_archives a WHERE a.project_id = s.project_id AND a.plan_id = s.plan_id)
ORDER BY s.sequence";

fn with_predicate(sql: &str) -> String {
    sql.replace("PACKAGE_REPORT_PREDICATE", PACKAGE_REPORT_PREDICATE)
}

/// The first row per key of a two-column query over the plan ids.
fn first_per_plan(
    kernel: &Kernel,
    sql: &str,
    ids_json: &str,
) -> KernelResult<BTreeMap<String, String>> {
    let rows = query_all(
        &kernel.database,
        &with_predicate(sql),
        rusqlite::params![kernel.project_id, ids_json],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    let mut out = BTreeMap::new();
    for (plan, id) in rows {
        out.entry(plan).or_insert(id);
    }
    Ok(out)
}

/// The batched guards of a set of plans: started review, running or merged integration, live superseding plan.
struct Batched {
    review: BTreeMap<String, String>,
    integration: BTreeMap<String, String>,
    superseded_by: BTreeMap<String, String>,
}

fn batched(kernel: &Kernel, ids: &[String]) -> KernelResult<Batched> {
    let ids_json = serde_json::to_string(ids)?;
    Ok(Batched {
        review: first_per_plan(kernel, STARTED_REVIEWS_SQL, &ids_json)?,
        integration: first_per_plan(kernel, INTEGRATION_GUARD_SQL, &ids_json)?,
        superseded_by: first_per_plan(kernel, SUPERSEDING_SQL, &ids_json)?,
    })
}

/// The landings of every package of an approved plan, one query for the rows and one per package.
struct Packages {
    rows: Vec<plan_packages::PackageRow>,
    landings: Vec<Vec<Landing>>,
}

fn load_packages(kernel: &Kernel, plan_id: &str) -> KernelResult<Packages> {
    let rows = plan_packages::plan_package_rows(kernel, plan_id)?;
    let landings = rows
        .iter()
        .map(|row| package_landings(kernel, plan_id, &row.package_id))
        .collect::<KernelResult<Vec<_>>>()?;
    Ok(Packages { rows, landings })
}

impl Packages {
    /// The first package that has an assignee and is neither cancelled nor settled-landed.
    fn live(&self) -> Option<&str> {
        self.rows
            .iter()
            .zip(&self.landings)
            .find(|(row, landings)| {
                row.assignee_agent_id.is_some()
                    && row.cancelled_at.is_none()
                    && !landings.iter().any(Landing::settled)
            })
            .map(|(row, _)| row.package_id.as_str())
    }

    /// Every package is cancelled or settled-landed (and there is one).
    fn done(&self) -> bool {
        !self.rows.is_empty()
            && self.rows.iter().zip(&self.landings).all(|(row, landings)| {
                row.cancelled_at.is_some() || landings.iter().any(Landing::settled)
            })
    }
}

/// The first guard in the order review, package, integration, superseded_by.
fn first_guard(batched: &Batched, plan_id: &str, packages: Option<&Packages>) -> Option<String> {
    if let Some(id) = batched.review.get(plan_id) {
        return Some(format!("review {id}"));
    }
    if let Some(id) = packages.and_then(Packages::live) {
        return Some(format!("package {id}"));
    }
    if let Some(id) = batched.integration.get(plan_id) {
        return Some(format!("integration {id}"));
    }
    batched
        .superseded_by
        .get(plan_id)
        .map(|id| format!("superseded_by {id}"))
}

/// For each busy plan among `plan_ids` the first guard as `<kind> <id>`; a plan that is not busy is absent. Stale
/// reporting and the archive operations both call it, so they agree on what busy means (the archive operations of the
/// next package are its caller; `plan_statuses` evaluates the same `first_guard` over the same batched statements).
pub(crate) fn plan_guards(
    kernel: &Kernel,
    plan_ids: &[String],
) -> KernelResult<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    if plan_ids.is_empty() {
        return Ok(out);
    }
    let batched = batched(kernel, plan_ids)?;
    for id in plan_ids {
        let packages = if batched.review.contains_key(id) {
            None
        } else {
            Some(load_packages(kernel, id)?)
        };
        if let Some(guard) = first_guard(&batched, id, packages.as_ref()) {
            out.insert(id.clone(), guard);
        }
    }
    Ok(out)
}

struct Entry {
    plan: PlanRow,
    status: &'static str,
    archive: Option<(String, String, String)>,
    signed_off: bool,
}

fn status_of(plan: &PlanRow, signed_off: bool, archived: bool) -> &'static str {
    if archived {
        "archived"
    } else if plan.cancelled_at.is_some() {
        "cancelled"
    } else if plan.state == "superseded" {
        "superseded"
    } else if signed_off && plan.state == "approved" {
        "signed_off"
    } else {
        match plan.state.as_str() {
            "approved" => "approved",
            "in_review" => "in_review",
            _ => "draft",
        }
    }
}

/// `planStatuses`: `[{planId, status, lastActivityAt, stale?, archived?}]` in plan sequence order.
pub fn plan_statuses(kernel: &Kernel, credential: &str, input: &Value) -> KernelResult<Value> {
    kernel.authorize(credential, "plan:read")?;
    let days = match input.get("staleAfterDays") {
        Some(Value::Number(n)) => n.as_i64().filter(|d| (0..=MAX_STALE_DAYS).contains(d)),
        _ => None,
    }
    .ok_or_else(|| {
        KernelError::type_error(format!(
            "staleAfterDays must be an integer from 0 to {MAX_STALE_DAYS}"
        ))
    })?;
    let with_stale = match input.get("withStale") {
        Some(Value::Bool(flag)) => *flag,
        _ => return Err(KernelError::type_error("withStale must be a boolean")),
    };
    let entries = query_all(
        &kernel.database,
        PLAN_STATUSES_SQL,
        [&kernel.project_id],
        |row| {
            let plan = PlanRow::from_row(row)?;
            let signed_off: bool = row.get("signed_off")?;
            let archived_at: Option<String> = row.get("archived_at")?;
            let archive = match archived_at {
                Some(at) => Some((at, row.get("archived_by")?, row.get("archive_reason")?)),
                None => None,
            };
            Ok(Entry {
                status: status_of(&plan, signed_off, archive.is_some()),
                plan,
                archive,
                signed_off,
            })
        },
    )?;
    let candidates: Vec<String> = if with_stale {
        entries
            .iter()
            .filter(|e| matches!(e.status, "draft" | "in_review" | "approved"))
            .map(|e| e.plan.plan_id.clone())
            .collect()
    } else {
        Vec::new()
    };
    let mut activity: BTreeMap<String, String> = BTreeMap::new();
    let mut stale: BTreeMap<String, Vec<&'static str>> = BTreeMap::new();
    if !candidates.is_empty() {
        let ids_json = serde_json::to_string(&candidates)?;
        activity = query_all(
            &kernel.database,
            &with_predicate(LAST_ACTIVITY_SQL),
            rusqlite::params![kernel.project_id, ids_json],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )?
        .into_iter()
        .collect();
        let guards = batched(kernel, &candidates)?;
        let threshold = iso_from_millis(kernel.env.now() - days * DAY_MS);
        let by_id: BTreeMap<&str, &Entry> = entries
            .iter()
            .map(|e| (e.plan.plan_id.as_str(), e))
            .collect();
        for id in &candidates {
            let entry = by_id[id.as_str()];
            if guards.review.contains_key(id) {
                continue;
            }
            let packages = if entry.status == "approved" {
                Some(load_packages(kernel, id)?)
            } else {
                None
            };
            if first_guard(&guards, id, packages.as_ref()).is_some() {
                continue;
            }
            let mut reasons = Vec::new();
            if entry.status == "draft" && plans::plan_rounds(kernel, &entry.plan)?.exhausted() {
                reasons.push("review_exhausted");
            }
            let last = activity
                .get(id)
                .map_or(entry.plan.updated_at.as_str(), String::as_str);
            if days > 0 && last < threshold.as_str() {
                reasons.push("idle");
            }
            if packages.as_ref().is_some_and(Packages::done) && !entry.signed_off {
                reasons.push("done_unsigned");
            }
            if !reasons.is_empty() {
                stale.insert(id.clone(), reasons);
            }
        }
    }
    let mut out = Vec::with_capacity(entries.len());
    for entry in &entries {
        let id = &entry.plan.plan_id;
        let last = activity
            .get(id)
            .cloned()
            .unwrap_or_else(|| entry.plan.updated_at.clone());
        let mut item = json!({"planId": id, "status": entry.status, "lastActivityAt": last});
        if let Some(reasons) = stale.get(id) {
            item["stale"] = json!(reasons);
        }
        if let Some((at, by, reason)) = &entry.archive {
            item["archived"] = json!({"archivedAt": at, "archivedBy": by, "reason": reason});
        }
        out.push(item);
    }
    Ok(Value::Array(out))
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "planStatuses" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(plan_statuses(kernel, credential, a.value(1))))
        }
        _ => None,
    }
}
