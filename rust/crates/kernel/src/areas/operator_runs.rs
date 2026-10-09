//! The operator runs (src/controller/operator-runs.ts).
//!
//! Every public function returns `Err(KernelError::Unported("operatorRuns.<method>"))` until the package that owns this file fills
//! its body. The signatures and `dispatch` are fixed by the kernel foundation: fill the bodies, do not change them.

#![allow(unused_imports)]

use crate::dispatch::{arg, call, Args, ErrorHandler, RestartFilter};
use crate::errors::{KernelError, KernelResult};
use crate::kernel::Kernel;
use crate::types::MutationContext;
use serde_json::Value;

/// `unclearedOperatorOrphans`.
#[allow(unused_variables)]
pub fn uncleared_operator_orphans(kernel: &Kernel) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "operatorRuns.unclearedOperatorOrphans".into(),
    ))
}

/// `claimOperatorRun`.
#[allow(unused_variables)]
pub fn claim_operator_run(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "operatorRuns.claimOperatorRun".into(),
    ))
}

/// `recordOperatorRunProcess`.
#[allow(unused_variables)]
pub fn record_operator_run_process(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "operatorRuns.recordOperatorRunProcess".into(),
    ))
}

/// `finishOperatorRun`.
#[allow(unused_variables)]
pub fn finish_operator_run(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "operatorRuns.finishOperatorRun".into(),
    ))
}

/// `abandonRunningOperatorRuns`.
#[allow(unused_variables)]
pub fn abandon_running_operator_runs(
    kernel: &Kernel,
    context: &MutationContext,
    skip_restarts_with_plan: Option<RestartFilter<'_>>,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "operatorRuns.abandonRunningOperatorRuns".into(),
    ))
}

/// `clearOperatorOrphan`.
#[allow(unused_variables)]
pub fn clear_operator_orphan(
    kernel: &Kernel,
    context: &MutationContext,
    proposal_id: &str,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "operatorRuns.clearOperatorOrphan".into(),
    ))
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "unclearedOperatorOrphans" => Some(call(uncleared_operator_orphans(kernel))),
        "claimOperatorRun" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(claim_operator_run(kernel, &context, input)))
        }
        "recordOperatorRunProcess" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(record_operator_run_process(kernel, &context, input)))
        }
        "finishOperatorRun" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(finish_operator_run(kernel, &context, input)))
        }
        "abandonRunningOperatorRuns" => {
            let context = arg!(a.ctx(0));
            Some(call(abandon_running_operator_runs(kernel, &context, None)))
        }
        "clearOperatorOrphan" => {
            let context = arg!(a.ctx(0));
            let proposal_id = arg!(a.str(1, "proposalId"));
            Some(call(clear_operator_orphan(kernel, &context, proposal_id)))
        }
        _ => None,
    }
}
