//! The messages (src/controller/messages.ts).
//!
//! Every public function returns `Err(KernelError::Unported("messages.<method>"))` until the package that owns this file fills
//! its body. The signatures and `dispatch` are fixed by the kernel foundation: fill the bodies, do not change them.

#![allow(unused_imports)]

use crate::dispatch::{arg, call, Args};
use crate::errors::{KernelError, KernelResult};
use crate::kernel::Kernel;
use crate::records::{AgentRow, MessageRow};
use crate::types::MutationContext;
use serde_json::Value;

/// `enqueueMessage`.
#[allow(unused_variables)]
pub fn enqueue_message(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported("messages.enqueueMessage".into()))
}

/// `message`.
#[allow(unused_variables)]
pub fn message(kernel: &Kernel, message_id: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("messages.message".into()))
}

/// `messagesFor`.
#[allow(unused_variables)]
pub fn messages_for(kernel: &Kernel, agent_id: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("messages.messagesFor".into()))
}

/// `openMessagesFor`.
#[allow(unused_variables)]
pub fn open_messages_for(kernel: &Kernel, agent_id: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("messages.openMessagesFor".into()))
}

/// `unresolvedMessages`.
#[allow(unused_variables)]
pub fn unresolved_messages(kernel: &Kernel, credential: &str, limit: i64) -> KernelResult<Value> {
    Err(KernelError::Unported("messages.unresolvedMessages".into()))
}

/// `inputClears`.
#[allow(unused_variables)]
pub fn input_clears(kernel: &Kernel, credential: &str, limit: i64) -> KernelResult<Value> {
    Err(KernelError::Unported("messages.inputClears".into()))
}

/// `senderOf`.
#[allow(unused_variables)]
pub fn sender_of(kernel: &Kernel, actor_id: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("messages.senderOf".into()))
}

/// `messageRejections`.
#[allow(unused_variables)]
pub fn message_rejections(kernel: &Kernel) -> KernelResult<Value> {
    Err(KernelError::Unported("messages.messageRejections".into()))
}

/// `agentInbox`.
#[allow(unused_variables)]
pub fn agent_inbox(
    kernel: &Kernel,
    credential: &str,
    agent_id: Option<&str>,
) -> KernelResult<Value> {
    Err(KernelError::Unported("messages.agentInbox".into()))
}

/// `pullMessage`.
#[allow(unused_variables)]
pub fn pull_message(kernel: &Kernel, context: &MutationContext) -> KernelResult<Value> {
    Err(KernelError::Unported("messages.pullMessage".into()))
}

/// `pullPending`.
#[allow(unused_variables)]
pub fn pull_pending(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("messages.pullPending".into()))
}

/// `unreadSummary`.
#[allow(unused_variables)]
pub fn unread_summary(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("messages.unreadSummary".into()))
}

/// `recordDeferral`.
#[allow(unused_variables)]
pub fn record_deferral(
    kernel: &Kernel,
    context: &MutationContext,
    message_id: &str,
    reason: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported("messages.recordDeferral".into()))
}

/// `recordInputClear`.
#[allow(unused_variables)]
pub fn record_input_clear(
    kernel: &Kernel,
    context: &MutationContext,
    message_id: &str,
    text: &str,
) -> KernelResult<Value> {
    Err(KernelError::Unported("messages.recordInputClear".into()))
}

/// `recordSent`.
#[allow(unused_variables)]
pub fn record_sent(
    kernel: &Kernel,
    context: &MutationContext,
    message_id: &str,
) -> KernelResult<Value> {
    Err(KernelError::Unported("messages.recordSent".into()))
}

/// `recordFailure`.
#[allow(unused_variables)]
pub fn record_failure(
    kernel: &Kernel,
    context: &MutationContext,
    message_id: &str,
    reason: &str,
) -> KernelResult<Value> {
    Err(KernelError::Unported("messages.recordFailure".into()))
}

/// `ackMessage`.
#[allow(unused_variables)]
pub fn ack_message(
    kernel: &Kernel,
    context: &MutationContext,
    message_id: &str,
) -> KernelResult<Value> {
    Err(KernelError::Unported("messages.ackMessage".into()))
}

