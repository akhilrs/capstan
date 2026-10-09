//! The reviews (src/controller/reviews.ts).

use crate::canonical::sha256;
use crate::dispatch::{arg, call, Args};
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{
    execute, js_trim, package_reviewed_notice, plan_needs_attention_notice, query_all, query_opt,
    review_notice, review_record, review_task, safe_id_str,
};
use crate::kernel::Kernel;
use crate::records::{
    AgentReportRow, AgentRow, MutationEvent, MutationOutput, ReviewRow, MAX_REVIEW_ROUNDS,
    MAX_REVIEW_TEXT_BYTES,
};
use crate::types::MutationContext;
use regex::Regex;
use serde_json::{json, Value};
use std::sync::LazyLock;

use super::integrations::{integration_row, IntegrationRow};
use super::{message_notices, messages, plan_packages, plans};

static SHA1: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9a-f]{40}$").expect("the pattern is valid"));

static UNSAFE_REVIEW_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[\p{Cc}\p{Cf}\p{Zl}\p{Zp}\p{Noncharacter_Code_Point}]")
        .expect("the pattern is valid")
});

fn field<'a>(input: &'a Value, key: &str) -> &'a Value {
    input.get(key).unwrap_or(&Value::Null)
}

fn text_field<'a>(input: &'a Value, key: &str, label: &str) -> KernelResult<&'a str> {
    match field(input, key).as_str() {
        Some(text) => safe_id_str(text, label),
        None => Err(KernelError::type_error(format!(
            "{label} must be 1-128 safe ASCII characters"
        ))),
    }
}

struct PlanCheck {
    revision: i64,
    author_agent_id: String,
    author_actor_id: String,
}

struct Checked {
    subject: &'static str,
    commit_sha: String,
    base_sha: String,
    round: i64,
    authors: Vec<AgentReportRow>,
    plan: Option<PlanCheck>,
}

