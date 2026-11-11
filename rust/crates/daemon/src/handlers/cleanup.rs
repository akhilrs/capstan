//! The cleanup command (`cstan cleanup`) and the cleanup that follows a confirmed integration.
//!
//! The engine is `capstan_kernel::cleanup`; the kernel thread runs it (it has no kernel dispatch op), like the
//! integration pipeline. The PM and the operator are the only callers.

use super::reports::with_integration_state;
use super::shared::{
    fail, map_kernel_error, ok, CommandCall, CommandEnv, CommandResponse, ErrorCode,
};
use super::HandlerMap;
use crate::deps::new_context;
use capstan_config::{
    Cleanup, CleanupBranches as ConfigBranches, CleanupWorktrees as ConfigWorktrees, Workspace,
};
use capstan_kernel::cleanup::{
    self, Applied, Candidate, CleanupBranches, CleanupDeps, CleanupError, CleanupResult,
    CleanupSettings, CleanupWorktrees, RepoScope,
};
use capstan_kernel::integrate::GitRepo;
use capstan_kernel::types::MutationContext;
use capstan_kernel::KernelError;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long one cleanup run may take on the kernel thread.
pub const BUDGET: Duration = Duration::from_secs(30);

/// Test hook: while set, the cleanup after a confirm fails with an engine error before it touches anything.
pub static FAIL_AFTER_CONFIRM: AtomicBool = AtomicBool::new(false);

/// A hook called with a worktree path.
pub type PathHook = std::sync::Arc<dyn Fn(&std::path::Path) + Send + Sync>;

/// Test hook: called with the path of a worktree right before its final safety re-check. It is the daemon's value for
/// `CleanupDeps::before_delete`; nothing in the daemon sets it (no command, config or file reaches it), so it is empty and
/// a no-op in production, like `FAIL_AFTER_CONFIRM`.
#[doc(hidden)]
pub static BEFORE_DELETE: std::sync::Mutex<Option<PathHook>> = std::sync::Mutex::new(None);

fn before_delete(path: &std::path::Path) {
    let hook = BEFORE_DELETE.lock().ok().and_then(|slot| slot.clone());
    if let Some(hook) = hook {
        hook(path);
    }
}

/// Registers the handler of the route this module serves (cleanup).
pub fn register(map: &mut HandlerMap) {
    map.insert("cleanup", cleanup_command);
}

fn settings_of(config: Cleanup) -> CleanupSettings {
    CleanupSettings {
        branches: match config.branches {
            ConfigBranches::Auto => CleanupBranches::Auto,
            ConfigBranches::Ask => CleanupBranches::Ask,
            ConfigBranches::Off => CleanupBranches::Off,
        },
        worktrees: match config.worktrees {
            ConfigWorktrees::Release => CleanupWorktrees::Release,
            ConfigWorktrees::Keep => CleanupWorktrees::Keep,
        },
        include_unlanded: config.include_unlanded,
    }
}

fn entry_value(c: &Candidate) -> Value {
    json!({
        "branch": c.branch, "sha": c.sha, "class": c.class.as_str(), "worktree": c.worktree_path,
        "reason": c.reason, "reportId": c.report_id, "integrationId": c.integration_id,
    })
}

fn applied_value(applied: &Applied) -> Value {
    json!({
        "deleted": applied.deleted.iter().map(|d| json!({
            "branch": d.branch, "sha": d.sha, "worktree": d.worktree_path,
        })).collect::<Vec<_>>(),
        "skipped": applied.skipped.iter().map(|s| json!({"branch": s.branch, "reason": s.reason})).collect::<Vec<_>>(),
        "restore": applied.restore,
    })
}

fn protected_reason(reason: &str) -> &'static str {
    match reason {
        "main_or_head" => "the project's main branch or the one it has checked out",
        "checked_out" => "checked out in a worktree that no ended agent owns",
        "active_agent" => "an agent is working on it",
        "integration_branch_unsettled" => "its integration is not confirmed or discarded yet",
        "dirty_worktree" => "its worktree has uncommitted changes",
        "kept_worktree" => "its worktree is kept by the cleanup settings",
        _ => "protected",
    }
}

