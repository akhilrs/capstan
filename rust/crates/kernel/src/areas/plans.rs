//! The plans (src/controller/plans.ts).
//!
//! Every public function returns `Err(KernelError::Unported("plans.<method>"))` until the package that owns this file fills
//! its body. The signatures and `dispatch` are fixed by the kernel foundation: fill the bodies, do not change them.

#![allow(unused_imports)]

use crate::dispatch::{arg, call, Args, ErrorHandler, RestartFilter};
use crate::errors::{KernelError, KernelResult};
use crate::kernel::Kernel;
use crate::types::MutationContext;
use serde_json::Value;

/// `openPlan`.
#[allow(unused_variables)]
pub fn open_plan(kernel: &Kernel, context: &MutationContext, input: &Value) -> KernelResult<Value> {
    Err(KernelError::Unported("plans.openPlan".into()))
}

/// `submitPlan`.
#[allow(unused_variables)]
pub fn submit_plan(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported("plans.submitPlan".into()))
}

/// `unannouncedPlanNotices`.
#[allow(unused_variables)]
pub fn unannounced_plan_notices(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("plans.unannouncedPlanNotices".into()))
}

/// `announcePlanNotice`.
#[allow(unused_variables)]
pub fn announce_plan_notice(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported("plans.announcePlanNotice".into()))
}

/// `planReviewRounds`.
#[allow(unused_variables)]
pub fn plan_review_rounds(kernel: &Kernel, credential: &str, plan_id: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("plans.planReviewRounds".into()))
}

/// `abandonPlanReview`.
#[allow(unused_variables)]
pub fn abandon_plan_review(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported("plans.abandonPlanReview".into()))
}

/// `planRecord`.
#[allow(unused_variables)]
pub fn plan_record(kernel: &Kernel, credential: &str, plan_id: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("plans.planRecord".into()))
}

/// `listPlans`.
#[allow(unused_variables)]
pub fn list_plans(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("plans.listPlans".into()))
}

/// `approvalNoticeNote`.
#[allow(unused_variables)]
pub fn approval_notice_note(kernel: &Kernel, plan_id: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("plans.approvalNoticeNote".into()))
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "openPlan" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(open_plan(kernel, &context, input)))
        }
        "submitPlan" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(submit_plan(kernel, &context, input)))
        }
        "unannouncedPlanNotices" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(unannounced_plan_notices(kernel, credential)))
        }
        "announcePlanNotice" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(announce_plan_notice(kernel, &context, input)))
        }
        "planReviewRounds" => {
            let credential = arg!(a.str(0, "credential"));
            let plan_id = arg!(a.str(1, "planId"));
            Some(call(plan_review_rounds(kernel, credential, plan_id)))
        }
        "abandonPlanReview" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(abandon_plan_review(kernel, &context, input)))
        }
        "planRecord" => {
            let credential = arg!(a.str(0, "credential"));
            let plan_id = arg!(a.str(1, "planId"));
            Some(call(plan_record(kernel, credential, plan_id)))
        }
        "listPlans" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(list_plans(kernel, credential)))
        }
        "approvalNoticeNote" => {
            let plan_id = arg!(a.str(0, "planId"));
            Some(call(approval_notice_note(kernel, plan_id)))
        }
        _ => None,
    }
}
