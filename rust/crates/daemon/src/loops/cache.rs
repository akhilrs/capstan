//! The cache loop: recovers what a killed prune left behind, then measures and prunes the tool caches every six hours
//! (sooner when the last measurement was over the cap). It has a thread of its own, so nothing here runs on the kernel
//! thread; prune commands go through the launcher's sanitising command runner with a timeout.

use super::driver::DriverLog;
use super::{guarded, spawn, StopFlag};
use crate::deps::Deps;
use crate::handlers::cache::{process_env, retrying, run_command, runtime_of_deps, CacheFailure};
use capstan_cache::{
    is_idle, prune_with_occupants, read_manifest, recover, Occupant, Policy, PruneHooks, Runtime,
};
use capstan_ledger::now_millis;
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How often the caches are measured and pruned.
pub const INTERVAL: Duration = Duration::from_secs(6 * 3600);
/// The earliest the first measurement runs after the daemon starts, so a start is not slowed by a disk walk.
pub const MIN_FIRST_DELAY: Duration = Duration::from_secs(30);

/// The least time between two cycles a verify trigger causes.
pub const TRIGGER_DEBOUNCE_MS: u64 = 5 * 60 * 1000;
/// How often the loop looks at its stop flag and at a pending trigger while it waits.
const SLICE: Duration = Duration::from_secs(1);

/// Where the loop reads the live occupants of the cache from (the verify service's stored list).
pub type OccupantSource = Arc<Mutex<Vec<Occupant>>>;

/// A request for a cycle outside the periodic ones: a verify run ended, or its scratch grew over the cap. Cycles it
/// causes are at least `TRIGGER_DEBOUNCE_MS` apart by its clock.
pub struct CacheTrigger {
    pending: AtomicBool,
    last_cycle_ms: AtomicU64,
    /// Bytes of the store (without occupants) the last cycle ended with; `u64::MAX` until a cycle ran.
    pub last_store_total: AtomicU64,
    cycles: AtomicU64,
    now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
    occupants: Mutex<Option<OccupantSource>>,
}

impl CacheTrigger {
    /// A trigger on the system clock.
    pub fn new() -> Arc<Self> {
        Self::with_clock(Arc::new(|| now_millis().max(0) as u64))
    }

    /// A trigger on `now_ms` (a test clock).
    pub fn with_clock(now_ms: Arc<dyn Fn() -> u64 + Send + Sync>) -> Arc<Self> {
        Arc::new(Self {
            pending: AtomicBool::new(false),
            last_cycle_ms: AtomicU64::new(0),
            last_store_total: AtomicU64::new(u64::MAX),
            cycles: AtomicU64::new(0),
            now_ms,
            occupants: Mutex::new(None),
        })
    }

    /// Asks for a cycle; the loop runs it when the last one ended at least five minutes ago.
    pub fn fire(&self) {
        self.pending.store(true, Ordering::SeqCst);
    }

    pub fn now_ms(&self) -> u64 {
        (self.now_ms)()
    }

    /// How many cycles `run_with_trigger` finished.
    pub fn cycles(&self) -> u64 {
        self.cycles.load(Ordering::SeqCst)
    }

    /// Links the live occupants the loop's cycles count against the cap.
    pub fn set_occupant_source(&self, source: OccupantSource) {
        *self.occupants.lock().unwrap_or_else(|p| p.into_inner()) = Some(source);
    }

    fn occupants(&self) -> Option<Vec<Occupant>> {
        let source = self
            .occupants
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()?;
        let list = source.lock().unwrap_or_else(|p| p.into_inner()).clone();
        Some(list)
    }

    fn due(&self) -> bool {
        self.pending.load(Ordering::SeqCst)
            && self
                .now_ms()
                .saturating_sub(self.last_cycle_ms.load(Ordering::SeqCst))
                >= TRIGGER_DEBOUNCE_MS
    }
}

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
    let trigger = CacheTrigger::new();
    if let Some(verify) = &deps.verify {
        verify.set_cache_trigger(trigger.clone());
    }
    let (stop, log) = (stop.clone(), log.clone());
    spawn("cache", move || {
        run_with_trigger(&runtime, &stop, timing, &log, &trigger)
    })
}

