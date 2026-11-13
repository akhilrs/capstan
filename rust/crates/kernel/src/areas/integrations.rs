//! The integrations (src/controller/integrations.ts).

use crate::canonical::sha256;
use crate::dispatch::{arg, call, Args};
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{
    execute, exists, integration_conflict_notice, query_all, query_opt, safe_id, safe_id_str,
};
use crate::json::stringify;
use crate::kernel::Kernel;
use crate::plan_body::{package_naming, package_of_body};
use crate::records::{
    AgentReportRow, MutationEvent, MutationOutput, MAX_CONFLICT_FILES, MAX_CONFLICT_PATH_CHARS,
    MAX_INTEGRATION_REPORTS, PATH_CUT_MARK_CHARS,
};
use crate::types::MutationContext;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

use super::workspace::{detach_set_member, remove_set_member, reset_set};
use super::{message_notices, messages, plan_packages, reviews};
use crate::integrate::{
    integrate_repo_with, member_branch, member_on_main, supersede, GitRepo, IntegrateError,
    IntegrateResult, IntegrationDeps, Landed, MemberStop, MemberTarget,
};

static SHA1: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9a-f]{40}$").expect("the pattern is valid"));
static PRINTABLE_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\x20-\x7e]+$").expect("the pattern is valid"));
static PRINTABLE_REASON: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\x20-\x7e]{1,300}$").expect("the pattern is valid"));

fn field<'a>(input: &'a Value, key: &str) -> &'a Value {
    input.get(key).unwrap_or(&Value::Null)
}

/// A row of `integrations`.
#[derive(Clone, Debug)]
pub struct IntegrationRow {
    pub integration_id: String,
    pub sequence: i64,
    pub base_sha: String,
    pub branch: String,
    pub requested_by: String,
    pub state: String,
    pub head_sha: Option<String>,
    pub conflict_report_id: Option<String>,
    pub conflict_files_json: Option<String>,
    pub conflict_files_omitted: Option<i64>,
    pub failure_reason: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

impl IntegrationRow {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            integration_id: row.get("integration_id")?,
            sequence: row.get("sequence")?,
            base_sha: row.get("base_sha")?,
            branch: row.get("branch")?,
            requested_by: row.get("requested_by")?,
            state: row.get("state")?,
            head_sha: row.get("head_sha")?,
            conflict_report_id: row.get("conflict_report_id")?,
            conflict_files_json: row.get("conflict_files_json")?,
            conflict_files_omitted: row.get("conflict_files_omitted")?,
            failure_reason: row.get("failure_reason")?,
            created_at: row.get("created_at")?,
            completed_at: row.get("completed_at")?,
        })
    }
}

pub fn integration_row(
    kernel: &Kernel,
    integration_id: &str,
) -> KernelResult<Option<IntegrationRow>> {
    query_opt(
        &kernel.database,
        "SELECT * FROM integrations WHERE project_id = ? AND integration_id = ?",
        [&kernel.project_id, integration_id],
        IntegrationRow::from_row,
    )
}

fn required_row(kernel: &Kernel, integration_id: &str) -> KernelResult<IntegrationRow> {
    integration_row(kernel, integration_id)?
        .ok_or_else(|| KernelError::controller("the integration does not exist"))
}

/// `#integrationRecord`.
fn integration_record(kernel: &Kernel, integration_id: &str) -> KernelResult<Value> {
    let row = required_row(kernel, integration_id)?;
    let conflict_files = match &row.conflict_files_json {
        None => Value::Null,
        Some(text) => serde_json::from_str(text)?,
    };
    let reports: Vec<Value> = reviews::integration_authors(kernel, integration_id)?
        .iter()
        .map(|r| json!({"reportId": r.report_id, "agentId": r.agent_id, "commitSha": r.commit_sha}))
        .collect();
    Ok(json!({
        "integrationId": row.integration_id,
        "sequence": row.sequence,
        "baseSha": row.base_sha,
        "branch": row.branch,
        "requestedBy": row.requested_by,
        "state": row.state,
        "headSha": row.head_sha,
        "conflictReportId": row.conflict_report_id,
        "conflictFilesOmitted": row.conflict_files_omitted,
        "conflictFiles": conflict_files,
        "failureReason": row.failure_reason,
        "reports": reports,
        "createdAt": row.created_at,
        "completedAt": row.completed_at,
    }))
}

fn present(kernel: &Kernel, sql: &str, params: impl rusqlite::Params) -> KernelResult<bool> {
    exists(&kernel.database, sql, params)
}

/// `beginIntegration`.
pub fn begin_integration(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let integration_id = safe_id(field(input, "integrationId"), "integration id")?;
    let requested_by = safe_id(field(input, "requestedBy"), "requester")?;
    match field(input, "baseSha").as_str() {
        Some(sha) if SHA1.is_match(sha) => {}
        _ => {
            return Err(KernelError::type_error(
                "the base commit must be a full lowercase sha1",
            ))
        }
    }
    let base_sha = field(input, "baseSha").as_str().unwrap_or_default();
    let report_ids: Vec<&Value> = match field(input, "reportIds").as_array() {
        Some(ids) if (1..=MAX_INTEGRATION_REPORTS).contains(&ids.len()) => ids.iter().collect(),
        _ => {
            return Err(KernelError::type_error(format!(
                "an integration takes 1 to {MAX_INTEGRATION_REPORTS} reports"
            )))
        }
    };
    let mut ids: Vec<&str> = Vec::new();
    for id in &report_ids {
        ids.push(safe_id(id, "report id")?);
    }
    kernel.mutate(
        context,
        "integration.begin",
        "controller:reconcile",
        input,
        |_| {
            let unique: BTreeSet<&&str> = ids.iter().collect();
            if unique.len() != ids.len() {
                return Err(KernelError::controller("a report is named twice"));
            }
            if present(
                kernel,
                "SELECT 1 AS present FROM integrations WHERE project_id = ? AND state = 'running'",
                [&kernel.project_id],
            )? {
                return Err(KernelError::controller("an integration is already running"));
            }
            // A report of several repos is in one integration per repo: the members of one set do not count as elsewhere.
            let own_set = field(input, "setId").as_str().map(str::to_string);
            for id in &ids {
                let report_state = query_opt(
                    &kernel.database,
                    "SELECT state FROM agent_reports WHERE project_id = ? AND report_id = ?",
                    [&kernel.project_id, *id],
                    |row| row.get::<_, String>(0),
                )?;
                let Some(report_state) = report_state else {
                    return Err(KernelError::controller(format!("report {id} does not exist")));
                };
                if report_state != "accepted" {
                    return Err(KernelError::controller(format!("report {id} was not accepted")));
                }
                let latest = query_opt(
                    &kernel.database,
                    "SELECT state FROM reviews WHERE project_id = ? AND subject_report_id = ? AND state IN ('passed', 'findings')
                     ORDER BY sequence DESC LIMIT 1",
                    [&kernel.project_id, *id],
                    |row| row.get::<_, String>(0),
                )?;
                if latest.as_deref() != Some("passed") {
                    return Err(KernelError::controller(format!(
                        "report {id} has no passed review as its latest verdict"
                    )));
                }
                if present(
                    kernel,
                    "SELECT 1 AS present FROM reviews WHERE project_id = ? AND subject_report_id = ? AND state = 'started'",
                    [&kernel.project_id, *id],
                )? {
                    return Err(KernelError::controller(format!(
                        "report {id} has a review still open"
                    )));
                }
                let elsewhere = query_opt(
                    &kernel.database,
                    "SELECT i.integration_id, i.state FROM integration_reports ir
                     JOIN integrations i ON i.project_id = ir.project_id AND i.integration_id = ir.integration_id
                     WHERE ir.project_id = ? AND ir.report_id = ? AND i.state IN ('confirmed', 'merged')
                     AND NOT EXISTS (SELECT 1 FROM integration_set_members m WHERE m.project_id = i.project_id
                         AND m.integration_id = i.integration_id AND m.set_id = ?)
                     ORDER BY i.sequence DESC LIMIT 1",
                    [&kernel.project_id, *id, own_set.as_deref().unwrap_or("")],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )?;
                if let Some((other_id, other_state)) = elsewhere {
                    return Err(KernelError::controller(if other_state == "confirmed" {
                        format!("report {id} was already integrated and confirmed in {other_id}")
                    } else {
                        format!(
                            "report {id} is already in the unsettled integration {other_id}; confirm or discard that one first"
                        )
                    }));
                }
            }
            for id in &ids {
                let archived = query_opt(
                    &kernel.database,
                    &format!(
                        "SELECT pp.plan_id FROM plan_packages pp JOIN agent_reports r ON r.project_id = pp.project_id
                         JOIN plan_archives a ON a.project_id = pp.project_id AND a.plan_id = pp.plan_id
                         WHERE pp.project_id = ? AND r.report_id = ? AND {} LIMIT 1",
                        super::plan_landing::PACKAGE_REPORT_PREDICATE
                    ),
                    [&kernel.project_id, *id],
                    |row| row.get::<_, String>(0),
                )?;
                if let Some(plan_id) = archived {
                    super::plan_archive::refuse_if_archived(kernel, &plan_id)?;
                }
            }
            check_integration_order(kernel, &ids)?;
            let now = kernel.now();
            let sequence = query_opt(
                &kernel.database,
                "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM integrations WHERE project_id = ?",
                [&kernel.project_id],
                |row| row.get::<_, i64>(0),
            )?
            .unwrap_or(1);
            // Node binds `input.branch` as given: an absent one cannot be bound.
            let Some(branch) = field(input, "branch").as_str() else {
                return Err(KernelError::type_error(
                    "Provided value cannot be bound to SQLite parameter 5.",
                ));
            };
            execute(
                &kernel.database,
                "INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, 'running', ?)",
                rusqlite::params![
                    kernel.project_id,
                    integration_id,
                    sequence,
                    base_sha,
                    branch,
                    requested_by,
                    now
                ],
            )?;
            for (index, report_id) in ids.iter().enumerate() {
                execute(
                    &kernel.database,
                    "INSERT INTO integration_reports(project_id, integration_id, position, report_id) VALUES (?, ?, ?, ?)",
                    rusqlite::params![kernel.project_id, integration_id, index as i64 + 1, report_id],
                )?;
            }
            let mut event = MutationEvent::new("integration", integration_id, 0).with_details(
                json!({"baseSha": base_sha, "reportIds": ids, "requestedBy": requested_by}),
            );
            event.to_state = Some("running".into());
            Ok(MutationOutput::new(integration_record(kernel, integration_id)?, event))
        },
    )
}

