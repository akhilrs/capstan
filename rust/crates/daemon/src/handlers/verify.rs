//! The verify command (`cstan verify <report-or-integration-id>`, `verify show`, `verify list`, `verify cancel`), and the
//! pieces the other handlers lean on: the automatic queueing after a report or a merged integration, and the gate that
//! `request-review` and `integrate` ask before they go on. The runs themselves are the `verify` service's.

use super::shared::{
    fail, is_safe_agent_id, map_kernel_error, ok, CommandCall, CommandEnv, CommandResponse,
    ErrorCode,
};
use super::HandlerMap;
use crate::deps::{new_context, Deps};
use crate::verify::VerifyService;
use capstan_kernel::helpers::{query_all, verify_required_error};
use capstan_kernel::types::Identity;
use capstan_kernel::{Core, KernelError, KernelResult};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How many runs `verify list` shows.
const LIST_LIMIT: i64 = 20;
/// How long `verify cancel` waits for a running run to end.
const CANCEL_WAIT: Duration = Duration::from_secs(15);

/// Registers the handler of the route this module serves (verify).
pub fn register(map: &mut HandlerMap) {
    map.insert("verify", verify_command);
}

fn usage() -> CommandResponse {
    fail(
        ErrorCode::InvalidRequest,
        "verify takes a report or integration id, or show <run-id>, list [<id>] or cancel <run-id>",
    )
}

fn not_configured() -> CommandResponse {
    fail(
        ErrorCode::Rejected,
        "verify_not_configured: the project has no [verify] section in capstan.toml",
    )
}

fn refused(error: &KernelError) -> CommandResponse {
    if error.is_controller_error() {
        fail(
            ErrorCode::Rejected,
            format!("verify_refused: {}", error.message()),
        )
    } else {
        map_kernel_error(error)
    }
}

fn verify_command(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let Some(service) = env.deps.verify.clone() else {
        return Some(not_configured());
    };
    let args = call.args;
    let Some(first) = args.first() else {
        return Some(usage());
    };
    if args.iter().any(|a| !is_safe_agent_id(a)) {
        return Some(fail(ErrorCode::InvalidRequest, "an id is not valid"));
    }
    Some(match (first.as_str(), args.len()) {
        ("show", 2) => show(env, &args[1]),
        ("list", 1 | 2) => list(env, args.get(1).cloned()),
        ("cancel", 2) => cancel(env, call, &service, &args[1]),
        ("show" | "list" | "cancel", _) => usage(),
        (_, 1) => queue(env, call, &service, first),
        _ => usage(),
    })
}

fn queue(
    env: &CommandEnv<'_>,
    call: &CommandCall<'_>,
    service: &Arc<VerifyService>,
    subject: &str,
) -> CommandResponse {
    if let Some(problem) = service.problem() {
        return fail(
            ErrorCode::Rejected,
            format!("verify_scratch_refused: {problem}"),
        );
    }
    let input = json!({
        "subjectId": subject,
        "commands": service.config().commands,
        "commandsSha": service.commands_sha(),
        "trigger": "manual",
    });
    let credential = call.credential.to_string();
    let queued = env.deps.kernel.run(move |core| {
        let context = new_context(core, &credential)?;
        core.queue_verify_run(&context, &input)
    });
    match queued {
        Ok(run) => {
            service.notify();
            ok(run)
        }
        Err(error) => refused(&error),
    }
}

fn show(env: &CommandEnv<'_>, run_id: &str) -> CommandResponse {
    let id = run_id.to_string();
    match env.deps.kernel.run(move |core| core.verify_run(&id)) {
        Ok(Value::Null) => fail(
            ErrorCode::Rejected,
            format!("unknown_run: no verify run {run_id}"),
        ),
        Ok(run) => ok(run),
        Err(error) => map_kernel_error(&error),
    }
}