/// The body of the thread.
pub fn run(runtime: &Runtime, stop: &StopFlag, timing: Timing, log: &DriverLog) {
    run_with_trigger(runtime, stop, timing, log, &CacheTrigger::new());
}

/// What ended a wait.
enum Woke {
    Stopped,
    Elapsed,
    Triggered,
}

fn wait(stop: &StopFlag, trigger: &CacheTrigger, duration: Duration) -> Woke {
    let end = Instant::now() + duration;
    loop {
        if trigger.due() {
            return Woke::Triggered;
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Woke::Elapsed;
        }
        if stop.sleep(left.min(SLICE)) {
            return Woke::Stopped;
        }
    }
}

/// `run` that also runs a cycle when `trigger` asks for one.
pub fn run_with_trigger(
    runtime: &Runtime,
    stop: &StopFlag,
    timing: Timing,
    log: &DriverLog,
    trigger: &CacheTrigger,
) {
    guarded(log, "cache_recover_failed", || {
        match retrying(|| recover(&runtime.root)) {
            Ok(removed) if removed.is_empty() => {}
            Ok(removed) => log("cache_recovered", json!({"removed": removed})),
            Err(error) => log("cache_recover_failed", json!({"error": error.to_string()})),
        }
    });
    let mut delay = timing.first_delay;
    loop {
        if matches!(wait(stop, trigger, delay), Woke::Stopped) {
            return;
        }
        // Cleared before the occupants are read: a fire during the cycle stays pending and runs after the debounce.
        trigger.pending.store(false, Ordering::SeqCst);
        guarded(log, "cache_prune_failed", || {
            let occupants = trigger.occupants();
            match cycle_inner(runtime, log, occupants.as_deref()) {
                Ok(total) => trigger.last_store_total.store(total, Ordering::SeqCst),
                Err(failure) => log(
                    "cache_prune_failed",
                    json!({"error": failure.message().to_string()}),
                ),
            }
        });
        trigger
            .last_cycle_ms
            .store(trigger.now_ms(), Ordering::SeqCst);
        trigger.cycles.fetch_add(1, Ordering::SeqCst);
        delay = timing.interval;
    }
}

/// One measurement and prune: the tool prune commands, then the age and size policy (entries are removed only when no
/// linked worktree exists). The manifest is written. The occupants (live verify scratch) are not known here.
pub fn cycle(runtime: &Runtime, log: &DriverLog) -> Result<(), CacheFailure> {
    cycle_inner(runtime, log, None).map(|_| ())
}

/// `cycle` that counts `occupants` against the cap and records them in the manifest.
pub fn cycle_with(
    runtime: &Runtime,
    log: &DriverLog,
    occupants: &[Occupant],
) -> Result<(), CacheFailure> {
    cycle_inner(runtime, log, Some(occupants)).map(|_| ())
}

/// The cycle; the bytes of the store (without occupants) it ended with.
fn cycle_inner(
    runtime: &Runtime,
    log: &DriverLog,
    occupants: Option<&[Occupant]>,
) -> Result<u64, CacheFailure> {
    let idle = is_idle(&runtime.project_root)?;
    let policy = Policy::for_runtime(runtime, idle);
    let env = process_env();
    let report = retrying(|| {
        prune_with_occupants(
            runtime,
            policy,
            &env,
            &run_command,
            &PruneHooks::default(),
            occupants,
        )
    })?;
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
            "skipped": crate::handlers::cache::skipped_json(&report.skipped),
            "totalBefore": report.total_before,
            "totalAfter": report.total_after,
            "failedCommands": failed,
            "errors": report.errors,
        }),
    );
    Ok(report.total_after)
}
