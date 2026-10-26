//! The plans (src/controller/plans.ts).

use crate::canonical::sha256;
use crate::dispatch::{arg, call, Args};
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{
    amended_plan_body_json, execute, plan_approved_notice, plan_body_package_ids,
    plan_needs_attention_notice, plan_record_of, plan_signed_off_notice, query_all, query_opt,
    safe_id, safe_id_str, safe_text,
};
use crate::kernel::Kernel;
use crate::records::{
    AgentRow, MutationEvent, MutationOutput, PlanRow, DEFAULT_PLAN_REVIEW_ROUNDS,
};
use crate::types::MutationContext;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::sync::LazyLock;

use super::plan_handover::{architect_changed_body, handover_body, HandoverFacts, HandoverPackage};
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
    let max_review_rounds: Option<i64> = match input.get("maxReviewRounds") {
        None | Some(Value::Null) => None,
        Some(value) => match value.as_i64() {
            Some(limit @ 1..=20) => Some(limit),
            _ => {
                return Err(KernelError::type_error(
                    "the plan review round limit must be an integer from 1 to 20",
                ))
            }
        },
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
                "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, supersedes_plan_id, max_review_rounds, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, 'draft', ?, ?, ?, ?, ?)",
                rusqlite::params![
                    kernel.project_id,
                    plan_id,
                    sequence,
                    title,
                    tier,
                    actor.actor_id,
                    supersedes,
                    max_review_rounds,
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

/// `amendPlan`: the plan's architect stores the next revision of a draft plan as an amendment of the current one, which the
/// latest review asked for with mechanical findings only. A normal plan is approved at once; with `review` (default: a
/// high-risk plan) it goes back to review.
pub fn amend_plan(
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
    let amendment_json = match field(input, "amendmentJson").as_str() {
        Some(text) if !text.trim().is_empty() => text,
        _ => return Err(KernelError::type_error("the amendment must be JSON text")),
    };
    let review = input.get("review").and_then(Value::as_bool);
    let mut payload = Map::new();
    payload.insert("planId".into(), json!(plan_id));
    payload.insert("amendmentSha".into(), json!(sha256(amendment_json)));
    payload.insert("bodySha".into(), json!(sha256(body_json)));
    payload.insert("baseSha".into(), json!(base_sha));
    if let Some(review) = review {
        payload.insert("review".into(), json!(review));
    }
    kernel.mutate(
        context,
        "plan.amend",
        "plan:write",
        &Value::Object(payload),
        |actor| {
            let refuse = |why: String| KernelError::controller(format!("amend_refused: {why}"));
            let agent = match kernel.agent_by_actor(&actor.actor_id)? {
                Some(agent) if agent.kind == "Developer" => agent,
                _ => return Err(refuse("only the plan's architect amends a plan".into())),
            };
            let Some(plan) = plan_row(kernel, plan_id)? else {
                return Err(refuse(format!("plan {plan_id} does not exist")));
            };
            if plan.architect_agent_id.as_deref() != Some(agent.agent_id.as_str()) {
                return Err(refuse(format!(
                    "plan {plan_id} belongs to the architect {}",
                    plan.architect_agent_id.as_deref().unwrap_or("nobody")
                )));
            }
            if plan.cancelled_at.is_some() {
                return Err(refuse(format!("plan {plan_id} was cancelled")));
            }
            if plan.state != "draft" {
                return Err(refuse(format!(
                    "plan {plan_id} is {}; only a draft plan is amended",
                    plan.state
                )));
            }
            let last = query_opt(
                &kernel.database,
                "SELECT review_id, subject_plan_revision, state, findings_class FROM reviews
                 WHERE project_id = ? AND subject_plan_id = ? AND state IN ('passed', 'findings')
                 ORDER BY sequence DESC LIMIT 1",
                [&kernel.project_id, plan_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )?;
            let Some((review_id, reviewed_revision, review_state, class)) = last else {
                return Err(refuse(format!("plan {plan_id} has no finished review to amend after")));
            };
            if reviewed_revision != plan.current_revision {
                return Err(refuse(format!(
                    "the latest review of plan {plan_id} answered revision {reviewed_revision}, not the current revision {}",
                    plan.current_revision
                )));
            }
            if review_state != "findings" || class.as_deref() != Some("mechanical") {
                return Err(refuse(format!(
                    "review {review_id} did not end in mechanical findings; submit a new revision with cstan plan submit"
                )));
            }
            let amended_already = query_opt(
                &kernel.database,
                "SELECT revision FROM plan_revisions WHERE project_id = ? AND amends_review_id = ?",
                [&kernel.project_id, &review_id],
                |row| row.get::<_, i64>(0),
            )?;
            if let Some(revision) = amended_already {
                return Err(refuse(format!(
                    "review {review_id} was already amended by revision {revision}"
                )));
            }
            if plan_rounds(kernel, &plan)?.exhausted() {
                return Err(refuse(format!(
                    "plan {plan_id} used its review rounds; the PM has been told"
                )));
            }
            let previous = query_opt(
                &kernel.database,
                "SELECT body_json FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
                rusqlite::params![kernel.project_id, plan_id, plan.current_revision],
                |row| row.get::<_, String>(0),
            )?
            .ok_or_else(|| refuse(format!("plan {plan_id} has no revision to amend")))?;
            let amended = amended_plan_body_json(&previous, amendment_json)
                .map_err(KernelError::controller)?;
            let same = serde_json::from_str::<Value>(&amended).ok()
                == serde_json::from_str::<Value>(body_json).ok();
            if !same {
                return Err(refuse(
                    "the body is not the current revision with the amendment applied".into(),
                ));
            }
            let now = kernel.now();
            let revision = plan.current_revision + 1;
            let to_review = review.unwrap_or(plan.tier == "high_risk");
            execute(
                &kernel.database,
                "INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at, amendment_json, amends_review_id)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    kernel.project_id,
                    plan_id,
                    revision,
                    base_sha,
                    body_json,
                    sha256(body_json),
                    agent.agent_id,
                    actor.actor_id,
                    now,
                    amendment_json,
                    review_id
                ],
            )?;
            execute(
                &kernel.database,
                "UPDATE plans SET current_revision = ?, state = ?, approved_revision = ?, updated_at = ?
                 WHERE project_id = ? AND plan_id = ?",
                rusqlite::params![
                    revision,
                    if to_review { "in_review" } else { "approved" },
                    if to_review { None } else { Some(revision) },
                    now,
                    kernel.project_id,
                    plan_id
                ],
            )?;
            if !to_review {
                settle_plan_approval(kernel, &plan, body_json, &now)?;
            }
            let event = MutationEvent::new("plan", plan_id, 0)
                .transition("draft", if to_review { "in_review" } else { "approved" })
                .with_details(json!({"revision": revision, "amendsReviewId": review_id}));
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
        "plan_notice",
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
        } else if plan_rounds(kernel, &plan)?.exhausted()
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
                let rounds = plan_rounds(kernel, &plan)?;
                plan_needs_attention_notice(&plan.plan_id, rounds.limit, rounds.design)
            };
            let announced = message_notices::notice_to_pm(kernel, "plan_notice", &body, &kernel.now(), true)?;
            Ok(MutationOutput::new(
                json!({"announced": announced}),
                MutationEvent::new("plan", plan_id, 0)
                    .with_details(json!({"announced": announced, "kind": field(input, "kind")})),
            ))
        },
    )
}

