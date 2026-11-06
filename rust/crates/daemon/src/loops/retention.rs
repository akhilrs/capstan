//! The ledger retention loop: deletes the `mutation_requests` rows older than `[ledger] retention_days`, in short batches
//! on the kernel thread. The first pass comes a few minutes after the daemon starts (so a start is not slowed), the next
//! ones an hour apart. The replay contract is in docs/daemon.md.

use super::{detail_log, guarded, spawn, StopFlag};
use crate::deps::{Deps, KernelHandle};
use capstan_ledger::iso_from_millis;
use serde_json::json;
use std::thread::JoinHandle;
use std::time::Duration;

/// The longest wait before the first pass after the daemon starts.
pub const FIRST_DELAY: Duration = Duration::from_secs(5 * 60);
/// How often a pass runs.
pub const INTERVAL: Duration = Duration::from_secs(60 * 60);
/// The most rows one kernel job deletes.
pub const BATCH_ROWS: i64 = 1000;
/// The pause after a full batch, so the jobs queued behind it run before the next one.
pub const PAUSE: Duration = Duration::from_millis(50);

const DAY_MS: i64 = 86_400_000;

/// What one pass did: the rows it deleted and, when a batch failed, why the pass ended early.
#[derive(Debug, PartialEq)]
pub struct Pass {
    pub deleted: i64,
    pub error: Option<String>,
}

/// When the ledger-maintenance loop runs: the first pass after `first_delay`, then one every `interval`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaintenanceTiming {
    pub first_delay: Duration,
    pub interval: Duration,
}

impl Default for MaintenanceTiming {
    fn default() -> Self {
        MaintenanceTiming {
            first_delay: Duration::from_secs(10 * 60),
            interval: Duration::from_secs(24 * 60 * 60),
        }
    }
}

/// Starts the ledger-maintenance loop whenever a configuration is present (`retention_days = 0` included); `None`
/// without one. Each pass runs `Kernel::ledger_maintenance` on the kernel thread (planner statistics, `optimize`, the
/// aged pre-migration backups) and logs `daemon:ledger_maintenance` with what it did. It is a thread of its own, so it
/// never waits for the retention loop and the retention loop never waits for it.
pub fn start_maintenance(
    deps: &Deps,
    stop: &StopFlag,
    timing: MaintenanceTiming,
) -> Option<JoinHandle<()>> {
    deps.options.capstan.as_ref()?;
    let (deps, stop) = (deps.clone(), stop.clone());
    spawn("ledger-maintenance", move || {
        let log = detail_log(&deps);
        if stop.sleep(timing.first_delay) {
            return;
        }
        loop {
            guarded(&log, "ledger_maintenance_failed", || {
                let credential = deps.credential().to_string();
                match deps
                    .kernel
                    .run(move |core| core.kernel().ledger_maintenance(&credential))
                {
                    Ok(report) => deps.detail_log("ledger_maintenance", report),
                    Err(error) => deps.detail_log(
                        "ledger_maintenance_failed",
                        json!({"error": error.to_string()}),
                    ),
                }
            });
            if stop.sleep(timing.interval) {
                return;
            }
        }
    })
}

/// Starts the loop; `None` (and nothing logged) without a configuration or with `retention_days = 0`.
pub fn start(deps: &Deps, stop: &StopFlag) -> Option<JoinHandle<()>> {
    let days = deps.options.capstan.as_ref()?.ledger.retention_days;
    if days <= 0 {
        return None;
    }
    deps.detail_log("ledger_retention_start", json!({"retentionDays": days}));
    let (deps, stop) = (deps.clone(), stop.clone());
    spawn("ledger-retention", move || {
        let log = detail_log(&deps);
        if stop.sleep(FIRST_DELAY) {
            return;
        }
        loop {
            guarded(&log, "ledger_retention_failed", || {
                let pass = run_pass(&deps.kernel, deps.credential(), days, &stop);
                if pass.deleted > 0 {
                    deps.detail_log(
                        "ledger_pruned",
                        json!({"deleted": pass.deleted, "retentionDays": days}),
                    );
                }
                if let Some(error) = pass.error {
                    deps.detail_log(
                        "ledger_retention_failed",
                        json!({"error": error, "deleted": pass.deleted}),
                    );
                }
            });
            if stop.sleep(INTERVAL) {
                return;
            }
        }
    })
}

/// One pass: batches of at most [`BATCH_ROWS`] until one deletes fewer, with [`PAUSE`] between them; a stop ends it after
/// the batch in progress. The cutoff is read on the kernel thread from the kernel's own clock, in the form
/// `Kernel::now` stamps rows with. A failed batch ends the pass; its error comes back with the rows deleted before it.
pub fn run_pass(
    kernel: &KernelHandle,
    credential: &str,
    retention_days: i64,
    stop: &StopFlag,
) -> Pass {
    let mut total = 0;
    loop {
        let credential = credential.to_string();
        let batch = kernel.run(move |core| {
            let cutoff = iso_from_millis(core.kernel().env.now() - retention_days * DAY_MS);
            capstan_kernel::areas::retention::prune_mutation_requests(
                core.kernel(),
                &credential,
                &cutoff,
                BATCH_ROWS,
            )
        });
        let deleted = match batch {
            Ok(deleted) => deleted,
            Err(error) => {
                return Pass {
                    deleted: total,
                    error: Some(error.to_string()),
                }
            }
        };
        total += deleted;
        if deleted < BATCH_ROWS || stop.sleep(PAUSE) {
            return Pass {
                deleted: total,
                error: None,
            };
        }
    }
}
