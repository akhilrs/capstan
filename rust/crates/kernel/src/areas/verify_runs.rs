//! The verify runs (migration 0042): the project's verification commands run against the commit of one report or
//! integration. This area only reads and writes the ledger; the daemon runs the commands. A run is queued, claimed once
//! (`running`), and finished once; the review and integration gates ask `verify_gate` / `verify_gate_for_subjects`.
//!
//! Operations by `ControllerCore` method name (`dispatch`): the mutating ones take the mutation context, then one input
//! object; `verifyRun` takes (runId), `verifyRuns` (subjectId|null, limit), `nextQueuedVerifyRuns` (limit),
//! `verifyGate` (commitSha, commandsSha), `verifyGateForSubjects` (subjectIds, commandsSha) and `verifyStatus` nothing.

use crate::canonical::sha256;
use crate::dispatch::{arg, call, Args};
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{
    execute, format_duration_ms, query_all, query_opt, safe_id, safe_id_str, tail_excerpt,
    VerifySummary,
};
use crate::kernel::Kernel;
use crate::records::{AgentRow, MutationEvent, MutationOutput};
use crate::types::MutationContext;
use serde_json::{json, Value};

use super::messaging::MessageClass;
use super::{integrations, message_notices, messages};

/// The capability to queue and cancel a run: the one every role that reports its work holds (a migration that added a
/// capability would have to grant it to every existing actor).
const REQUEST: &str = "work:report";
/// The capability of the daemon's own run bookkeeping.
const CONTROL: &str = "controller:reconcile";

/// The most output a run keeps, from its end.
pub const MAX_OUTPUT_TAIL_BYTES: usize = 12288;
const MAX_COMMANDS: usize = 32;
const MAX_COMMAND_BYTES: usize = 4096;
/// How many recent finished runs the status shows.
const STATUS_RECENT: i64 = 5;
const ACTIVE: &str = "('queued', 'running')";

const COLUMNS: &str = "run_id, sequence, subject_report_id, subject_integration_id, commit_sha, commands_json, commands_sha, trigger,
  requested_by_actor_id, state, queued_at, started_at, finished_at, exit_code, failed_step, duration_ms, output_tail, output_truncated,
  log_path, scratch_path, pgid, leader_start";

fn field<'a>(input: &'a Value, key: &str) -> &'a Value {
    input.get(key).unwrap_or(&Value::Null)
}

fn one_of<'a>(input: &'a Value, key: &str, allowed: &[&str]) -> KernelResult<&'a str> {
    match field(input, key).as_str() {
        Some(value) if allowed.contains(&value) => Ok(value),
        _ => Err(KernelError::type_error(format!(
            "{key} must be one of {}",
            allowed.join(", ")
        ))),
    }
}

fn opt_i64(input: &Value, key: &str) -> KernelResult<Option<i64>> {
    match field(input, key) {
        Value::Null => Ok(None),
        value => value
            .as_i64()
            .map(Some)
            .ok_or_else(|| KernelError::type_error(format!("{key} must be an integer"))),
    }
}

/// A row of `verify_runs`.
#[derive(Clone, Debug)]
struct RunRow {
    run_id: String,
    sequence: i64,
    subject_report_id: Option<String>,
    subject_integration_id: Option<String>,
    commit_sha: String,
    commands_json: String,
    commands_sha: String,
    trigger: String,
    requested_by_actor_id: String,
    state: String,
    queued_at: String,
    started_at: Option<String>,
    finished_at: Option<String>,
    exit_code: Option<i64>,
    failed_step: Option<i64>,
    duration_ms: Option<i64>,
    output_tail: String,
    output_truncated: bool,
    log_path: Option<String>,
    scratch_path: Option<String>,
    pgid: Option<i64>,
    leader_start: Option<String>,
}