fn list(env: &CommandEnv<'_>, subject: Option<String>) -> CommandResponse {
    match env
        .deps
        .kernel
        .run(move |core| core.verify_runs(subject.as_deref(), LIST_LIMIT))
    {
        Ok(mut answer) => {
            for run in answer["runs"].as_array_mut().into_iter().flatten() {
                if let Some(map) = run.as_object_mut() {
                    map.remove("outputTail");
                }
            }
            ok(answer)
        }
        Err(error) => map_kernel_error(&error),
    }
}

/// The agent ids of the architects of the plans the subject's reports belong to.
fn subject_architects(core: &Core, run: &Value) -> KernelResult<Vec<String>> {
    let kernel = core.kernel();
    let subject = run["subjectId"].as_str().unwrap_or("").to_string();
    let reports: Vec<String> = if run["subjectKind"] == "report" {
        vec![subject]
    } else {
        query_all(
            &kernel.database,
            "SELECT report_id FROM integration_reports WHERE project_id = ? AND integration_id = ?",
            [&kernel.project_id, &subject],
            |row| row.get::<_, String>(0),
        )?
    };
    let mut architects = Vec::new();
    for report in reports {
        architects.extend(query_all(
            &kernel.database,
            "SELECT DISTINCT pl.architect_agent_id FROM agent_reports r
             JOIN agents a ON a.project_id = r.project_id AND a.agent_id = r.agent_id AND a.generation = r.generation
             JOIN plan_packages pp ON pp.project_id = a.project_id AND pp.assignee_agent_id = a.agent_id
             JOIN plans pl ON pl.project_id = pp.project_id AND pl.plan_id = pp.plan_id
             WHERE r.project_id = ? AND r.report_id = ? AND pl.architect_agent_id IS NOT NULL",
            [&kernel.project_id, &report],
            |row| row.get::<_, String>(0),
        )?);
    }
    Ok(architects)
}

/// Who may cancel a run: the operator, the PM, whoever asked for it, and the architect of the subject's plan.
fn may_cancel(core: &Core, identity: &Identity, run: &Value) -> KernelResult<bool> {
    if identity.role == "operator" {
        return Ok(true);
    }
    let Some(agent) = &identity.agent else {
        return Ok(false);
    };
    if agent.kind == "PM" || run["requestedBy"] == agent.agent_id.as_str() {
        return Ok(true);
    }
    Ok(subject_architects(core, run)?.contains(&agent.agent_id))
}

fn cancel(
    env: &CommandEnv<'_>,
    call: &CommandCall<'_>,
    service: &Arc<VerifyService>,
    run_id: &str,
) -> CommandResponse {
    let (id, controller) = (run_id.to_string(), env.deps.credential().to_string());
    // The decision is made here; the ledger step runs as the daemon, because the capability the kernel asks for belongs
    // to every reporting agent.
    let decided = env.deps.kernel.run({
        let (id, identity) = (id.clone(), call.identity.clone());
        move |core| {
            let run = core.verify_run(&id)?;
            if run.is_null() {
                return Ok(Value::Null);
            }
            Ok(json!({"allowed": may_cancel(core, &identity, &run)?}))
        }
    });
    let decided = match decided {
        Ok(Value::Null) => {
            return fail(
                ErrorCode::Rejected,
                format!("unknown_run: no verify run {run_id}"),
            )
        }
        Ok(decided) => decided,
        Err(error) => return map_kernel_error(&error),
    };
    if decided["allowed"] != true {
        return fail(
            ErrorCode::Forbidden,
            "only the operator, the PM, the agent that requested the run or the plan's architect can cancel it",
        );
    }
    let cancelled = env.deps.kernel.run({
        let id = id.clone();
        move |core| {
            let context = new_context(core, &controller)?;
            core.cancel_verify_run(&context, &json!({"runId": id}))
        }
    });
    let record = match cancelled {
        Ok(record) => record,
        Err(error) => return refused(&error),
    };
    if record["needsAbort"] != true {
        return ok(json!({"runId": record["runId"], "state": record["state"]}));
    }
    service.abort(run_id);
    let started = Instant::now();
    loop {
        let seen = env.deps.kernel.change_counter();
        let id = id.clone();
        let state = env
            .deps
            .kernel
            .run(move |core| core.verify_run(&id))
            .map(|run| run["state"].clone());
        match state {
            Ok(state) if state != "running" && !state.is_null() => {
                return ok(json!({"runId": run_id, "state": state}))
            }
            Err(error) => return map_kernel_error(&error),
            Ok(_) => {}
        }
        if started.elapsed() >= CANCEL_WAIT {
            return ok(json!({"runId": run_id, "state": "running"}));
        }
        env.deps
            .kernel
            .wait_change(seen, Duration::from_millis(250), None);
    }
}

