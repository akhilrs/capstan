//! The prompt relay (src/controller/prompt-relay.ts).
//!
//! Every public function returns `Err(KernelError::Unported("promptRelay.<method>"))` until the package that owns this file fills
//! its body. The signatures and `dispatch` are fixed by the kernel foundation: fill the bodies, do not change them.

#![allow(unused_imports)]

use crate::dispatch::{arg, call, Args, ErrorHandler, RestartFilter};
use crate::errors::{KernelError, KernelResult};
use crate::kernel::Kernel;
use crate::types::MutationContext;
use serde_json::Value;

/// `configurePromptRelay`.
#[allow(unused_variables)]
pub fn configure(kernel: &Kernel, config: &Value) -> KernelResult<Value> {
    Err(KernelError::Unported("promptRelay.configure".into()))
}

/// `promptRelayEnabled`.
#[allow(unused_variables)]
pub fn enabled(kernel: &Kernel) -> KernelResult<bool> {
    Err(KernelError::Unported("promptRelay.enabled".into()))
}

/// `checkPromptAnswer`.
#[allow(unused_variables)]
pub fn check_prompt_answer(
    kernel: &Kernel,
    relay_id: &str,
    hash: &Value,
    answer: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "promptRelay.checkPromptAnswer".into(),
    ))
}

/// `promptRelay`.
#[allow(unused_variables)]
pub fn prompt_relay(kernel: &Kernel, relay_id: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("promptRelay.promptRelay".into()))
}

/// `listPromptRelays`.
#[allow(unused_variables)]
pub fn list_prompt_relays(kernel: &Kernel, limit: Option<i64>) -> KernelResult<Value> {
    Err(KernelError::Unported("promptRelay.listPromptRelays".into()))
}

/// `promptRelayStatus`.
#[allow(unused_variables)]
pub fn prompt_relay_status(kernel: &Kernel) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "promptRelay.promptRelayStatus".into(),
    ))
}

/// `recordPromptCapture`.
#[allow(unused_variables)]
pub fn record_prompt_capture(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "promptRelay.recordPromptCapture".into(),
    ))
}

/// `beginPromptAnswer`.
#[allow(unused_variables)]
pub fn begin_prompt_answer(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "promptRelay.beginPromptAnswer".into(),
    ))
}

/// `refusePromptAnswer`.
#[allow(unused_variables)]
pub fn refuse_prompt_answer(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "promptRelay.refusePromptAnswer".into(),
    ))
}

/// `finishPromptAnswer`.
#[allow(unused_variables)]
pub fn finish_prompt_answer(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "promptRelay.finishPromptAnswer".into(),
    ))
}

/// `hasExpiredPromptCaptures`.
#[allow(unused_variables)]
pub fn has_expired_prompt_captures(kernel: &Kernel) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "promptRelay.hasExpiredPromptCaptures".into(),
    ))
}

/// `expirePromptCaptures`.
#[allow(unused_variables)]
pub fn expire_prompt_captures(kernel: &Kernel, context: &MutationContext) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "promptRelay.expirePromptCaptures".into(),
    ))
}

/// `failInterruptedPromptRelays`.
#[allow(unused_variables)]
pub fn fail_interrupted_prompt_relays(
    kernel: &Kernel,
    context: &MutationContext,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "promptRelay.failInterruptedPromptRelays".into(),
    ))
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "configurePromptRelay" => {
            let config = a.value(0);
            Some(call(configure(kernel, config)))
        }
        "promptRelayEnabled" => Some(call(enabled(kernel).map(Value::Bool))),
        "checkPromptAnswer" => {
            let relay_id = arg!(a.str(0, "relayId"));
            let hash = a.value(1);
            let answer = a.value(2);
            Some(call(check_prompt_answer(kernel, relay_id, hash, answer)))
        }
        "promptRelay" => {
            let relay_id = arg!(a.str(0, "relayId"));
            Some(call(prompt_relay(kernel, relay_id)))
        }
        "listPromptRelays" => {
            let limit = arg!(a.opt_i64(0, "limit"));
            Some(call(list_prompt_relays(kernel, limit)))
        }
        "promptRelayStatus" => Some(call(prompt_relay_status(kernel))),
        "recordPromptCapture" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(record_prompt_capture(kernel, &context, input)))
        }
        "beginPromptAnswer" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(begin_prompt_answer(kernel, &context, input)))
        }
        "refusePromptAnswer" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(refuse_prompt_answer(kernel, &context, input)))
        }
        "finishPromptAnswer" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(finish_prompt_answer(kernel, &context, input)))
        }
        "hasExpiredPromptCaptures" => Some(call(has_expired_prompt_captures(kernel))),
        "expirePromptCaptures" => {
            let context = arg!(a.ctx(0));
            Some(call(expire_prompt_captures(kernel, &context)))
        }
        "failInterruptedPromptRelays" => {
            let context = arg!(a.ctx(0));
            Some(call(fail_interrupted_prompt_relays(kernel, &context)))
        }
        _ => None,
    }
}