impl RunRow {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            run_id: row.get("run_id")?,
            sequence: row.get("sequence")?,
            subject_report_id: row.get("subject_report_id")?,
            subject_integration_id: row.get("subject_integration_id")?,
            commit_sha: row.get("commit_sha")?,
            commands_json: row.get("commands_json")?,
            commands_sha: row.get("commands_sha")?,
            trigger: row.get("trigger")?,
            requested_by_actor_id: row.get("requested_by_actor_id")?,
            state: row.get("state")?,
            queued_at: row.get("queued_at")?,
            started_at: row.get("started_at")?,
            finished_at: row.get("finished_at")?,
            exit_code: row.get("exit_code")?,
            failed_step: row.get("failed_step")?,
            duration_ms: row.get("duration_ms")?,
            output_tail: row.get("output_tail")?,
            output_truncated: row.get::<_, i64>("output_truncated")? != 0,
            log_path: row.get("log_path")?,
            scratch_path: row.get("scratch_path")?,
            pgid: row.get("pgid")?,
            leader_start: row.get("leader_start")?,
        })
    }

    fn subject_kind(&self) -> &'static str {
        if self.subject_report_id.is_some() {
            "report"
        } else {
            "integration"
        }
    }

    fn subject_id(&self) -> &str {
        self.subject_report_id
            .as_deref()
            .or(self.subject_integration_id.as_deref())
            .unwrap_or("")
    }

    /// The run record of the shared JSON contract.
    fn record(&self, kernel: &Kernel) -> KernelResult<Value> {
        let requested_by = kernel
            .agent_by_actor(&self.requested_by_actor_id)?
            .map_or_else(|| self.requested_by_actor_id.clone(), |a| a.agent_id);
        Ok(json!({
            "runId": self.run_id,
            "sequence": self.sequence,
            "subjectKind": self.subject_kind(),
            "subjectId": self.subject_id(),
            "commitSha": self.commit_sha,
            "commands": serde_json::from_str::<Value>(&self.commands_json)?,
            "commandsSha": self.commands_sha,
            "trigger": self.trigger,
            "requestedBy": requested_by,
            "state": self.state,
            "queuedAt": self.queued_at,
            "startedAt": self.started_at,
            "finishedAt": self.finished_at,
            "exitCode": self.exit_code,
            "failedStep": self.failed_step,
            "durationMs": self.duration_ms,
            "outputTail": self.output_tail,
            "outputTruncated": self.output_truncated,
            "logPath": self.log_path,
        }))
    }
}

fn run_row(kernel: &Kernel, run_id: &str) -> KernelResult<Option<RunRow>> {
    query_opt(
        &kernel.database,
        &format!("SELECT {COLUMNS} FROM verify_runs WHERE project_id = ? AND run_id = ?"),
        [&kernel.project_id, run_id],
        RunRow::from_row,
    )
}

fn require_run(kernel: &Kernel, run_id: &str) -> KernelResult<RunRow> {
    run_row(kernel, run_id)?.ok_or_else(|| KernelError::controller("the verify run does not exist"))
}

fn run_value(kernel: &Kernel, run_id: &str) -> KernelResult<Value> {
    require_run(kernel, run_id)?.record(kernel)
}

fn without_tail(mut record: Value) -> Value {
    if let Some(map) = record.as_object_mut() {
        map.remove("outputTail");
    }
    record
}

/// The runs of the project matching `condition`, with `extra` bound after the project id, in `order`.
fn runs_where(
    kernel: &Kernel,
    condition: &str,
    extra: &[&dyn rusqlite::ToSql],
    order: &str,
) -> KernelResult<Vec<RunRow>> {
    let mut params: Vec<&dyn rusqlite::ToSql> = vec![&kernel.project_id];
    params.extend_from_slice(extra);
    query_all(
        &kernel.database,
        &format!(
            "SELECT {COLUMNS} FROM verify_runs WHERE project_id = ? AND {condition} ORDER BY {order}"
        ),
        rusqlite::params_from_iter(params),
        RunRow::from_row,
    )
}

/// What a subject id names: the commit a run would verify.
struct Subject {
    report_id: Option<String>,
    integration_id: Option<String>,
    /// `None` for an integration that has no merged commit yet.
    commit_sha: Option<String>,
}

fn subject_of(kernel: &Kernel, subject_id: &str) -> KernelResult<Subject> {
    safe_id_str(subject_id, "report or integration id")?;
    let report = query_opt(
        &kernel.database,
        "SELECT commit_sha FROM agent_reports WHERE project_id = ? AND report_id = ?",
        [&kernel.project_id, subject_id],
        |row| row.get::<_, String>(0),
    )?;
    if let Some(commit_sha) = report {
        return Ok(Subject {
            report_id: Some(subject_id.to_string()),
            integration_id: None,
            commit_sha: Some(commit_sha),
        });
    }
    match integrations::integration_row(kernel, subject_id)? {
        Some(integration) => Ok(Subject {
            report_id: None,
            integration_id: Some(subject_id.to_string()),
            commit_sha: integration.head_sha,
        }),
        None => Err(KernelError::controller(
            "the report or integration does not exist",
        )),
    }
}

