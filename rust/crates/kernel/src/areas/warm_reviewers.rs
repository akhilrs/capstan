//! Warm reviewers (migration 0043): the Verifier that keeps reviewing one plan's reports and integrations. The ledger records
//! which reviewer is warm for which plan and how many reviews it gave; the daemon decides when to keep or drop it.

use crate::dispatch::{arg, call, Args};
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{execute, query_all, query_opt, safe_id_str};
use crate::kernel::Kernel;
use crate::records::{AgentReportRow, AgentRow, MutationEvent, MutationOutput};
use crate::types::MutationContext;
use serde_json::{json, Value};
use std::collections::BTreeSet;

use super::{plan_packages, reviews};

/// Why a warm row is released.
const RELEASE_REASONS: [&str; 7] = [
    "signoff",
    "cancel",
    "idle",
    "max_reviews",
    "evicted",
    "retarget_failed",
    "lost",
];

/// Longest replacement chain followed in either direction.
const MAX_CHAIN: usize = 64;

/// An unreleased warm row.
#[derive(Clone, Debug)]
pub struct WarmRow {
    pub plan_id: String,
    pub reviewer_role: String,
    pub agent_id: String,
    pub reviews_count: i64,
    pub last_used_at: String,
}

fn warm_row_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<WarmRow> {
    Ok(WarmRow {
        plan_id: row.get("plan_id")?,
        reviewer_role: row.get("reviewer_role")?,
        agent_id: row.get("agent_id")?,
        reviews_count: row.get("reviews_count")?,
        last_used_at: row.get("last_used_at")?,
    })
}

/// The unreleased warm row of a plan.
pub fn warm_of_plan(kernel: &Kernel, plan_id: &str) -> KernelResult<Option<WarmRow>> {
    query_opt(
        &kernel.database,
        "SELECT * FROM warm_reviewers WHERE project_id = ? AND plan_id = ? AND released_at IS NULL",
        [&kernel.project_id, plan_id],
        warm_row_of,
    )
}

/// The unreleased warm row an agent holds.
pub fn warm_of_agent(kernel: &Kernel, agent_id: &str) -> KernelResult<Option<WarmRow>> {
    query_opt(
        &kernel.database,
        "SELECT * FROM warm_reviewers WHERE project_id = ? AND agent_id = ? AND released_at IS NULL",
        [&kernel.project_id, agent_id],
        warm_row_of,
    )
}

/// The plan a review subject belongs to and, for a report, its package: a report maps to the package whose current
/// assignee reported it; an integration maps to the plan when every merged report belongs to packages of one plan. A plan
/// subject, an unknown subject and a mixed integration map to nothing.
pub fn plan_of_subject(
    kernel: &Kernel,
    subject_id: &str,
) -> KernelResult<Option<(String, Option<String>)>> {
    let report = query_opt(
        &kernel.database,
        "SELECT * FROM agent_reports WHERE project_id = ? AND report_id = ?",
        [&kernel.project_id, subject_id],
        AgentReportRow::from_row,
    )?;
    if let Some(report) = report {
        return Ok(plan_packages::package_of_report(
            kernel,
            &report.report_id,
            &report.agent_id,
            false,
        )?
        .map(|(plan_id, package_id)| (plan_id, Some(package_id))));
    }
    let merged = reviews::integration_authors(kernel, subject_id)?;
    let mut plan: Option<String> = None;
    for report in &merged {
        let Some((plan_id, _)) =
            plan_packages::package_of_report(kernel, &report.report_id, &report.agent_id, false)?
        else {
            return Ok(None);
        };
        match &plan {
            Some(known) if *known != plan_id => return Ok(None),
            _ => plan = Some(plan_id),
        }
    }
    Ok(plan.map(|plan_id| (plan_id, None)))
}

/// `reviewPlanOf`.
pub fn review_plan_of(kernel: &Kernel, subject_id: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    safe_id_str(subject_id, "report or integration id")?;
    Ok(match plan_of_subject(kernel, subject_id)? {
        Some((plan_id, package_id)) => json!({"planId": plan_id, "packageId": package_id}),
        None => Value::Null,
    })
}

/// Every agent joined to `start` by a recorded replacement, in either direction.
fn replacement_family(kernel: &Kernel, start: &str) -> KernelResult<BTreeSet<String>> {
    let mut family: BTreeSet<String> = BTreeSet::from([start.to_string()]);
    let mut frontier = vec![start.to_string()];
    for _ in 0..MAX_CHAIN {
        let Some(agent) = frontier.pop() else { break };
        let pairs = query_all(
            &kernel.database,
            "SELECT entity_id, json_extract(payload_json, '$.details.successorId') FROM controller_events
             WHERE project_id = ?1 AND entity_type = 'agent' AND to_state = 'replaced'
               AND (entity_id = ?2 OR json_extract(payload_json, '$.details.successorId') = ?2)",
            [&kernel.project_id, &agent],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )?;
        for (predecessor, successor) in pairs {
            for id in std::iter::once(predecessor).chain(successor) {
                if family.insert(id.clone()) {
                    frontier.push(id);
                }
            }
        }
    }
    Ok(family)
}

