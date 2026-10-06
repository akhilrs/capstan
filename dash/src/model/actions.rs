//! The dashboard's actions as data: what is offered for a row, the confirm text, and the daemon call.
use serde::{Deserialize, Serialize};

use super::text::clean;
use super::types::{AgentRow, MessageRow};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Retry,
    Skip,
    Cancel,
}

impl Decision {
    pub const ALL: [Decision; 3] = [Decision::Retry, Decision::Skip, Decision::Cancel];

    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Retry => "retry",
            Decision::Skip => "skip",
            Decision::Cancel => "cancel",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Decision::Retry => "y",
            Decision::Skip => "s",
            Decision::Cancel => "c",
        }
    }
}

// The contract names `message: MessageRow` by value; actions are short-lived, so the size gap is harmless.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum DashAction {
    Observe {
        agent_id: String,
    },
    Resolve {
        decision: Decision,
        message: MessageRow,
    },
}

/// The key of each decision in the queue panel.
pub const ACTION_KEYS: [(Decision, &str); 3] = [
    (Decision::Retry, "y"),
    (Decision::Skip, "s"),
    (Decision::Cancel, "c"),
];

/// A message the daemon can still act on: it was not delivered and acknowledged, cancelled or the like.
fn is_unresolved(state: &str) -> bool {
    matches!(
        state,
        "deferred" | "sent" | "unacked" | "expired" | "failed"
    )
}

/// The state a decision moves a message to, or `None` when it cannot apply (the core's `resolutionTarget`).
fn resolution_target(decision: Decision, from: &str) -> Option<&'static str> {
    match decision {
        Decision::Retry => is_unresolved(from).then_some("queued"),
        Decision::Skip | Decision::Cancel => {
            (is_unresolved(from) || from == "queued").then_some("cancelled")
        }
    }
}

/// The decisions worth offering for a message. The daemon still decides; this only hides keys that cannot work.
pub fn available_decisions(message: &MessageRow) -> Vec<Decision> {
    Decision::ALL
        .into_iter()
        .filter(|&decision| {
            resolution_target(decision, &message.state).is_some()
                && (decision != Decision::Retry || message.recipient_active)
        })
        .collect()
}

pub fn observe_action(agent: &AgentRow) -> Option<DashAction> {
    (agent.state == "active").then(|| DashAction::Observe {
        agent_id: agent.agent_id.clone(),
    })
}

/// Retrying a message in flight may deliver it twice.
pub fn duplicate_warning(message: &MessageRow) -> bool {
    message.state == "sent" || message.state == "unacked"
}

pub fn confirm_text(action: &DashAction) -> String {
    match action {
        DashAction::Observe { agent_id } => format!("Observe {}?", clean(agent_id)),
        DashAction::Resolve { decision, message } => {
            let verb = match decision {
                Decision::Retry => "Retry",
                Decision::Skip => "Skip",
                Decision::Cancel => "Cancel",
            };
            let warning = if *decision == Decision::Retry && duplicate_warning(message) {
                " The recipient may already have received it."
            } else {
                ""
            };
            format!(
                "{verb} message {} to {} (state {})?{warning} Press y again to confirm, any other key cancels.",
                message.message_id, message.recipient_agent_id, message.state
            )
        }
    }
}

/// The daemon command for an action: the same routes `cstan resolve` and `cstan cancel` use.
pub fn to_wire_call(action: &DashAction) -> (String, Vec<String>) {
    match action {
        DashAction::Observe { agent_id } => {
            ("peek".to_string(), vec![agent_id.clone(), "40".to_string()])
        }
        DashAction::Resolve { decision, message } => match decision {
            Decision::Cancel => ("cancel".to_string(), vec![message.message_id.clone()]),
            _ => (
                "resolve".to_string(),
                vec![message.message_id.clone(), decision.as_str().to_string()],
            ),
        },
    }
}