/// The most bytes a stored `details_json` may hold.
pub const INTEGRATION_DETAILS_MAX_BYTES: usize = 16 * 1024;

/// `finishIntegration`.
pub fn finish_integration(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    finish_integration_with_details(kernel, context, input, None)
}

/// `finishIntegration`, and for a merged outcome the details of the merge (see `integration_impact`), inserted in the
/// same mutation. The details are not part of the input, so the recorded request, its hash and the event are the same
/// with and without them; a repeat of the mutation is answered from the stored request and inserts nothing.
pub fn finish_integration_with_details(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
    details: Option<&Value>,
) -> KernelResult<Value> {
    let integration_id = safe_id(field(input, "integrationId"), "integration id")?;
    let outcome = field(input, "outcome");
    let kind = field(outcome, "kind").as_str().unwrap_or_default();
    if kind == "merged" {
        match field(outcome, "headSha").as_str() {
            Some(sha) if SHA1.is_match(sha) => {}
            _ => {
                return Err(KernelError::type_error(
                    "the integrated commit must be a full lowercase sha1",
                ))
            }
        }
    }
    let mut files: Vec<&str> = Vec::new();
    let mut omitted: i64 = 0;
    if kind == "conflicted" {
        let Some(list) = field(outcome, "files").as_array() else {
            return Err(KernelError::type_error(
                "Cannot read properties of undefined (reading 'length')",
            ));
        };
        let omitted_value = field(outcome, "omitted");
        let omitted_ok = omitted_value
            .as_i64()
            .or_else(|| {
                omitted_value
                    .as_f64()
                    .filter(|f| f.fract() == 0.0)
                    .map(|f| f as i64)
            })
            .filter(|n| *n >= 0);
        let names: Vec<Option<&str>> = list.iter().map(Value::as_str).collect();
        let bad_file = names.iter().any(|name| match name {
            Some(file) => {
                !PRINTABLE_PATH.is_match(file)
                    || file.encode_utf16().count() > MAX_CONFLICT_PATH_CHARS + PATH_CUT_MARK_CHARS
            }
            None => true,
        });
        if list.is_empty() || list.len() > MAX_CONFLICT_FILES || omitted_ok.is_none() || bad_file {
            return Err(KernelError::type_error(
                "the conflict files must be a short list of printable paths",
            ));
        }
        omitted = omitted_ok.unwrap_or(0);
        files = names.into_iter().flatten().collect();
    }
    if kind == "failed" {
        match field(outcome, "reason").as_str() {
            Some(reason) if PRINTABLE_REASON.is_match(reason) => {}
            _ => {
                return Err(KernelError::type_error(
                    "the failure reason must be short printable text",
                ))
            }
        }
    }
    kernel.mutate(
        context,
        "integration.finish",
        "controller:reconcile",
        &json!({"integrationId": integration_id, "outcome": outcome}),
        |_| {
            let row = required_row(kernel, integration_id)?;
            if row.state != "running" {
                return Err(KernelError::controller("the integration is not running"));
            }
            let now = kernel.now();
            let conflict_report_id = field(outcome, "reportId").as_str();
            execute(
                &kernel.database,
                "UPDATE integrations SET state = ?, head_sha = ?, conflict_report_id = ?, conflict_files_json = ?, conflict_files_omitted = ?, failure_reason = ?, completed_at = ?
                 WHERE project_id = ? AND integration_id = ?",
                rusqlite::params![
                    kind,
                    if kind == "merged" { field(outcome, "headSha").as_str() } else { None },
                    if kind == "conflicted" { conflict_report_id } else { None },
                    if kind == "conflicted" { Some(stringify(&json!(files))) } else { None },
                    if kind == "conflicted" { Some(omitted) } else { None },
                    if kind == "failed" { field(outcome, "reason").as_str() } else { None },
                    now,
                    kernel.project_id,
                    integration_id
                ],
            )?;
            if let Some(details) = details.filter(|_| kind == "merged") {
                let stored = stringify(details);
                // The builder keeps the text within the table's cap; a longer one is dropped rather than failing the merge.
                if stored.len() <= INTEGRATION_DETAILS_MAX_BYTES {
                    execute(
                        &kernel.database,
                        "INSERT OR IGNORE INTO integration_details(project_id, integration_id, details_json, computed_at) VALUES (?, ?, ?, ?)",
                        rusqlite::params![kernel.project_id, integration_id, stored, now],
                    )?;
                }
            }
            if kind == "conflicted" && row.requested_by == "operator" {
                notice_conflict(
                    kernel,
                    integration_id,
                    conflict_report_id.unwrap_or_default(),
                    &files,
                    omitted,
                    &now,
                )?;
            }
            let details = match kind {
                "conflicted" => json!({"reportId": conflict_report_id, "files": files, "omitted": omitted}),
                "failed" => json!({"reason": field(outcome, "reason")}),
                _ => json!({"headSha": field(outcome, "headSha")}),
            };
            let event = MutationEvent::new("integration", integration_id, 0)
                .transition("running", kind)
                .with_details(details);
            Ok(MutationOutput::new(integration_record(kernel, integration_id)?, event))
        },
    )
}

fn notice_conflict(
    kernel: &Kernel,
    integration_id: &str,
    report_id: &str,
    files: &[&str],
    omitted: i64,
    now: &str,
) -> KernelResult<()> {
    let Some(parties) = message_notices::notice_parties(kernel)? else {
        return Ok(());
    };
    let files: Vec<String> = files.iter().map(|f| f.to_string()).collect();
    let body = integration_conflict_notice(integration_id, report_id, &files, omitted as usize);
    messages::insert_queued_message(
        kernel,
        &parties.controller_actor_id,
        &parties.pm,
        &body,
        &sha256(&body),
        now,
        true,
        messages::class_for_kind(kernel, "integration_conflict"),
    )?;
    Ok(())
}

/// `settleIntegration`.
pub fn settle_integration(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let integration_id = safe_id(field(input, "integrationId"), "integration id")?;
    let outcome = match field(input, "outcome").as_str() {
        Some(outcome @ ("confirmed" | "discarded")) => outcome,
        _ => {
            return Err(KernelError::type_error(
                "the outcome must be confirmed or discarded",
            ))
        }
    };
    // Kernel-internal: only a daemon handler that built this input itself sets it; no wire parameter reaches here.
    let member = match field(input, "member") {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        _ => return Err(KernelError::type_error("member must be true or false")),
    };
    // Kernel-internal as well: a redo lets go of a member whose set review is still running; that review no longer counts.
    let redo = member && field(input, "redo") == &Value::Bool(true);
    kernel.mutate(
        context,
        "integration.settle",
        "controller:reconcile",
        input,
        |_| apply_settle_checked(kernel, integration_id, outcome, member, !redo),
    )
}

/// Refuses to settle an integration that is a member of a set: confirm and discard per repo come with plan-184.
pub fn refuse_set_member(kernel: &Kernel, integration_id: &str) -> KernelResult<()> {
    match super::workspace::set_of_integration(kernel, integration_id)? {
        Some(set_id) => Err(KernelError::controller(format!(
            "integration {integration_id} belongs to set {set_id}; confirm and discard per repo come with plan-184"
        ))),
        None => Ok(()),
    }
}

/// The body of `integration.settle`, run inside a mutation: refused unless the integration is merged (or conflicted,
/// for a discard) and no review of it is open; a confirm needs its latest review passed.
pub fn apply_settle(
    kernel: &Kernel,
    integration_id: &str,
    outcome: &str,
) -> KernelResult<MutationOutput> {
    apply_settle_as(kernel, integration_id, outcome, false)
}

/// `apply_settle` for a set member when `member` is true (and refused for any integration that is not a member); without
/// it a member is refused as before.
pub fn apply_settle_as(
    kernel: &Kernel,
    integration_id: &str,
    outcome: &str,
    member: bool,
) -> KernelResult<MutationOutput> {
    apply_settle_checked(kernel, integration_id, outcome, member, true)
}