fn check_commands(input: &Value) -> KernelResult<Vec<&str>> {
    let commands = match field(input, "commands").as_array() {
        Some(items) if !items.is_empty() && items.len() <= MAX_COMMANDS => items,
        _ => {
            return Err(KernelError::type_error(format!(
                "commands must be a list of 1-{MAX_COMMANDS} commands"
            )))
        }
    };
    commands
        .iter()
        .map(|c| match c.as_str() {
            Some(text) if !text.trim().is_empty() && text.len() <= MAX_COMMAND_BYTES => Ok(text),
            _ => Err(KernelError::type_error(format!(
                "a command must be text of 1-{MAX_COMMAND_BYTES} bytes"
            ))),
        })
        .collect()
}

/// `queueVerifyRun`: queues a run for the commit of a report or an integration, or answers the active run for the same
/// commit and commands (with `existing: true`).
pub fn queue_verify_run(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let subject_id = safe_id(field(input, "subjectId"), "subject id")?;
    let commands = check_commands(input)?;
    let commands_sha = safe_id(field(input, "commandsSha"), "commands sha")?;
    let trigger = one_of(input, "trigger", &["manual", "auto"])?;
    kernel.mutate(context, "verify.queue", REQUEST, input, |actor| {
        let subject = subject_of(kernel, subject_id)?;
        let discarded = match &subject.integration_id {
            Some(id) => integrations::integration_row(kernel, id)?.is_some_and(|i| i.state == "discarded"),
            None => false,
        };
        let Some(commit_sha) = subject.commit_sha.filter(|_| !discarded) else {
            return Err(KernelError::controller(
                "the integration has no merged commit to verify",
            ));
        };
        let active = runs_where(
            kernel,
            &format!("commit_sha = ? AND commands_sha = ? AND state IN {ACTIVE}"),
            &[&commit_sha, &commands_sha],
            "sequence",
        )?;
        if let Some(run) = active.first() {
            let mut record = run.record(kernel)?;
            record["existing"] = Value::Bool(true);
            let event = MutationEvent::new("verify_run", &run.run_id, 0)
                .with_details(json!({"subjectId": subject_id, "existing": true}));
            return Ok(MutationOutput::new(record, event));
        }
        let run_id = kernel.env.uuid();
        let sequence = query_opt(
            &kernel.database,
            "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM verify_runs WHERE project_id = ?",
            [&kernel.project_id],
            |row| row.get::<_, i64>(0),
        )?
        .unwrap_or(1);
        execute(
            &kernel.database,
            "INSERT INTO verify_runs(project_id, run_id, sequence, subject_report_id, subject_integration_id, commit_sha,
               commands_json, commands_sha, trigger, requested_by_actor_id, state, queued_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'queued', ?)",
            rusqlite::params![
                kernel.project_id,
                run_id,
                sequence,
                subject.report_id,
                subject.integration_id,
                commit_sha,
                serde_json::to_string(&commands)?,
                commands_sha,
                trigger,
                actor.actor_id,
                kernel.now()
            ],
        )?;
        let mut event = MutationEvent::new("verify_run", &run_id, 0).with_details(json!({
            "subjectId": subject_id, "commitSha": commit_sha, "trigger": trigger,
        }));
        event.to_state = Some("queued".into());
        Ok(MutationOutput::new(run_value(kernel, &run_id)?, event))
    })
}

