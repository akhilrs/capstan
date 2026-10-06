//! Pure helpers over a built model: what each panel lists, what changed, and the history samples.
use std::collections::HashMap;

use indexmap::IndexMap;

use super::time::parse_iso_ms;
use super::types::{AgentRow, ChangeState, DashModel, HistorySample, MessageRow, PipelineItem};

pub const WORKING_WINDOW_MS: i64 = 30_000;
/// The daemon returns at most this many reports, reviews, integrations and findings, newest first.
pub const PIPELINE_ITEMS: usize = 20;
pub const HIGHLIGHT_POLLS: i64 = 2;
/// How many of the most recently ended agents the panel shows unless "show all ended" is on.
pub const RECENT_ENDED_SHOWN: usize = 5;
pub const TASK_NONE: &str = "-";

/// Workers are every active agent except the PM and the Supervisor.
pub fn is_worker_kind(kind: &str) -> bool {
    kind != "PM" && kind != "Supervisor"
}

/// The agents the panel lists: every active agent, any ended agent still flagged lost, stalled or blocked, and the most
/// recently ended few (the model keeps ended agents newest first).
pub fn visible_agents(model: &DashModel, show_all_ended: bool) -> Vec<&AgentRow> {
    if show_all_ended {
        return model.agents.iter().collect();
    }
    let mut recent = 0;
    model
        .agents
        .iter()
        .filter(|a| {
            if a.state == "active" || a.lost || a.stalled || a.blocked {
                return true;
            }
            recent += 1;
            recent <= RECENT_ENDED_SHOWN
        })
        .collect()
}

/// Messages shown in the queue panel: all of them, or only delivery problems.
pub fn queue_rows(model: &DashModel, problems_only: bool) -> Vec<&MessageRow> {
    model
        .queue
        .messages
        .iter()
        .filter(|m| !problems_only || m.problem.is_some())
        .collect()
}

/// The queue panel shrinks to a stub while no message is unresolved.
pub fn queue_collapsed(model: &DashModel) -> bool {
    model.queue.messages.is_empty()
}

/// Pipeline items of all three stages, newest first.
pub fn pipeline_items(model: &DashModel) -> Vec<&PipelineItem> {
    let pipeline = &model.pipeline;
    let mut items: Vec<&PipelineItem> = pipeline
        .reports
        .items
        .iter()
        .chain(&pipeline.reviews.items)
        .chain(&pipeline.integrations.items)
        .collect();
    items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    items
}

fn tracked_rows(model: &DashModel) -> Vec<(String, &str)> {
    let pipeline = &model.pipeline;
    let mut rows: Vec<(String, &str)> = Vec::new();
    rows.extend(
        model
            .agents
            .iter()
            .map(|r| (format!("a:{}", r.id), r.fingerprint.as_str())),
    );
    rows.extend(
        model
            .queue
            .messages
            .iter()
            .map(|r| (format!("m:{}", r.id), r.fingerprint.as_str())),
    );
    rows.extend(
        model
            .findings
            .iter()
            .map(|r| (format!("f:{}", r.id), r.fingerprint.as_str())),
    );
    rows.extend(
        model
            .work
            .iter()
            .map(|r| (format!("w:{}", r.id), r.fingerprint.as_str())),
    );
    rows.extend(
        pipeline
            .reports
            .items
            .iter()
            .chain(&pipeline.reviews.items)
            .chain(&pipeline.integrations.items)
            .map(|r| (format!("p:{}", r.id), r.fingerprint.as_str())),
    );
    rows
}

/// Rows that changed since the previous poll keep their highlight for `HIGHLIGHT_POLLS` polls. The first poll
/// (no previous fingerprints) highlights nothing.
pub fn track_changes(previous: &ChangeState, model: &DashModel) -> ChangeState {
    let before: HashMap<&str, &str> = previous
        .fingerprints
        .iter()
        .map(|(id, fingerprint)| (id.as_str(), fingerprint.as_str()))
        .collect();
    let earlier: HashMap<&str, i64> = previous
        .highlight
        .iter()
        .map(|(id, left)| (id.as_str(), *left))
        .collect();
    let first = previous.fingerprints.is_empty();
    let mut fingerprints: IndexMap<String, String> = IndexMap::new();
    let mut highlight: IndexMap<String, i64> = IndexMap::new();
    for (id, fingerprint) in tracked_rows(model) {
        fingerprints.insert(id.clone(), fingerprint.to_string());
        if !first && before.get(id.as_str()) != Some(&fingerprint) {
            highlight.insert(id, HIGHLIGHT_POLLS);
        } else {
            let left = earlier.get(id.as_str()).copied().unwrap_or(0) - 1;
            if left > 0 {
                highlight.insert(id, left);
            }
        }
    }
    ChangeState {
        fingerprints: fingerprints.into_iter().collect(),
        highlight: highlight.into_iter().collect(),
    }
}

pub fn history_sample(model: &DashModel, now_ms: i64) -> HistorySample {
    let mut oldest: i64 = 0;
    for message in &model.queue.messages {
        if let Some(queued) = parse_iso_ms(&message.queued_at) {
            oldest = oldest.max((now_ms - queued).div_euclid(1000));
        }
    }
    HistorySample {
        unresolved: model.counts.unresolved as f64,
        working: model.counts.working as f64,
        oldest_seconds: oldest as f64,
    }
}