/// `resolveMessage`.
#[allow(unused_variables)]
pub fn resolve_message(
    kernel: &Kernel,
    context: &MutationContext,
    message_id: &str,
    decision: &Value,
    note: Option<&str>,
) -> KernelResult<Value> {
    Err(KernelError::Unported("messages.resolveMessage".into()))
}

/// The agent's messages, oldest first (`messageRowsFor`).
#[allow(unused_variables)]
pub fn message_rows_for(kernel: &Kernel, agent_id: &str) -> KernelResult<Vec<MessageRow>> {
    Err(KernelError::Unported("messages.messageRowsFor".into()))
}

/// Queues a message from `sender_actor_id` to `recipient` and returns its id (`insertQueuedMessage`). The caller owns the transaction.
#[allow(unused_variables, clippy::too_many_arguments)]
pub fn insert_queued_message(
    kernel: &Kernel,
    sender_actor_id: &str,
    recipient: &AgentRow,
    body: &str,
    body_hash: &str,
    now: &str,
    action_needed: bool,
) -> KernelResult<String> {
    Err(KernelError::Unported("messages.insertQueuedMessage".into()))
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "enqueueMessage" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(enqueue_message(kernel, &context, input)))
        }
        "message" => {
            let message_id = arg!(a.str(0, "messageId"));
            Some(call(message(kernel, message_id)))
        }
        "messagesFor" => {
            let agent_id = arg!(a.str(0, "agentId"));
            Some(call(messages_for(kernel, agent_id)))
        }
        "openMessagesFor" => {
            let agent_id = arg!(a.str(0, "agentId"));
            Some(call(open_messages_for(kernel, agent_id)))
        }
        "unresolvedMessages" => {
            let credential = arg!(a.str(0, "credential"));
            let limit = arg!(a.i64(1, "limit"));
            Some(call(unresolved_messages(kernel, credential, limit)))
        }
        "inputClears" => {
            let credential = arg!(a.str(0, "credential"));
            let limit = arg!(a.i64(1, "limit"));
            Some(call(input_clears(kernel, credential, limit)))
        }
        "senderOf" => {
            let actor_id = arg!(a.str(0, "actorId"));
            Some(call(sender_of(kernel, actor_id)))
        }
        "messageRejections" => Some(call(message_rejections(kernel))),
        "agentInbox" => {
            let credential = arg!(a.str(0, "credential"));
            let agent_id = arg!(a.opt_str(1, "agentId"));
            Some(call(agent_inbox(kernel, credential, agent_id)))
        }
        "pullMessage" => {
            let context = arg!(a.ctx(0));
            Some(call(pull_message(kernel, &context)))
        }
        "pullPending" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(pull_pending(kernel, credential)))
        }
        "unreadSummary" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(unread_summary(kernel, credential)))
        }
        "recordDeferral" => {
            let context = arg!(a.ctx(0));
            let message_id = arg!(a.str(1, "messageId"));
            let reason = a.value(2);
            Some(call(record_deferral(kernel, &context, message_id, reason)))
        }
        "recordInputClear" => {
            let context = arg!(a.ctx(0));
            let message_id = arg!(a.str(1, "messageId"));
            let text = arg!(a.str(2, "text"));
            Some(call(record_input_clear(kernel, &context, message_id, text)))
        }
        "recordSent" => {
            let context = arg!(a.ctx(0));
            let message_id = arg!(a.str(1, "messageId"));
            Some(call(record_sent(kernel, &context, message_id)))
        }
        "recordFailure" => {
            let context = arg!(a.ctx(0));
            let message_id = arg!(a.str(1, "messageId"));
            let reason = arg!(a.str(2, "reason"));
            Some(call(record_failure(kernel, &context, message_id, reason)))
        }
        "ackMessage" => {
            let context = arg!(a.ctx(0));
            let message_id = arg!(a.str(1, "messageId"));
            Some(call(ack_message(kernel, &context, message_id)))
        }
        "resolveMessage" => {
            let context = arg!(a.ctx(0));
            let message_id = arg!(a.str(1, "messageId"));
            let decision = a.value(2);
            let note = arg!(a.opt_str(3, "note"));
            Some(call(resolve_message(
                kernel, &context, message_id, decision, note,
            )))
        }
        _ => None,
    }
}
