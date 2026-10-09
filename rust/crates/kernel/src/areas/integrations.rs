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

use super::{message_notices, messages, plan_packages, reviews};

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
pub(crate) struct IntegrationRow {
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

pub(crate) fn integration_row(
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
                     ORDER BY i.sequence DESC LIMIT 1",
                    [&kernel.project_id, *id],
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

/// `finishIntegration`.
pub fn finish_integration(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
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
    kernel.mutate(
        context,
        "integration.settle",
        "controller:reconcile",
        input,
        |_| {
            let row = required_row(kernel, integration_id)?;
            if row.state != "merged" {
                return Err(KernelError::controller(
                    "only a merged integration can be settled",
                ));
            }
            if present(
                kernel,
                "SELECT 1 AS present FROM reviews WHERE project_id = ? AND subject_integration_id = ? AND state = 'started'",
                [&kernel.project_id, integration_id],
            )? {
                return Err(KernelError::controller(
                    "a review of this integration is open",
                ));
            }
            if outcome == "confirmed" {
                let latest = query_opt(
                    &kernel.database,
                    "SELECT state FROM reviews WHERE project_id = ? AND subject_integration_id = ? AND state IN ('passed', 'findings')
                     ORDER BY sequence DESC LIMIT 1",
                    [&kernel.project_id, integration_id],
                    |r| r.get::<_, String>(0),
                )?;
                if latest.as_deref() != Some("passed") {
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
                .transition("merged", outcome)
                .with_details(json!({}));
            Ok(MutationOutput::new(integration_record(kernel, integration_id)?, event))
        },
    )
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
    let Some(ids) = report_ids.as_array() else {
        return Err(KernelError::type_error("reportIds is not iterable"));
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
#[allow(dead_code)]
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