/// `apply_settle_as`, with `check_review` false for a redo's discard: an open review does not hold it back.
fn apply_settle_checked(
    kernel: &Kernel,
    integration_id: &str,
    outcome: &str,
    member: bool,
    check_review: bool,
) -> KernelResult<MutationOutput> {
    let row = required_row(kernel, integration_id)?;
    if member {
        if super::workspace::set_of_integration(kernel, integration_id)?.is_none() {
            return Err(KernelError::controller(format!(
                "integration {integration_id} is not a set member"
            )));
        }
    } else {
        refuse_set_member(kernel, integration_id)?;
    }
    // A conflicted integration holds no branch; it can only be discarded.
    let discarding_conflicted = row.state == "conflicted" && outcome == "discarded";
    if row.state != "merged" && !discarding_conflicted {
        return Err(KernelError::controller(
            "only a merged integration can be settled",
        ));
    }
    if check_review
        && present(
            kernel,
            "SELECT 1 AS present FROM reviews WHERE project_id = ? AND subject_integration_id = ? AND state = 'started'",
            [&kernel.project_id, integration_id],
        )?
    {
        return Err(KernelError::controller(
            "a review of this integration is open",
        ));
    }
    if check_review && member && outcome == "confirmed" {
        if let Some(set_id) = super::workspace::set_of_integration(kernel, integration_id)? {
            if present(
                kernel,
                "SELECT 1 AS present FROM reviews WHERE project_id = ? AND state = 'started' AND subject_integration_id IN
                   (SELECT integration_id FROM integration_set_members WHERE project_id = ? AND integration_id IS NOT NULL AND set_id = ?)",
                [&kernel.project_id, &kernel.project_id, &set_id],
            )? {
                return Err(KernelError::controller(format!(
                    "a review of set {set_id} is open"
                )));
            }
        }
    }
    if outcome == "confirmed" {
        let latest = query_opt(
            &kernel.database,
            "SELECT state FROM reviews WHERE project_id = ? AND subject_integration_id = ? AND state IN ('passed', 'findings')
             ORDER BY sequence DESC LIMIT 1",
            [&kernel.project_id, integration_id],
            |r| r.get::<_, String>(0),
        )?;
        // A set is reviewed as one subject (its first live member): a member passes with the set's review.
        let set_passed = match (
            member,
            super::workspace::set_of_integration(kernel, integration_id)?,
        ) {
            (true, Some(set_id)) => {
                super::workspace::set_gate(kernel, &set_id, "")?["review"]["state"] == "passed"
            }
            _ => false,
        };
        if latest.as_deref() != Some("passed") && !set_passed {
            return Err(KernelError::controller(
                "an integration is confirmed only after its latest review passed",
            ));
        }
    }
    execute(
        &kernel.database,
        "UPDATE integrations SET state = ? WHERE project_id = ? AND integration_id = ?",
        [outcome, &kernel.project_id, integration_id],
    )?;
    let event = MutationEvent::new("integration", integration_id, 0)
        .transition(&row.state, outcome)
        .with_details(json!({}));
    Ok(MutationOutput::new(
        integration_record(kernel, integration_id)?,
        event,
    ))
}

/// A plan package's report may only be integrated after the reports of its depends_on packages: those must already be in a
/// merged or confirmed integration, or come earlier in this list. Reports outside plans are not checked. Only approved,
/// uncancelled plans count, cancelled packages are not checked, and a cancelled dependency is skipped. A dependency that is
/// not cancelled but has no report yet is refused.
fn check_integration_order(kernel: &Kernel, report_ids: &[&str]) -> KernelResult<()> {
    let position: HashMap<&str, usize> = report_ids
        .iter()
        .enumerate()
        .map(|(index, id)| (*id, index))
        .collect();
    let plans = query_all(
        &kernel.database,
        "SELECT plan_id, approved_revision FROM plans WHERE project_id = ? AND state = 'approved' AND cancelled_at IS NULL AND approved_revision IS NOT NULL",
        [&kernel.project_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
    )?;
    for (plan_id, approved_revision) in plans {
        let rows = query_all(
            &kernel.database,
            "SELECT package_id, assignee_agent_id, assigned_at, cancelled_at FROM plan_packages WHERE project_id = ? AND plan_id = ?",
            [&kernel.project_id, &plan_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )?;
        let cancelled: BTreeSet<&str> = rows
            .iter()
            .filter(|r| r.3.is_some())
            .map(|r| r.0.as_str())
            .collect();
        // A Map keyed by package id, in row order.
        let mut reports: Vec<(&str, Option<String>)> = Vec::new();
        for (package_id, assignee, assigned_at, _) in &rows {
            let report = plan_packages::package_report(
                kernel,
                &plan_id,
                package_id,
                assignee.as_deref(),
                assigned_at.as_deref(),
            )?;
            reports.push((package_id.as_str(), report));
        }
        let report_of = |package: &str| -> Option<&Option<String>> {
            reports
                .iter()
                .find(|(id, _)| *id == package)
                .map(|(_, r)| r)
        };
        for (package_id, report_id) in &reports {
            let Some(report_id) = report_id else { continue };
            let Some(at) = position.get(report_id.as_str()).copied() else {
                continue;
            };
            if cancelled.contains(package_id) {
                continue;
            }
            let revision = query_opt(
                &kernel.database,
                "SELECT body_json FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
                rusqlite::params![kernel.project_id, plan_id, approved_revision],
                |row| row.get::<_, String>(0),
            )?;
            let view = revision.and_then(|body| package_of_body(&body, package_id));
            for dep in view.map(|v| v.depends_on).unwrap_or_default() {
                if cancelled.contains(dep.as_str()) {
                    continue;
                }
                let dep_report = report_of(&dep).and_then(|r| r.as_deref());
                let Some(dep_report) = dep_report else {
                    return Err(KernelError::controller(format!(
                        "integration_order: report {report_id} (package {package_id}) needs {dep}, which has no report yet"
                    )));
                };
                if position
                    .get(dep_report)
                    .is_some_and(|earlier| *earlier < at)
                {
                    continue;
                }
                if present(
                    kernel,
                    "SELECT 1 AS present FROM integration_reports ir
                     JOIN integrations i ON i.project_id = ir.project_id AND i.integration_id = ir.integration_id
                     WHERE ir.project_id = ? AND ir.report_id = ? AND i.state IN ('merged', 'confirmed')",
                    [&kernel.project_id, dep_report],
                )? {
                    continue;
                }
                return Err(KernelError::controller(format!(
                    "integration_order: report {report_id} (package {package_id}) needs {dep} integrated first or earlier in this list"
                )));
            }
        }
    }
    Ok(())
}

/// `reportInConfirmedIntegration`.
pub(crate) fn report_in_confirmed_integration(
    kernel: &Kernel,
    report_id: &str,
) -> KernelResult<bool> {
    Ok(present(
        kernel,
        "SELECT 1 AS present FROM integration_reports ir
         JOIN integrations i ON i.project_id = ir.project_id AND i.integration_id = ir.integration_id
         WHERE ir.project_id = ? AND ir.report_id = ? AND i.state = 'confirmed'",
        [&kernel.project_id, report_id],
    )? || present(
        kernel,
        "SELECT 1 AS present FROM integration_covered_reports c
         JOIN integrations i ON i.project_id = c.project_id AND i.integration_id = c.integration_id
         WHERE c.project_id = ? AND c.report_id = ? AND i.state = 'confirmed'",
        [&kernel.project_id, report_id],
    )?)
}

/// `integration`.
pub fn integration(kernel: &Kernel, integration_id: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    safe_id_str(integration_id, "integration id")?;
    integration_record(kernel, integration_id)
}

/// `integrationCommitInfo`: what the squash commit of an integration says.
pub fn integration_commit_info(kernel: &Kernel, integration_id: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    safe_id_str(integration_id, "integration id")?;
    commit_info(
        kernel,
        &reviews::integration_authors(kernel, integration_id)?,
    )
}

/// `plannedCommitInfo`: the commit info of reports that are not yet an integration, in the order given.
pub fn planned_commit_info(kernel: &Kernel, report_ids: &Value) -> KernelResult<Value> {
    kernel.assert_open()?;
    // A string is iterable in JavaScript: `for (const id of "r1")` visits its characters.
    let characters: Vec<Value>;
    let ids = match report_ids {
        Value::Array(ids) => ids,
        Value::String(text) => {
            characters = text.chars().map(|c| Value::String(c.to_string())).collect();
            &characters
        }
        _ => return Err(KernelError::type_error("reportIds is not iterable")),
    };
    let mut reports: Vec<AgentReportRow> = Vec::new();
    for id in ids {
        let id = safe_id(id, "report id")?;
        let row = query_opt(
            &kernel.database,
            "SELECT * FROM agent_reports WHERE project_id = ? AND report_id = ?",
            [&kernel.project_id, id],
            AgentReportRow::from_row,
        )?;
        if let Some(row) = row {
            reports.push(row);
        }
    }
    commit_info(kernel, &reports)
}

/// Whether an integration set already uses `branch` as its base name.
pub fn set_branch_recorded(kernel: &Kernel, branch: &str) -> KernelResult<bool> {
    kernel.assert_open()?;
    present(
        kernel,
        "SELECT 1 AS present FROM integration_sets WHERE project_id = ? AND branch = ?",
        [&kernel.project_id, branch],
    )
}

/// One commit a workspace report names for a repo, with the report's author and place in the report order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportCommit {
    pub report_id: String,
    pub agent_id: String,
    pub sequence: i64,
    pub repo: String,
    pub commit_sha: String,
}

/// The author and sequence of each report, by id; a report that does not exist is left out.
pub fn report_order(
    kernel: &Kernel,
    report_ids: &[String],
) -> KernelResult<Vec<(String, String, i64)>> {
    kernel.assert_open()?;
    let mut rows = Vec::new();
    for id in report_ids {
        safe_id_str(id, "report id")?;
        if let Some(row) = query_opt(
            &kernel.database,
            "SELECT report_id, agent_id, sequence FROM agent_reports WHERE project_id = ? AND report_id = ?",
            [&kernel.project_id, id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?)),
        )? {
            rows.push(row);
        }
    }
    Ok(rows)
}

/// The commits per repo of the given reports, in report order. A report without `report_commits` rows was made
/// before workspace mode and is refused.
pub fn workspace_commits(
    kernel: &Kernel,
    report_ids: &[String],
) -> KernelResult<Vec<ReportCommit>> {
    kernel.assert_open()?;
    let mut commits = Vec::new();
    for id in report_ids {
        safe_id_str(id, "report id")?;
        let Some((_, agent_id, sequence)) = report_order(kernel, std::slice::from_ref(id))?
            .into_iter()
            .next()
        else {
            return Err(KernelError::controller(format!(
                "report {id} does not exist"
            )));
        };
        let rows = super::workspace::report_commits(kernel, id)?;
        let rows = rows.as_array().cloned().unwrap_or_default();
        if rows.is_empty() {
            return Err(KernelError::controller(format!(
                "report {id} predates workspace mode; report again"
            )));
        }
        for row in rows {
            commits.push(ReportCommit {
                report_id: id.clone(),
                agent_id: agent_id.clone(),
                sequence,
                repo: field(&row, "repo").as_str().unwrap_or_default().to_string(),
                commit_sha: field(&row, "commitSha")
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            });
        }
    }
    commits.sort_by_key(|c| c.sequence);
    Ok(commits)
}