/// `claimVerifyRun`: moves a queued run to running unless `maxParallel` runs already are.
pub fn claim_verify_run(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let run_id = safe_id(field(input, "runId"), "run id")?;
    let max_parallel = opt_i64(input, "maxParallel")?
        .filter(|n| *n >= 1)
        .ok_or_else(|| KernelError::type_error("maxParallel must be a positive integer"))?;
    let scratch_path = field(input, "scratchPath")
        .as_str()
        .filter(|p| !p.is_empty());
    let log_path = field(input, "logPath").as_str().filter(|p| !p.is_empty());
    if scratch_path.is_none() || log_path.is_none() {
        return Err(KernelError::type_error(
            "scratchPath and logPath must be non-empty text",
        ));
    }
    kernel.mutate(context, "verify.claim", CONTROL, input, |_| {
        let run = require_run(kernel, run_id)?;
        let running = runs_where(kernel, "state = 'running'", &[], "sequence")?.len() as i64;
        let claimed = run.state == "queued" && running < max_parallel;
        let mut event = MutationEvent::new("verify_run", run_id, 0)
            .with_details(json!({"claimed": claimed, "running": running}));
        if claimed {
            execute(
                &kernel.database,
                "UPDATE verify_runs SET state = 'running', started_at = ?, scratch_path = ?, log_path = ?
                 WHERE project_id = ? AND run_id = ?",
                rusqlite::params![kernel.now(), scratch_path, log_path, kernel.project_id, run_id],
            )?;
            event = event.transition("queued", "running");
        }
        Ok(MutationOutput::new(
            json!({"claimed": claimed, "run": run_value(kernel, run_id)?}),
            event,
        ))
    })
}

/// `recordVerifyProcess`: the process group and leader start time of a running run, recorded once.
pub fn record_verify_process(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let run_id = safe_id(field(input, "runId"), "run id")?;
    let pgid = opt_i64(input, "pgid")?
        .filter(|n| *n > 1)
        .ok_or_else(|| KernelError::type_error("pgid must be an integer above 1"))?;
    let leader_start = field(input, "leaderStart")
        .as_str()
        .filter(|p| !p.is_empty())
        .ok_or_else(|| KernelError::type_error("leaderStart must be non-empty text"))?;
    kernel.mutate(context, "verify.process", CONTROL, input, |_| {
        let run = require_run(kernel, run_id)?;
        if run.state != "running" {
            return Err(KernelError::controller(format!(
                "the verify run is {}, not running",
                run.state
            )));
        }
        execute(
            &kernel.database,
            "UPDATE verify_runs SET pgid = ?, leader_start = ? WHERE project_id = ? AND run_id = ?",
            rusqlite::params![pgid, leader_start, kernel.project_id, run_id],
        )?;
        let event = MutationEvent::new("verify_run", run_id, 0)
            .with_details(json!({"pgid": pgid, "leaderStart": leader_start}));
        Ok(MutationOutput::new(run_value(kernel, run_id)?, event))
    })
}

/// The last `MAX_OUTPUT_TAIL_BYTES` of `text`, cut at a character boundary; whether anything was cut.
fn bounded_tail(text: &str) -> (&str, bool) {
    if text.len() <= MAX_OUTPUT_TAIL_BYTES {
        return (text, false);
    }
    let mut cut = text.len() - MAX_OUTPUT_TAIL_BYTES;
    while !text.is_char_boundary(cut) {
        cut += 1;
    }
    (&text[cut..], true)
}

fn notice_text(run: &RunRow) -> String {
    let head = format!(
        "Verify run {} for {} {} at {}",
        run.run_id,
        run.subject_kind(),
        run.subject_id(),
        run.commit_sha
    );
    let duration = run
        .duration_ms
        .map_or("unknown time".to_string(), format_duration_ms);
    let excerpt = tail_excerpt(&run.output_tail);
    let tail = if excerpt.is_empty() {
        String::new()
    } else {
        format!(
            "\nLast output lines: {}",
            serde_json::to_string(&excerpt).unwrap_or_default()
        )
    };
    match run.state.as_str() {
        "passed" => format!("{head}: passed in {duration}"),
        "failed" => format!(
            "{head}: failed at step {} (exit {}) after {duration}{tail}",
            run.failed_step.map_or("null".into(), |n| n.to_string()),
            run.exit_code.map_or("null".into(), |n| n.to_string()),
        ),
        "timeout" => format!("{head}: timed out after {duration}{tail}"),
        "interrupted" => format!("{head}: interrupted after {duration}{tail}"),
        other => format!("{head}: {other} after {duration}{tail}"),
    }
}