// ------------------------------------------------------------------------------------------------ automatic runs

/// Which automatic trigger fired.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Auto {
    Report,
    Integration,
}

/// Queues a run with trigger `auto` for the subject when `[verify] auto` names the trigger. A run for the same commit
/// and commands that is already active is the answer; a failure is logged and never fails the command that caused it.
pub fn auto_queue(deps: &Deps, trigger: Auto, subject_id: &str) {
    let Some(service) = &deps.verify else { return };
    if service.problem().is_some() {
        return;
    }
    let config = service.config();
    let wanted = match trigger {
        Auto::Report => config.auto_report,
        Auto::Integration => config.auto_integration,
    };
    if !wanted {
        return;
    }
    let input = json!({
        "subjectId": subject_id,
        "commands": config.commands,
        "commandsSha": service.commands_sha(),
        "trigger": "auto",
    });
    let credential = deps.credential().to_string();
    let queued = deps.kernel.run(move |core| {
        let context = new_context(core, &credential)?;
        core.queue_verify_run(&context, &input)
    });
    match queued {
        Ok(run) => {
            deps.detail_log(
                "verify_auto_queued",
                json!({"subjectId": subject_id, "runId": run["runId"], "existing": run["existing"]}),
            );
            service.notify();
        }
        Err(error) => deps.detail_log(
            "verify_auto_failed",
            json!({"subjectId": subject_id, "error": error.to_string()}),
        ),
    }
}

// ------------------------------------------------------------------------------------------------ the gate

/// What the gate guards.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    Review,
    Integrate,
}

/// The `verify_required` refusal for failing subjects: the kernel's message, and for a run that is still queued or
/// running its id and state, with what to wait for.
pub fn required_error(failures: &[Value]) -> KernelError {
    let error = verify_required_error(failures);
    let mut message = error.message().to_string();
    for failure in failures {
        if let (Some(run_id), Some(state @ ("queued" | "running"))) =
            (failure["runId"].as_str(), failure["state"].as_str())
        {
            message.push_str(&format!(
                " Run {run_id} is {state}: ask again after its \"Verify run {run_id} ... passed\" notice."
            ));
        }
    }
    KernelError::MutationConflict(message)
}

/// Whether the subjects may go on: `Ok` when no `[verify]` gate guards `gate` or every subject has a passed run at its
/// commit with the current commands. A ledger read only; nothing is run or waited for.
pub fn check_gate(deps: &Deps, gate: Gate, subjects: &[String]) -> Result<(), KernelError> {
    let Some(service) = &deps.verify else {
        return Ok(());
    };
    let config = service.config();
    let guarded = match gate {
        Gate::Review => config.require_review,
        Gate::Integrate => config.require_integrate,
    };
    if !guarded {
        return Ok(());
    }
    let (subjects, sha) = (subjects.to_vec(), service.commands_sha());
    let answer = deps
        .kernel
        .run(move |core| core.verify_gate_for_subjects(&subjects, &sha))?;
    if answer["ok"] == true {
        return Ok(());
    }
    let failures = answer["failures"].as_array().cloned().unwrap_or_default();
    Err(required_error(&failures))
}
