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
        "verify takes a report or integration id (<id> [--repo <repo>]), or show <run-id>, list [<id>] or cancel <run-id>",
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
    // `<id> --repo <repo>` reruns one repo of a workspace report.
    let (args, repo) = match args.iter().position(|a| a == "--repo") {
        None => (args, None),
        Some(1) if args.len() == 3 && !matches!(first.as_str(), "show" | "list" | "cancel") => {
            (&args[..1], Some(args[2].as_str()))
        }
        Some(_) => return Some(usage()),
    };
    if args.iter().any(|a| !is_safe_agent_id(a)) || repo.is_some_and(|r| !is_safe_agent_id(r)) {
        return Some(fail(ErrorCode::InvalidRequest, "an id is not valid"));
    }
    Some(match (first.as_str(), args.len()) {
        ("show", 2) => show(env, &args[1]),
        ("list", 1 | 2) => list(env, args.get(1).cloned()),
        ("cancel", 2) => cancel(env, call, &service, &args[1]),
        ("show" | "list" | "cancel", _) => usage(),
        (_, 1) => queue(env, call, &service, first, repo),
        _ => usage(),
    })
}

/// Whether the project is a workspace (it has `[repos]`).
pub(crate) fn is_workspace(deps: &Deps) -> bool {
    deps.options
        .capstan
        .as_ref()
        .is_some_and(|config| config.workspace.is_some())
}

/// The members of the set a subject id names (the set's own id or a member's integration id), as the set gate reads them
/// under any commands: `None` when the subject is not in a set.
pub(crate) fn set_view(core: &Core, subject: &str) -> KernelResult<Option<Value>> {
    match core.set_of_subject(subject)? {
        Some(set_id) => Ok(Some(core.set_gate(&set_id, "")?)),
        None => Ok(None),
    }
}

/// The commands sha of `repo`. A repo without its own `[repos.<r>.verify]` runs the top-level commands (the entry named
/// ''); an empty sha would accept a run under any commands, and a [verify] section always gives a sha.
pub(crate) fn sha_for(shas: &[(String, String)], repo: &str) -> String {
    let found = |name: &str| {
        shas.iter()
            .find(|(n, _)| n == name)
            .map(|(_, sha)| sha.clone())
    };
    found(repo).or_else(|| found("")).unwrap_or_default()
}

/// The gate of a set under each repo's current commands: one failure per member without a counted passed run.
pub(crate) fn set_gate_failures(
    deps: &Deps,
    service: &VerifyService,
    set_id: &str,
) -> Result<Vec<Value>, KernelError> {
    let shas = repo_shas(deps, service);
    let id = set_id.to_string();
    deps.kernel.run(move |core| {
        let gate = core.set_gate_by(&id, &|repo| sha_for(&shas, repo))?;
        let mut failures = Vec::new();
        for member in gate["members"].as_array().into_iter().flatten() {
            let code = member["code"].as_str().unwrap_or("");
            if code == "passed" {
                continue;
            }
            failures.push(json!({
                "subjectId": id,
                "repo": member["repo"],
                "commitSha": member["commitSha"],
                "code": code,
                "runId": member["runId"],
                "state": if code == "verify_running" { json!("running") } else { Value::Null },
            }));
        }
        Ok(failures)
    })
}

/// One run queued for `subject` in `repo` (`None`: the subject's own repo) with that repo's commands.
fn queue_one(
    deps: &Deps,
    service: &VerifyService,
    credential: &str,
    subject: &str,
    repo: Option<&str>,
    trigger: &str,
) -> Result<Value, KernelError> {
    let (verify, sha) = match repo {
        Some(repo) => (service.verify_for(repo), service.commands_sha_for(repo)),
        None => (service.config().clone(), service.commands_sha()),
    };
    let mut input = json!({
        "subjectId": subject,
        "commands": verify.commands,
        "commandsSha": sha,
        "trigger": trigger,
    });
    if let Some(repo) = repo {
        input["repo"] = json!(repo);
    }
    let credential = credential.to_string();
    deps.kernel.run(move |core| {
        let context = new_context(core, &credential)?;
        core.queue_verify_run(&context, &input)
    })
}

