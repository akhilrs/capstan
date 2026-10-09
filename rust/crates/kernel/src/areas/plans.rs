//! The plans (src/controller/plans.ts).

use crate::canonical::sha256;
use crate::dispatch::{arg, call, Args};
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{
    execute, plan_approved_notice, plan_body_package_ids, plan_needs_attention_notice,
    plan_record_of, plan_signed_off_notice, query_all, query_opt, safe_id, safe_id_str, safe_text,
};
use crate::kernel::Kernel;
use crate::records::{AgentRow, MutationEvent, MutationOutput, PlanRow, MAX_REVIEW_ROUNDS};
use crate::types::MutationContext;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::sync::LazyLock;

use super::{message_notices, messages, plan_packages};

static SHA1: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9a-f]{40}$").expect("the pattern is valid"));

fn field<'a>(input: &'a Value, key: &str) -> &'a Value {
    input.get(key).unwrap_or(&Value::Null)
}

/// JavaScript truthiness of a JSON value.
fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(text)) => !text.is_empty(),
        Some(_) => true,
    }
}

/// `planRow`.
pub(crate) fn plan_row(kernel: &Kernel, plan_id: &str) -> KernelResult<Option<PlanRow>> {
    query_opt(
        &kernel.database,
        "SELECT * FROM plans WHERE project_id = ? AND plan_id = ?",
        [&kernel.project_id, plan_id],
        PlanRow::from_row,
    )
}

fn plan_value(kernel: &Kernel, plan_id: &str) -> KernelResult<Value> {
    let row = plan_row(kernel, plan_id)?
        .ok_or_else(|| KernelError::controller(format!("plan {plan_id} does not exist")))?;
    Ok(serde_json::to_value(plan_record_of(&row))?)
}

/// `openPlan`: opens a draft plan; only the PM or the operator may.
pub fn open_plan(kernel: &Kernel, context: &MutationContext, input: &Value) -> KernelResult<Value> {
    let tier = match field(input, "tier").as_str() {
        Some(tier @ ("normal" | "high_risk")) => tier,
        _ => {
            return Err(KernelError::type_error(
                "the plan tier must be normal or high_risk",
            ))
        }
    };
    let title = safe_text(field(input, "title"), "plan title", 200, false)?;
    let supersedes: Option<&str> = match input.get("supersedesPlanId") {
        None => None,
        Some(value) => Some(safe_id(value, "superseded plan id")?),
    };
    kernel.mutate(
        context,
        "plan.open",
        "plan:write",
        &json!({"tier": tier, "title": title, "supersedesPlanId": supersedes}),
        |actor| {
            if actor.role != "PM" && actor.role != "operator" {
                return Err(KernelError::controller(
                    "only the PM or the operator opens a plan",
                ));
            }
            if let Some(supersedes) = supersedes {
                let Some(old) = plan_row(kernel, supersedes)? else {
                    return Err(KernelError::controller(format!(
                        "plan {supersedes} does not exist"
                    )));
                };
                if old.cancelled_at.is_some() {
                    return Err(KernelError::controller(format!(
                        "plan_cancelled: {supersedes} was cancelled; open a fresh plan without naming it"
                    )));
                }
                if old.state != "approved" {
                    return Err(KernelError::controller(format!(
                        "plan {supersedes} is {}; only an approved plan can be superseded",
                        old.state
                    )));
                }
            }
            let now = kernel.now();
            let sequence = query_opt(
                &kernel.database,
                "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM plans WHERE project_id = ?",
                [&kernel.project_id],
                |row| row.get::<_, i64>(0),
            )?
            .unwrap_or(1);
            let plan_id = format!("plan-{sequence}");
            execute(
                &kernel.database,
                "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, supersedes_plan_id, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, 'draft', ?, ?, ?, ?)",
                rusqlite::params![
                    kernel.project_id,
                    plan_id,
                    sequence,
                    title,
                    tier,
                    actor.actor_id,
                    supersedes,
                    now,
                    now
                ],
            )?;
            let mut event = MutationEvent::new("plan", &plan_id, 0)
                .with_details(json!({"tier": tier, "supersedesPlanId": supersedes}));
            event.to_state = Some("draft".into());
            Ok(MutationOutput::new(plan_value(kernel, &plan_id)?, event))
        },
    )
}

