//! What a notification says. The text comes from the fields of a [`PushEvent`] only, never from message bodies.

use super::transport::PublishRequest;
use capstan_config::{Ntfy, NtfyEvent};
use capstan_kernel::areas::push_events::PushEvent;

const MESSAGE_MAX_CHARS: usize = 200;

/// The label that follows the title prefix.
pub fn label(event: NtfyEvent) -> &'static str {
    match event {
        NtfyEvent::UserQuestion => "Question",
        NtfyEvent::OperatorProposal => "Approval needed",
        NtfyEvent::PromptWaiting => "Prompt waiting",
        NtfyEvent::DesignWaiting => "Design review",
        NtfyEvent::PlanSignedOff => "Plan signed off",
        NtfyEvent::AgentBlocked => "Agent blocked",
        NtfyEvent::AgentStalled => "Agent stalled",
        NtfyEvent::AgentLost => "Agent lost",
        NtfyEvent::VerifyFailed => "Verify failed",
        NtfyEvent::IntegrationConflicted => "Conflict",
        NtfyEvent::Test => "Test",
    }
}

/// The message text of `event`.
pub fn text(event: &PushEvent) -> String {
    let subject = event.subject.as_str();
    let agent = event.agent_id.as_deref().unwrap_or(subject);
    let plan = event.plan_id.as_deref().unwrap_or(subject);
    let detail = event.detail.as_deref();
    let message = match event.event {
        NtfyEvent::UserQuestion => {
            format!("{agent} asked you a question (#{subject}). Answer in Capstan.")
        }
        NtfyEvent::OperatorProposal => format!("Operator proposal #{subject} waits for you."),
        NtfyEvent::PromptWaiting => format!("{agent} is waiting on a prompt."),
        NtfyEvent::DesignWaiting => {
            format!("Design {subject} of {plan} waits for approval.")
        }
        NtfyEvent::PlanSignedOff => {
            format!("{plan} signed off ({}).", detail.unwrap_or("?"))
        }
        NtfyEvent::AgentBlocked => format!("{agent} is blocked."),
        NtfyEvent::AgentStalled => format!("{agent} has stalled."),
        NtfyEvent::AgentLost => format!("{agent} was lost."),
        NtfyEvent::VerifyFailed => match detail {
            Some(detail) => format!("Verify {subject} failed for {detail}."),
            None => format!("Verify {subject} failed."),
        },
        NtfyEvent::IntegrationConflicted => {
            format!(
                "Integration {subject} conflicted ({} files).",
                detail.unwrap_or("0")
            )
        }
        NtfyEvent::Test => "Test message from Capstan.".to_string(),
    };
    message.chars().take(MESSAGE_MAX_CHARS).collect()
}

/// The request for `event`: the configured topic, the title `<prefix>: <label>` and the event's style.
pub fn request(ntfy: &Ntfy, prefix: &str, event: &PushEvent) -> PublishRequest {
    let style = ntfy.style(event.event);
    PublishRequest {
        topic: ntfy.topic.clone(),
        title: format!("{prefix}: {}", label(event.event)),
        message: text(event),
        priority: Some(style.priority),
        tags: style.tags,
        click: style.click,
    }
}