fn queue(
    env: &CommandEnv<'_>,
    call: &CommandCall<'_>,
    service: &Arc<VerifyService>,
    subject: &str,
    repo: Option<&str>,
) -> CommandResponse {
    if let Some(problem) = service.problem() {
        return fail(
            ErrorCode::Rejected,
            format!("verify_scratch_refused: {problem}"),
        );
    }
    let workspace = is_workspace(env.deps);
    if repo.is_some() && !workspace {
        return fail(
            ErrorCode::Rejected,
            "verify_refused: --repo is only for workspace projects",
        );
    }
    let credential = call.credential.to_string();
    if !workspace {
        return match queue_one(env.deps, service, &credential, subject, None, "manual") {
            Ok(run) => {
                service.notify();
                ok(run)
            }
            Err(error) => refused(&error),
        };
    }
    let id = subject.to_string();
    let looked = env.deps.kernel.run(move |core| {
        if let Some(gate) = set_view(core, &id)? {
            return Ok(Looked::Set(gate));
        }
        let rows = core.report_commits(&id)?;
        Ok(Looked::Repos(
            rows.as_array()
                .into_iter()
                .flatten()
                .filter_map(|row| row["repo"].as_str().map(str::to_string))
                .collect::<Vec<_>>(),
        ))
    });
    // Each target is the subject a run is queued for and the repo it runs in.
    let (targets, set_id): (Vec<(String, String)>, Option<String>) = match looked {
        Ok(Looked::Set(gate)) => match set_targets(&gate, repo) {
            Ok(targets) => (targets, gate["setId"].as_str().map(str::to_string)),
            Err(why) => return fail(ErrorCode::Rejected, format!("verify_refused: {why}")),
        },
        Ok(Looked::Repos(repos)) => {
            if repos.is_empty() {
                return fail(
                    ErrorCode::Rejected,
                    format!("verify_refused: {subject} names no commit per repo (a report from before workspace mode, or not a report); report again to verify it"),
                );
            }
            let repos = match repo {
                Some(repo) if !repos.iter().any(|r| r == repo) => {
                    return fail(
                        ErrorCode::Rejected,
                        format!("verify_refused: repo {repo} is not in report {subject}"),
                    )
                }
                // `report_commits` never holds the root repo ('') in workspace mode, so --repo cannot name it.
                Some(repo) => vec![repo.to_string()],
                None => repos,
            };
            (
                repos
                    .into_iter()
                    .map(|r| (subject.to_string(), r))
                    .collect(),
                None,
            )
        }
        Err(error) => return map_kernel_error(&error),
    };
    let mut runs = Vec::new();
    for (target_subject, target) in &targets {
        match queue_one(
            env.deps,
            service,
            &credential,
            target_subject,
            Some(target),
            "manual",
        ) {
            Ok(run) => runs.push(run),
            Err(error) => {
                // What was queued before the refusal runs now, not at the next idle poll.
                service.notify();
                let ids: Vec<String> = runs
                    .iter()
                    .map(|r| {
                        format!(
                            "{} ({})",
                            r["runId"].as_str().unwrap_or(""),
                            r["repo"].as_str().unwrap_or("")
                        )
                    })
                    .collect();
                let mut answer = refused(&error);
                if !ids.is_empty() {
                    answer = fail(
                        ErrorCode::Rejected,
                        format!(
                            "verify_refused: repo {target}: {}. Queued before that: {}",
                            error.message(),
                            ids.join(", ")
                        ),
                    );
                }
                return answer;
            }
        }
    }
    service.notify();
    let mut answer = runs[0].clone();
    if let Some(set_id) = set_id {
        answer["setId"] = json!(set_id);
    }
    answer["runs"] = Value::Array(
        runs.iter()
            .map(|run| {
                json!({
                    "repo": run["repo"],
                    "runId": run["runId"],
                    "state": run["state"],
                    "existing": run["existing"] == true,
                })
            })
            .collect(),
    );
    ok(answer)
}

/// What a workspace verify request names.
enum Looked {
    /// A set (its id or a member's): its gate view under any commands.
    Set(Value),
    /// A report: the repos it names a commit for.
    Repos(Vec<String>),
}