/// `submitPlan`: stores the next revision of a draft plan, written by an active Developer-kind agent. With `review` the plan
/// moves to in_review, otherwise it is approved at once.
pub fn submit_plan(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let plan_id = safe_id(field(input, "planId"), "plan id")?;
    let base_sha = match field(input, "baseSha").as_str() {
        Some(sha) if SHA1.is_match(sha) => sha,
        _ => {
            return Err(KernelError::type_error(
                "the base commit must be a full lowercase sha1",
            ))
        }
    };
    plan_body_package_ids(field(input, "bodyJson"))?;
    let body_json = field(input, "bodyJson").as_str().unwrap_or_default();
    let review = truthy(input.get("review"));
    let mut payload = Map::new();
    payload.insert("planId".into(), json!(plan_id));
    payload.insert("bodySha".into(), json!(sha256(body_json)));
    payload.insert("baseSha".into(), json!(base_sha));
    if let Some(value) = input.get("review") {
        payload.insert("review".into(), value.clone());
    }
    kernel.mutate(
        context,
        "plan.submit",
        "plan:write",
        &Value::Object(payload),
        |actor| {
            let agent = match kernel.agent_by_actor(&actor.actor_id)? {
                Some(agent) if agent.kind == "Developer" => agent,
                _ => {
                    return Err(KernelError::controller(
                        "only an active developer-kind agent submits a plan",
                    ))
                }
            };
            let Some(plan) = plan_row(kernel, plan_id)? else {
                return Err(KernelError::controller(format!("plan {plan_id} does not exist")));
            };
            if plan.cancelled_at.is_some() {
                return Err(KernelError::controller(format!(
                    "plan_cancelled: plan {plan_id} was cancelled"
                )));
            }
            if plan.state != "draft" {
                return Err(KernelError::controller(format!(
                    "plan {plan_id} is {}, not a draft",
                    plan.state
                )));
            }
            if let Some(architect) = &plan.architect_agent_id {
                if *architect != agent.agent_id {
                    return Err(KernelError::controller(format!(
                        "plan {plan_id} belongs to the architect {architect}"
                    )));
                }
            }
            let now = kernel.now();
            let revision = plan.current_revision + 1;
            execute(
                &kernel.database,
                "INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    kernel.project_id,
                    plan_id,
                    revision,
                    base_sha,
                    body_json,
                    sha256(body_json),
                    agent.agent_id,
                    actor.actor_id,
                    now
                ],
            )?;
            execute(
                &kernel.database,
                "UPDATE plans SET current_revision = ?, architect_agent_id = ?, state = ?, approved_revision = ?, updated_at = ?
                 WHERE project_id = ? AND plan_id = ?",
                rusqlite::params![
                    revision,
                    agent.agent_id,
                    if review { "in_review" } else { "approved" },
                    if review { None } else { Some(revision) },
                    now,
                    kernel.project_id,
                    plan_id
                ],
            )?;
            if !review {
                settle_plan_approval(kernel, &plan, body_json, &now)?;
            }
            let event = MutationEvent::new("plan", plan_id, 0)
                .transition("draft", if review { "in_review" } else { "approved" })
                .with_details(json!({"revision": revision, "agentId": agent.agent_id}));
            Ok(MutationOutput::new(plan_value(kernel, plan_id)?, event))
        },
    )
}