/// `integrationBranchRecorded`.
pub fn integration_branch_recorded(kernel: &Kernel, branch: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    Ok(Value::Bool(present(
        kernel,
        "SELECT 1 AS present FROM integrations WHERE project_id = ? AND branch = ?",
        [&kernel.project_id, branch],
    )?))
}

struct PlanOfReports {
    plan_id: String,
    title: String,
    approved_revision: Option<i64>,
}

fn commit_info(kernel: &Kernel, reports: &[AgentReportRow]) -> KernelResult<Value> {
    // A Map keyed by plan id: a later set replaces the value and keeps the position.
    let mut plans: Vec<PlanOfReports> = Vec::new();
    for report in reports {
        let rows = query_all(
            &kernel.database,
            "SELECT DISTINCT pl.plan_id, pl.title, pl.approved_revision FROM plan_packages pp
             JOIN agents a ON a.project_id = pp.project_id AND a.agent_id = pp.assignee_agent_id
             JOIN plans pl ON pl.project_id = pp.project_id AND pl.plan_id = pp.plan_id
             WHERE pp.project_id = ? AND pp.assignee_agent_id = ? AND a.generation = ?",
            rusqlite::params![kernel.project_id, report.agent_id, report.generation],
            |row| {
                Ok(PlanOfReports {
                    plan_id: row.get(0)?,
                    title: row.get(1)?,
                    approved_revision: row.get(2)?,
                })
            },
        )?;
        for row in rows {
            match plans.iter().position(|p| p.plan_id == row.plan_id) {
                Some(index) => plans[index] = row,
                None => plans.push(row),
            }
        }
    }
    let mut common: Vec<&PlanOfReports> = Vec::new();
    for plan in &plans {
        let mut every = true;
        for report in reports {
            let holds = present(
                kernel,
                "SELECT 1 FROM plan_packages pp JOIN agents a ON a.project_id = pp.project_id AND a.agent_id = pp.assignee_agent_id
                 WHERE pp.project_id = ? AND pp.plan_id = ? AND pp.assignee_agent_id = ? AND a.generation = ?",
                rusqlite::params![kernel.project_id, plan.plan_id, report.agent_id, report.generation],
            )?;
            if !holds {
                every = false;
                break;
            }
        }
        if every {
            common.push(plan);
        }
    }
    let plan = if !reports.is_empty() && common.len() == 1 {
        Some(common[0])
    } else {
        None
    };
    let mut packages: Vec<Value> = Vec::new();
    if let Some(plan) = plan {
        if let Some(approved_revision) = plan.approved_revision {
            let body = query_opt(
                &kernel.database,
                "SELECT body_json FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
                rusqlite::params![kernel.project_id, plan.plan_id, approved_revision],
                |row| row.get::<_, String>(0),
            )?;
            let mut agents: Vec<&str> = Vec::new();
            for report in reports {
                if !agents.contains(&report.agent_id.as_str()) {
                    agents.push(report.agent_id.as_str());
                }
            }
            let mut seen: Vec<String> = Vec::new();
            for agent_id in agents {
                let assigned = query_all(
                    &kernel.database,
                    "SELECT package_id FROM plan_packages WHERE project_id = ? AND plan_id = ? AND assignee_agent_id = ? ORDER BY package_id",
                    [&kernel.project_id, &plan.plan_id, agent_id],
                    |row| row.get::<_, String>(0),
                )?;
                for package_id in assigned {
                    if seen.contains(&package_id) {
                        continue;
                    }
                    let mut entry = Map::new();
                    entry.insert("packageId".into(), json!(package_id));
                    match &body {
                        None => {
                            entry.insert("type".into(), Value::Null);
                            entry.insert("scope".into(), Value::Null);
                            entry.insert("breaking".into(), Value::Bool(false));
                        }
                        Some(body) => {
                            let naming = package_naming(body, &package_id);
                            entry.insert("type".into(), json!(naming.kind));
                            entry.insert("scope".into(), json!(naming.scope));
                            entry.insert("breaking".into(), Value::Bool(naming.breaking));
                        }
                    }
                    packages.push(Value::Object(entry));
                    seen.push(package_id);
                }
            }
        }
    }
    Ok(json!({
        "planId": plan.map(|p| p.plan_id.as_str()),
        "planTitle": plan.map(|p| p.title.as_str()),
        "packages": packages,
        "reports": reports.iter().map(|r| json!({
            "reportId": r.report_id,
            "agentId": r.agent_id,
            "summary": r.summary,
            "branch": r.branch,
        })).collect::<Vec<_>>(),
    }))
}

fn integration_records(kernel: &Kernel, ids: Vec<String>) -> KernelResult<Value> {
    Ok(Value::Array(
        ids.iter()
            .map(|id| integration_record(kernel, id))
            .collect::<KernelResult<Vec<_>>>()?,
    ))
}

/// `integrations`.
pub fn integrations(kernel: &Kernel, credential: &str, limit: Option<i64>) -> KernelResult<Value> {
    kernel.authorize(credential, "controller:reconcile")?;
    let ids = query_all(
        &kernel.database,
        "SELECT integration_id FROM integrations WHERE project_id = ? ORDER BY sequence DESC LIMIT ?",
        rusqlite::params![kernel.project_id, limit.unwrap_or(20)],
        |row| row.get::<_, String>(0),
    )?;
    integration_records(kernel, ids)
}

/// `settledIntegrations`: confirmed or discarded integrations, newest first.
pub fn settled_integrations(
    kernel: &Kernel,
    credential: &str,
    limit: Option<i64>,
) -> KernelResult<Value> {
    kernel.authorize(credential, "controller:reconcile")?;
    let ids = query_all(
        &kernel.database,
        "SELECT integration_id FROM integrations WHERE project_id = ? AND state IN ('confirmed', 'discarded') ORDER BY sequence DESC LIMIT ?",
        rusqlite::params![kernel.project_id, limit.unwrap_or(200)],
        |row| row.get::<_, String>(0),
    )?;
    integration_records(kernel, ids)
}

/// `coverageCandidates`: reports a confirmed integration may cover without having merged them; null for an integration
/// that is not confirmed.
pub fn coverage_candidates(
    kernel: &Kernel,
    credential: &str,
    integration_id: &str,
) -> KernelResult<Value> {
    kernel.authorize(credential, "controller:reconcile")?;
    safe_id_str(integration_id, "integration id")?;
    let integration = required_row(kernel, integration_id)?;
    let Some(head_sha) = integration
        .head_sha
        .clone()
        .filter(|_| integration.state == "confirmed")
    else {
        return Ok(Value::Null);
    };
    let rows = query_all(
        &kernel.database,
        "SELECT r.report_id, r.commit_sha FROM agent_reports r
         WHERE r.project_id = ? AND r.state = 'accepted' AND r.created_at <= ?
           AND NOT EXISTS (
             SELECT 1 FROM agent_reports later
             WHERE later.project_id = r.project_id AND later.agent_id = r.agent_id
               AND later.generation = r.generation AND later.state = 'accepted' AND later.sequence > r.sequence)
           AND NOT EXISTS (
             SELECT 1 FROM integration_reports ir
             JOIN integrations i ON i.project_id = ir.project_id AND i.integration_id = ir.integration_id
             WHERE ir.project_id = r.project_id AND ir.report_id = r.report_id AND i.state = 'confirmed')
           AND NOT EXISTS (
             SELECT 1 FROM integration_covered_reports c
             WHERE c.project_id = r.project_id AND c.integration_id = ? AND c.report_id = r.report_id)
         ORDER BY r.sequence",
        rusqlite::params![kernel.project_id, integration.created_at, integration_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    let member_commits: Vec<String> = reviews::integration_authors(kernel, integration_id)?
        .into_iter()
        .map(|r| r.commit_sha)
        .collect();
    let mut reports = Vec::new();
    for (report_id, commit_sha) in rows {
        let heads = query_all(
            &kernel.database,
            "SELECT i.head_sha FROM integration_reports ir
             JOIN integrations i ON i.project_id = ir.project_id AND i.integration_id = ir.integration_id
             WHERE ir.project_id = ? AND ir.report_id = ? AND i.integration_id <> ? AND i.head_sha IS NOT NULL
             ORDER BY i.sequence",
            [&kernel.project_id, &report_id, integration_id],
            |row| row.get::<_, String>(0),
        )?;
        reports.push(
            json!({"reportId": report_id, "commitSha": commit_sha, "integrationHeads": heads}),
        );
    }
    Ok(json!({"headSha": head_sha, "memberCommits": member_commits, "reports": reports}))
}

/// `recordCoveredReports`: stores which reports a confirmed integration covers; storing the same row again changes nothing.
pub fn record_covered_reports(
    kernel: &Kernel,
    credential: &str,
    integration_id: &str,
    covered: &Value,
) -> KernelResult<Value> {
    kernel.authorize(credential, "controller:reconcile")?;
    safe_id_str(integration_id, "integration id")?;
    let Some(rows) = covered.as_array() else {
        return Err(KernelError::type_error("covered is not iterable"));
    };
    kernel.database.exec("BEGIN")?;
    let outcome = (|| -> KernelResult<()> {
        for row in rows {
            let (Some(report_id), Some(how)) =
                (field(row, "reportId").as_str(), field(row, "how").as_str())
            else {
                return Err(KernelError::type_error(
                    "Provided value cannot be bound to SQLite parameter 3.",
                ));
            };
            execute(
                &kernel.database,
                "INSERT OR IGNORE INTO integration_covered_reports(project_id, integration_id, report_id, how) VALUES (?, ?, ?, ?)",
                [&kernel.project_id, integration_id, report_id, how],
            )?;
        }
        Ok(())
    })();
    match outcome {
        Ok(()) => kernel.database.exec("COMMIT")?,
        Err(error) => {
            let _ = kernel.database.exec("ROLLBACK");
            return Err(error);
        }
    }
    Ok(Value::Null)
}

/// `runningIntegrations`.
pub fn running_integrations(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    kernel.authorize(credential, "controller:reconcile")?;
    let ids = query_all(
        &kernel.database,
        "SELECT integration_id FROM integrations WHERE project_id = ? AND state = 'running' ORDER BY sequence",
        [&kernel.project_id],
        |row| row.get::<_, String>(0),
    )?;
    integration_records(kernel, ids)
}

/// `mergedIntegrationsForSummary`; `agents.rs` calls it once its placeholder is swapped for this.
pub(crate) fn merged_integrations_for_summary(kernel: &Kernel) -> KernelResult<Vec<Value>> {
    query_all(
        &kernel.database,
        "SELECT integration_id, branch, head_sha FROM integrations WHERE project_id = ? AND state = 'merged' ORDER BY sequence",
        [&kernel.project_id],
        |row| {
            Ok(json!({
                "integrationId": row.get::<_, String>(0)?,
                "branch": row.get::<_, String>(1)?,
                "headSha": row.get::<_, Option<String>>(2)?,
            }))
        },
    )
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "beginIntegration" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(begin_integration(kernel, &context, input)))
        }
        "finishIntegration" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(finish_integration(kernel, &context, input)))
        }
        "settleIntegration" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(settle_integration(kernel, &context, input)))
        }
        "integration" => {
            let integration_id = arg!(a.str(0, "integrationId"));
            Some(call(integration(kernel, integration_id)))
        }
        "integrationCommitInfo" => {
            let integration_id = arg!(a.str(0, "integrationId"));
            Some(call(integration_commit_info(kernel, integration_id)))
        }
        "plannedCommitInfo" => {
            let report_ids = a.value(0);
            Some(call(planned_commit_info(kernel, report_ids)))
        }
        "integrationBranchRecorded" => {
            let branch = arg!(a.str(0, "branch"));
            Some(call(integration_branch_recorded(kernel, branch)))
        }
        "integrations" => {
            let credential = arg!(a.str(0, "credential"));
            let limit = arg!(a.opt_i64(1, "limit"));
            Some(call(integrations(kernel, credential, limit)))
        }
        "settledIntegrations" => {
            let credential = arg!(a.str(0, "credential"));
            let limit = arg!(a.opt_i64(1, "limit"));
            Some(call(settled_integrations(kernel, credential, limit)))
        }
        "coverageCandidates" => {
            let credential = arg!(a.str(0, "credential"));
            let integration_id = arg!(a.str(1, "integrationId"));
            Some(call(coverage_candidates(
                kernel,
                credential,
                integration_id,
            )))
        }
        "recordCoveredReports" => {
            let credential = arg!(a.str(0, "credential"));
            let integration_id = arg!(a.str(1, "integrationId"));
            let covered = a.value(2);
            Some(call(record_covered_reports(
                kernel,
                credential,
                integration_id,
                covered,
            )))
        }
        "runningIntegrations" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(running_integrations(kernel, credential)))
        }
        _ => None,
    }
}

