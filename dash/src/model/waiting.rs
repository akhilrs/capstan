//! What waits for the operator: confirms, proposals and pauses.
use serde_json::Value;

use super::build::{get, is_objectish, list, slice_units, text, text_of, text_or_null};
use super::types::{WaitingItem, WaitingKind};

/// Confirms, proposals and pauses that wait for the operator. Each source is optional; a missing one adds nothing.
pub(super) fn waiting_items(status: &Value, pause: &Value) -> Vec<WaitingItem> {
    let mut items = Vec::new();
    for i in list(get(status, "awaitingConfirm")) {
        let id = text_of(i, "integrationId");
        let review = Some(text_of(i, "reviewState"))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "none".to_string());
        items.push(WaitingItem {
            id: format!("integration:{id}"),
            kind: WaitingKind::Integration,
            label: format!(
                "integration {} merged, review {review}: cstan integrate confirm pending",
                slice_units(&id, 8)
            ),
            since: text_or_null(get(i, "createdAt")),
        });
    }
    for p in list(get(status, "pendingProposals")) {
        let id = text_of(p, "proposalId");
        items.push(WaitingItem {
            label: format!(
                "proposal {id} {} by {}: {}",
                text_of(p, "kind"),
                text_of(p, "proposer"),
                text_of(p, "reason")
            ),
            id: format!("proposal:{id}"),
            kind: WaitingKind::Proposal,
            since: text_or_null(get(p, "createdAt")),
        });
    }
    if let Some(run) = get(pause, "run").filter(|r| is_objectish(r)) {
        items.push(WaitingItem {
            id: "run-paused".to_string(),
            kind: WaitingKind::RunPaused,
            label: format!("run paused: {}", text(get(run, "reason"))),
            since: text_or_null(get(run, "pausedAt")),
        });
    }
    for a in list(get(pause, "agents")) {
        let id = text_of(a, "agentId");
        items.push(WaitingItem {
            label: format!("{id} paused: {}", text_of(a, "reason")),
            id: format!("agent-paused:{id}"),
            kind: WaitingKind::AgentPaused,
            since: text_or_null(get(a, "pausedAt")),
        });
    }
    items
}