/// What a cleanup run on the kernel thread gave back, with the log entries it wrote.
struct Ran<T> {
    result: CleanupResult<T>,
    logs: Vec<(String, Value)>,
}

/// Runs `job` on the kernel thread with the engine's dependencies. `None` when the controller is closed.
fn on_kernel<T, F>(env: &CommandEnv<'_>, job: F) -> Option<Ran<T>>
where
    T: Send + 'static,
    F: FnOnce(&CleanupDeps) -> CleanupResult<T> + Send + 'static,
{
    let root = env.deps.options.workspace_root.clone();
    on_kernel_at(env, root, job)
}

/// `on_kernel` with the repo `root` the cleanup looks at (a set member's repo for a confirm-time cleanup).
fn on_kernel_at<T, F>(env: &CommandEnv<'_>, root: std::path::PathBuf, job: F) -> Option<Ran<T>>
where
    T: Send + 'static,
    F: FnOnce(&CleanupDeps) -> CleanupResult<T> + Send + 'static,
{
    let controller = env.deps.credential().to_string();
    let settings = settings_of(env.config().map(|c| c.cleanup).unwrap_or_default());
    env.deps
        .kernel
        .call(move |core| {
            let repo = GitRepo::new(root);
            let logs: RefCell<Vec<(String, Value)>> = RefCell::new(Vec::new());
            let context = |credential: &str| -> MutationContext {
                new_context(core, credential).unwrap_or_else(|_| MutationContext {
                    credential: credential.to_string(),
                    request_id: String::new(),
                    idempotency_key: String::new(),
                    expected_version: 0,
                    input_revision: 0,
                })
            };
            let log =
                |event: &str, details: Value| logs.borrow_mut().push((event.to_string(), details));
            let now = Instant::now;
            let result = with_integration_state(|state| {
                let deps = CleanupDeps {
                    kernel: core.kernel(),
                    git: &repo,
                    integrations: state,
                    credential: &controller,
                    context: &context,
                    log: &log,
                    settings,
                    budget: BUDGET,
                    now: &now,
                    before_delete: &before_delete,
                };
                job(&deps)
            });
            Ran {
                result,
                logs: logs.into_inner(),
            }
        })
        .ok()
}

fn refusal(error: &CleanupError) -> CommandResponse {
    match error {
        CleanupError::Refused { message, .. } => fail(ErrorCode::Rejected, message.clone()),
        CleanupError::Git(_) => fail(
            ErrorCode::Error,
            "the controller could not run git just now",
        ),
        CleanupError::Kernel(error) if error.is_controller_error() => fail(
            ErrorCode::Rejected,
            format!("cleanup_refused: {}", error.message()),
        ),
        CleanupError::Kernel(error) => map_kernel_error(error),
    }
}

fn removed_note(repo: &str) -> String {
    format!("repo {repo} left the config; landed and clean cannot be checked; name it to delete (cstan cleanup apply <id> --branch <branch>); only the worktree folder is removed, the repo's .git/worktrees metadata is left for git worktree prune")
}

/// The workspace of the configuration, when it has `[repos]`.
fn workspace_of(env: &CommandEnv<'_>) -> Option<Workspace> {
    env.config().and_then(|c| c.workspace.clone())
}

/// Where a workspace cleanup of `repo` runs: `None` for the project root (single-repo behaviour).
struct Target {
    repo: String,
    root: std::path::PathBuf,
    gone: bool,
}

impl Target {
    fn scope<'a>(&'a self, worktree_dir: &'a std::path::Path) -> RepoScope<'a> {
        RepoScope {
            repo: &self.repo,
            repo_root: &self.root,
            worktree_dir,
            gone: self.gone,
        }
    }
}

fn target_of(env: &CommandEnv<'_>, ws: &Workspace, repo: &str) -> Target {
    Target {
        repo: repo.to_string(),
        root: env.deps.repo_root(repo),
        gone: ws.repo(repo).is_none(),
    }
}

/// Runs `job` for every repo of the workspace (configured ones in merge order, then removed ones) and collects what it
/// answers. A refusal or an error ends the run; the offers already recorded stay.
fn per_repo<F>(
    env: &CommandEnv<'_>,
    ws: &Workspace,
    job: F,
) -> Result<Vec<(Target, Value)>, CommandResponse>
where
    F: Fn(&CleanupDeps, &Target, &std::path::Path) -> CleanupResult<Value> + Clone + Send + 'static,
{
    let mut targets: Vec<Target> = ws
        .merge_order
        .iter()
        .map(|repo| target_of(env, ws, repo))
        .collect();
    let configured: Vec<String> = ws.merge_order.clone();
    let removed = on_kernel(env, move |deps| cleanup::removed_repos(deps, &configured));
    let Some(removed) = removed else {
        return Err(map_kernel_error(&KernelError::controller(
            "controller is closed",
        )));
    };
    for (event, details) in removed.logs {
        env.log(&event, details);
    }
    match removed.result {
        Ok(names) => targets.extend(names.iter().map(|repo| target_of(env, ws, repo))),
        Err(error) => return Err(refusal(&error)),
    }
    let mut out = Vec::new();
    for target in targets {
        let run = job.clone();
        let worktree_dir = ws.worktree_dir.clone();
        let root = target.root.clone();
        let on = Target {
            repo: target.repo.clone(),
            root: target.root.clone(),
            gone: target.gone,
        };
        let Some(ran) = on_kernel_at(env, root, move |deps| run(deps, &on, &worktree_dir)) else {
            return Err(map_kernel_error(&KernelError::controller(
                "controller is closed",
            )));
        };
        for (event, details) in ran.logs {
            env.log(&event, details);
        }
        match ran.result {
            Ok(value) => out.push((target, value)),
            Err(error) => return Err(refusal(&error)),
        }
    }
    Ok(out)
}

/// The cleanup of a workspace: one cleanup per repo that has candidates.
fn workspace_offer(
    env: &CommandEnv<'_>,
    ws: &Workspace,
    requested_by: String,
    include_unlanded: bool,
) -> CommandResponse {
    let ran = per_repo(env, ws, move |deps, target, worktree_dir| {
        let scope = target.scope(worktree_dir);
        let offer = cleanup::offer_in(
            deps,
            "manual",
            None,
            &requested_by,
            include_unlanded,
            Some(&scope),
        )?;
        Ok(match offer.cleanup_id {
            Some(id) => {
                let mut value = json!({
                    "cleanupId": id, "repo": target.repo, "candidates": offer.entries.len(),
                });
                if target.gone {
                    value["note"] = json!(removed_note(&target.repo));
                }
                value
            }
            None => Value::Null,
        })
    });
    match ran {
        Ok(rows) => ok(json!({
            "cleanups": rows.into_iter().map(|(_, v)| v).filter(|v| !v.is_null()).collect::<Vec<_>>(),
        })),
        Err(response) => response,
    }
}

/// `--dry-run` in a workspace: the groups of every configured repo, each entry named with its repo.
fn workspace_dry_run(env: &CommandEnv<'_>, ws: &Workspace) -> CommandResponse {
    let ran = per_repo(env, ws, move |deps, target, _| {
        if target.gone {
            return Ok(Value::Null);
        }
        let c = cleanup::classify(deps)?;
        let tag = |mut value: Value| {
            value["repo"] = json!(target.repo);
            value
        };
        Ok(json!({
            "landed": c.landed.iter().map(|e| tag(entry_value(e))).collect::<Vec<_>>(),
            "unlanded": c.unlanded.iter().map(|e| tag(entry_value(e))).collect::<Vec<_>>(),
            "protected": c.protected.iter().map(|p| tag(json!({
                "branch": p.branch, "sha": p.sha, "worktree": Value::Null,
                "reason": protected_reason(p.reason), "code": p.reason,
            }))).collect::<Vec<_>>(),
        }))
    });
    let rows = match ran {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let mut out = json!({"dryRun": true, "landed": [], "unlanded": [], "protected": []});
    for (_, value) in rows {
        for key in ["landed", "unlanded", "protected"] {
            if let (Some(into), Some(from)) = (out[key].as_array_mut(), value[key].as_array()) {
                into.extend(from.iter().cloned());
            }
        }
    }
    ok(out)
}

/// The workspace and the repo a cleanup id belongs to (`None` without `[repos]`, or for a cleanup of the project root).
fn cleanup_target(
    env: &CommandEnv<'_>,
    workspace: Option<&Workspace>,
    id: &str,
) -> Option<(Workspace, Target)> {
    let ws = workspace?;
    let id = id.to_string();
    let repo = env
        .deps
        .kernel
        .run(move |core| core.cleanup_repo(&id))
        .ok()?;
    (!repo.is_empty()).then(|| (ws.clone(), target_of(env, ws, &repo)))
}

fn cleanup_command(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let Some(requested_by) = env.worker_manager(call.identity) else {
        return Some(fail(
            ErrorCode::Forbidden,
            "only the PM or the operator may clean up branches",
        ));
    };
    let args = call.args;
    let usage = |text: &str| Some(fail(ErrorCode::InvalidRequest, text));
    let include_unlanded = env.config().is_some_and(|c| c.cleanup.include_unlanded);
    let workspace = workspace_of(env);
    let ran = match args.first().map(String::as_str) {
        None if workspace.is_some() => {
            let ws = workspace.as_ref().expect("checked");
            return Some(workspace_offer(env, ws, requested_by, include_unlanded));
        }
        Some("--dry-run") if args.len() == 1 && workspace.is_some() => {
            let ws = workspace.as_ref().expect("checked");
            return Some(workspace_dry_run(env, ws));
        }
        None => on_kernel(env, move |deps| {
            let offer = cleanup::offer(deps, "manual", None, &requested_by, include_unlanded)?;
            Ok(json!({
                "cleanupId": offer.cleanup_id,
                "offered": offer.entries.len(),
                "entries": offer.entries.iter().map(entry_value).collect::<Vec<_>>(),
                "protected": offer.protected.iter().map(|p| json!({"branch": p.branch, "sha": p.sha, "reason": p.reason})).collect::<Vec<_>>(),
            }))
        }),
        Some("--dry-run") if args.len() == 1 => on_kernel(env, |deps| {
            let c = cleanup::classify(deps)?;
            Ok(json!({
                "dryRun": true,
                "landed": c.landed.iter().map(entry_value).collect::<Vec<_>>(),
                "unlanded": c.unlanded.iter().map(entry_value).collect::<Vec<_>>(),
                "protected": c.protected.iter().map(|p| json!({
                    "branch": p.branch, "sha": p.sha, "worktree": Value::Null,
                    "reason": protected_reason(p.reason), "code": p.reason,
                })).collect::<Vec<_>>(),
            }))
        }),
        Some("apply") if args.len() >= 2 => {
            let id = args[1].clone();
            let mut branches: Vec<String> = Vec::new();
            let mut rest = args[2..].iter();
            while let Some(arg) = rest.next() {
                if arg == "--branch" {
                    match rest.next() {
                        Some(name) => branches.push(name.clone()),
                        None => return usage("--branch needs a branch name"),
                    }
                } else {
                    branches.push(arg.clone());
                }
            }
            let selection = (!branches.is_empty()).then_some(branches);
            match cleanup_target(env, workspace.as_ref(), &id) {
                Some((ws, target)) => {
                    let root = target.root.clone();
                    let worktree_dir = ws.worktree_dir;
                    on_kernel_at(env, root, move |deps| {
                        let scope = target.scope(&worktree_dir);
                        let applied = cleanup::apply_in(deps, &id, selection, Some(&scope))?;
                        let mut value = applied_value(&applied);
                        value["cleanupId"] = json!(id);
                        value["state"] = json!("applied");
                        value["repo"] = json!(target.repo);
                        if target.gone {
                            value["note"] = json!(removed_note(&target.repo));
                        }
                        Ok(value)
                    })
                }
                None => on_kernel(env, move |deps| {
                    let applied = cleanup::apply(deps, &id, selection)?;
                    let mut value = applied_value(&applied);
                    value["cleanupId"] = json!(id);
                    value["state"] = json!("applied");
                    Ok(value)
                }),
            }
        }
        Some("decline") if args.len() == 2 => {
            let id = args[1].clone();
            let repo = cleanup_target(env, workspace.as_ref(), &id).map(|(_, t)| t.repo);
            on_kernel(env, move |deps| {
                cleanup::decline(deps, &id)?;
                let mut value = json!({"cleanupId": id, "state": "declined"});
                if let Some(repo) = repo {
                    value["repo"] = json!(repo);
                }
                Ok(value)
            })
        }
        _ => {
            return usage("cleanup takes --dry-run, apply <id> [<branch>...] or decline <id>");
        }
    };
    let Some(ran) = ran else {
        return Some(map_kernel_error(&KernelError::controller(
            "controller is closed",
        )));
    };
    for (event, details) in ran.logs {
        env.log(&event, details);
    }
    Some(match ran.result {
        Ok(value) => ok(value),
        Err(error) => refusal(&error),
    })
}

/// The cleanup that follows a confirmed integration, by the project's `[cleanup]` settings. Returns the `cleanup` member
/// of the confirm answer (`None` when nothing was offered or done). A failure is logged and reported as `{ error }`; it
/// never fails the confirm.
pub fn after_confirm(
    env: &CommandEnv<'_>,
    integration_id: &str,
    requested_by: &str,
) -> Option<Value> {
    let settings = settings_of(env.config().map(|c| c.cleanup).unwrap_or_default());
    if settings.branches == CleanupBranches::Off {
        return None;
    }
    let id = integration_id.to_string();
    let by = requested_by.to_string();
    // A member of a set is cleaned in its own repo.
    let root = {
        let member = id.clone();
        let repo = env
            .deps
            .kernel
            .run(move |core| core.repo_of_integration(&member))
            .ok()
            .flatten();
        match repo {
            Some(repo) => env.deps.repo_root(&repo),
            None => env.deps.options.workspace_root.clone(),
        }
    };
    let ran = on_kernel_at(env, root, move |deps| {
        if FAIL_AFTER_CONFIRM.load(Ordering::SeqCst) {
            return Err(CleanupError::Kernel(KernelError::controller(
                "injected cleanup failure",
            )));
        }
        let mut deleted = 0;
        let mut cleanup_id = None;
        if settings.branches == CleanupBranches::Auto {
            let applied = cleanup::apply_auto(deps, &id)?;
            deleted = applied.deleted.len();
            cleanup_id = applied.cleanup_id;
        }
        let mut offered = 0;
        if settings.branches == CleanupBranches::Ask || settings.include_unlanded {
            let offer = cleanup::offer(deps, "confirm", Some(&id), &by, settings.include_unlanded)?;
            if offer.cleanup_id.is_some() {
                offered = offer.entries.len();
                cleanup_id = offer.cleanup_id;
            }
        }
        Ok(cleanup_id.map(
            |cleanup_id| json!({"cleanupId": cleanup_id, "offered": offered, "deleted": deleted}),
        ))
    });
    let failure = |message: String| {
        env.log(
            "cleanup_after_confirm_failed",
            json!({"integrationId": integration_id, "error": message}),
        );
        Some(json!({"error": message}))
    };
    match ran {
        None => failure("controller is closed".to_string()),
        Some(ran) => {
            for (event, details) in ran.logs {
                env.log(&event, details);
            }
            match ran.result {
                Ok(member) => member,
                Err(error) => failure(error.message()),
            }
        }
    }
}