/// `integrationDetails`: the details recorded when the integration was merged, or `None` (an integration that did not
/// merge, a recovered finish, or one merged before details were recorded). One primary-key read.
pub fn integration_details(kernel: &Kernel, integration_id: &str) -> KernelResult<Option<Value>> {
    kernel.assert_open()?;
    safe_id_str(integration_id, "integration id")?;
    let stored = query_opt(
        &kernel.database,
        "SELECT details_json FROM integration_details WHERE project_id = ? AND integration_id = ?",
        [&kernel.project_id, integration_id],
        |row| row.get::<_, String>(0),
    )?;
    Ok(stored.and_then(|text| serde_json::from_str(&text).ok()))
}

/// The package each of the first `limit` reports was written for: the one package of the report's agent generation
/// (within `plan_id` when given). A report with none or several packages has no entry. `commit_info` links reports to
/// packages the same way.
pub(crate) fn report_package_ids(
    kernel: &Kernel,
    plan_id: Option<&str>,
    report_ids: &[&str],
) -> KernelResult<HashMap<String, String>> {
    let mut found = HashMap::new();
    for report_id in report_ids {
        let packages = query_all(
            &kernel.database,
            "SELECT pp.package_id FROM agent_reports r
             JOIN plan_packages pp ON pp.project_id = r.project_id AND pp.assignee_agent_id = r.agent_id
             JOIN agents a ON a.project_id = pp.project_id AND a.agent_id = pp.assignee_agent_id AND a.generation = r.generation
             WHERE r.project_id = ?1 AND r.report_id = ?2 AND (?3 IS NULL OR pp.plan_id = ?3)
             ORDER BY pp.package_id LIMIT 2",
            rusqlite::params![kernel.project_id, report_id, plan_id],
            |row| row.get::<_, String>(0),
        )?;
        if let [only] = packages.as_slice() {
            found.insert((*report_id).to_string(), only.clone());
        }
    }
    Ok(found)
}

// ------------------------------------------------------------------------------------------------ redo of a set member

/// The newest-first reports that could fix a conflicted member of a set: accepted reports with a `report_commits` row for
/// `repo`, whose sequence is greater than `since`, by an agent on a package of the plan the set's reports belong to (or, with
/// no plan, by an author of those reports). A report without a row for the repo is never one.
pub fn redo_candidates(
    kernel: &Kernel,
    set_report_ids: &[String],
    repo: &str,
    since: i64,
) -> KernelResult<Vec<ReportCommit>> {
    kernel.assert_open()?;
    let info = planned_commit_info(kernel, &json!(set_report_ids))?;
    let agents: Vec<String> = match info.get("planId").and_then(Value::as_str) {
        Some(plan_id) => query_all(
            &kernel.database,
            "SELECT DISTINCT assignee_agent_id FROM plan_packages WHERE project_id = ? AND plan_id = ? AND assignee_agent_id IS NOT NULL",
            [&kernel.project_id, plan_id],
            |row| row.get::<_, String>(0),
        )?,
        None => report_order(kernel, set_report_ids)?
            .into_iter()
            .map(|(_, agent, _)| agent)
            .collect(),
    };
    let rows = query_all(
        &kernel.database,
        "SELECT r.report_id, r.agent_id, r.sequence, c.repo, c.commit_sha FROM agent_reports r
         JOIN report_commits c ON c.project_id = r.project_id AND c.report_id = r.report_id
         WHERE r.project_id = ? AND r.state = 'accepted' AND c.repo = ? AND r.sequence > ?
         ORDER BY r.sequence DESC",
        rusqlite::params![kernel.project_id, repo, since],
        |row| {
            Ok(ReportCommit {
                report_id: row.get(0)?,
                agent_id: row.get(1)?,
                sequence: row.get(2)?,
                repo: row.get(3)?,
                commit_sha: row.get(4)?,
            })
        },
    )?;
    Ok(rows
        .into_iter()
        .filter(|row| agents.contains(&row.agent_id))
        .collect())
}

/// Every integration that belongs to a set, attached or not: the members' current ones and the earlier attempts, whose
/// branches are the set's branch with `-<repo>` or `-<repo>-<attempt>` after it. The reports of a member that never started
/// (or was removed) are known only from these.
fn set_integration_ids(
    kernel: &Kernel,
    set_branch: &str,
    members: &[Value],
) -> KernelResult<Vec<String>> {
    let escaped = set_branch
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let rows = query_all(
        &kernel.database,
        "SELECT integration_id, branch FROM integrations WHERE project_id = ? AND branch LIKE ? ESCAPE '\\' ORDER BY sequence",
        rusqlite::params![kernel.project_id, format!("{escaped}-%")],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    let mut ids: Vec<String> = Vec::new();
    for (id, branch) in rows {
        // Another set's branch (`<set branch>-2`) starts with this one: its first part after the dash is a number.
        let rest = &branch[set_branch.len() + 1..];
        let numbered = rest
            .split_once('-')
            .is_some_and(|(head, _)| !head.is_empty() && head.chars().all(|c| c.is_ascii_digit()));
        if !numbered {
            ids.push(id);
        }
    }
    for member in members {
        if let Some(id) = member["integrationId"].as_str() {
            if !ids.iter().any(|known| known == id) {
                ids.push(id.to_string());
            }
        }
    }
    Ok(ids)
}

/// What a redo is given: the member to redo and the places of the repos.
pub struct RedoRun<'a> {
    pub set_id: &'a str,
    pub repo: &'a str,
    pub requested_by: &'a str,
    pub main_branch_of: &'a dyn Fn(&str) -> String,
    pub git_for: &'a dyn Fn(&str) -> GitRepo,
    pub before_member: &'a dyn Fn(&str) -> Result<(), MemberStop>,
    /// Stops the process of a running verify run (the process belongs to the daemon).
    pub abort_run: &'a dyn Fn(&str),
}

fn redo_refusal(message: String) -> IntegrateError {
    IntegrateError::Integration {
        code: "workspace_integrate".into(),
        message,
    }
}

fn printable(text: &str, max: usize) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| if (' '..='~').contains(&c) { c } else { '?' })
        .take(max)
        .collect();
    if cleaned.is_empty() {
        "unknown".into()
    } else {
        cleaned
    }
}