/// The agents that hear of a finished run, deduplicated: the PM, the architect of the subject's plan, the requester.
fn notice_recipients(kernel: &Kernel, run: &RunRow, pm: &AgentRow) -> KernelResult<Vec<AgentRow>> {
    let mut recipients = vec![pm.clone()];
    let report_ids: Vec<String> = match &run.subject_integration_id {
        None => vec![run.subject_id().to_string()],
        Some(id) => super::reviews::integration_authors(kernel, id)?
            .into_iter()
            .map(|r| r.report_id)
            .collect(),
    };
    let mut architects: Vec<String> = Vec::new();
    for report_id in &report_ids {
        let found = query_all(
            &kernel.database,
            "SELECT DISTINCT pl.architect_agent_id FROM agent_reports r
             JOIN agents a ON a.project_id = r.project_id AND a.agent_id = r.agent_id AND a.generation = r.generation
             JOIN plan_packages pp ON pp.project_id = a.project_id AND pp.assignee_agent_id = a.agent_id
             JOIN plans pl ON pl.project_id = pp.project_id AND pl.plan_id = pp.plan_id
             WHERE r.project_id = ? AND r.report_id = ? AND pl.architect_agent_id IS NOT NULL",
            [&kernel.project_id, report_id],
            |row| row.get::<_, String>(0),
        )?;
        for id in found {
            if !architects.contains(&id) {
                architects.push(id);
            }
        }
    }
    for id in architects {
        if let Some(agent) = kernel.agent_row(&id)? {
            if agent.state == "active" {
                recipients.push(agent);
            }
        }
    }
    if let Some(requester) = kernel.agent_by_actor(&run.requested_by_actor_id)? {
        recipients.push(requester);
    }
    let mut seen: Vec<String> = Vec::new();
    recipients.retain(|agent| {
        let fresh = !seen.contains(&agent.agent_id);
        if fresh {
            seen.push(agent.agent_id.clone());
        }
        fresh
    });
    Ok(recipients)
}

/// `finishVerifyRun`: records how a running run ended and queues one controller notice per recipient.
pub fn finish_verify_run(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let run_id = safe_id(field(input, "runId"), "run id")?;
    let state = one_of(
        input,
        "state",
        &["passed", "failed", "timeout", "error", "interrupted"],
    )?;
    let exit_code = opt_i64(input, "exitCode")?;
    let failed_step = opt_i64(input, "failedStep")?;
    let duration_ms = opt_i64(input, "durationMs")?;
    if state == "passed" && exit_code != Some(0) {
        return Err(KernelError::type_error("a passed run has exit code 0"));
    }
    if failed_step.is_some_and(|n| n < 1) || duration_ms.is_some_and(|n| n < 0) {
        return Err(KernelError::type_error(
            "failedStep must be at least 1 and durationMs not negative",
        ));
    }
    let tail = match field(input, "outputTail") {
        Value::Null => "",
        Value::String(text) => text.as_str(),
        _ => return Err(KernelError::type_error("outputTail must be text")),
    };
    let supplied_truncated = match field(input, "outputTruncated") {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        _ => return Err(KernelError::type_error("outputTruncated must be a boolean")),
    };
    let (tail, cut) = bounded_tail(tail);
    let truncated = supplied_truncated || cut;
    kernel.mutate(context, "verify.finish", CONTROL, input, |_| {
        let run = require_run(kernel, run_id)?;
        if run.state != "running" {
            return Err(KernelError::controller(format!(
                "the verify run is {}, not running",
                run.state
            )));
        }
        let now = kernel.now();
        execute(
            &kernel.database,
            "UPDATE verify_runs SET state = ?, finished_at = ?, exit_code = ?, failed_step = ?, duration_ms = ?,
               output_tail = ?, output_truncated = ? WHERE project_id = ? AND run_id = ?",
            rusqlite::params![
                state,
                now,
                exit_code,
                failed_step,
                duration_ms,
                tail,
                truncated as i64,
                kernel.project_id,
                run_id
            ],
        )?;
        let finished = require_run(kernel, run_id)?;
        let mut notified = 0;
        if let Some(parties) = message_notices::notice_parties(kernel)? {
            let body = notice_text(&finished);
            let hash = sha256(&body);
            let mut message_id: Option<String> = None;
            for recipient in notice_recipients(kernel, &finished, &parties.pm)? {
                let id = messages::insert_queued_message(
                    kernel,
                    &parties.controller_actor_id,
                    &recipient,
                    &body,
                    &hash,
                    &now,
                    false,
                    if state == "passed" {
                        messages::class_for_kind(kernel, "verify_finished")
                    } else {
                        MessageClass::Action
                    },
                )?;
                notified += 1;
                message_id.get_or_insert(id);
            }
            execute(
                &kernel.database,
                "UPDATE verify_runs SET notified_message_id = ? WHERE project_id = ? AND run_id = ?",
                rusqlite::params![message_id, kernel.project_id, run_id],
            )?;
        }
        let event = MutationEvent::new("verify_run", run_id, 0)
            .transition("running", state)
            .with_details(json!({"exitCode": exit_code, "failedStep": failed_step, "notices": notified}));
        Ok(MutationOutput::new(run_value(kernel, run_id)?, event))
    })
}

