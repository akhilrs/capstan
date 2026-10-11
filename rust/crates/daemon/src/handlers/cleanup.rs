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
    Cleanup, CleanupBranches as ConfigBranches, CleanupWorktrees as ConfigWorktrees,
};
use capstan_kernel::cleanup::{
    self, Applied, Candidate, CleanupBranches, CleanupDeps, CleanupError, CleanupResult,
    CleanupSettings, CleanupWorktrees,
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
    let ran = match args.first().map(String::as_str) {
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
            on_kernel(env, move |deps| {
                let applied = cleanup::apply(deps, &id, selection)?;
                let mut value = applied_value(&applied);
                value["cleanupId"] = json!(id);
                value["state"] = json!("applied");
                Ok(value)
            })
        }
        Some("decline") if args.len() == 2 => {
            let id = args[1].clone();
            on_kernel(env, move |deps| {
                cleanup::decline(deps, &id)?;
                Ok(json!({"cleanupId": id, "state": "declined"}))
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
    let ran = on_kernel(env, move |deps| {
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