/// Moves the set to `state` unless it is there already in a state that takes no reason.
fn move_set(
    deps: &IntegrationDeps,
    set_id: &str,
    from: &str,
    state: &str,
    reason: &str,
) -> IntegrateResult<()> {
    if from == state && state == "merged" {
        return Ok(());
    }
    let reason = if state == "merged" {
        Value::Null
    } else {
        json!(printable(reason, 480))
    };
    super::workspace::set_integration_state(
        deps.kernel,
        &(deps.context)(deps.credential),
        &json!({"setId": set_id, "state": state, "reason": reason}),
    )?;
    Ok(())
}

/// Recomputes the state of a set whose members changed (a member was dropped): the first member that is not merged,
/// confirmed or dropped decides, as in a redo. A set that is running or discarded is left alone. Returns the state.
fn recompute_set(
    kernel: &Kernel,
    context: &dyn Fn() -> MutationContext,
    set_id: &str,
) -> IntegrateResult<String> {
    let set = super::workspace::integration_set(kernel, set_id)?;
    let from = set["state"].as_str().unwrap_or_default().to_string();
    if from == "running" || from == "discarded" {
        return Ok(from);
    }
    let (state, reason, _) = set_outcome(&set, kernel)?;
    if from == state && (state == "merged" || set["reason"].as_str() == Some(reason.as_str())) {
        return Ok(state);
    }
    let reason = if state == "merged" {
        Value::Null
    } else {
        json!(printable(&reason, 480))
    };
    super::workspace::set_integration_state(
        kernel,
        &context(),
        &json!({"setId": set_id, "state": state, "reason": reason}),
    )?;
    Ok(state)
}

/// The state a set is in once its members are as they are: the first member that is not merged or confirmed decides.
/// Returns `(state, reason, repo that holds it up)`.
fn set_outcome(set: &Value, kernel: &Kernel) -> IntegrateResult<(String, String, Option<String>)> {
    for member in set["members"].as_array().into_iter().flatten() {
        let repo = member["repo"].as_str().unwrap_or_default();
        let Some(id) = member["integrationId"].as_str() else {
            return Ok((
                "failed".into(),
                format!("repo {repo} is not integrated yet"),
                Some(repo.to_string()),
            ));
        };
        let record = integration(kernel, id)?;
        match record["state"].as_str().unwrap_or_default() {
            "merged" | "confirmed" => {}
            // Dropped: discarded and not redone; the set goes on without it.
            "discarded" => {}
            "conflicted" => {
                let files: Vec<&str> = record["conflictFiles"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                return Ok((
                    "conflicted".into(),
                    format!("repo {repo} conflicts in {}", files.join(", ")),
                    Some(repo.to_string()),
                ));
            }
            _ => {
                return Ok((
                    "failed".into(),
                    format!(
                        "repo {repo} failed: {}",
                        record["failureReason"]
                            .as_str()
                            .unwrap_or("it did not merge")
                    ),
                    Some(repo.to_string()),
                ))
            }
        }
    }
    Ok(("merged".into(), String::new(), None))
}

/// Discards the integration of a member that is leaving its place (merged or conflicted; any other state has nothing to
/// discard) and removes its branch from the member's repo.
fn discard_member_integration(
    deps: &IntegrationDeps,
    git: &GitRepo,
    record: &Value,
) -> IntegrateResult<Option<String>> {
    let state = record["state"].as_str().unwrap_or_default();
    if state != "merged" && state != "conflicted" {
        return Ok(None);
    }
    let id = record["integrationId"].as_str().unwrap_or_default();
    settle_integration(
        deps.kernel,
        &(deps.context)(deps.credential),
        &json!({"integrationId": id, "outcome": "discarded", "member": true, "redo": true}),
    )?;
    if let (true, Some(head)) = (state == "merged", record["headSha"].as_str()) {
        let branch = record["branch"].as_str().unwrap_or_default();
        if git.delete_branch_at(branch, head).unwrap_or(false) {
            return Ok(Some(branch.to_string()));
        }
        deps.state.remember(id);
        (deps.log)(
            "integration_branch_not_removed",
            json!({"integrationId": id, "branch": branch}),
        );
    }
    Ok(None)
}

/// Cancels the queued and running verify runs of an integration (only those of `repo` when one is given); the ids of the
/// runs cancelled are added to `cancelled`, and `abort` stops the process of one that was running.
fn cancel_member_runs(
    kernel: &Kernel,
    context: &dyn Fn() -> MutationContext,
    integration_id: &str,
    repo: Option<&str>,
    abort: &dyn Fn(&str),
    cancelled: &mut Vec<String>,
) -> KernelResult<()> {
    let runs = super::verify_runs::verify_runs(kernel, Some(integration_id), 200)?;
    for run_record in runs["runs"].as_array().into_iter().flatten() {
        let state = run_record["state"].as_str().unwrap_or_default();
        let run_id = run_record["runId"].as_str().unwrap_or_default();
        if (state != "queued" && state != "running")
            || repo.is_some_and(|repo| run_record["repo"] != repo)
            || cancelled.iter().any(|known| known == run_id)
        {
            continue;
        }
        let record =
            super::verify_runs::cancel_verify_run(kernel, &context(), &json!({"runId": run_id}))?;
        if record["needsAbort"] == true {
            abort(run_id);
        }
        cancelled.push(run_id.to_string());
    }
    Ok(())
}

/// `integrate redo <set> --repo <repo>`: the repo's unconfirmed member is let go and the repo is integrated again into the
/// same set, on its current main tip, under the member's old position. A member whose change is on main already is
/// refused (it is to be confirmed) or, when its commits have landed, removed from the set. See the answer for the set's
/// new state and the next step.
pub fn integrate_redo(deps: &IntegrationDeps, run: &RedoRun) -> IntegrateResult<Value> {
    let (set_id, repo) = (run.set_id, run.repo);
    let ctx = || (deps.context)(deps.credential);
    let set = super::workspace::integration_set(deps.kernel, set_id)?;
    if set.is_null() {
        return Err(redo_refusal(format!("set {set_id} does not exist")));
    }
    let set_state = set["state"].as_str().unwrap_or_default().to_string();
    let set_branch = set["branch"].as_str().unwrap_or_default().to_string();
    match set_state.as_str() {
        "running" => {
            return Err(redo_refusal(format!(
                "set {set_id} is running; wait for it to settle"
            )))
        }
        "discarded" => return Err(redo_refusal(format!("set {set_id} is discarded"))),
        _ => {}
    }
    let members = set["members"].as_array().cloned().unwrap_or_default();
    let Some(member) = members.iter().find(|m| m["repo"] == repo).cloned() else {
        return Err(redo_refusal(format!(
            "set {set_id} has no member for repo {repo}"
        )));
    };
    let old_id = member["integrationId"].as_str().map(str::to_string);
    let old = match &old_id {
        Some(id) => integration(deps.kernel, id)?,
        None => Value::Null,
    };
    let old_state = old["state"].as_str().unwrap_or_default().to_string();
    if old_state == "confirmed" {
        return Err(redo_refusal(format!(
            "repo {repo} of set {set_id} is confirmed; its code is on main"
        )));
    }
    if old_state == "running" {
        return Err(redo_refusal(format!(
            "repo {repo} of set {set_id} is still merging"
        )));
    }
    let git = (run.git_for)(repo);
    if !git.merge_tree_supported() {
        return Err(redo_refusal(
            "git 2.38 or newer is needed for workspace integrate".into(),
        ));
    }
    let main = (run.main_branch_of)(repo);
    let Some(tip) = git.branch_tip(&main)? else {
        return Err(redo_refusal(format!("repo {repo} has no branch {main}")));
    };
    // A member the user merged by hand is to be confirmed, not redone.
    if let (Some(head), true) = (old["headSha"].as_str(), old_state == "merged") {
        let base = old["baseSha"].as_str().unwrap_or_default();
        if member_on_main(&git, &main, base, head)? {
            return Err(redo_refusal(format!(
                "repo {repo} main already contains {}; confirm it with integrate confirm {set_id} --repo {repo}",
                old["branch"].as_str().unwrap_or_default()
            )));
        }
    }
    // The reports of the set, and of this member.
    let mut set_reports: Vec<AgentReportRow> = Vec::new();
    for id in set_integration_ids(deps.kernel, &set_branch, &members)? {
        for row in reviews::integration_authors(deps.kernel, &id)? {
            if !set_reports.iter().any(|r| r.report_id == row.report_id) {
                set_reports.push(row);
            }
        }
    }
    let own_reports: Vec<AgentReportRow> = match &old_id {
        Some(id) => reviews::integration_authors(deps.kernel, id)?,
        None => Vec::new(),
    };
    let mut fix_report: Option<String> = None;
    let commits: Vec<(String, String)> = if old_state == "conflicted" {
        let since = own_reports.iter().map(|r| r.sequence).max().unwrap_or(0);
        let ids: Vec<String> = set_reports.iter().map(|r| r.report_id.clone()).collect();
        let found = redo_candidates(deps.kernel, &ids, repo, since)?;
        let Some(best) = found.into_iter().next() else {
            return Err(redo_refusal(format!(
                "no report for repo {repo} since the conflict; report the fix first"
            )));
        };
        fix_report = Some(best.report_id.clone());
        vec![(best.report_id, best.commit_sha)]
    } else {
        let source = if old_id.is_some() {
            &own_reports
        } else {
            &set_reports
        };
        let ids: Vec<String> = source.iter().map(|r| r.report_id.clone()).collect();
        let own: Vec<(String, String)> = workspace_commits(deps.kernel, &ids)?
            .into_iter()
            .filter(|c| c.repo == repo)
            .map(|c| (c.report_id, c.commit_sha))
            .collect();
        supersede(deps.kernel, &own)?
    };
    if commits.is_empty() {
        return Err(redo_refusal(format!(
            "repo {repo} has nothing to integrate"
        )));
    }
    let mut keep: Vec<(String, String)> = Vec::new();
    for (report_id, sha) in &commits {
        match git.tree_landed(&tip, sha)? {
            Landed::Yes => {}
            Landed::No => keep.push((report_id.clone(), sha.clone())),
            Landed::Unknown(code) => {
                return Err(redo_refusal(format!(
                    "git could not compare repo {repo} with {main} (exit {code})"
                )))
            }
        }
    }
    let attempt = member["attempt"].as_i64().unwrap_or(0) as usize + 1;
    let branch = member_branch(&set_branch, repo, attempt);
    if !keep.is_empty() && git.branch_tip(&branch)?.is_some() {
        return Err(redo_refusal(format!(
            "branch {branch} already exists in repo {repo}"
        )));
    }
    // From here the member is let go. Its verify runs stop first, then the set's gates reset.
    let mut subject_before: Option<String> = None;
    for m in &members {
        if let Some(id) = m["integrationId"].as_str() {
            if integration(deps.kernel, id)?["state"] != "discarded" {
                subject_before = Some(id.to_string());
                break;
            }
        }
    }
    let mut cancelled: Vec<String> = Vec::new();
    if let Some(id) = &old_id {
        cancel_member_runs(
            deps.kernel,
            &|| (deps.context)(deps.credential),
            id,
            Some(repo),
            run.abort_run,
            &mut cancelled,
        )?;
    }
    let review_open = match &subject_before {
        Some(subject) => present(
            deps.kernel,
            "SELECT 1 AS present FROM reviews WHERE project_id = ? AND subject_integration_id = ? AND state = 'started'",
            [&deps.kernel.project_id, subject],
        )?,
        None => false,
    };
    if !old.is_null() {
        discard_member_integration(deps, &git, &old)?;
    }
    let mut answer = json!({
        "setId": set_id,
        "repo": repo,
        "cancelledRuns": cancelled,
        "reviewMustBeRequestedAgain": review_open,
        "reviewNotice": if review_open { json!("a running review of the set no longer counts; request the review again") } else { Value::Null },
    });
    if keep.is_empty() {
        // Every commit of the repo is on main: the member leaves the set.
        let removed =
            remove_set_member(deps.kernel, &ctx(), &json!({"setId": set_id, "repo": repo}));
        removed?;
        let after = super::workspace::integration_set(deps.kernel, set_id)?;
        let left = after["members"].as_array().map_or(0, Vec::len);
        let (state, reason, holder) = set_outcome(&after, deps.kernel)?;
        move_set(deps, set_id, &set_state, &state, &reason)?;
        let next = match (&state[..], &holder) {
            ("merged", _) if left == 0 => {
                "nothing to review: every repo of the set is on main already".to_string()
            }
            ("merged", _) => "merge each member branch into its repo's main branch".to_string(),
            (_, Some(holder)) => format!("cstan integrate redo {set_id} --repo {holder}"),
            _ => String::new(),
        };
        answer["landed"] = json!(true);
        answer["removed"] = json!(true);
        answer["state"] = json!(state);
        answer["reason"] = if state == "merged" {
            Value::Null
        } else {
            json!(reason)
        };
        answer["members"] = after["members"].clone();
        answer["next"] = json!(next);
        answer["message"] = json!(format!(
            "repo {repo} is on main already and left set {set_id}{}",
            if left == 0 {
                "; nothing is left to review"
            } else {
                ""
            }
        ));
        return Ok(answer);
    }
    if old_id.is_some() {
        detach_set_member(deps.kernel, &ctx(), &json!({"setId": set_id, "repo": repo}))?;
    }
    reset_set(deps.kernel, &ctx(), &json!({"setId": set_id}))?;
    if set_state == "merged" || set_state == "conflicted" {
        move_set(
            deps,
            set_id,
            &set_state,
            "failed",
            &format!("redoing repo {repo}"),
        )?;
    }
    let target = MemberTarget {
        set_id,
        branch: &branch,
        main_branch: &main,
        base_sha: &tip,
    };
    let member_deps = IntegrationDeps {
        kernel: deps.kernel,
        git: &git,
        state: deps.state,
        credential: deps.credential,
        context: deps.context,
        log: deps.log,
        impact: deps.impact,
    };
    let record = match integrate_repo_with(
        &member_deps,
        repo,
        &keep,
        run.requested_by,
        &target,
        run.before_member,
    ) {
        Ok(record) => record,
        Err(error) => {
            let _ = move_set(
                deps,
                set_id,
                "failed",
                "failed",
                &format!("the redo of repo {repo} stopped: {}", error.display()),
            );
            return Err(error);
        }
    };
    let after = super::workspace::integration_set(deps.kernel, set_id)?;
    let (state, reason, holder) = set_outcome(&after, deps.kernel)?;
    // The set was failed while the redo ran, or merged when it was and nothing else moved.
    move_set(deps, set_id, "failed", &state, &reason)?;
    let next = match (&state[..], &holder) {
        ("merged", _) => "merge each member branch into its repo's main branch".to_string(),
        ("conflicted", Some(holder)) => format!(
            "resolve with a new report for repo {holder}, then cstan integrate redo {set_id} --repo {holder}"
        ),
        (_, Some(holder)) => format!(
            "cstan integrate redo {set_id} --repo {holder}, or cstan integrate discard {set_id} --repo {holder}"
        ),
        _ => String::new(),
    };
    let conflicted = record["state"] == "conflicted";
    answer["landed"] = json!(false);
    answer["state"] = json!(state);
    answer["reason"] = if state == "merged" {
        Value::Null
    } else {
        json!(reason)
    };
    answer["next"] = json!(next);
    answer["fixReport"] = json!(fix_report);
    answer["member"] = json!({
        "repo": repo,
        "integrationId": record["integrationId"],
        "state": record["state"],
        "branch": record["branch"],
        "head": record["headSha"],
        "attempt": attempt,
        "reports": record["reports"].as_array().into_iter().flatten().map(|r| r["reportId"].clone()).collect::<Vec<_>>(),
        "conflict": if conflicted {
            json!({"reportId": record["conflictReportId"], "files": record["conflictFiles"]})
        } else {
            Value::Null
        },
        "failure": record["failureReason"],
    });
    Ok(answer)
}

// ------------------------------------------------------------------------------------------------ discard of a set or a member

/// What a discard is given: the set, the member to discard (the whole set when none) and the places of the repos.
pub struct DiscardRun<'a> {
    pub set_id: &'a str,
    pub repo: Option<&'a str>,
    pub main_branch_of: &'a dyn Fn(&str) -> String,
    pub git_for: &'a dyn Fn(&str) -> GitRepo,
    /// Stops the process of a running verify run (the process belongs to the daemon).
    pub abort_run: &'a dyn Fn(&str),
}

