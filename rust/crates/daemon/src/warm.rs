//! Warm reviewers in the daemon: the plan's reviewer that stays alive between review requests. The ledger records who is
//! warm (`warm_reviewers`); this module decides when the pane is kept, evicted for room, or released, and does the
//! launcher side of each release.

use crate::deps::Deps;
use crate::reviews::REVIEWER_RELEASE_DELAY_MS;
use capstan_kernel::areas::messaging::parse_iso_ms;
use capstan_launcher::api::{
    LauncherError, LauncherResult, LauncherService, SpawnOptions, SpawnResult,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// When this process last kept a reviewer after a verdict, by agent: a retarget waits until the reviewer's own
/// `cstan review` call has finished printing.
fn kept_at() -> &'static Mutex<HashMap<String, Instant>> {
    static KEPT: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    KEPT.get_or_init(Mutex::default)
}

/// Notes that the reviewer just gave a verdict and stays warm.
pub fn note_verdict(agent_id: &str) {
    kept_at()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(agent_id.to_string(), Instant::now());
}

/// Waits out what is left of the delay since the reviewer's last verdict.
pub fn wait_for_verdict_print(agent_id: &str) {
    let since = kept_at()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(agent_id)
        .copied();
    if let Some(since) = since {
        let delay = Duration::from_millis(REVIEWER_RELEASE_DELAY_MS);
        if let Some(left) = delay.checked_sub(since.elapsed()) {
            std::thread::sleep(left);
        }
    }
}

/// Releases the warm row of `agent_id` in the ledger. True when a row was released (with `only_if_idle`, never while the
/// reviewer has a started review).
pub fn release_row(deps: &Deps, agent_id: &str, reason: &str, only_if_idle: bool) -> bool {
    let input = json!({"agentId": agent_id, "reason": reason, "onlyIfIdle": only_if_idle});
    let credential = deps.credential().to_string();
    let released = deps.kernel.run(move |core| {
        let context = crate::deps::new_context(core, &credential)?;
        core.release_warm_reviewer(&context, &input)
    });
    match released {
        Ok(answer) => answer["released"].as_bool() == Some(true),
        Err(error) => {
            deps.detail_log(
                "warm_reviewer_release_failed",
                json!({"agentId": agent_id, "reason": reason, "error": error.message()}),
            );
            false
        }
    }
}

/// Releases the row and ends the reviewer now (it is idle, so nothing is printing).
pub fn release_now(
    deps: &Deps,
    launcher: &Arc<dyn LauncherService>,
    agent_id: &str,
    reason: &str,
) -> bool {
    let released = release_row(deps, agent_id, reason, true);
    if released {
        if let Err(error) = launcher.release(agent_id) {
            deps.detail_log(
                "review_reviewer_not_released",
                json!({"agentId": agent_id, "error": format!("LauncherError: {}", error.message)}),
            );
        }
    }
    released
}

/// The unreleased warm rows, least recently used first.
pub fn rows(deps: &Deps) -> Vec<Value> {
    let credential = deps.credential().to_string();
    match deps
        .kernel
        .run(move |core| core.warm_reviewers(&credential))
    {
        Ok(Value::Array(rows)) => rows,
        _ => Vec::new(),
    }
}

/// Ends the least recently used idle warm reviewer to make room (`role` limits it to one of that role). False when there
/// is none to end.
fn evict_one(deps: &Deps, launcher: &Arc<dyn LauncherService>, role: Option<&str>) -> bool {
    for row in rows(deps) {
        if row["busy"].as_bool() != Some(false) {
            continue;
        }
        if role.is_some_and(|role| row["reviewerRole"].as_str() != Some(role)) {
            continue;
        }
        let agent_id = row["agentId"].as_str().unwrap_or("");
        if release_now(deps, launcher, agent_id, "evicted") {
            deps.detail_log(
                "warm_reviewer_evicted",
                json!({"agentId": agent_id, "planId": row["planId"]}),
            );
            return true;
        }
    }
    false
}

/// `launcher.spawn`, and on a role or worker limit one retry after an idle warm reviewer made room.
pub fn spawn_with_eviction(
    deps: &Deps,
    launcher: &Arc<dyn LauncherService>,
    role: &str,
    options: &SpawnOptions,
) -> LauncherResult<SpawnResult> {
    match launcher.spawn(role, options) {
        Err(error) if error.code == "role_limit" || error.code == "worker_limit" => {
            let only_role = (error.code == "role_limit").then_some(role);
            if evict_one(deps, launcher, only_role) {
                launcher.spawn(role, options)
            } else {
                Err(error)
            }
        }
        other => other,
    }
}

/// Releases the warm reviewers that sat idle for `idle_minutes` or more at `now_ms`.
pub fn release_idle(deps: &Deps, now_ms: i64, idle_minutes: i64) {
    let Some(launcher) = deps.launcher.as_ref() else {
        return;
    };
    for row in rows(deps) {
        if row["busy"].as_bool() != Some(false) {
            continue;
        }
        let used = parse_iso_ms(row["lastUsedAt"].as_str().unwrap_or(""));
        if now_ms - used < idle_minutes * 60_000 {
            continue;
        }
        let agent_id = row["agentId"].as_str().unwrap_or("");
        if release_now(deps, launcher, agent_id, "idle") {
            deps.detail_log(
                "warm_reviewer_idle_released",
                json!({"agentId": agent_id, "planId": row["planId"]}),
            );
        }
    }
}

/// At daemon start: a held warm reviewer whose agent is no longer active is marked released.
pub fn recover_lost(deps: &Deps) {
    for row in rows(deps) {
        let agent_id = row["agentId"].as_str().unwrap_or("").to_string();
        let probe = agent_id.clone();
        let active = deps.kernel.run(move |core| {
            Ok(core
                .kernel()
                .agent_row(&probe)?
                .is_some_and(|agent| agent.state == "active"))
        });
        if matches!(active, Ok(false)) && release_row(deps, &agent_id, "lost", false) {
            deps.detail_log(
                "warm_reviewer_lost",
                json!({"agentId": agent_id, "planId": row["planId"]}),
            );
        }
    }
}

/// A retarget refusal, for the log.
pub fn retarget_failed(deps: &Deps, error: &LauncherError, agent_id: &str) {
    deps.detail_log(
        "review_retarget_failed",
        json!({"agentId": agent_id, "code": error.code}),
    );
}