fn review_checks(kernel: &Kernel, subject_id: &str, reviewer_role: &str) -> KernelResult<Checked> {
    let report = query_opt(
        &kernel.database,
        "SELECT * FROM agent_reports WHERE project_id = ? AND report_id = ?",
        [&kernel.project_id, subject_id],
        AgentReportRow::from_row,
    )?;
    let integration: Option<IntegrationRow> = if report.is_none() {
        integration_row(kernel, subject_id)?
    } else {
        None
    };
    let plan_row = if report.is_none() && integration.is_none() {
        plans::plan_row(kernel, subject_id)?
    } else {
        None
    };
    if report.is_none() && integration.is_none() && plan_row.is_none() {
        return Err(KernelError::controller(
            "the report or integration does not exist",
        ));
    }
    let column = if plan_row.is_some() {
        "subject_plan_id"
    } else if report.is_none() {
        "subject_integration_id"
    } else {
        "subject_report_id"
    };
    let base_sha: Option<String>;
    let commit_sha: String;
    let mut authors: Vec<AgentReportRow> = Vec::new();
    let mut plan: Option<PlanCheck> = None;
    if let Some(plan_row) = &plan_row {
        if plan_row.state != "in_review" {
            return Err(KernelError::controller(format!(
                "plan {} is {}; only a plan in review can be reviewed",
                plan_row.plan_id, plan_row.state
            )));
        }
        let revision = query_opt(
            &kernel.database,
            "SELECT * FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
            rusqlite::params![
                kernel.project_id,
                plan_row.plan_id,
                plan_row.current_revision
            ],
            |row| {
                Ok((
                    row.get::<_, String>("base_sha")?,
                    row.get::<_, String>("author_agent_id")?,
                    row.get::<_, String>("author_actor_id")?,
                ))
            },
        )?;
        let Some((revision_base, author_agent_id, author_actor_id)) = revision else {
            return Err(KernelError::controller(
                "the plan has no revision to review",
            ));
        };
        base_sha = Some(revision_base.clone());
        commit_sha = revision_base;
        plan = Some(PlanCheck {
            revision: plan_row.current_revision,
            author_agent_id,
            author_actor_id,
        });
    } else if let Some(report) = report {
        if report.state != "accepted" {
            return Err(KernelError::controller(
                "only an accepted report can be reviewed",
            ));
        }
        base_sha = serde_json::from_str::<Value>(&report.evidence_json)
            .ok()
            .and_then(|evidence| {
                evidence
                    .get("baseSha")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
        commit_sha = report.commit_sha.clone();
        authors = vec![report];
    } else {
        let integration = integration.expect("checked above");
        if integration.state != "merged" {
            return Err(KernelError::controller(
                "only a merged, unconfirmed integration can be reviewed",
            ));
        }
        base_sha = Some(integration.base_sha.clone());
        commit_sha = integration.head_sha.clone().unwrap_or_default();
        authors = integration_authors(kernel, &integration.integration_id)?;
    }
    let Some(base_sha) = base_sha.filter(|sha| SHA1.is_match(sha)) else {
        return Err(KernelError::controller(
            "the subject has no usable base commit",
        ));
    };
    let open = query_opt(
        &kernel.database,
        &format!(
            "SELECT 1 AS present FROM reviews WHERE project_id = ? AND {column} = ? AND state = 'started'"
        ),
        [&kernel.project_id, subject_id],
        |_| Ok(()),
    )?;
    if open.is_some() {
        return Err(KernelError::controller(
            "a review of this subject is already open",
        ));
    }
    let done = query_opt(
        &kernel.database,
        &format!(
            "SELECT COUNT(*) AS n FROM reviews WHERE project_id = ? AND {column} = ? AND state IN ('passed', 'findings')"
        ),
        [&kernel.project_id, subject_id],
        |row| row.get::<_, i64>(0),
    )?
    .unwrap_or(0);
    if done >= MAX_REVIEW_ROUNDS {
        return Err(KernelError::controller(
            "the review limit for this subject is reached",
        ));
    }
    let role = query_opt(
        &kernel.database,
        "SELECT kind FROM role_definitions WHERE project_id = ? AND role_name = ? AND state = 'active'",
        [&kernel.project_id, reviewer_role],
        |row| row.get::<_, String>(0),
    )?;
    if role.as_deref() != Some("Verifier") {
        return Err(KernelError::controller(
            "the reviewer role must be an active Verifier role",
        ));
    }
    let round = query_opt(
        &kernel.database,
        &format!(
            "SELECT COALESCE(MAX(round), 0) + 1 AS next FROM reviews WHERE project_id = ? AND {column} = ?"
        ),
        [&kernel.project_id, subject_id],
        |row| row.get::<_, i64>(0),
    )?
    .unwrap_or(1);
    Ok(Checked {
        subject: if plan.is_some() {
            "plan"
        } else if column == "subject_integration_id" {
            "integration"
        } else {
            "report"
        },
        commit_sha,
        base_sha,
        round,
        authors,
        plan,
    })
}

/// The reports an integration merged, in merge order (`integrationAuthors`).
pub fn integration_authors(
    kernel: &Kernel,
    integration_id: &str,
) -> KernelResult<Vec<AgentReportRow>> {
    query_all(
        &kernel.database,
        "SELECT r.* FROM integration_reports ir
         JOIN agent_reports r ON r.project_id = ir.project_id AND r.report_id = ir.report_id
         WHERE ir.project_id = ? AND ir.integration_id = ? ORDER BY ir.position",
        [&kernel.project_id, integration_id],
        AgentReportRow::from_row,
    )
}

/// `checkReviewRequest`.
pub fn check_review_request(
    kernel: &Kernel,
    subject_id: &str,
    reviewer_role: &str,
) -> KernelResult<Value> {
    kernel.assert_open()?;
    safe_id_str(subject_id, "report or integration id")?;
    safe_id_str(reviewer_role, "reviewer role")?;
    let checked = review_checks(kernel, subject_id, reviewer_role)?;
    let authors: Vec<&str> = match &checked.plan {
        None => checked
            .authors
            .iter()
            .map(|a| a.agent_id.as_str())
            .collect(),
        Some(plan) => vec![plan.author_agent_id.as_str()],
    };
    Ok(json!({
        "commitSha": checked.commit_sha,
        "baseSha": checked.base_sha,
        "round": checked.round,
        "authorAgentIds": authors,
    }))
}

fn review_by_id(kernel: &Kernel, review_id: &str) -> KernelResult<Option<ReviewRow>> {
    query_opt(
        &kernel.database,
        "SELECT * FROM reviews WHERE project_id = ? AND review_id = ?",
        [&kernel.project_id, review_id],
        ReviewRow::from_row,
    )
}

fn review_value(row: &ReviewRow) -> KernelResult<Value> {
    Ok(serde_json::to_value(review_record(row))?)
}

/// `beginReview`.
pub fn begin_review(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let subject_id = text_field(input, "subjectId", "report or integration id")?;
    let reviewer_role = text_field(input, "reviewerRole", "reviewer role")?;
    let reviewer_agent_id = text_field(input, "reviewerAgentId", "reviewer agent id")?;
    kernel.mutate(context, "review.begin", "review:request", input, |actor| {
        let checked = review_checks(kernel, subject_id, reviewer_role)?;
        let reviewer = kernel.agent_row(reviewer_agent_id)?;
        let reviewer = match reviewer {
            Some(reviewer)
                if reviewer.state == "active"
                    && reviewer.kind == "Verifier"
                    && reviewer.role_name == reviewer_role =>
            {
                reviewer
            }
            _ => {
                return Err(KernelError::controller(
                    "the reviewer agent is not an active agent of the requested Verifier role",
                ))
            }
        };
        let author_parties: Vec<(String, String)> = match &checked.plan {
            None => checked
                .authors
                .iter()
                .map(|a| (a.agent_id.clone(), a.actor_id.clone()))
                .collect(),
            Some(plan) => vec![(plan.author_agent_id.clone(), plan.author_actor_id.clone())],
        };
        if author_parties.iter().any(|(agent_id, actor_id)| {
            reviewer.actor_id == *actor_id || reviewer.agent_id == *agent_id
        }) {
            return Err(KernelError::controller("a reviewer cannot be an author"));
        }
        let single = if checked.subject == "integration" {
            None
        } else {
            author_parties.first()
        };
        let controller = query_opt(
            &kernel.database,
            "SELECT actor_id FROM actors WHERE project_id = ? AND is_internal = 1 AND role = 'controller' AND active = 1 AND revoked_at IS NULL",
            [&kernel.project_id],
            |row| row.get::<_, String>(0),
        )?;
        let Some(controller) = controller else {
            return Err(KernelError::controller("the controller actor is missing"));
        };
        let now = kernel.now();
        let review_id = kernel.env.uuid();
        let sequence = query_opt(
            &kernel.database,
            "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM reviews WHERE project_id = ?",
            [&kernel.project_id],
            |row| row.get::<_, i64>(0),
        )?
        .unwrap_or(1);
        execute(
            &kernel.database,
            "INSERT INTO reviews(project_id, review_id, sequence, round, subject_report_id, subject_integration_id, subject_plan_id, subject_plan_revision,
               commit_sha, base_sha, author_agent_id, author_actor_id, requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, state, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'started', ?)",
            rusqlite::params![
                kernel.project_id,
                review_id,
                sequence,
                checked.round,
                (checked.subject == "report").then_some(subject_id),
                (checked.subject == "integration").then_some(subject_id),
                checked.plan.is_some().then_some(subject_id),
                checked.plan.as_ref().map(|plan| plan.revision),
                checked.commit_sha,
                checked.base_sha,
                single.map(|(agent_id, _)| agent_id.as_str()),
                single.map(|(_, actor_id)| actor_id.as_str()),
                actor.actor_id,
                reviewer_role,
                reviewer.agent_id,
                reviewer.actor_id,
                now
            ],
        )?;
        let row = review_by_id(kernel, &review_id)?
            .ok_or_else(|| KernelError::controller("the review row is missing"))?;
        let authors: Vec<&AgentReportRow> = checked.authors.iter().collect();
        let task = review_task(&row, &authors);
        messages::insert_queued_message(
            kernel,
            &controller,
            &reviewer,
            &task,
            &sha256(&task),
            &now,
            false,
        )?;
        let mut event = MutationEvent::new("review", &review_id, 0).with_details(json!({
            "subjectId": subject_id,
            "round": checked.round,
            "reviewerAgentId": reviewer.agent_id,
            "authorAgentIds": author_parties.iter().map(|(agent_id, _)| agent_id.as_str()).collect::<Vec<_>>(),
        }));
        event.to_state = Some("started".into());
        Ok(MutationOutput::new(review_value(&row)?, event))
    })
}

/// `completeReview`.
pub fn complete_review(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let verdict = match field(input, "verdict").as_str() {
        Some(verdict @ ("pass" | "findings")) => verdict,
        _ => {
            return Err(KernelError::type_error(
                "the verdict must be pass or findings",
            ))
        }
    };
    let text = match field(input, "text").as_str() {
        Some(text)
            if !js_trim(text).is_empty()
                && !UNSAFE_REVIEW_TEXT.is_match(&text.replace('\n', ""))
                && text.len() <= MAX_REVIEW_TEXT_BYTES =>
        {
            text
        }
        _ => {
            return Err(KernelError::type_error(format!(
                "the review text must be printable text of at most {MAX_REVIEW_TEXT_BYTES} bytes"
            )))
        }
    };
    kernel.mutate(
        context,
        "review.complete",
        "review:submit",
        &json!({"verdict": verdict, "textHash": sha256(text)}),
        |actor| {
            // By agent, not by actor: a replaced generation gets a new actor but is the same reviewer.
            let caller = kernel.agent_by_actor(&actor.actor_id)?;
            let row = match caller {
                Some(caller) => query_opt(
                    &kernel.database,
                    "SELECT * FROM reviews WHERE project_id = ? AND reviewer_agent_id = ? AND state = 'started'",
                    [&kernel.project_id, &caller.agent_id],
                    ReviewRow::from_row,
                )?,
                None => None,
            };
            let Some(row) = row else {
                return Err(KernelError::controller("you have no review in progress"));
            };
            let now = kernel.now();
            execute(
                &kernel.database,
                "UPDATE reviews SET state = ?, verdict_text = ?, completed_at = ? WHERE project_id = ? AND review_id = ?",
                [
                    if verdict == "pass" { "passed" } else { "findings" },
                    text,
                    &now,
                    &kernel.project_id,
                    &row.review_id,
                ],
            )?;
            kernel.touch_agent(&row.reviewer_agent_id, &now)?;
            if row.subject_plan_id.is_some() {
                settle_plan_review(kernel, &row, verdict, &now)?;
            } else if verdict == "pass" && row.subject_report_id.is_some() {
                notice_package_reviewed(kernel, &row, &now)?;
            }
            announce(kernel, &row.review_id, &now)?;
            let done = review_by_id(kernel, &row.review_id)?
                .ok_or_else(|| KernelError::controller("the review row is missing"))?;
            let subject = row
                .subject_report_id
                .as_deref()
                .or(row.subject_integration_id.as_deref())
                .or(row.subject_plan_id.as_deref());
            let event = MutationEvent::new("review", &row.review_id, 0)
                .transition("started", &done.state)
                .with_details(json!({"subjectId": subject, "round": row.round}));
            Ok(MutationOutput::new(review_value(&done)?, event))
        },
    )
}

/// A finished plan review moves its plan: pass approves it (package rows, superseded plan), findings send it back to draft.
/// A plan that is no longer in review is left as it is. The caller owns the transaction.
fn settle_plan_review(
    kernel: &Kernel,
    row: &ReviewRow,
    verdict: &str,
    now: &str,
) -> KernelResult<()> {
    let plan_id = row.subject_plan_id.as_deref().unwrap_or_default();
    let Some(plan) = plans::plan_row(kernel, plan_id)? else {
        return Ok(());
    };
    if plan.state != "in_review" || plan.cancelled_at.is_some() {
        return Ok(());
    }
    if verdict == "findings" {
        execute(
            &kernel.database,
            "UPDATE plans SET state = 'draft', updated_at = ? WHERE project_id = ? AND plan_id = ?",
            [now, &kernel.project_id, &plan.plan_id],
        )?;
        if plans::finished_plan_reviews(kernel, &plan.plan_id)? >= MAX_REVIEW_ROUNDS {
            message_notices::notice_to_pm(
                kernel,
                &plan_needs_attention_notice(&plan.plan_id),
                now,
                true,
            )?;
        }
        return Ok(());
    }
    let body_json = query_opt(
        &kernel.database,
        "SELECT body_json FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
        rusqlite::params![kernel.project_id, plan.plan_id, row.subject_plan_revision],
        |r| r.get::<_, String>(0),
    )?
    .ok_or_else(|| KernelError::type_error("Cannot read properties of undefined (reading 'body_json')"))?;
    execute(
        &kernel.database,
        "UPDATE plans SET state = 'approved', approved_revision = ?, updated_at = ? WHERE project_id = ? AND plan_id = ?",
        rusqlite::params![row.subject_plan_revision, now, kernel.project_id, plan.plan_id],
    )?;
    plans::settle_plan_approval(kernel, &plan, &body_json, now)
}

/// Tells the PM that a package's report passed review, once per report, when the package's plan has a `plan` link: this is
/// the one status change the PM has no other signal for. A cancelled plan or package is not announced.
fn notice_package_reviewed(kernel: &Kernel, row: &ReviewRow, now: &str) -> KernelResult<()> {
    let candidates = query_all(
        &kernel.database,
        "SELECT p.plan_id, p.package_id, p.assignee_agent_id, p.assigned_at FROM plan_packages p
         JOIN plans pl ON pl.project_id = p.project_id AND pl.plan_id = p.plan_id
         JOIN external_links l ON l.project_id = p.project_id AND l.ref_kind = 'plan' AND l.ref_id = p.plan_id AND l.system = 'nexora'
         WHERE p.project_id = ? AND p.assignee_agent_id = ? AND p.cancelled_at IS NULL AND pl.cancelled_at IS NULL
         ORDER BY p.plan_id, p.package_id",
        rusqlite::params![kernel.project_id, row.author_agent_id],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        },
    )?;
    let mut found = None;
    for (plan_id, package_id, assignee, assigned_at) in candidates {
        let report = plan_packages::package_report(
            kernel,
            &plan_id,
            &package_id,
            Some(&assignee),
            Some(&assigned_at),
        )?;
        if report.is_some() && report == row.subject_report_id {
            found = Some((plan_id, package_id));
            break;
        }
    }
    let Some((plan_id, package_id)) = found else {
        return Ok(());
    };
    let body = package_reviewed_notice(
        &plan_id,
        &package_id,
        row.subject_report_id.as_deref().unwrap_or_default(),
        &row.commit_sha,
    );
    let sent = query_opt(
        &kernel.database,
        "SELECT 1 AS present FROM messages WHERE project_id = ? AND body = ?",
        [&kernel.project_id, &body],
        |_| Ok(()),
    )?;
    if sent.is_none() {
        message_notices::notice_to_pm(kernel, &body, now, true)?;
    }
    Ok(())
}