/// The finished review rounds of a plan against its limit. A round that ended in mechanical findings is answered by an
/// amendment and does not count as a design round; the total still bounds the plan at twice the limit.
pub(crate) struct PlanRounds {
    pub design: i64,
    pub total: i64,
    pub limit: i64,
}

impl PlanRounds {
    pub(crate) fn exhausted(&self) -> bool {
        self.design >= self.limit || self.total >= 2 * self.limit
    }
}

pub(crate) fn plan_rounds(kernel: &Kernel, plan: &PlanRow) -> KernelResult<PlanRounds> {
    let (total, design) = query_opt(
        &kernel.database,
        "SELECT COALESCE(SUM(state IN ('passed', 'findings')), 0),
                COALESCE(SUM(state = 'findings' AND COALESCE(findings_class, 'design') = 'design'), 0)
         FROM reviews WHERE project_id = ? AND subject_plan_id = ?",
        [&kernel.project_id, &plan.plan_id],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    )?
    .unwrap_or((0, 0));
    Ok(PlanRounds {
        design,
        total,
        limit: plan.max_review_rounds.unwrap_or(DEFAULT_PLAN_REVIEW_ROUNDS),
    })
}

/// `planReviewRounds`: the design review rounds of a plan (findings of class design, or of no class).
pub fn plan_review_rounds(kernel: &Kernel, credential: &str, plan_id: &str) -> KernelResult<Value> {
    kernel.authorize(credential, "plan:read")?;
    safe_id_str(plan_id, "plan id")?;
    Ok(match plan_row(kernel, plan_id)? {
        Some(plan) => json!(plan_rounds(kernel, &plan)?.design),
        None => json!(0),
    })
}