/// The (member integration, repo) pairs a verify of a set queues: one per unconfirmed member (a confirmed member's code is
/// on main and keeps its last passed run), or the one member `repo` names. Refused unless the set is merged.
fn set_targets(gate: &Value, repo: Option<&str>) -> Result<Vec<(String, String)>, String> {
    let set_id = gate["setId"].as_str().unwrap_or("");
    let state = gate["state"].as_str().unwrap_or("");
    if state != "merged" {
        return Err(format!(
            "set {set_id} is {state}; only a merged set is verified"
        ));
    }
    let members: Vec<&Value> = gate["members"].as_array().into_iter().flatten().collect();
    let pair = |m: &Value| {
        (
            m["integrationId"].as_str().unwrap_or("").to_string(),
            m["repo"].as_str().unwrap_or("").to_string(),
        )
    };
    match repo {
        Some(repo) => match members.iter().find(|m| m["repo"] == repo) {
            None => Err(format!("repo {repo} is not in set {set_id}")),
            Some(m) if m["confirmed"] == true => Err(format!(
                "repo {repo} of set {set_id} is confirmed; its code is on main and keeps its last passed run"
            )),
            Some(m) => Ok(vec![pair(m)]),
        },
        None => {
            let targets: Vec<_> = members
                .iter()
                .filter(|m| m["confirmed"] != true)
                .map(|m| pair(m))
                .collect();
            if targets.is_empty() {
                Err(format!("every member of set {set_id} is confirmed; nothing to verify"))
            } else {
                Ok(targets)
            }
        }
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

/// The answer of a cancel that waited for a running run: its real final state, and `alreadyFinished` when the run ended
/// some other way than by this cancel (it finished first).
fn cancel_answer(run_id: &str, state: Value) -> Value {
    let by_cancel = state == "interrupted" || state == "cancelled";
    let mut answer = json!({"runId": run_id, "state": state});
    if !by_cancel {
        answer["alreadyFinished"] = Value::Bool(true);
    }
    answer
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
                return ok(cancel_answer(run_id, state))
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
    let credential = deps.credential().to_string();
    // A workspace report is verified in every repo it names a commit for.
    let repos: Vec<Option<String>> = if is_workspace(deps) && trigger == Auto::Report {
        let id = subject_id.to_string();
        let rows = deps.kernel.run(move |core| core.report_commits(&id));
        let listed: Vec<Option<String>> = rows
            .ok()
            .and_then(|rows| rows.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|row| row["repo"].as_str().map(|r| Some(r.to_string())))
            .collect();
        if listed.is_empty() {
            vec![None]
        } else {
            listed
        }
    } else {
        vec![None]
    };
    for repo in repos {
        match queue_one(
            deps,
            service,
            &credential,
            subject_id,
            repo.as_deref(),
            "auto",
        ) {
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
    // A workspace gates integrate whenever [verify] is configured; a single-repo project keeps its switch.
    let workspace = is_workspace(deps);
    let guarded = match gate {
        Gate::Review => config.require_review,
        Gate::Integrate => config.require_integrate || workspace,
    };
    if !guarded {
        return Ok(());
    }
    if workspace {
        if gate == Gate::Review {
            // A set (its id or a member's) is gated as a whole: every member needs a counted passed run.
            let ids = subjects.to_vec();
            let sets = deps.kernel.run(move |core| {
                let mut sets: Vec<(String, Option<String>)> = Vec::new();
                for id in &ids {
                    sets.push((id.clone(), core.set_of_subject(id)?));
                }
                Ok(sets)
            })?;
            let mut rest = Vec::new();
            for (id, set) in sets {
                match set {
                    Some(set_id) => {
                        let failures = set_gate_failures(deps, service, &set_id)?;
                        if !failures.is_empty() {
                            return Err(required_error(&failures));
                        }
                    }
                    None => rest.push(id),
                }
            }
            if rest.is_empty() {
                return Ok(());
            }
            return workspace_gate(deps, service, &rest);
        }
        return workspace_gate(deps, service, subjects);
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

/// The commands sha of each repo of the workspace ('' is the root) under the current configuration.
pub(crate) fn repo_shas(deps: &Deps, service: &VerifyService) -> Vec<(String, String)> {
    let mut repos: Vec<String> = deps
        .options
        .capstan
        .as_ref()
        .and_then(|config| config.workspace.as_ref())
        .map(|w| w.repos.iter().map(|r| r.name.clone()).collect())
        .unwrap_or_default();
    repos.push(String::new());
    repos
        .into_iter()
        .map(|repo| {
            let sha = service.commands_sha_for(&repo);
            (repo, sha)
        })
        .collect()
}

/// The gate of a workspace: every report passes only when each repo it names a commit for has a passed run at
/// that commit with that repo's current commands.
pub(crate) fn workspace_gate(
    deps: &Deps,
    service: &VerifyService,
    subjects: &[String],
) -> Result<(), KernelError> {
    let shas = repo_shas(deps, service);
    let subjects = subjects.to_vec();
    let failures = deps.kernel.run(move |core| {
        let mut failures: Vec<Value> = Vec::new();
        for subject in &subjects {
            let rows = core.report_commits(subject)?;
            let rows: Vec<Value> = rows.as_array().cloned().unwrap_or_default();
            if rows.is_empty() {
                // No per-repo commits: an integration (a set member carries its repo) or a report that names none.
                let repo = core.repo_of_integration(subject)?.unwrap_or_default();
                let sha = shas
                    .iter()
                    .find(|(name, _)| *name == repo)
                    .map_or("", |(_, sha)| sha.as_str());
                let answer = core.verify_gate_for_subjects(std::slice::from_ref(subject), sha)?;
                if answer["ok"] != true {
                    failures.extend(answer["failures"].as_array().cloned().unwrap_or_default());
                }
                continue;
            }
            for row in rows {
                let repo = row["repo"].as_str().unwrap_or("");
                let commit = row["commitSha"].as_str().unwrap_or("");
                let sha = shas
                    .iter()
                    .find(|(name, _)| name == repo)
                    .map_or("", |(_, sha)| sha.as_str());
                let answer = core.verify_gate_in_repo(repo, commit, sha)?;
                if answer["ok"] == true {
                    continue;
                }
                failures.push(json!({
                    "subjectId": subject,
                    "repo": repo,
                    "commitSha": commit,
                    "code": answer["code"],
                    "runId": answer["run"]["runId"],
                    "state": answer["run"]["state"],
                }));
            }
        }
        Ok(failures)
    })?;
    if failures.is_empty() {
        Ok(())
    } else {
        Err(required_error(&failures))
    }
}