/// `cancelVerifyRun`: a queued run is cancelled; a running one is left for the caller to abort (`needsAbort`).
pub fn cancel_verify_run(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let run_id = safe_id(field(input, "runId"), "run id")?;
    kernel.mutate(context, "verify.cancel", REQUEST, input, |_| {
        let run = require_run(kernel, run_id)?;
        match run.state.as_str() {
            "queued" => {
                execute(
                    &kernel.database,
                    "UPDATE verify_runs SET state = 'cancelled', finished_at = ? WHERE project_id = ? AND run_id = ?",
                    [&kernel.now(), &kernel.project_id, run_id],
                )?;
                let event = MutationEvent::new("verify_run", run_id, 0).transition("queued", "cancelled");
                let mut value = run_value(kernel, run_id)?;
                value["needsAbort"] = Value::Bool(false);
                Ok(MutationOutput::new(value, event))
            }
            "running" => {
                let mut value = run.record(kernel)?;
                value["needsAbort"] = Value::Bool(true);
                let event = MutationEvent::new("verify_run", run_id, 0)
                    .with_details(json!({"needsAbort": true}));
                Ok(MutationOutput::new(value, event))
            }
            other => Err(KernelError::controller(format!(
                "the verify run is already {other}"
            ))),
        }
    })
}

/// `interruptActiveVerifyRuns`: every running run becomes interrupted (the daemon restarted or stopped); answers the
/// process each one left behind.
pub fn interrupt_active_verify_runs(
    kernel: &Kernel,
    context: &MutationContext,
) -> KernelResult<Value> {
    kernel.mutate(context, "verify.interrupt", CONTROL, &json!({}), |_| {
        let running = runs_where(kernel, "state = 'running'", &[], "sequence")?;
        let now = kernel.now();
        let mut interrupted = Vec::new();
        for run in &running {
            execute(
                &kernel.database,
                "UPDATE verify_runs SET state = 'interrupted', finished_at = ? WHERE project_id = ? AND run_id = ?",
                [&now, &kernel.project_id, &run.run_id],
            )?;
            interrupted.push(json!({
                "runId": run.run_id, "pgid": run.pgid, "leaderStart": run.leader_start, "scratchPath": run.scratch_path,
            }));
        }
        let event = MutationEvent::new("verify_run", &kernel.project_id, 0)
            .with_details(json!({"interrupted": running.len()}));
        Ok(MutationOutput::new(json!({"runs": interrupted}), event))
    })
}

/// `clearVerifyOrphan`: notes that the process of a finished run was found gone or killed.
pub fn clear_verify_orphan(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let run_id = safe_id(field(input, "runId"), "run id")?;
    kernel.mutate(context, "verify.orphan", CONTROL, input, |_| {
        require_run(kernel, run_id)?;
        execute(
            &kernel.database,
            "UPDATE verify_runs SET orphan_cleared_at = COALESCE(orphan_cleared_at, ?) WHERE project_id = ? AND run_id = ?",
            [&kernel.now(), &kernel.project_id, run_id],
        )?;
        let event = MutationEvent::new("verify_run", run_id, 0).with_details(json!({"orphanCleared": true}));
        Ok(MutationOutput::new(run_value(kernel, run_id)?, event))
    })
}

