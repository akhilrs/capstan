//! The shapes of the Node view model (`src/dash/model.ts`), field for field.
//!
//! A field the Node model sets to `null` is an `Option` that serialises as `null`; a field that is absent in Node
//! (`undefined`) is an `Option` that is skipped when `None`.
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRow {
    pub id: String,
    pub agent_id: String,
    pub role_name: String,
    pub kind: String,
    pub generation: i64,
    pub state: String,
    pub last_activity_at: String,
    /// Inferred: activity within 30 s, or a message in flight to the agent.
    pub working: bool,
    pub stalled: bool,
    pub lost: bool,
    pub blocked: bool,
    /// When delivery to the agent was paused, or `None`.
    pub paused_at: Option<String>,
    pub pane_id: Option<String>,
    pub queue_depth: i64,
    /// What the agent works on, one line; `-` when nothing is known.
    pub task: String,
    pub fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRow {
    pub id: String,
    pub message_id: String,
    pub recipient_agent_id: String,
    pub recipient_active: bool,
    pub state: String,
    pub sequence: i64,
    pub queued_at: String,
    pub deferred_reason: Option<String>,
    pub state_reason: Option<String>,
    pub notified: bool,
    /// Why this is a delivery problem, or `None`.
    pub problem: Option<String>,
    pub fingerprint: String,
}

/// One report, review or integration; `stage` is `report`, `review` or `integration`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineItem {
    pub id: String,
    pub stage: String,
    pub state: String,
    pub label: String,
    pub created_at: String,
    pub fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineStage {
    /// Per-state counts in the order the model lists them.
    pub counts: IndexMap<String, i64>,
    pub total: i64,
    /// The list reached the daemon's cap, so older rows may exist.
    pub capped: bool,
    pub items: Vec<PipelineItem>,
}

/// One finding; `target_state` is `active`, `ended` or `unknown`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingRow {
    pub id: String,
    pub finding_id: String,
    pub target_agent_id: String,
    pub severity: String,
    pub state: String,
    pub interventions: i64,
    pub state_reason: Option<String>,
    pub needs_operator: bool,
    pub target_state: String,
    /// The target is not active, so nobody can act on the finding any more.
    pub stale: bool,
    pub fingerprint: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WaitingKind {
    #[serde(rename = "integration")]
    Integration,
    #[serde(rename = "proposal")]
    Proposal,
    #[serde(rename = "agent-paused")]
    AgentPaused,
    #[serde(rename = "run-paused")]
    RunPaused,
}

/// Something that waits for the operator: a confirm, a proposal to decide, or a pause to lift.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitingItem {
    pub id: String,
    pub kind: WaitingKind,
    /// One line, without the age.
    pub label: String,
    /// ISO time the wait began, the source of the age shown; `None` when unknown.
    pub since: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkRow {
    pub id: String,
    pub work_item_id: String,
    pub title: String,
    pub state: String,
    pub owner: Option<String>,
    pub fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupervisorRef {
    pub id: String,
    pub state: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastCheck {
    pub state: String,
    pub queued_at: String,
    pub acked_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupervisionHeader {
    /// `[supervision] enabled` in the config.
    pub enabled: bool,
    pub supervisor: Option<SupervisorRef>,
    /// The newest routine check queued; `acked_at` is `None` until it is acknowledged.
    pub last_check: Option<LastCheck>,
    pub open_findings: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PmMailHeader {
    pub pending: i64,
    pub oldest_age_seconds: f64,
    pub stale: bool,
}

/// One thing being worked on: `id` is the short name, `label` the full line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskEntry {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunPause {
    pub paused_at: String,
    pub reason: String,
    pub actor_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Header {
    /// Active tasks; `None` when the status carries no task data, an empty list means idle.
    pub tasks: Option<Vec<TaskEntry>>,
    /// The PM's pending mail; `None` when the status carries none.
    pub pm_mail: Option<PmMailHeader>,
    pub project_id: String,
    pub run_state: String,
    /// The open run pause, or `None`.
    pub run_pause: Option<RunPause>,
    /// The live supervision loop; `None` when the status carries no `supervisionState`.
    pub supervision: Option<SupervisionHeader>,
    pub workers: i64,
    pub worker_limit: Option<i64>,
    /// Whole minutes of full auto left; present only while it is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_auto_minutes: Option<i64>,
    /// Session grants in force; present only when there are any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grants: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pipeline {
    pub reports: PipelineStage,
    pub reviews: PipelineStage,
    pub integrations: PipelineStage,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Queue {
    pub messages: Vec<MessageRow>,
    pub truncated: bool,
    pub input_clears: Vec<String>,
    pub input_clear_count: i64,
    pub cleanup_failed: Vec<String>,
    pub orphan_panes: Vec<String>,
}

/// Samples for the sparklines.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub unresolved: i64,
    pub working: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashModel {
    pub header: Header,
    /// Active agents first, then every ended agent, most recent activity first.
    pub agents: Vec<AgentRow>,
    pub pipeline: Pipeline,
    pub queue: Queue,
    pub findings: Vec<FindingRow>,
    pub work: Vec<WorkRow>,
    /// What waits for the operator, in the order shown.
    pub waiting: Vec<WaitingItem>,
    pub counts: Counts,
}

/// Row fingerprints from the last poll and the polls left to highlight each changed row.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeState {
    pub fingerprints: Vec<(String, String)>,
    pub highlight: Vec<(String, i64)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistorySample {
    pub unresolved: f64,
    pub working: f64,
    /// Seconds since the oldest unresolved message was queued; 0 when there is none.
    pub oldest_seconds: f64,
}