/// On approval: one package row per package of the approved body, and the superseded plan is retired. The caller owns the
/// transaction and has already moved the plan to approved.
pub(crate) fn settle_plan_approval(
    kernel: &Kernel,
    plan: &PlanRow,
    body_json: &str,
    now: &str,
) -> KernelResult<()> {
    for package_id in plan_body_package_ids(&json!(body_json))? {
        execute(
            &kernel.database,
            "INSERT INTO plan_packages(project_id, plan_id, package_id) VALUES (?, ?, ?)",
            [&kernel.project_id, &plan.plan_id, &package_id],
        )?;
    }
    if let Some(superseded) = &plan.supersedes_plan_id {
        let old = plan_row(kernel, superseded)?;
        if old.as_ref().is_none_or(|old| old.cancelled_at.is_none()) {
            if old.as_ref().map(|old| old.state.as_str()) != Some("approved") {
                return Err(KernelError::controller(format!(
                    "plan {superseded} is no longer approved and cannot be superseded"
                )));
            }
            execute(
                &kernel.database,
                "UPDATE plans SET state = 'superseded', updated_at = ? WHERE project_id = ? AND plan_id = ?",
                [now, &kernel.project_id, superseded],
            )?;
        }
    }
    let note = approval_note(kernel, plan)?;
    message_notices::notice_to_pm(
        kernel,
        &plan_approved_notice(&plan.plan_id, body_json, note.as_deref())?,
        now,
        true,
    )?;
    Ok(())
}

/// A notice counts as sent when a controller message with its leading text exists.
fn notice_sent(kernel: &Kernel, plan_id: &str, lead: &str) -> KernelResult<bool> {
    let text = format!("Plan {plan_id} {lead}");
    Ok(query_opt(
        &kernel.database,
        "SELECT 1 AS present FROM messages WHERE project_id = ? AND substr(body, 1, ?) = ?",
        rusqlite::params![kernel.project_id, text.encode_utf16().count() as i64, text],
        |_| Ok(()),
    )?
    .is_some())
}