fn review_author_ids(kernel: &Kernel, row: &ReviewRow) -> KernelResult<Vec<String>> {
    if row.subject_plan_id.is_some() {
        return Ok(vec![row.author_agent_id.clone().unwrap_or_default()]);
    }
    match &row.subject_integration_id {
        None => Ok(vec![row.author_agent_id.clone().unwrap_or_default()]),
        Some(integration_id) => Ok(integration_authors(kernel, integration_id)?
            .into_iter()
            .map(|r| r.agent_id)
            .collect()),
    }
}

/// Queues the notice for a finished review that has none, to the agent that requested it (the architect) and to the PM when
/// the requester is the PM, the operator or no longer active; false when no PM is the sole active one. The caller owns the
/// transaction.
fn announce(kernel: &Kernel, review_id: &str, now: &str) -> KernelResult<bool> {
    let row = query_opt(
        &kernel.database,
        "SELECT * FROM reviews WHERE project_id = ? AND review_id = ? AND state IN ('passed', 'findings') AND notified_message_id IS NULL",
        [&kernel.project_id, review_id],
        ReviewRow::from_row,
    )?;
    let Some(row) = row else {
        return Ok(false);
    };
    let requester = kernel.agent_by_actor(&row.requested_by_actor_id)?;
    let parties = message_notices::notice_parties(kernel)?;
    let recipient: Option<AgentRow> = match requester {
        Some(requester) if requester.state == "active" && requester.kind == "Developer" => {
            Some(requester)
        }
        _ => parties.as_ref().map(|p| p.pm.clone()),
    };
    let Some(recipient) = recipient else {
        return Ok(false);
    };
    let body = review_notice(&row, &review_author_ids(kernel, &row)?);
    let sender = match &parties {
        Some(parties) => parties.controller_actor_id.clone(),
        None => kernel.controller_actor_id()?,
    };
    let message_id = messages::insert_queued_message(
        kernel,
        &sender,
        &recipient,
        &body,
        &sha256(&body),
        now,
        false,
    )?;
    execute(
        &kernel.database,
        "UPDATE reviews SET notified_message_id = ? WHERE project_id = ? AND review_id = ? AND notified_message_id IS NULL",
        [&message_id, &kernel.project_id, review_id],
    )?;
    Ok(true)
}

