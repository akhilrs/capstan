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