/// `planReviewStatus`: the review rounds of a plan, its limit, and whether the plan has used them up.
pub fn plan_review_status(kernel: &Kernel, credential: &str, plan_id: &str) -> KernelResult<Value> {
    kernel.authorize(credential, "plan:read")?;
    safe_id_str(plan_id, "plan id")?;
    let Some(plan) = plan_row(kernel, plan_id)? else {
        return Ok(Value::Null);
    };
    let rounds = plan_rounds(kernel, &plan)?;
    Ok(json!({
        "designRounds": rounds.design,
        "totalRounds": rounds.total,
        "limit": rounds.limit,
        "exhausted": rounds.exhausted(),
    }))
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
                        messages::class_for_kind(kernel, "plan_review_failed"),
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

/// The reassignments of a plan, oldest first, each `{fromAgentId, toAgentId, actorId, reason, createdAt}`.
pub(crate) fn reassignments_of(kernel: &Kernel, plan_id: &str) -> KernelResult<Vec<Value>> {
    query_all(
        &kernel.database,
        "SELECT from_agent_id, to_agent_id, actor_id, reason, created_at FROM plan_reassignments
         WHERE project_id = ? AND plan_id = ? ORDER BY sequence",
        [&kernel.project_id, plan_id],
        |row| {
            Ok(json!({
                "fromAgentId": row.get::<_, String>(0)?,
                "toAgentId": row.get::<_, String>(1)?,
                "actorId": row.get::<_, String>(2)?,
                "reason": row.get::<_, Option<String>>(3)?,
                "createdAt": row.get::<_, String>(4)?,
            }))
        },
    )
}

/// The newest reassignment of a plan, or `None`.
pub(crate) fn last_reassignment(kernel: &Kernel, plan_id: &str) -> KernelResult<Option<Value>> {
    Ok(reassignments_of(kernel, plan_id)?.pop())
}

fn ids_of(kernel: &Kernel, sql: &str, plan_id: &str) -> KernelResult<Vec<String>> {
    query_all(
        &kernel.database,
        sql,
        [&kernel.project_id, plan_id],
        |row| row.get::<_, String>(0),
    )
}

/// A live package with an assignee: `(assignee, package, progress)`.
type HeldPackage = (String, String, String);

/// What the new owner of a plan inherits: its live packages and the reports, reviews and integrations still pending.
fn handover_facts(
    kernel: &Kernel,
    plan: &PlanRow,
    from_agent_id: &str,
    reason: Option<&str>,
) -> KernelResult<(HandoverFacts, Vec<HeldPackage>)> {
    let mut packages = Vec::new();
    let mut report_ids = Vec::new();
    let mut pending_reports = Vec::new();
    // (assignee, package, progress) of the live packages, in package order.
    let mut held = Vec::new();
    for row in plan_packages::plan_package_rows(kernel, &plan.plan_id)? {
        if row.cancelled_at.is_some() {
            continue;
        }
        let facts = plan_packages::package_facts(kernel, &plan.plan_id, &row, false)?;
        let report = plan_packages::package_report(
            kernel,
            &plan.plan_id,
            &row.package_id,
            row.assignee_agent_id.as_deref(),
            row.assigned_at.as_deref(),
        )?;
        if let Some(report_id) = report {
            if facts.progress == "reported" || facts.progress == "findings" {
                pending_reports.push(report_id.clone());
            }
            report_ids.push(report_id);
        }
        if let Some(assignee) = &row.assignee_agent_id {
            held.push((
                assignee.clone(),
                row.package_id.clone(),
                facts.progress.to_string(),
            ));
        }
        packages.push(HandoverPackage {
            package_id: row.package_id,
            assignee_agent_id: row.assignee_agent_id,
            progress: facts.progress.to_string(),
        });
    }
    let mut integration_ids: Vec<String> = Vec::new();
    for report_id in &report_ids {
        for id in query_all(
            &kernel.database,
            "SELECT ir.integration_id FROM integration_reports ir
             JOIN integrations i ON i.project_id = ir.project_id AND i.integration_id = ir.integration_id
             WHERE ir.project_id = ? AND ir.report_id = ? AND i.state IN ('running', 'merged', 'conflicted', 'confirmed')
             ORDER BY i.sequence",
            [&kernel.project_id, report_id],
            |row| row.get::<_, String>(0),
        )? {
            if !integration_ids.contains(&id) {
                integration_ids.push(id);
            }
        }
    }
    let mut pending_integrations = Vec::new();
    for id in &integration_ids {
        let signed = query_opt(
            &kernel.database,
            "SELECT 1 AS present FROM plan_signoffs WHERE project_id = ? AND plan_id = ? AND integration_id = ?",
            [&kernel.project_id, &plan.plan_id, id],
            |_| Ok(()),
        )?;
        if signed.is_none() {
            pending_integrations.push(id.clone());
        }
    }
    let mut pending_reviews = ids_of(
        kernel,
        "SELECT review_id FROM reviews WHERE project_id = ? AND subject_plan_id = ? AND state = 'started' ORDER BY sequence",
        &plan.plan_id,
    )?;
    for (column, ids) in [
        ("subject_report_id", &report_ids),
        ("subject_integration_id", &integration_ids),
    ] {
        for id in ids {
            let found = query_all(
                &kernel.database,
                &format!(
                    "SELECT review_id FROM reviews WHERE project_id = ? AND {column} = ? AND state = 'started' ORDER BY sequence"
                ),
                [&kernel.project_id, id],
                |row| row.get::<_, String>(0),
            )?;
            for review in found {
                if !pending_reviews.contains(&review) {
                    pending_reviews.push(review);
                }
            }
        }
    }
    let facts = HandoverFacts {
        plan_id: plan.plan_id.clone(),
        state: plan.state.clone(),
        current_revision: plan.current_revision,
        approved_revision: plan.approved_revision,
        from_agent_id: from_agent_id.to_string(),
        reason: reason.map(str::to_string),
        packages,
        pending_reports,
        pending_reviews,
        pending_integrations,
    };
    Ok((facts, held))
}

/// `reassignPlan`: the PM or the operator hands a plan whose architect is lost to another active architect. The owner must
/// have stopped (a live owner is released first); the ledger accepts the change only against the reassignment recorded here.
pub fn reassign_plan(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let plan_id = safe_id(field(input, "planId"), "plan id")?;
    let agent_id = safe_id(field(input, "agentId"), "agent id")?;
    let role = safe_id(field(input, "architectRole"), "architect role")?;
    let reason: Option<String> = match input.get("reason") {
        None | Some(Value::Null) => None,
        Some(value) => Some(safe_text(value, "the reason", 500, false)?),
    };
    let mut payload = json!({"planId": plan_id, "agentId": agent_id, "architectRole": role});
    if let Some(reason) = &reason {
        payload["reason"] = json!(reason);
    }
    kernel.mutate(context, "plan.reassign", "plan:write", &payload, |actor| {
        let refuse = |code: &str, text: String| Err(KernelError::controller(format!("{code}: {text}")));
        if actor.role != "PM" && actor.role != "operator" {
            return refuse("not_manager", "only the PM or the operator reassigns a plan".into());
        }
        let Some(plan) = plan_row(kernel, plan_id)? else {
            return refuse("plan_not_found", format!("plan {plan_id} does not exist"));
        };
        if plan.cancelled_at.is_some() {
            return refuse("plan_cancelled", format!("plan {plan_id} was cancelled"));
        }
        if plan.state == "superseded" {
            return refuse("plan_superseded", format!("plan {plan_id} was superseded"));
        }
        let signed_off = query_opt(
            &kernel.database,
            "SELECT 1 AS present FROM plan_signoffs WHERE project_id = ? AND plan_id = ?",
            [&kernel.project_id, plan_id],
            |_| Ok(()),
        )?;
        if signed_off.is_some() {
            return refuse("plan_signed_off", format!("plan {plan_id} is signed off"));
        }
        let Some(owner_id) = plan.architect_agent_id.clone() else {
            return refuse(
                "no_owner",
                format!("plan {plan_id} has no architect yet; the first developer to submit it becomes its architect"),
            );
        };
        if owner_id == agent_id {
            return refuse("already_owner", format!("{agent_id} already owns plan {plan_id}"));
        }
        if let Some(owner) = kernel.agent_row(&owner_id)?.filter(|o| o.state == "active") {
            return refuse(
                "owner_active",
                format!(
                    "{} still owns plan {plan_id} and is active; release it first",
                    owner.agent_id
                ),
            );
        }
        let target = kernel
            .agent_row(agent_id)?
            .filter(|t| t.state == "active" && t.kind == "Developer" && t.role_name == role);
        let Some(target) = target else {
            return refuse(
                "not_architect",
                format!("{agent_id} is not an active developer-kind agent of role {role}"),
            );
        };
        let rows = plan_packages::plan_package_rows(kernel, plan_id)?;
        if rows
            .iter()
            .any(|row| row.cancelled_at.is_none() && row.assignee_agent_id.as_deref() == Some(agent_id))
        {
            return refuse(
                "not_architect",
                format!("{agent_id} holds a package of plan {plan_id} and cannot also own the plan"),
            );
        }
        let now = kernel.now();
        let sequence = query_opt(
            &kernel.database,
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM plan_reassignments WHERE project_id = ? AND plan_id = ?",
            [&kernel.project_id, plan_id],
            |row| row.get::<_, i64>(0),
        )?
        .unwrap_or(1);
        execute(
            &kernel.database,
            "INSERT INTO plan_reassignments(project_id, plan_id, sequence, from_agent_id, to_agent_id, actor_id, reason, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                kernel.project_id,
                plan_id,
                sequence,
                owner_id,
                agent_id,
                actor.actor_id,
                reason,
                now
            ],
        )?;
        execute(
            &kernel.database,
            "UPDATE plans SET architect_agent_id = ?, updated_at = ? WHERE project_id = ? AND plan_id = ?",
            [agent_id, now.as_str(), kernel.project_id.as_str(), plan_id],
        )?;
        let (facts, held) = handover_facts(kernel, &plan, &owner_id, reason.as_deref())?;
        let sender = kernel.controller_actor_id()?;
        let body = handover_body(&facts);
        let handover_message_id = messages::insert_queued_message(
            kernel,
            &sender,
            &target,
            &body,
            &sha256(&body),
            &now,
            true,
            messages::class_for_kind(kernel, "plan_handover"),
        )?;
        let mut notified: Vec<String> = Vec::new();
        for (assignee, _, progress) in &held {
            if notified.contains(assignee)
                || !matches!(progress.as_str(), "assigned" | "reported" | "findings" | "reviewed")
            {
                continue;
            }
            let Some(developer) = kernel.agent_row(assignee)?.filter(|d| d.state == "active") else {
                continue;
            };
            let packages: Vec<String> = held
                .iter()
                .filter(|(a, _, p)| {
                    a == assignee && matches!(p.as_str(), "assigned" | "reported" | "findings" | "reviewed")
                })
                .map(|(_, package, _)| package.clone())
                .collect();
            let body = architect_changed_body(plan_id, &packages, agent_id);
            messages::insert_queued_message(
                kernel,
                &sender,
                &developer,
                &body,
                &sha256(&body),
                &now,
                false,
                messages::class_for_kind(kernel, "architect_changed"),
            )?;
            notified.push(assignee.clone());
        }
        let value = json!({
            "planId": plan_id,
            "fromAgentId": owner_id,
            "toAgentId": agent_id,
            "state": plan.state,
            "currentRevision": plan.current_revision,
            "reason": reason,
            "reassignedAt": now,
            "handoverMessageId": handover_message_id,
            "notifiedDevelopers": notified,
        });
        Ok(MutationOutput::new(
            value,
            MutationEvent::new("plan", plan_id, 0).with_details(json!({
                "fromAgentId": owner_id,
                "toAgentId": agent_id,
                "reason": reason,
            })),
        ))
    })
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
                "amendment": row
                    .get::<_, Option<String>>("amendment_json")?
                    .and_then(|text| serde_json::from_str::<Value>(&text).ok()),
                "amendsReviewId": row.get::<_, Option<String>>("amends_review_id")?,
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
    let mut plan_value = serde_json::to_value(plan_record_of(&plan))?;
    let reassignments = reassignments_of(kernel, plan_id)?;
    if !reassignments.is_empty() {
        plan_value["reassignments"] = Value::Array(reassignments);
    }
    Ok(json!({
        "plan": plan_value,
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
    let mut lines: Vec<String> = old
        .filter(|old| old.cancelled_at.is_some())
        .map(|old| {
            format!(
                "Plan {} was cancelled before this plan was approved; it was not superseded",
                old.plan_id
            )
        })
        .into_iter()
        .collect();
    // The approved revision is read from the ledger: the caller may hold the row from before the approval.
    let amended = query_opt(
        &kernel.database,
        "SELECT r.revision, r.amends_review_id FROM plans p
         JOIN plan_revisions r ON r.project_id = p.project_id AND r.plan_id = p.plan_id AND r.revision = p.approved_revision
         WHERE p.project_id = ? AND p.plan_id = ? AND r.amends_review_id IS NOT NULL",
        [&kernel.project_id, &plan.plan_id],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
    )?;
    if let Some((revision, review_id)) = amended {
        lines.push(format!(
            "Approved at revision {revision} by amendment of revision {} after review {review_id} (mechanical findings)",
            revision - 1
        ));
    }
    Ok((!lines.is_empty()).then(|| lines.join("\n")))
}

/// Queues a controller notice to the plan's architect (while active) and to the sole active PM; returns the agents told. The
/// caller owns the transaction.
pub(crate) fn queue_plan_notice(
    kernel: &Kernel,
    kind: &str,
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
            messages::class_for_kind(kernel, kind),
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
        "amendPlan" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(amend_plan(kernel, &context, input)))
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
        "planReviewStatus" => {
            let credential = arg!(a.str(0, "credential"));
            let plan_id = arg!(a.str(1, "planId"));
            Some(call(plan_review_status(kernel, credential, plan_id)))
        }
        "abandonPlanReview" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(abandon_plan_review(kernel, &context, input)))
        }
        "reassignPlan" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(reassign_plan(kernel, &context, input)))
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
