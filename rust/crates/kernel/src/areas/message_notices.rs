//! The message notices and waits (src/controller/message-notices.ts).
//!
//! Every public function returns `Err(KernelError::Unported("messageNotices.<method>"))` until the package that owns this file fills
//! its body. The signatures and `dispatch` are fixed by the kernel foundation: fill the bodies, do not change them.

#![allow(unused_imports)]

use crate::dispatch::{arg, call, Args};
use crate::errors::{KernelError, KernelResult};
use crate::kernel::Kernel;
use crate::records::AgentRow;
use crate::types::MutationContext;
use serde_json::Value;

/// `openWaits`.
#[allow(unused_variables)]
pub fn open_waits(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("messageNotices.openWaits".into()))
}

/// `recordNotification`.
#[allow(unused_variables)]
pub fn record_notification(
    kernel: &Kernel,
    context: &MutationContext,
    message_id: &str,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "messageNotices.recordNotification".into(),
    ))
}

/// `beginWait`.
#[allow(unused_variables)]
pub fn begin_wait(kernel: &Kernel, context: &MutationContext) -> KernelResult<Value> {
    Err(KernelError::Unported("messageNotices.beginWait".into()))
}

/// `endWait`.
#[allow(unused_variables)]
pub fn end_wait(kernel: &Kernel, context: &MutationContext, wait_id: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("messageNotices.endWait".into()))
}

/// `endWaitAsController`.
#[allow(unused_variables)]
pub fn end_wait_as_controller(
    kernel: &Kernel,
    context: &MutationContext,
    wait_id: &str,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "messageNotices.endWaitAsController".into(),
    ))
}

/// `advanceMessaging`.
#[allow(unused_variables)]
pub fn advance_messaging(
    kernel: &Kernel,
    context: &MutationContext,
    timers: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "messageNotices.advanceMessaging".into(),
    ))
}

/// `queueMissingDeliveryNotices`.
#[allow(unused_variables)]
pub fn queue_missing_delivery_notices(
    kernel: &Kernel,
    context: &MutationContext,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "messageNotices.queueMissingDeliveryNotices".into(),
    ))
}

/// `queueInputBlockedNotice`.
#[allow(unused_variables)]
pub fn queue_input_blocked_notice(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "messageNotices.queueInputBlockedNotice".into(),
    ))
}

/// `queueAttentionNotices`.
#[allow(unused_variables)]
pub fn queue_attention_notices(
    kernel: &Kernel,
    context: &MutationContext,
    episodes: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "messageNotices.queueAttentionNotices".into(),
    ))
}

/// `recordPmWake`.
#[allow(unused_variables)]
pub fn record_pm_wake(
    kernel: &Kernel,
    context: &MutationContext,
    message_id: &str,
) -> KernelResult<Value> {
    Err(KernelError::Unported("messageNotices.recordPmWake".into()))
}

/// `queueSupervisionCheck`.
#[allow(unused_variables)]
pub fn queue_supervision_check(
    kernel: &Kernel,
    context: &MutationContext,
    interval_seconds: f64,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "messageNotices.queueSupervisionCheck".into(),
    ))
}

/// The controller's own actor and the one active PM (`noticeParties`).
pub struct NoticeParties {
    pub controller_actor_id: String,
    pub pm: AgentRow,
}

/// The parties of a controller notice, or `None` when either is missing.
#[allow(unused_variables)]
pub fn notice_parties(kernel: &Kernel) -> KernelResult<Option<NoticeParties>> {
    Err(KernelError::Unported("messageNotices.noticeParties".into()))
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "openWaits" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(open_waits(kernel, credential)))
        }
        "recordNotification" => {
            let context = arg!(a.ctx(0));
            let message_id = arg!(a.str(1, "messageId"));
            Some(call(record_notification(kernel, &context, message_id)))
        }
        "beginWait" => {
            let context = arg!(a.ctx(0));
            Some(call(begin_wait(kernel, &context)))
        }
        "endWait" => {
            let context = arg!(a.ctx(0));
            let wait_id = arg!(a.str(1, "waitId"));
            Some(call(end_wait(kernel, &context, wait_id)))
        }
        "endWaitAsController" => {
            let context = arg!(a.ctx(0));
            let wait_id = arg!(a.str(1, "waitId"));
            Some(call(end_wait_as_controller(kernel, &context, wait_id)))
        }
        "advanceMessaging" => {
            let context = arg!(a.ctx(0));
            let timers = a.value(1);
            Some(call(advance_messaging(kernel, &context, timers)))
        }
        "queueMissingDeliveryNotices" => {
            let context = arg!(a.ctx(0));
            Some(call(queue_missing_delivery_notices(kernel, &context)))
        }
        "queueInputBlockedNotice" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(queue_input_blocked_notice(kernel, &context, input)))
        }
        "queueAttentionNotices" => {
            let context = arg!(a.ctx(0));
            let episodes = a.value(1);
            Some(call(queue_attention_notices(kernel, &context, episodes)))
        }
        "recordPmWake" => {
            let context = arg!(a.ctx(0));
            let message_id = arg!(a.str(1, "messageId"));
            Some(call(record_pm_wake(kernel, &context, message_id)))
        }
        "queueSupervisionCheck" => {
            let context = arg!(a.ctx(0));
            let interval_seconds = arg!(a.f64(1, "intervalSeconds"));
            Some(call(queue_supervision_check(
                kernel,
                &context,
                interval_seconds,
            )))
        }
        _ => None,
    }
}
