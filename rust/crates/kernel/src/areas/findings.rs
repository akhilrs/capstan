//! The supervisor findings (src/controller/findings.ts).
//!
//! Every public function returns `Err(KernelError::Unported("findings.<method>"))` until the package that owns this file fills
//! its body. The signatures and `dispatch` are fixed by the kernel foundation: fill the bodies, do not change them.

#![allow(unused_imports)]

use crate::dispatch::{arg, call, Args, ErrorHandler, RestartFilter};
use crate::errors::{KernelError, KernelResult};
use crate::kernel::Kernel;
use crate::types::MutationContext;
use serde_json::Value;

/// `raiseFinding`.
#[allow(unused_variables)]
pub fn raise_finding(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported("findings.raiseFinding".into()))
}

/// `checkFinding`.
#[allow(unused_variables)]
pub fn check_finding(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported("findings.checkFinding".into()))
}

/// `unannouncedFindingNotices`.
#[allow(unused_variables)]
pub fn unannounced_finding_notices(
    kernel: &Kernel,
    credential: &str,
    limit: Option<i64>,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "findings.unannouncedFindingNotices".into(),
    ))
}

/// `announceFindingNotice`.
#[allow(unused_variables)]
pub fn announce_finding_notice(
    kernel: &Kernel,
    context: &MutationContext,
    notice_id: &str,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "findings.announceFindingNotice".into(),
    ))
}

/// `assertCanObserve`.
#[allow(unused_variables)]
pub fn assert_can_observe(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("findings.assertCanObserve".into()))
}

/// `findings`.
#[allow(unused_variables)]
pub fn findings(kernel: &Kernel, credential: &str, limit: Option<i64>) -> KernelResult<Value> {
    Err(KernelError::Unported("findings.findings".into()))
}

/// `sweepFindings`.
#[allow(unused_variables)]
pub fn sweep_findings(
    kernel: &Kernel,
    new_context: &dyn Fn() -> MutationContext,
    deadline_seconds: f64,
    on_error: Option<ErrorHandler<'_>>,
) -> KernelResult<Value> {
    Err(KernelError::Unported("findings.sweepFindings".into()))
}

/// `supervisionReason`.
#[allow(unused_variables)]
pub fn supervision_reason(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("findings.supervisionReason".into()))
}

/// `supervisionActivity`.
#[allow(unused_variables)]
pub fn supervision_activity(kernel: &Kernel) -> KernelResult<Value> {
    Err(KernelError::Unported("findings.supervisionActivity".into()))
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "raiseFinding" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(raise_finding(kernel, &context, input)))
        }
        "checkFinding" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(check_finding(kernel, &context, input)))
        }
        "unannouncedFindingNotices" => {
            let credential = arg!(a.str(0, "credential"));
            let limit = arg!(a.opt_i64(1, "limit"));
            Some(call(unannounced_finding_notices(kernel, credential, limit)))
        }
        "announceFindingNotice" => {
            let context = arg!(a.ctx(0));
            let notice_id = arg!(a.str(1, "noticeId"));
            Some(call(announce_finding_notice(kernel, &context, notice_id)))
        }
        "assertCanObserve" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(assert_can_observe(kernel, credential)))
        }
        "findings" => {
            let credential = arg!(a.str(0, "credential"));
            let limit = arg!(a.opt_i64(1, "limit"));
            Some(call(findings(kernel, credential, limit)))
        }
        "supervisionReason" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(supervision_reason(kernel, credential)))
        }
        "supervisionActivity" => Some(call(supervision_activity(kernel))),
        _ => None,
    }
}