/// `unannouncedPlanNotices`: plan notices the PM has not received.
pub fn unannounced_plan_notices(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    kernel.authorize(credential, "controller:reconcile")?;
    let mut out: Vec<Value> = Vec::new();
    let plans = query_all(
        &kernel.database,
        "SELECT * FROM plans WHERE project_id = ? AND state IN ('approved', 'draft') ORDER BY sequence",
        [&kernel.project_id],
        PlanRow::from_row,
    )?;
    for plan in plans {
        if plan.cancelled_at.is_some() {
            continue;
        }
        if plan.state == "approved" {
            if !notice_sent(kernel, &plan.plan_id, "approved")? {
                out.push(json!({"planId": plan.plan_id, "kind": "approved"}));
            }
        } else if finished_plan_reviews(kernel, &plan.plan_id)? >= MAX_REVIEW_ROUNDS
            && !notice_sent(kernel, &plan.plan_id, "needs attention")?
        {
            out.push(json!({"planId": plan.plan_id, "kind": "needs_attention"}));
        }
    }
    let signoffs = query_all(
        &kernel.database,
        "SELECT s.plan_id, s.integration_id FROM plan_signoffs s JOIN plans p
           ON p.project_id = s.project_id AND p.plan_id = s.plan_id
         WHERE s.project_id = ? AND p.cancelled_at IS NULL ORDER BY p.sequence, s.created_at, s.integration_id",
        [&kernel.project_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    for (plan_id, integration_id) in signoffs {
        if !notice_sent(
            kernel,
            &plan_id,
            &format!("signed off. Integration {integration_id} "),
        )? {
            out.push(
                json!({"planId": plan_id, "kind": "signed_off", "integrationId": integration_id}),
            );
        }
    }
    Ok(Value::Array(out))
}

/// `announcePlanNotice`: queues one missing plan notice to the PM; false when no PM is the sole active one.
pub fn announce_plan_notice(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let plan_id = safe_id(field(input, "planId"), "plan id")?;
    let kind = field(input, "kind").as_str().unwrap_or_default();
    if kind == "signed_off" {
        safe_id(field(input, "integrationId"), "integration id")?;
    }
    kernel.mutate(
        context,
        "plan.announce",
        "controller:reconcile",
        input,
        |_| {
            let Some(plan) = plan_row(kernel, plan_id)? else {
                return Err(KernelError::controller(format!("plan {plan_id} does not exist")));
            };
            if plan.cancelled_at.is_some() {
                return Ok(MutationOutput::new(
                    json!({"announced": false}),
                    MutationEvent::new("plan", plan_id, 0)
                        .with_details(json!({"announced": false, "kind": field(input, "kind")})),
                ));
            }
            let body = if kind == "approved" {
                let revision = query_opt(
                    &kernel.database,
                    "SELECT body_json FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
                    rusqlite::params![kernel.project_id, plan.plan_id, plan.approved_revision],
                    |row| row.get::<_, String>(0),
                )?;
                let revision = match revision {
                    Some(revision) if plan.state == "approved" => revision,
                    _ => {
                        return Err(KernelError::controller(format!(
                            "plan {plan_id} is not approved"
                        )))
                    }
                };
                let note = approval_note(kernel, &plan)?;
                plan_approved_notice(&plan.plan_id, &revision, note.as_deref())?
            } else if kind == "signed_off" {
                let integration_id = field(input, "integrationId").as_str().unwrap_or_default();
                let signoff = query_opt(
                    &kernel.database,
                    "SELECT s.summary, i.branch, i.head_sha, i.state FROM plan_signoffs s JOIN integrations i
                       ON i.project_id = s.project_id AND i.integration_id = s.integration_id
                     WHERE s.project_id = ? AND s.plan_id = ? AND s.integration_id = ?",
                    [&kernel.project_id, &plan.plan_id, integration_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )?;
                let Some((summary, branch, head_sha, state)) = signoff else {
                    return Err(KernelError::controller(format!(
                        "plan {plan_id} is not signed off for integration {integration_id}"
                    )));
                };
                plan_signed_off_notice(
                    &plan.plan_id,
                    integration_id,
                    &branch,
                    head_sha.as_deref(),
                    &summary,
                    state == "confirmed",
                    &plan_packages::signoff_extra_reports(kernel, &plan.plan_id, integration_id)?,
                )
            } else {
                plan_needs_attention_notice(&plan.plan_id)
            };
            let announced = message_notices::notice_to_pm(kernel, &body, &kernel.now(), true)?;
            Ok(MutationOutput::new(
                json!({"announced": announced}),
                MutationEvent::new("plan", plan_id, 0)
                    .with_details(json!({"announced": announced, "kind": field(input, "kind")})),
            ))
        },
    )
}

/// `planReviewRounds`: review rounds of a plan that finished with a verdict.
pub fn plan_review_rounds(kernel: &Kernel, credential: &str, plan_id: &str) -> KernelResult<Value> {
    kernel.authorize(credential, "plan:read")?;
    safe_id_str(plan_id, "plan id")?;
    Ok(json!(finished_plan_reviews(kernel, plan_id)?))
}

pub(crate) fn finished_plan_reviews(kernel: &Kernel, plan_id: &str) -> KernelResult<i64> {
    Ok(query_opt(
        &kernel.database,
        "SELECT COUNT(*) AS n FROM reviews WHERE project_id = ? AND subject_plan_id = ? AND state IN ('passed', 'findings')",
        [&kernel.project_id, plan_id],
        |row| row.get::<_, i64>(0),
    )?
    .unwrap_or(0))
}

/// `abandonPlanReview`: a plan review that could not start; the plan goes back to draft and its architect is told why.
pub fn abandon_plan_review(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let plan_id = safe_id(field(input, "planId"), "plan id")?;
    let reason = safe_text(field(input, "reason"), "reason", 500, false)?;
    kernel.mutate(
        context,
        "plan.review_abandon",
        "plan:write",
        &json!({"planId": plan_id, "reason": reason}),
        |_| {
            let Some(plan) = plan_row(kernel, plan_id)? else {
                return Err(KernelError::controller(format!("plan {plan_id} does not exist")));
            };
            if plan.state == "in_review" {
                let open = query_opt(
                    &kernel.database,
                    "SELECT 1 AS present FROM reviews WHERE project_id = ? AND subject_plan_id = ? AND state = 'started'",
                    [&kernel.project_id, plan_id],
                    |_| Ok(()),
                )?;
                if open.is_some() {
                    return Err(KernelError::controller(format!(
                        "plan {plan_id} has a review in progress"
                    )));
                }
                let now = kernel.now();
                execute(
                    &kernel.database,
                    "UPDATE plans SET state = 'draft', updated_at = ? WHERE project_id = ? AND plan_id = ?",
                    [now.as_str(), kernel.project_id.as_str(), plan_id],
                )?;
                let architect = match &plan.architect_agent_id {
                    Some(id) => kernel.agent_row(id)?,
                    None => None,
                };
                let parties = message_notices::notice_parties(kernel)?;
                if let Some(architect) = architect.filter(|a| a.state == "active") {
                    let body = format!(
                        "Plan {plan_id} review could not start: {reason}. The plan is a draft again; submit it again with cstan plan submit."
                    );
                    let sender = match &parties {
                        Some(parties) => parties.controller_actor_id.clone(),
                        None => kernel.controller_actor_id()?,
                    };
                    messages::insert_queued_message(
                        kernel,
                        &sender,
                        &architect,
                        &body,
                        &sha256(&body),
                        &now,
                        false,
                    )?;
                }
            }
            let mut event = MutationEvent::new("plan", plan_id, 0)
                .with_details(json!({"reason": reason}));
            event.to_state = Some("draft".into());
            Ok(MutationOutput::new(plan_value(kernel, plan_id)?, event))
        },
    )
}

/// `planRecord`: the plan with its approved revision (the current one while it is not approved), packages with derived
/// progress, and sign-offs; null for an unknown plan.
pub fn plan_record(kernel: &Kernel, credential: &str, plan_id: &str) -> KernelResult<Value> {
    kernel.authorize(credential, "plan:read")?;
    safe_id_str(plan_id, "plan id")?;
    let Some(plan) = plan_row(kernel, plan_id)? else {
        return Ok(Value::Null);
    };
    let shown = plan.approved_revision.unwrap_or(plan.current_revision);
    let revision = query_opt(
        &kernel.database,
        "SELECT * FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
        rusqlite::params![kernel.project_id, plan_id, shown],
        |row| {
            Ok(json!({
                "revision": row.get::<_, i64>("revision")?,
                "baseSha": row.get::<_, String>("base_sha")?,
                "bodyJson": row.get::<_, String>("body_json")?,
                "bodySha": row.get::<_, String>("body_sha")?,
                "authorAgentId": row.get::<_, String>("author_agent_id")?,
                "createdAt": row.get::<_, String>("created_at")?,
            }))
        },
    )?;
    let signoff_rows = query_all(
        &kernel.database,
        "SELECT * FROM plan_signoffs WHERE project_id = ? AND plan_id = ? ORDER BY created_at, integration_id",
        [&kernel.project_id, plan_id],
        |row| {
            Ok((
                row.get::<_, String>("integration_id")?,
                row.get::<_, String>("architect_agent_id")?,
                row.get::<_, String>("summary")?,
                row.get::<_, String>("created_at")?,
            ))
        },
    )?;
    let mut signoffs = Vec::new();
    for (integration_id, architect_agent_id, summary, created_at) in signoff_rows {
        signoffs.push(json!({
            "integrationId": integration_id,
            "architectAgentId": architect_agent_id,
            "summary": summary,
            "createdAt": created_at,
            "extraReports": plan_packages::signoff_extra_reports(kernel, plan_id, &integration_id)?,
        }));
    }
    Ok(json!({
        "plan": serde_json::to_value(plan_record_of(&plan))?,
        "revision": revision,
        "packages": plan_packages::plan_packages(kernel, plan_id)?,
        "signoffs": signoffs,
    }))
}

/// `listPlans`.
pub fn list_plans(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    kernel.authorize(credential, "plan:read")?;
    let rows = query_all(
        &kernel.database,
        "SELECT * FROM plans WHERE project_id = ? ORDER BY sequence",
        [&kernel.project_id],
        PlanRow::from_row,
    )?;
    Ok(Value::Array(
        rows.iter()
            .map(|row| Ok(serde_json::to_value(plan_record_of(row))?))
            .collect::<KernelResult<Vec<_>>>()?,
    ))
}

/// `approvalNoticeNote`: the extra line of the `Plan <id> approved` notice when the plan it supersedes was cancelled first.
pub fn approval_notice_note(kernel: &Kernel, plan_id: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    safe_id_str(plan_id, "plan id")?;
    match plan_row(kernel, plan_id)? {
        None => Ok(Value::Null),
        Some(plan) => Ok(approval_note(kernel, &plan)?.map_or(Value::Null, Value::String)),
    }
}

fn approval_note(kernel: &Kernel, plan: &PlanRow) -> KernelResult<Option<String>> {
    let old = match &plan.supersedes_plan_id {
        Some(id) => plan_row(kernel, id)?,
        None => None,
    };
    Ok(old.filter(|old| old.cancelled_at.is_some()).map(|old| {
        format!(
            "Plan {} was cancelled before this plan was approved; it was not superseded",
            old.plan_id
        )
    }))
}

/// Queues a controller notice to the plan's architect (while active) and to the sole active PM; returns the agents told. The
/// caller owns the transaction.
pub(crate) fn queue_plan_notice(
    kernel: &Kernel,
    plan: &PlanRow,
    body: &str,
    now: &str,
    action_needed: bool,
) -> KernelResult<Vec<String>> {
    let Some(parties) = message_notices::notice_parties(kernel)? else {
        return Ok(Vec::new());
    };
    let mut recipients: Vec<AgentRow> = vec![parties.pm.clone()];
    let architect = match &plan.architect_agent_id {
        Some(id) => kernel.agent_row(id)?,
        None => None,
    };
    if let Some(architect) = architect.filter(|a| a.state == "active") {
        recipients.push(architect);
    }
    for recipient in &recipients {
        messages::insert_queued_message(
            kernel,
            &parties.controller_actor_id,
            recipient,
            body,
            &sha256(body),
            now,
            action_needed,
        )?;
    }
    Ok(recipients.into_iter().map(|r| r.agent_id).collect())
}

/// `openPlansForSummary`: plans a restarted PM still has to follow; `agents.rs` calls it once its placeholder is swapped
/// for this.
pub(crate) fn open_plans_for_summary(kernel: &Kernel) -> KernelResult<Vec<Value>> {
    let rows = query_all(
        &kernel.database,
        "SELECT p.* FROM plans p WHERE p.project_id = ? AND p.cancelled_at IS NULL
         AND p.state IN ('draft', 'in_review', 'approved')
         AND NOT EXISTS (
           SELECT 1 FROM plan_signoffs s JOIN integrations i
             ON i.project_id = s.project_id AND i.integration_id = s.integration_id
           WHERE s.project_id = p.project_id AND s.plan_id = p.plan_id AND i.state = 'confirmed')
         ORDER BY p.sequence",
        [&kernel.project_id],
        PlanRow::from_row,
    )?;
    let mut out = Vec::new();
    for row in rows {
        let signed_off = query_all(
            &kernel.database,
            "SELECT integration_id FROM plan_signoffs WHERE project_id = ? AND plan_id = ? ORDER BY created_at, integration_id",
            [&kernel.project_id, &row.plan_id],
            |r| r.get::<_, String>(0),
        )?;
        out.push(json!({
            "planId": row.plan_id,
            "title": row.title,
            "tier": row.tier,
            "state": row.state,
            "packages": plan_packages::plan_packages(kernel, &row.plan_id)?.len(),
            "signedOff": signed_off,
        }));
    }
    Ok(out)
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "openPlan" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(open_plan(kernel, &context, input)))
        }
        "submitPlan" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(submit_plan(kernel, &context, input)))
        }
        "unannouncedPlanNotices" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(unannounced_plan_notices(kernel, credential)))
        }
        "announcePlanNotice" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(announce_plan_notice(kernel, &context, input)))
        }
        "planReviewRounds" => {
            let credential = arg!(a.str(0, "credential"));
            let plan_id = arg!(a.str(1, "planId"));
            Some(call(plan_review_rounds(kernel, credential, plan_id)))
        }
        "abandonPlanReview" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(abandon_plan_review(kernel, &context, input)))
        }
        "planRecord" => {
            let credential = arg!(a.str(0, "credential"));
            let plan_id = arg!(a.str(1, "planId"));
            Some(call(plan_record(kernel, credential, plan_id)))
        }
        "listPlans" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(list_plans(kernel, credential)))
        }
        "approvalNoticeNote" => {
            let plan_id = arg!(a.str(0, "planId"));
            Some(call(approval_notice_note(kernel, plan_id)))
        }
        _ => None,
    }
}
