//! The plan packages (src/controller/plan-packages.ts).
//!
//! Every public function returns `Err(KernelError::Unported("planPackages.<method>"))` until the package that owns this file fills
//! its body. The signatures and `dispatch` are fixed by the kernel foundation: fill the bodies, do not change them.

#![allow(unused_imports)]

use crate::dispatch::{arg, call, Args, ErrorHandler, RestartFilter};
use crate::errors::{KernelError, KernelResult};
use crate::kernel::Kernel;
use crate::types::MutationContext;
use serde_json::Value;

/// `assignPackage`.
#[allow(unused_variables)]
pub fn assign_package(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported("planPackages.assignPackage".into()))
}

/// `recordSignoff`.
#[allow(unused_variables)]
pub fn record_signoff(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported("planPackages.recordSignoff".into()))
}

/// `cancelPlan`.
#[allow(unused_variables)]
pub fn cancel_plan(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported("planPackages.cancelPlan".into()))
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "assignPackage" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(assign_package(kernel, &context, input)))
        }
        "recordSignoff" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(record_signoff(kernel, &context, input)))
        }
        "cancelPlan" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(cancel_plan(kernel, &context, input)))
        }
        _ => None,
    }
}