fn records(rows: &[ReviewRow]) -> KernelResult<Value> {
    Ok(Value::Array(
        rows.iter()
            .map(review_value)
            .collect::<KernelResult<Vec<_>>>()?,
    ))
}

/// `unannouncedReviews`.
pub fn unannounced_reviews(
    kernel: &Kernel,
    credential: &str,
    limit: Option<i64>,
) -> KernelResult<Value> {
    kernel.authorize(credential, "controller:reconcile")?;
    let rows = query_all(
        &kernel.database,
        "SELECT * FROM reviews WHERE project_id = ? AND state IN ('passed', 'findings') AND notified_message_id IS NULL ORDER BY sequence LIMIT ?",
        rusqlite::params![kernel.project_id, limit.unwrap_or(50)],
        ReviewRow::from_row,
    )?;
    records(&rows)
}

/// `announceReview`.
pub fn announce_review(
    kernel: &Kernel,
    context: &MutationContext,
    review_id: &str,
) -> KernelResult<Value> {
    safe_id_str(review_id, "review id")?;
    kernel.mutate(
        context,
        "review.announce",
        "controller:reconcile",
        &json!({"reviewId": review_id}),
        |_| {
            let announced = announce(kernel, review_id, &kernel.now())?;
            Ok(MutationOutput::new(
                json!({"announced": announced}),
                MutationEvent::new("review", review_id, 0)
                    .with_details(json!({"announced": announced})),
            ))
        },
    )
}

