//! Retention of the mutation request ledger: the idempotency rows older than the retention window are deleted in short
//! batches. A row is only the stored result of one request, so deleting it ends the replay of that request and nothing
//! else; `controller_events` and every other table stay as they are. The replay contract is in docs/daemon.md.

use crate::errors::KernelResult;
use crate::helpers::execute;
use crate::kernel::Kernel;

const CAPABILITY: &str = "controller:reconcile";

/// Deletes at most `batch_rows` rows of this project's `mutation_requests` whose `created_at` is before `cutoff_iso`
/// (`YYYY-MM-DDTHH:MM:SS.mmmZ`, the form `Kernel::now` writes, so the text comparison orders by time) and returns how many
/// it deleted. It is not a `Kernel::mutate`: it writes no event and no request row, and `state_version` is unchanged.
pub fn prune_mutation_requests(
    kernel: &Kernel,
    credential: &str,
    cutoff_iso: &str,
    batch_rows: i64,
) -> KernelResult<i64> {
    kernel.authorize(credential, CAPABILITY)?;
    kernel.database.exec("BEGIN IMMEDIATE")?;
    let outcome = execute(
        &kernel.database,
        "DELETE FROM mutation_requests WHERE project_id = ?1 AND idempotency_key IN (
           SELECT idempotency_key FROM mutation_requests WHERE project_id = ?1 AND created_at < ?2 LIMIT ?3)",
        rusqlite::params![kernel.project_id, cutoff_iso, batch_rows],
    );
    match outcome {
        Ok(count) => {
            kernel.database.exec("COMMIT")?;
            Ok(count as i64)
        }
        Err(error) => {
            let _ = kernel.database.exec("ROLLBACK");
            Err(error)
        }
    }
}
