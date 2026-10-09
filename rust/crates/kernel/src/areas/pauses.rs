//! The run and agent pauses (src/controller/pauses.ts).
//!
//! Every public function returns `Err(KernelError::Unported("pauses.<method>"))` until the package that owns this file fills
//! its body. The signatures and `dispatch` are fixed by the kernel foundation: fill the bodies, do not change them.

#![allow(unused_imports)]

use crate::dispatch::{arg, call, Args, ErrorHandler, RestartFilter};
use crate::errors::{KernelError, KernelResult};
use crate::kernel::Kernel;
use crate::types::MutationContext;
use serde_json::Value;

/// `transitionRun`.
#[allow(unused_variables)]
pub fn transition_run(
    kernel: &Kernel,
    context: &MutationContext,
    to_state: &Value,
    reason: Option<&str>,
) -> KernelResult<Value> {
    Err(KernelError::Unported("pauses.transitionRun".into()))
}

/// `pauseState`.
#[allow(unused_variables)]
pub fn pause_state(kernel: &Kernel) -> KernelResult<Value> {
    Err(KernelError::Unported("pauses.pauseState".into()))
}

/// `isDeliveryPaused`.
#[allow(unused_variables)]
pub fn is_delivery_paused(kernel: &Kernel, agent_id: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("pauses.isDeliveryPaused".into()))
}

/// `assertRunNotPaused`.
#[allow(unused_variables)]
pub fn assert_run_not_paused(kernel: &Kernel, action: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("pauses.assertRunNotPaused".into()))
}

/// `pauseAgent`.
#[allow(unused_variables)]
pub fn pause_agent(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported("pauses.pauseAgent".into()))
}

/// `resumeAgent`.
#[allow(unused_variables)]
pub fn resume_agent(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported("pauses.resumeAgent".into()))
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "transitionRun" => {
            let context = arg!(a.ctx(0));
            let to_state = a.value(1);
            let reason = arg!(a.opt_str(2, "reason"));
            Some(call(transition_run(kernel, &context, to_state, reason)))
        }
        "pauseState" => Some(call(pause_state(kernel))),
        "isDeliveryPaused" => {
            let agent_id = arg!(a.str(0, "agentId"));
            Some(call(is_delivery_paused(kernel, agent_id)))
        }
        "assertRunNotPaused" => {
            let action = arg!(a.str(0, "action"));
            Some(call(assert_run_not_paused(kernel, action)))
        }
        "pauseAgent" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(pause_agent(kernel, &context, input)))
        }
        "resumeAgent" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(resume_agent(kernel, &context, input)))
        }
        _ => None,
    }
}