/// `reviews`.
pub fn reviews(kernel: &Kernel, credential: &str, limit: Option<i64>) -> KernelResult<Value> {
    kernel.authorize(credential, "controller:reconcile")?;
    let rows = query_all(
        &kernel.database,
        "SELECT * FROM reviews WHERE project_id = ? ORDER BY sequence DESC LIMIT ?",
        rusqlite::params![kernel.project_id, limit.unwrap_or(20)],
        ReviewRow::from_row,
    )?;
    records(&rows)
}

/// `reviewsToRelease`: finished reviews whose reviewer agent is still active.
pub fn reviews_to_release(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    kernel.authorize(credential, "controller:reconcile")?;
    let rows = query_all(
        &kernel.database,
        "SELECT r.* FROM reviews r JOIN agents a ON a.project_id = r.project_id AND a.agent_id = r.reviewer_agent_id
         WHERE r.project_id = ? AND r.state IN ('passed', 'findings') AND a.state = 'active' ORDER BY r.sequence",
        [&kernel.project_id],
        ReviewRow::from_row,
    )?;
    records(&rows)
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "checkReviewRequest" => {
            let subject_id = arg!(a.str(0, "subjectId"));
            let reviewer_role = arg!(a.str(1, "reviewerRole"));
            Some(call(check_review_request(
                kernel,
                subject_id,
                reviewer_role,
            )))
        }
        "beginReview" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(begin_review(kernel, &context, input)))
        }
        "completeReview" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(complete_review(kernel, &context, input)))
        }
        "unannouncedReviews" => {
            let credential = arg!(a.str(0, "credential"));
            let limit = arg!(a.opt_i64(1, "limit"));
            Some(call(unannounced_reviews(kernel, credential, limit)))
        }
        "announceReview" => {
            let context = arg!(a.ctx(0));
            let review_id = arg!(a.str(1, "reviewId"));
            Some(call(announce_review(kernel, &context, review_id)))
        }
        "reviews" => {
            let credential = arg!(a.str(0, "credential"));
            let limit = arg!(a.opt_i64(1, "limit"));
            Some(call(reviews(kernel, credential, limit)))
        }
        "reviewsToRelease" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(reviews_to_release(kernel, credential)))
        }
        _ => None,
    }
}
