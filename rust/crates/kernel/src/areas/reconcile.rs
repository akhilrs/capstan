//! Reconciliation of assignments a restart left uncertain (src/controller/reconcile.ts).
//!
//! The foundation ships the case `ControllerCore.open` needs on a ledger with nothing to reconcile; a ledger that has
//! uncertain assignments returns `Unported` until the reconcile package fills `reconcile_uncertain_assignments`.

use crate::errors::{KernelError, KernelResult};
use crate::helpers::query_all;
use crate::kernel::Kernel;

/// `reconcileUncertainAssignments`: returns how many assignments it reconciled.
pub fn reconcile_uncertain_assignments(kernel: &Kernel) -> KernelResult<usize> {
    kernel.database.exec("BEGIN IMMEDIATE")?;
    let rows = query_all(
        &kernel.database,
        "SELECT c.command_id
         FROM commands c
         JOIN assignments a ON a.project_id = c.project_id AND a.assignment_id = c.assignment_id
         JOIN assignment_attempts at ON at.project_id = c.project_id
           AND at.assignment_id = c.assignment_id AND at.attempt = c.attempt
         JOIN work_items w ON w.project_id = a.project_id AND w.work_item_id = a.work_item_id
         WHERE c.project_id = ? AND c.state IN ('queued', 'attempting', 'acknowledged', 'started', 'completed')
           AND a.authority_state = 'active'
           AND at.state IN ('created', 'dispatched', 'acknowledged', 'running', 'reported')
         ORDER BY c.command_id",
        [&kernel.project_id],
        |row| row.get::<_, String>(0),
    );
    match rows {
        Ok(rows) if rows.is_empty() => {
            kernel.database.exec("COMMIT")?;
            Ok(0)
        }
        Ok(_) => {
            let _ = kernel.database.exec("ROLLBACK");
            Err(KernelError::Unported(
                "reconcile.reconcileUncertainAssignments".into(),
            ))
        }
        Err(error) => {
            let _ = kernel.database.exec("ROLLBACK");
            Err(error)
        }
    }
}

/// Reconciliation has no `ControllerCore` operation of its own, so nothing is dispatched by name.
pub(crate) fn dispatch(
    _kernel: &Kernel,
    _op: &str,
    _args: &[serde_json::Value],
) -> Option<KernelResult<serde_json::Value>> {
    None
}