/// The state of a member's integration, null state when it has none yet.
fn member_state(deps: &IntegrationDeps, member: &Value) -> IntegrateResult<(Value, String)> {
    match member["integrationId"].as_str() {
        Some(id) => {
            let record = integration(deps.kernel, id)?;
            let state = record["state"].as_str().unwrap_or_default().to_string();
            Ok((record, state))
        }
        None => Ok((Value::Null, String::new())),
    }
}

/// `integrate discard <set>` and `integrate discard <set> --repo <repo>`: the whole set, or one unconfirmed member, is
/// thrown away. A set with a confirmed member is refused as a whole (its code is on main); a member whose change is on main
/// already is to be confirmed instead. Runs of the discarded integrations are cancelled and the set's gates reset. A set
/// whose every member integration is discarded moves to `discarded`.
pub fn integrate_discard(deps: &IntegrationDeps, run: &DiscardRun) -> IntegrateResult<Value> {
    let set_id = run.set_id;
    let ctx = || (deps.context)(deps.credential);
    let set = super::workspace::integration_set(deps.kernel, set_id)?;
    if set.is_null() {
        return Err(redo_refusal(format!("set {set_id} does not exist")));
    }
    let set_state = set["state"].as_str().unwrap_or_default().to_string();
    match set_state.as_str() {
        "running" => {
            return Err(redo_refusal(format!(
                "set {set_id} is running; wait for it to settle"
            )))
        }
        "discarded" => return Err(redo_refusal(format!("set {set_id} is discarded"))),
        _ => {}
    }
    let members: Vec<Value> = set["members"].as_array().cloned().unwrap_or_default();
    let targets: Vec<Value> = match run.repo {
        Some(repo) => {
            let Some(member) = members.iter().find(|m| m["repo"] == repo) else {
                return Err(redo_refusal(format!(
                    "set {set_id} has no member for repo {repo}"
                )));
            };
            vec![member.clone()]
        }
        None => members.clone(),
    };
    // Refusals first: nothing changes unless the whole request can be done.
    let mut states: Vec<(Value, Value, String)> = Vec::new();
    for member in &targets {
        let repo = member["repo"].as_str().unwrap_or_default();
        let (record, state) = member_state(deps, member)?;
        if state == "confirmed" {
            return Err(redo_refusal(if run.repo.is_some() {
                format!("repo {repo} of set {set_id} is confirmed; its code is on main")
            } else {
                format!("repo {repo} is already merged; discard the others with --repo")
            }));
        }
        if state == "running" {
            return Err(redo_refusal(format!(
                "repo {repo} of set {set_id} is still merging"
            )));
        }
        states.push((member.clone(), record, state));
    }
    for (member, record, state) in &states {
        let repo = member["repo"].as_str().unwrap_or_default();
        if let (Some(head), "merged") = (record["headSha"].as_str(), state.as_str()) {
            let git = (run.git_for)(repo);
            let main = (run.main_branch_of)(repo);
            let base = record["baseSha"].as_str().unwrap_or_default();
            if git.branch_tip(&main)?.is_some() && member_on_main(&git, &main, base, head)? {
                let branch = record["branch"].as_str().unwrap_or_default();
                return Err(redo_refusal(if run.repo.is_some() {
                    format!("repo {repo} main already contains {branch}; confirm it instead")
                } else {
                    format!(
                        "repo {repo} main already contains {branch}; confirm it or discard the others with --repo"
                    )
                }));
            }
        }
    }
    let review_open = present(
        deps.kernel,
        "SELECT 1 AS present FROM reviews WHERE project_id = ? AND subject_integration_id IN
           (SELECT integration_id FROM integration_set_members WHERE project_id = ? AND set_id = ? AND integration_id IS NOT NULL)
           AND state = 'started'",
        [&deps.kernel.project_id, &deps.kernel.project_id, set_id],
    )?;
    let mut cancelled: Vec<String> = Vec::new();
    let mut discarded: Vec<String> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    let mut removed_branches: Vec<Value> = Vec::new();
    for (member, record, state) in &states {
        let repo = member["repo"].as_str().unwrap_or_default().to_string();
        if let Some(id) = member["integrationId"].as_str() {
            cancel_member_runs(
                deps.kernel,
                &|| (deps.context)(deps.credential),
                id,
                Some(&repo),
                run.abort_run,
                &mut cancelled,
            )?;
        }
        match state.as_str() {
            "merged" | "conflicted" => {
                let git = (run.git_for)(&repo);
                if let Some(branch) = discard_member_integration(deps, &git, record)? {
                    removed_branches.push(json!({"repo": repo, "branch": branch}));
                }
                discarded.push(repo);
            }
            "" => {
                // Never started: only its row; the whole set keeps it (nothing to settle), one member is removed.
                if run.repo.is_some() {
                    remove_set_member(
                        deps.kernel,
                        &ctx(),
                        &json!({"setId": set_id, "repo": repo}),
                    )?;
                    removed.push(repo);
                }
            }
            "discarded" => {}
            _ => {
                // A failed integration has no branch and cannot be settled: it is let go.
                if run.repo.is_some() {
                    remove_set_member(
                        deps.kernel,
                        &ctx(),
                        &json!({"setId": set_id, "repo": repo}),
                    )?;
                    removed.push(repo);
                } else {
                    detach_set_member(
                        deps.kernel,
                        &ctx(),
                        &json!({"setId": set_id, "repo": repo}),
                    )?;
                }
            }
        }
    }
    reset_set(deps.kernel, &ctx(), &json!({"setId": set_id}))?;
    let after = super::workspace::integration_set(deps.kernel, set_id)?;
    let mut all_discarded = true;
    for member in after["members"].as_array().into_iter().flatten() {
        let (_, state) = member_state(deps, member)?;
        if run.repo.is_none() && state.is_empty() {
            continue;
        }
        if state != "discarded" {
            all_discarded = false;
        }
    }
    let state = if all_discarded {
        move_set(deps, set_id, &set_state, "discarded", "discarded")?;
        "discarded".to_string()
    } else {
        recompute_set(deps.kernel, &|| (deps.context)(deps.credential), set_id)?
    };
    let message = match run.repo {
        Some(repo) if !removed.iter().any(|r| r == repo) => json!(format!(
            "repo {repo} dropped from set {set_id}; the set continues without it; integrate redo {set_id} --repo {repo} brings it back"
        )),
        Some(repo) => json!(format!("repo {repo} removed from set {set_id}")),
        None => json!(format!("set {set_id} discarded")),
    };
    let message = if removed_branches.is_empty() || run.repo.is_some() {
        message
    } else {
        let list: Vec<String> = removed_branches
            .iter()
            .map(|b| {
                format!(
                    "{}: {}",
                    b["repo"].as_str().unwrap_or_default(),
                    b["branch"].as_str().unwrap_or_default()
                )
            })
            .collect();
        json!(format!(
            "{}; removed branches: {}",
            message.as_str().unwrap_or_default(),
            list.join(", ")
        ))
    };
    Ok(json!({
        "message": message,
        "setId": set_id,
        "repo": run.repo,
        "state": state,
        "discarded": discarded,
        "removed": removed,
        "removedBranches": removed_branches,
        "cancelledRuns": cancelled,
        "members": after["members"],
        "reviewMustBeRequestedAgain": review_open && state != "discarded",
        "reviewNotice": if review_open && state != "discarded" { json!("a running review of the set no longer counts; request the review again") } else { Value::Null },
    }))
}

