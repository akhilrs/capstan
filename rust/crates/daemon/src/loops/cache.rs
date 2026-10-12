//! The cache loop: recovers what a killed prune left behind, then measures and prunes the tool caches every six hours
//! (sooner when the last measurement was over the cap). It has a thread of its own, so nothing here runs on the kernel
//! thread; prune commands go through the launcher's sanitising command runner with a timeout.

use super::driver::DriverLog;
use super::{guarded, spawn, StopFlag};
use crate::deps::Deps;
use crate::handlers::cache::{process_env, retrying, run_command, runtime_of_deps, CacheFailure};
use capstan_cache::{is_idle, prune, read_manifest, recover, Policy, Runtime};
use capstan_ledger::now_millis;
use serde_json::json;
use std::thread::JoinHandle;
use std::time::Duration;

/// How often the caches are measured and pruned.
pub const INTERVAL: Duration = Duration::from_secs(6 * 3600);
/// The earliest the first measurement runs after the daemon starts, so a start is not slowed by a disk walk.
pub const MIN_FIRST_DELAY: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug)]
pub struct Timing {
    pub first_delay: Duration,
    pub interval: Duration,
}

/// When the first cycle runs: an interval after the last measurement in the manifest (at least `MIN_FIRST_DELAY`), at
/// once when that measurement was over the cap, and after `MIN_FIRST_DELAY` when there is none.
pub fn first_delay(runtime: &Runtime, now_ms: u64, interval: Duration) -> Duration {
    let Some(manifest) = read_manifest(&runtime.root).ok().flatten() else {
        return MIN_FIRST_DELAY;
    };
    let total: u64 = manifest.entries.iter().map(|e| e.bytes).sum();
    if total > runtime.max_bytes {
        return MIN_FIRST_DELAY;
    }
    let due = manifest
        .measured_at_ms
        .saturating_add(interval.as_millis() as u64);
    Duration::from_millis(due.saturating_sub(now_ms)).max(MIN_FIRST_DELAY)
}

/// Starts the loop; `None` when capstan.toml has no `[cache]` section.
pub fn start(deps: &Deps, stop: &StopFlag, log: &DriverLog) -> Option<JoinHandle<()>> {
    let runtime = runtime_of_deps(deps)?;
    let timing = Timing {
        first_delay: first_delay(&runtime, now_millis().max(0) as u64, INTERVAL),
        interval: INTERVAL,
    };
    let (stop, log) = (stop.clone(), log.clone());
    spawn("cache", move || run(&runtime, &stop, timing, &log))
}

/// The body of the thread.
pub fn run(runtime: &Runtime, stop: &StopFlag, timing: Timing, log: &DriverLog) {
    guarded(log, "cache_recover_failed", || {
        match retrying(|| recover(&runtime.root)) {
            Ok(removed) if removed.is_empty() => {}
            Ok(removed) => log("cache_recovered", json!({"removed": removed})),
            Err(error) => log("cache_recover_failed", json!({"error": error.to_string()})),
        }
    });
    if stop.sleep(timing.first_delay) {
        return;
    }
    loop {
        guarded(log, "cache_prune_failed", || {
            if let Err(failure) = cycle(runtime, log) {
                log(
                    "cache_prune_failed",
                    json!({"error": failure.message().to_string()}),
                );
            }
        });
        if stop.sleep(timing.interval) {
            return;
        }
    }
}

/// One measurement and prune: the tool prune commands, then the age and size policy (entries are removed only when no
/// linked worktree exists). The manifest is written.
pub fn cycle(runtime: &Runtime, log: &DriverLog) -> Result<(), CacheFailure> {
    let idle = is_idle(&runtime.project_root)?;
    let policy = Policy::for_runtime(runtime, idle);
    let env = process_env();
    let report = retrying(|| prune(runtime, policy, &env, &run_command))?;
    let failed: Vec<String> = report
        .commands
        .iter()
        .filter(|c| !c.ok)
        .map(|c| format!("{}: {}: {}", c.entry, c.command, c.message))
        .collect();
    log(
        "cache_pruned",
        json!({
            "idle": idle,
            "cleared": report.cleared,
            "wouldClear": report.would_clear,
            "totalBefore": report.total_before,
            "totalAfter": report.total_after,
            "failedCommands": failed,
            "errors": report.errors,
        }),
    );
    Ok(())
}