/// `verifyRun`: one run record; null when there is none.
pub fn verify_run(kernel: &Kernel, run_id: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    match run_row(kernel, run_id)? {
        Some(run) => run.record(kernel),
        None => Ok(Value::Null),
    }
}

/// `verifyRuns`: the runs of one subject (or all), newest first.
pub fn verify_runs(kernel: &Kernel, subject_id: Option<&str>, limit: i64) -> KernelResult<Value> {
    kernel.assert_open()?;
    let limit = limit.clamp(1, 200);
    let rows = match subject_id {
        Some(id) => {
            let order = format!("sequence DESC LIMIT {limit}");
            runs_where(
                kernel,
                "(subject_report_id = ? OR subject_integration_id = ?)",
                &[&id, &id],
                &order,
            )?
        }
        None => runs_where(
            kernel,
            "1 = 1",
            &[],
            &format!("sequence DESC LIMIT {limit}"),
        )?,
    };
    let records = rows
        .iter()
        .map(|run| run.record(kernel))
        .collect::<KernelResult<Vec<_>>>()?;
    Ok(json!({"runs": records}))
}

/// `nextQueuedVerifyRuns`: the oldest queued runs, oldest first.
pub fn next_queued_verify_runs(kernel: &Kernel, limit: i64) -> KernelResult<Value> {
    kernel.assert_open()?;
    let rows = runs_where(
        kernel,
        "state = 'queued'",
        &[],
        &format!("sequence LIMIT {}", limit.clamp(1, 200)),
    )?;
    let records = rows
        .iter()
        .map(|run| run.record(kernel))
        .collect::<KernelResult<Vec<_>>>()?;
    Ok(json!({"runs": records}))
}

/// The gate answer for a commit and commands: a passed run under the commands first, then one still active, then the
/// latest that did not pass; a passed run under other commands only is stale.
fn gate(
    kernel: &Kernel,
    commit_sha: &str,
    commands_sha: &str,
) -> KernelResult<(&'static str, Option<RunRow>)> {
    let runs = runs_where(kernel, "commit_sha = ?", &[&commit_sha], "sequence DESC")?;
    let same: Vec<&RunRow> = runs
        .iter()
        .filter(|r| r.commands_sha == commands_sha)
        .collect();
    if let Some(run) = same.iter().find(|r| r.state == "passed") {
        return Ok(("passed", Some((*run).clone())));
    }
    if let Some(run) = same
        .iter()
        .find(|r| r.state == "queued" || r.state == "running")
    {
        return Ok(("verify_running", Some((*run).clone())));
    }
    if let Some(run) = same.first() {
        return Ok(("verify_failed", Some((*run).clone())));
    }
    if runs.iter().any(|r| r.state == "passed") {
        return Ok(("verify_stale_commands", None));
    }
    Ok(("verify_missing", None))
}

/// The verify run a review brief shows for a commit: the one that decides the gate, or the latest run at the commit
/// (`missing` when there is none).
pub fn summary_at(
    kernel: &Kernel,
    commit_sha: &str,
    commands_sha: &str,
) -> KernelResult<VerifySummary> {
    let (_, run) = gate(kernel, commit_sha, commands_sha)?;
    let run = match run {
        Some(run) => Some(run),
        None => runs_where(
            kernel,
            "commit_sha = ?",
            &[&commit_sha],
            "sequence DESC LIMIT 1",
        )?
        .into_iter()
        .next(),
    };
    Ok(match run {
        Some(run) => VerifySummary {
            tail_excerpt: tail_excerpt(&run.output_tail),
            run_id: run.run_id,
            state: run.state,
            commit_sha: run.commit_sha,
            duration_ms: run.duration_ms,
            failed_step: run.failed_step,
            exit_code: run.exit_code,
        },
        None => VerifySummary {
            run_id: String::new(),
            state: "missing".into(),
            commit_sha: commit_sha.to_string(),
            duration_ms: None,
            failed_step: None,
            exit_code: None,
            tail_excerpt: String::new(),
        },
    })
}

/// `verifyGate`: whether the commit passed the commands, with the run that decides it.
pub fn verify_gate(kernel: &Kernel, commit_sha: &str, commands_sha: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    let (code, run) = gate(kernel, commit_sha, commands_sha)?;
    Ok(json!({
        "ok": code == "passed",
        "code": code,
        "run": run.map(|r| r.record(kernel)).transpose()?,
    }))
}

