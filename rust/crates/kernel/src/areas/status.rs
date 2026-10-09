//! The status (src/controller/status.ts).
//!
//! Every public function returns `Err(KernelError::Unported("status.<method>"))` until the package that owns this file fills
//! its body. The signatures and `dispatch` are fixed by the kernel foundation: fill the bodies, do not change them.

#![allow(unused_imports)]

use crate::dispatch::{arg, call, Args, ErrorHandler, RestartFilter};
use crate::errors::{KernelError, KernelResult};
use crate::kernel::Kernel;
use crate::types::MutationContext;
use serde_json::Value;

/// `pipelineCounts`.
#[allow(unused_variables)]
pub fn pipeline_counts(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("status.pipelineCounts".into()))
}

/// `awaitingConfirm`.
#[allow(unused_variables)]
pub fn awaiting_confirm(
    kernel: &Kernel,
    credential: &str,
    limit: Option<i64>,
) -> KernelResult<Value> {
    Err(KernelError::Unported("status.awaitingConfirm".into()))
}

/// `activeTasks`.
#[allow(unused_variables)]
pub fn active_tasks(kernel: &Kernel, credential: &str, caps: &Value) -> KernelResult<Value> {
    Err(KernelError::Unported("status.activeTasks".into()))
}

/// `readiness`.
#[allow(unused_variables)]
pub fn readiness(kernel: &Kernel, work_item_id: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("status.readiness".into()))
}

/// `statusSnapshot`.
#[allow(unused_variables)]
pub fn status_snapshot(kernel: &Kernel) -> KernelResult<Value> {
    Err(KernelError::Unported("status.statusSnapshot".into()))
}

/// `inspect`.
#[allow(unused_variables)]
pub fn inspect(kernel: &Kernel, id: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("status.inspect".into()))
}

/// `busyIndicators`.
#[allow(unused_variables)]
pub fn busy_indicators(kernel: &Kernel) -> KernelResult<Value> {
    Err(KernelError::Unported("status.busyIndicators".into()))
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "pipelineCounts" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(pipeline_counts(kernel, credential)))
        }
        "awaitingConfirm" => {
            let credential = arg!(a.str(0, "credential"));
            let limit = arg!(a.opt_i64(1, "limit"));
            Some(call(awaiting_confirm(kernel, credential, limit)))
        }
        "activeTasks" => {
            let credential = arg!(a.str(0, "credential"));
            let caps = a.value(1);
            Some(call(active_tasks(kernel, credential, caps)))
        }
        "readiness" => {
            let work_item_id = arg!(a.str(0, "workItemId"));
            Some(call(readiness(kernel, work_item_id)))
        }
        "statusSnapshot" => Some(call(status_snapshot(kernel))),
        "inspect" => {
            let id = arg!(a.str(0, "id"));
            Some(call(inspect(kernel, id)))
        }
        "busyIndicators" => Some(call(busy_indicators(kernel))),
        _ => None,
    }
}