// ------------------------------------------------------------------------------------------------ confirm of a member

/// What a confirm is given: the set, the member's repo (none when the caller did not name one) and the places of the repos.
pub struct ConfirmRun<'a> {
    pub set_id: &'a str,
    pub repo: Option<&'a str>,
    pub main_branch_of: &'a dyn Fn(&str) -> String,
    pub git_for: &'a dyn Fn(&str) -> GitRepo,
}

/// `integrate confirm <set> --repo <repo>`: one member of a merged set is confirmed once its repo's main holds its change
/// (however it got there: fast-forward, squash or rebase). Without `--repo` the caller is asked to name the repo. A member
/// confirmed before an earlier live member is confirmed anyway and the answer carries the warning
/// `repo <r> confirmed before <x> (merge_order)`. The member's branch is removed from its repo.
pub fn integrate_confirm(deps: &IntegrationDeps, run: &ConfirmRun) -> IntegrateResult<Value> {
    let set_id = run.set_id;
    let set = super::workspace::integration_set(deps.kernel, set_id)?;
    if set.is_null() {
        return Err(redo_refusal(format!("set {set_id} does not exist")));
    }
    let members: Vec<Value> = set["members"].as_array().cloned().unwrap_or_default();
    let Some(repo) = run.repo else {
        let names: Vec<&str> = members.iter().filter_map(|m| m["repo"].as_str()).collect();
        return Err(redo_refusal(format!(
            "name the repo: integrate confirm {set_id} --repo <repo> (one of {})",
            names.join(", ")
        )));
    };
    let Some(member) = members.iter().find(|m| m["repo"] == repo) else {
        return Err(redo_refusal(format!(
            "set {set_id} has no member for repo {repo}"
        )));
    };
    let (record, state) = member_state(deps, member)?;
    match state.as_str() {
        "merged" => {}
        "confirmed" => {
            return Err(redo_refusal(format!(
                "repo {repo} of set {set_id} is already confirmed"
            )))
        }
        "" => {
            return Err(redo_refusal(format!(
                "repo {repo} of set {set_id} has no integration yet"
            )))
        }
        other => {
            return Err(redo_refusal(format!(
                "repo {repo} of set {set_id} is {other}; only a merged member can be confirmed"
            )))
        }
    }
    let (Some(head), Some(id)) = (record["headSha"].as_str(), record["integrationId"].as_str())
    else {
        return Err(redo_refusal(format!(
            "repo {repo} of set {set_id} has no merged head"
        )));
    };
    let branch = record["branch"].as_str().unwrap_or_default().to_string();
    let git = (run.git_for)(repo);
    let main = (run.main_branch_of)(repo);
    if git.branch_tip(&main)?.is_none()
        || !member_on_main(
            &git,
            &main,
            record["baseSha"].as_str().unwrap_or_default(),
            head,
        )?
    {
        return Err(redo_refusal(format!(
            "repo {repo} main does not contain {branch}"
        )));
    }
    // An earlier live member that is not confirmed yet: the merge order was not followed.
    let mut before: Option<String> = None;
    for earlier in &members {
        if earlier["repo"] == repo {
            break;
        }
        let (_, earlier_state) = member_state(deps, earlier)?;
        if earlier_state != "confirmed" && earlier_state != "discarded" {
            before = earlier["repo"].as_str().map(str::to_string);
            break;
        }
    }
    let warning = before.map(|x| format!("repo {repo} confirmed before {x} (merge_order)"));
    let settled = settle_integration(
        deps.kernel,
        &(deps.context)(deps.credential),
        &json!({"integrationId": id, "outcome": "confirmed", "member": true}),
    )?;
    let removed = git.delete_branch_at(&branch, head).unwrap_or(false);
    if !removed {
        deps.state.remember(id);
        (deps.log)(
            "integration_branch_not_removed",
            json!({"integrationId": id, "branch": branch}),
        );
    }
    if let Some(warning) = &warning {
        (deps.log)(
            "integration_confirm_out_of_order",
            json!({"setId": set_id, "repo": repo, "warning": warning}),
        );
    }
    Ok(json!({
        "record": settled,
        "branchRemoved": removed,
        "setId": set_id,
        "repo": repo,
        "warning": warning,
    }))
}

/// After a member's integration was discarded on its own (a remote decision): its queued and running runs are cancelled
/// (`abort` stops the process of a running one), the set's gates reset, and a set whose every member integration is
/// discarded moves to `discarded`. Returns the ids of the runs cancelled.
pub fn after_member_discard(
    kernel: &Kernel,
    context: &dyn Fn() -> MutationContext,
    integration_id: &str,
    abort: &dyn Fn(&str),
) -> KernelResult<Vec<String>> {
    let Some(set_id) = super::workspace::set_of_integration(kernel, integration_id)? else {
        return Ok(Vec::new());
    };
    let mut cancelled = Vec::new();
    cancel_member_runs(kernel, context, integration_id, None, abort, &mut cancelled)?;
    reset_set(kernel, &context(), &json!({"setId": set_id}))?;
    let set = super::workspace::integration_set(kernel, &set_id)?;
    let mut all_discarded = true;
    for member in set["members"].as_array().into_iter().flatten() {
        if let Some(id) = member["integrationId"].as_str() {
            if integration(kernel, id)?["state"] != "discarded" {
                all_discarded = false;
            }
        } else {
            all_discarded = false;
        }
    }
    let state = set["state"].as_str().unwrap_or_default();
    if all_discarded && state != "discarded" && state != "running" {
        super::workspace::set_integration_state(
            kernel,
            &context(),
            &json!({"setId": set_id, "state": "discarded", "reason": "discarded"}),
        )?;
    } else {
        recompute_set(kernel, context, &set_id).map_err(|error| match error {
            IntegrateError::Kernel(error) => error,
            other => KernelError::controller(other.display()),
        })?;
    }
    Ok(cancelled)
}