/// `verifyGateForSubjects`: the gate for the commit of each report or integration. `failures` has one entry per subject
/// that does not pass.
pub fn verify_gate_for_subjects(
    kernel: &Kernel,
    subject_ids: &[String],
    commands_sha: &str,
) -> KernelResult<Value> {
    kernel.assert_open()?;
    let mut failures = Vec::new();
    for subject_id in subject_ids {
        let subject = subject_of(kernel, subject_id)?;
        let (code, run) = match &subject.commit_sha {
            Some(commit_sha) => gate(kernel, commit_sha, commands_sha)?,
            None => ("verify_missing", None),
        };
        if code == "passed" {
            continue;
        }
        failures.push(json!({
            "subjectId": subject_id,
            "commitSha": subject.commit_sha,
            "code": code,
            "runId": run.as_ref().map(|r| r.run_id.clone()),
            "state": run.as_ref().map(|r| r.state.clone()),
        }));
    }
    Ok(json!({"ok": failures.is_empty(), "failures": failures}))
}

/// `verifyStatus`: the runs that are queued or running and the latest finished ones, without their output.
pub fn verify_status(kernel: &Kernel) -> KernelResult<Value> {
    kernel.assert_open()?;
    let slim = |rows: Vec<RunRow>| -> KernelResult<Vec<Value>> {
        rows.iter()
            .map(|r| r.record(kernel).map(without_tail))
            .collect()
    };
    let active = runs_where(kernel, &format!("state IN {ACTIVE}"), &[], "sequence")?;
    let recent = runs_where(
        kernel,
        &format!("state NOT IN {ACTIVE}"),
        &[],
        &format!("finished_at DESC, sequence DESC LIMIT {STATUS_RECENT}"),
    )?;
    Ok(json!({"active": slim(active)?, "recent": slim(recent)?}))
}

pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "queueVerifyRun" => {
            let context = arg!(a.ctx(0));
            Some(call(queue_verify_run(kernel, &context, a.value(1))))
        }
        "claimVerifyRun" => {
            let context = arg!(a.ctx(0));
            Some(call(claim_verify_run(kernel, &context, a.value(1))))
        }
        "recordVerifyProcess" => {
            let context = arg!(a.ctx(0));
            Some(call(record_verify_process(kernel, &context, a.value(1))))
        }
        "finishVerifyRun" => {
            let context = arg!(a.ctx(0));
            Some(call(finish_verify_run(kernel, &context, a.value(1))))
        }
        "cancelVerifyRun" => {
            let context = arg!(a.ctx(0));
            Some(call(cancel_verify_run(kernel, &context, a.value(1))))
        }
        "interruptActiveVerifyRuns" => {
            let context = arg!(a.ctx(0));
            Some(call(interrupt_active_verify_runs(kernel, &context)))
        }
        "clearVerifyOrphan" => {
            let context = arg!(a.ctx(0));
            Some(call(clear_verify_orphan(kernel, &context, a.value(1))))
        }
        "verifyRun" => {
            let run_id = arg!(a.str(0, "runId"));
            Some(call(verify_run(kernel, run_id)))
        }
        "verifyRuns" => {
            let subject_id = arg!(a.opt_str(0, "subjectId"));
            let limit = arg!(a.opt_i64(1, "limit")).unwrap_or(20);
            Some(call(verify_runs(kernel, subject_id, limit)))
        }
        "nextQueuedVerifyRuns" => {
            let limit = arg!(a.opt_i64(0, "limit")).unwrap_or(20);
            Some(call(next_queued_verify_runs(kernel, limit)))
        }
        "verifyGate" => {
            let commit_sha = arg!(a.str(0, "commitSha"));
            let commands_sha = arg!(a.str(1, "commandsSha"));
            Some(call(verify_gate(kernel, commit_sha, commands_sha)))
        }
        "verifyGateForSubjects" => {
            let ids = arg!(a.opt_str_list(0)).unwrap_or_default();
            let commands_sha = arg!(a.str(1, "commandsSha"));
            Some(call(verify_gate_for_subjects(kernel, &ids, commands_sha)))
        }
        "verifyStatus" => Some(call(verify_status(kernel))),
        _ => None,
    }
}