/// True when `reviewer` is one of `authors` (agent id or actor id), holds or ever held an author's actor in any generation
/// of its seat, or is a predecessor or successor of an author in the recorded replacements.
pub fn is_author(
    kernel: &Kernel,
    reviewer: &AgentRow,
    authors: &[(String, String)],
) -> KernelResult<bool> {
    if authors.iter().any(|(agent_id, actor_id)| {
        reviewer.actor_id == *actor_id || reviewer.agent_id == *agent_id
    }) {
        return Ok(true);
    }
    let seat_actors: BTreeSet<String> = query_all(
        &kernel.database,
        "SELECT actor_id FROM actors WHERE project_id = ? AND seat_id = ?",
        [&kernel.project_id, &reviewer.seat_id],
        |row| row.get::<_, String>(0),
    )?
    .into_iter()
    .collect();
    if authors
        .iter()
        .any(|(_, actor_id)| seat_actors.contains(actor_id))
    {
        return Ok(true);
    }
    for (agent_id, _) in authors {
        if replacement_family(kernel, agent_id)?.contains(&reviewer.agent_id) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The `(agent id, actor id)` of the authors of a report or an integration; empty for another subject.
fn authors_of(kernel: &Kernel, subject_id: &str) -> KernelResult<Vec<(String, String)>> {
    let report = query_opt(
        &kernel.database,
        "SELECT * FROM agent_reports WHERE project_id = ? AND report_id = ?",
        [&kernel.project_id, subject_id],
        AgentReportRow::from_row,
    )?;
    let rows = match report {
        Some(report) => vec![report],
        None => reviews::integration_authors(kernel, subject_id)?,
    };
    Ok(rows.into_iter().map(|r| (r.agent_id, r.actor_id)).collect())
}

fn has_started_review(kernel: &Kernel, agent_id: &str) -> KernelResult<bool> {
    Ok(query_opt(
        &kernel.database,
        "SELECT 1 AS present FROM reviews WHERE project_id = ? AND reviewer_agent_id = ? AND state = 'started'",
        [&kernel.project_id, agent_id],
        |_| Ok(()),
    )?
    .is_some())
}

/// When the reviewer last gave a verdict, from the review records (so it survives a daemon restart).
fn last_verdict_at(kernel: &Kernel, agent_id: &str) -> KernelResult<Option<String>> {
    Ok(query_opt(
        &kernel.database,
        "SELECT MAX(completed_at) AS at FROM reviews
         WHERE project_id = ? AND reviewer_agent_id = ? AND state IN ('passed', 'findings')",
        [&kernel.project_id, agent_id],
        |row| row.get::<_, Option<String>>("at"),
    )?
    .flatten())
}

/// `warmReviewerFor`: the plan's warm reviewer when it can take the subject.
pub fn warm_reviewer_for(
    kernel: &Kernel,
    subject_id: &str,
    reviewer_role: &str,
    max_reviews: i64,
) -> KernelResult<Value> {
    kernel.assert_open()?;
    safe_id_str(subject_id, "report or integration id")?;
    safe_id_str(reviewer_role, "reviewer role")?;
    let answer =
        |reason: &str, agent_id: Option<&str>| json!({"agentId": agent_id, "reason": reason});
    let Some((plan_id, _)) = plan_of_subject(kernel, subject_id)? else {
        return Ok(answer("none", None));
    };
    let Some(warm) = warm_of_plan(kernel, &plan_id)? else {
        return Ok(answer("none", None));
    };
    let agent = kernel.agent_row(&warm.agent_id)?;
    let Some(agent) = agent
        .filter(|a| a.state == "active" && a.kind == "Verifier" && a.role_name == reviewer_role)
    else {
        return Ok(answer("inactive", None));
    };
    if is_author(kernel, &agent, &authors_of(kernel, subject_id)?)? {
        return Ok(answer("author", None));
    }
    if has_started_review(kernel, &agent.agent_id)? {
        return Ok(answer("busy", None));
    }
    if warm.reviews_count >= max_reviews {
        return Ok(answer("exhausted", None));
    }
    Ok(answer("ok", Some(&agent.agent_id)))
}

/// Records that `reviewer` gives a review for `plan_id`. Returns false (and writes nothing) when the plan already has
/// another warm reviewer or the reviewer is warm for another plan, so that review stays one-shot. The caller owns the
/// transaction.
pub fn hold_for_review(
    kernel: &Kernel,
    plan_id: &str,
    reviewer: &AgentRow,
    now: &str,
) -> KernelResult<bool> {
    match warm_of_plan(kernel, plan_id)? {
        Some(warm) if warm.agent_id == reviewer.agent_id => {
            execute(
                &kernel.database,
                "UPDATE warm_reviewers SET reviews_count = reviews_count + 1, last_used_at = ?
                 WHERE project_id = ? AND plan_id = ? AND agent_id = ? AND released_at IS NULL",
                [now, &kernel.project_id, plan_id, &reviewer.agent_id],
            )?;
            Ok(true)
        }
        Some(_) => Ok(false),
        None => {
            if warm_of_agent(kernel, &reviewer.agent_id)?.is_some() {
                return Ok(false);
            }
            execute(
                &kernel.database,
                "INSERT INTO warm_reviewers(project_id, plan_id, reviewer_role, agent_id, reviews_count, created_at, last_used_at)
                 VALUES (?, ?, ?, ?, 1, ?, ?)",
                [
                    &kernel.project_id,
                    plan_id,
                    &reviewer.role_name,
                    &reviewer.agent_id,
                    now,
                    now,
                ],
            )?;
            Ok(true)
        }
    }
}

/// Releases the plan's unreleased warm row; the released agent's id, if there was one. The caller owns the transaction.
pub fn release_plan(
    kernel: &Kernel,
    plan_id: &str,
    reason: &str,
    now: &str,
) -> KernelResult<Option<String>> {
    let Some(warm) = warm_of_plan(kernel, plan_id)? else {
        return Ok(None);
    };
    release_row(kernel, &warm, reason, now)?;
    Ok(Some(warm.agent_id))
}

fn release_row(kernel: &Kernel, warm: &WarmRow, reason: &str, now: &str) -> KernelResult<()> {
    execute(
        &kernel.database,
        "UPDATE warm_reviewers SET released_at = ?, release_reason = ?
         WHERE project_id = ? AND plan_id = ? AND agent_id = ? AND released_at IS NULL",
        [
            now,
            reason,
            &kernel.project_id,
            &warm.plan_id,
            &warm.agent_id,
        ],
    )?;
    Ok(())
}

/// `releaseWarmReviewer`: `{agentId, reason, onlyIfIdle}`. Idempotent. With `onlyIfIdle` it does nothing when the reviewer
/// has a started review, checked inside the transaction.
pub fn release_warm_reviewer(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let agent_id = match input.get("agentId").and_then(Value::as_str) {
        Some(id) => safe_id_str(id, "agent id")?,
        None => {
            return Err(KernelError::type_error(
                "agent id must be 1-128 safe ASCII characters",
            ))
        }
    };
    let reason = match input.get("reason").and_then(Value::as_str) {
        Some(reason) if RELEASE_REASONS.contains(&reason) => reason,
        _ => {
            return Err(KernelError::type_error(
                "the release reason is not one of the known reasons",
            ))
        }
    };
    let only_if_idle = input
        .get("onlyIfIdle")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    kernel.mutate(
        context,
        "review.warm_release",
        "controller:reconcile",
        input,
        |_| {
            let mut plan_id = None;
            if let Some(warm) = warm_of_agent(kernel, agent_id)? {
                if !(only_if_idle && has_started_review(kernel, agent_id)?) {
                    release_row(kernel, &warm, reason, &kernel.now())?;
                    plan_id = Some(warm.plan_id);
                }
            }
            Ok(MutationOutput::new(
                json!({"released": plan_id.is_some(), "planId": plan_id}),
                MutationEvent::new("review", agent_id, 0).with_details(
                    json!({"released": plan_id.is_some(), "planId": plan_id, "reason": reason}),
                ),
            ))
        },
    )
}

/// `warmReviewers`: the unreleased rows, least recently used first.
pub fn warm_reviewers(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    kernel.authorize(credential, "controller:reconcile")?;
    let rows = query_all(
        &kernel.database,
        "SELECT * FROM warm_reviewers WHERE project_id = ? AND released_at IS NULL ORDER BY last_used_at, plan_id",
        [&kernel.project_id],
        warm_row_of,
    )?;
    let mut out = Vec::new();
    for row in rows {
        out.push(json!({
            "planId": row.plan_id,
            "agentId": row.agent_id,
            "reviewerRole": row.reviewer_role,
            "reviewsCount": row.reviews_count,
            "lastUsedAt": row.last_used_at,
            "busy": has_started_review(kernel, &row.agent_id)?,
            "lastVerdictAt": last_verdict_at(kernel, &row.agent_id)?,
        }));
    }
    Ok(Value::Array(out))
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "reviewPlanOf" => {
            let subject_id = arg!(a.str(0, "subjectId"));
            Some(call(review_plan_of(kernel, subject_id)))
        }
        "warmReviewerFor" => {
            let subject_id = arg!(a.str(0, "subjectId"));
            let reviewer_role = arg!(a.str(1, "reviewerRole"));
            let max_reviews = arg!(a.opt_i64(2, "maxReviews")).unwrap_or(i64::MAX);
            Some(call(warm_reviewer_for(
                kernel,
                subject_id,
                reviewer_role,
                max_reviews,
            )))
        }
        "releaseWarmReviewer" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(release_warm_reviewer(kernel, &context, input)))
        }
        "warmReviewers" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(warm_reviewers(kernel, credential)))
        }
        _ => None,
    }
}
