//! Row shapes and the limits of the controller core: src/controller/records.ts.
//!
//! The constants and the rows the shared helpers read are here. Record types that only one area produces live in that
//! area's file.

use crate::types::AgentRecord;
use rusqlite::Row;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_MESSAGE_BYTES: usize = 16 * 1024;
/// What a restart proposal stores as its command text; the hash covers it with the kind and the force flag.
pub const RESTART_COMMAND_TEXT: &str = "restart";
pub const MAX_OPERATOR_TAIL_BYTES: usize = 12288;
pub const MAX_INPUT_CLEAR_BYTES: usize = 64 * 1024;
pub const MAX_SUMMARY_MESSAGES: usize = 50;
pub const MAX_SUMMARY_BODY: usize = 2000;
pub const MAX_SUMMARY_WORK: usize = 200;
pub const MAX_SUMMARY_LINKS: usize = 50;
pub const MAX_SUMMARY_BYTES: usize = 32 * 1024;
pub const MAX_OBJECTIVE_BYTES: usize = 8 * 1024;
pub const TRUNCATION_MARKER: &str = "[truncated]";
pub const MAX_REPORT_SUMMARY_BYTES: usize = 1000;
/// After this many rejected reports for one agent generation, further reports are refused without a new row.
pub const MAX_REJECTED_REPORTS: usize = 100;
pub const MAX_REVIEW_TEXT_BYTES: usize = 4000;
/// Rounds that ended in a verdict; failed and cancelled rounds do not use the budget.
pub const MAX_REVIEW_ROUNDS: i64 = 5;
pub const PLAN_NOTICE_BUDGET_BYTES: usize = 12 * 1024;
pub const MAX_PLAN_BODY_BYTES: usize = 32 * 1024;
pub const MAX_PLAN_PACKAGES: usize = 20;
pub const MAX_INTEGRATION_REPORTS: usize = 20;
pub const MAX_CONFLICT_FILES: usize = 50;
pub const MAX_CONFLICT_PATH_CHARS: usize = 200;
/// Room for the cut marker: `...#` and twelve hex digits.
pub const PATH_CUT_MARK_CHARS: usize = 16;
pub const MAX_FINDING_EVIDENCE_BYTES: usize = 1500;
pub const MAX_FINDING_CORRECTION_BYTES: usize = 600;
pub const MAX_FINDING_CONDITION_BYTES: usize = 300;
pub const FINDING_INTERVENTIONS: i64 = 2;
pub const FINDING_SEVERITIES: [&str; 5] = ["info", "low", "medium", "high", "critical"];
pub const FAILURE_REASON_POINTS: usize = 200;
/// Message ids a loss notice names; the rest is counted.
pub const LOST_NOTICE_IDS: usize = 10;
/// Message ids a loss event stores.
pub const LOST_EVENT_IDS: usize = 50;
pub const SEED_MESSAGES: usize = 20;
pub const SEED_REPORTS: usize = 10;
pub const SEED_FINDINGS: usize = 5;
/// The longest free-text answer the controller accepts: short enough that it cannot wrap the worker's text field.
pub const PROMPT_RELAY_TEXT_MAX_CHARS: usize = 200;
pub const SUPERVISION_CHECK_TEXT: &str = "Routine check from the controller. Run cstan status, then cstan observe each active worker, and apply your standing instructions. Raise a finding only for the same failing command repeated, a step that cannot work, or no progress while looking busy; otherwise raise nothing. Then cstan ack this message.";

/// The names of the messaging timers (src/controller/messaging.ts `MESSAGING_TIMER_NAMES`).
pub const TIMER_NAMES: [&str; 9] = [
    "maxDeferralSeconds",
    "maxBusyDeferralSeconds",
    "pmAckTimeoutSeconds",
    "pmNotifyAfterSeconds",
    "notifyIntervalSeconds",
    "stallAfterSeconds",
    "workerAckTimeoutSeconds",
    "pmWakeAfterSeconds",
    "pmWakeIntervalSeconds",
];

/// Message states that end a message (messaging.ts `FINAL_STATES`).
pub const FINAL_MESSAGE_STATES: [&str; 3] = ["acked", "acked_late", "cancelled"];
/// Message states that wait for the PM to resolve them (messaging.ts `UNRESOLVED_STATES`).
pub const UNRESOLVED_MESSAGE_STATES: [&str; 5] =
    ["deferred", "sent", "unacked", "expired", "failed"];

pub fn is_final_state(state: &str) -> bool {
    FINAL_MESSAGE_STATES.contains(&state)
}

pub fn is_unresolved_state(state: &str) -> bool {
    UNRESOLVED_MESSAGE_STATES.contains(&state)
}

pub fn escalation_reason_text(reason: &str) -> Option<&'static str> {
    match reason {
        "second_unresolved" => Some("the target did not recover after two corrections"),
        "timed_out" => Some("no check was recorded before the deadline (the supervisor did not check, or the correction was not acknowledged)"),
        _ => None,
    }
}

pub fn cancel_reason_text(reason: &str) -> Option<&'static str> {
    match reason {
        "target_ended" => Some("the target agent ended"),
        "raiser_ended" => Some("the supervisor that raised it ended"),
        _ => None,
    }
}

/// What the controller checked in git for a report; the agent never supplies it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReportEvidence {
    pub generation: i64,
    pub branch: String,
    pub base_sha: Option<String>,
    pub commit_exists: bool,
    pub branch_tip: Option<String>,
    pub is_ancestor_of_tip: bool,
    pub is_ancestor_of_base: bool,
    pub checked_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentReportRecord {
    pub report_id: String,
    pub sequence: i64,
    pub agent_id: String,
    pub generation: i64,
    pub actor_id: String,
    pub commit_sha: String,
    pub branch: Option<String>,
    pub summary: String,
    pub state: String,
    pub reason: Option<String>,
    pub evidence: ReportEvidence,
    pub notified_message_id: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentReportRow {
    pub report_id: String,
    pub sequence: i64,
    pub agent_id: String,
    pub generation: i64,
    pub actor_id: String,
    pub commit_sha: String,
    pub branch: Option<String>,
    pub summary: String,
    pub state: String,
    pub reason: Option<String>,
    pub evidence_json: String,
    pub notified_message_id: Option<String>,
    pub created_at: String,
}

impl AgentReportRow {
    /// Reads a row of `SELECT * FROM agent_reports`, by column name.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            report_id: row.get("report_id")?,
            sequence: row.get("sequence")?,
            agent_id: row.get("agent_id")?,
            generation: row.get("generation")?,
            actor_id: row.get("actor_id")?,
            commit_sha: row.get("commit_sha")?,
            branch: row.get("branch")?,
            summary: row.get("summary")?,
            state: row.get("state")?,
            reason: row.get("reason")?,
            evidence_json: row.get("evidence_json")?,
            notified_message_id: row.get("notified_message_id")?,
            created_at: row.get("created_at")?,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewRecord {
    pub review_id: String,
    pub sequence: i64,
    pub round: i64,
    pub report_id: Option<String>,
    pub integration_id: Option<String>,
    pub plan_id: Option<String>,
    pub plan_revision: Option<i64>,
    pub commit_sha: String,
    pub base_sha: String,
    pub author_agent_id: Option<String>,
    pub author_actor_id: Option<String>,
    pub requested_by_actor_id: String,
    pub reviewer_role: String,
    pub reviewer_agent_id: String,
    pub reviewer_actor_id: String,
    pub state: String,
    pub verdict_text: Option<String>,
    pub failure_reason: Option<String>,
    pub notified_message_id: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReviewRow {
    pub review_id: String,
    pub sequence: i64,
    pub round: i64,
    pub subject_report_id: Option<String>,
    pub subject_integration_id: Option<String>,
    pub subject_plan_id: Option<String>,
    pub subject_plan_revision: Option<i64>,
    pub commit_sha: String,
    pub base_sha: String,
    pub author_agent_id: Option<String>,
    pub author_actor_id: Option<String>,
    pub requested_by_actor_id: String,
    pub reviewer_role: String,
    pub reviewer_agent_id: String,
    pub reviewer_actor_id: String,
    pub state: String,
    pub verdict_text: Option<String>,
    pub failure_reason: Option<String>,
    pub notified_message_id: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

impl ReviewRow {
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            review_id: row.get("review_id")?,
            sequence: row.get("sequence")?,
            round: row.get("round")?,
            subject_report_id: row.get("subject_report_id")?,
            subject_integration_id: row.get("subject_integration_id")?,
            subject_plan_id: row.get("subject_plan_id")?,
            subject_plan_revision: row.get("subject_plan_revision")?,
            commit_sha: row.get("commit_sha")?,
            base_sha: row.get("base_sha")?,
            author_agent_id: row.get("author_agent_id")?,
            author_actor_id: row.get("author_actor_id")?,
            requested_by_actor_id: row.get("requested_by_actor_id")?,
            reviewer_role: row.get("reviewer_role")?,
            reviewer_agent_id: row.get("reviewer_agent_id")?,
            reviewer_actor_id: row.get("reviewer_actor_id")?,
            state: row.get("state")?,
            verdict_text: row.get("verdict_text")?,
            failure_reason: row.get("failure_reason")?,
            notified_message_id: row.get("notified_message_id")?,
            created_at: row.get("created_at")?,
            completed_at: row.get("completed_at")?,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanRecord {
    pub plan_id: String,
    pub sequence: i64,
    pub title: String,
    pub tier: String,
    pub state: String,
    pub requested_by: String,
    pub architect_agent_id: Option<String>,
    pub current_revision: i64,
    pub approved_revision: Option<i64>,
    pub supersedes_plan_id: Option<String>,
    pub cancelled_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanRow {
    pub plan_id: String,
    pub sequence: i64,
    pub title: String,
    pub tier: String,
    pub state: String,
    pub requested_by: String,
    pub architect_agent_id: Option<String>,
    pub current_revision: i64,
    pub approved_revision: Option<i64>,
    pub supersedes_plan_id: Option<String>,
    pub cancelled_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl PlanRow {
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            plan_id: row.get("plan_id")?,
            sequence: row.get("sequence")?,
            title: row.get("title")?,
            tier: row.get("tier")?,
            state: row.get("state")?,
            requested_by: row.get("requested_by")?,
            architect_agent_id: row.get("architect_agent_id")?,
            current_revision: row.get("current_revision")?,
            approved_revision: row.get("approved_revision")?,
            supersedes_plan_id: row.get("supersedes_plan_id")?,
            cancelled_at: row.get("cancelled_at")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentFindingRow {
    pub finding_id: String,
    pub sequence: i64,
    pub target_agent_id: String,
    pub raised_by_agent_id: String,
    pub raised_by_actor_id: String,
    pub severity: String,
    pub evidence_text: String,
    pub requested_correction: String,
    pub resolution_condition: String,
    pub state: String,
    pub interventions: i64,
    pub state_reason: Option<String>,
    pub created_at: String,
    pub closed_at: Option<String>,
}

impl AgentFindingRow {
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            finding_id: row.get("finding_id")?,
            sequence: row.get("sequence")?,
            target_agent_id: row.get("target_agent_id")?,
            raised_by_agent_id: row.get("raised_by_agent_id")?,
            raised_by_actor_id: row.get("raised_by_actor_id")?,
            severity: row.get("severity")?,
            evidence_text: row.get("evidence_text")?,
            requested_correction: row.get("requested_correction")?,
            resolution_condition: row.get("resolution_condition")?,
            state: row.get("state")?,
            interventions: row.get("interventions")?,
            state_reason: row.get("state_reason")?,
            created_at: row.get("created_at")?,
            closed_at: row.get("closed_at")?,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentRow {
    pub agent_id: String,
    pub role_name: String,
    pub kind: String,
    pub seat_id: String,
    pub actor_id: String,
    pub generation: i64,
    pub state: String,
    pub last_activity_at: String,
}

impl AgentRow {
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            agent_id: row.get("agent_id")?,
            role_name: row.get("role_name")?,
            kind: row.get("kind")?,
            seat_id: row.get("seat_id")?,
            actor_id: row.get("actor_id")?,
            generation: row.get("generation")?,
            state: row.get("state")?,
            last_activity_at: row.get("last_activity_at")?,
        })
    }

    pub fn record(&self) -> AgentRecord {
        AgentRecord {
            agent_id: self.agent_id.clone(),
            role_name: self.role_name.clone(),
            kind: self.kind.clone(),
            seat_id: self.seat_id.clone(),
            actor_id: self.actor_id.clone(),
            generation: self.generation,
            state: self.state.clone(),
            last_activity_at: self.last_activity_at.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MessageRecord {
    pub message_id: String,
    pub sequence: i64,
    pub recipient_agent_id: String,
    pub recipient_generation: i64,
    pub sender_actor_id: String,
    pub body: String,
    pub state: String,
    pub state_version: i64,
    pub queued_at: String,
    pub deferred_at: Option<String>,
    pub deferred_reason: Option<String>,
    pub sent_at: Option<String>,
    pub acked_at: Option<String>,
    pub send_attempts: i64,
    pub state_reason: Option<String>,
    pub notified_at: Option<String>,
    pub last_notified_at: Option<String>,
    pub action_needed: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MessageRow {
    pub message_id: String,
    pub sequence: i64,
    pub recipient_agent_id: String,
    pub recipient_generation: i64,
    pub sender_actor_id: String,
    pub body: String,
    pub state: String,
    pub state_version: i64,
    pub queued_at: String,
    pub deferred_at: Option<String>,
    pub deferred_reason: Option<String>,
    pub deferral_count: i64,
    pub sent_at: Option<String>,
    pub acked_at: Option<String>,
    pub send_attempts: i64,
    pub state_reason: Option<String>,
    pub notified_at: Option<String>,
    pub last_notified_at: Option<String>,
    pub action_needed: i64,
}

impl MessageRow {
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            message_id: row.get("message_id")?,
            sequence: row.get("sequence")?,
            recipient_agent_id: row.get("recipient_agent_id")?,
            recipient_generation: row.get("recipient_generation")?,
            sender_actor_id: row.get("sender_actor_id")?,
            body: row.get("body")?,
            state: row.get("state")?,
            state_version: row.get("state_version")?,
            queued_at: row.get("queued_at")?,
            deferred_at: row.get("deferred_at")?,
            deferred_reason: row.get("deferred_reason")?,
            deferral_count: row.get("deferral_count")?,
            sent_at: row.get("sent_at")?,
            acked_at: row.get("acked_at")?,
            send_attempts: row.get("send_attempts")?,
            state_reason: row.get("state_reason")?,
            notified_at: row.get("notified_at")?,
            last_notified_at: row.get("last_notified_at")?,
            action_needed: row.get("action_needed")?,
        })
    }
}

/// `MessageRejection`: what a message refusal looks like inside a result.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MessageRejection {
    pub rejected: bool,
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct MutationEvent {
    pub entity_type: String,
    pub entity_id: String,
    pub state_version: i64,
    pub from_state: Option<String>,
    pub to_state: Option<String>,
    pub details: Option<Value>,
}

impl MutationEvent {
    pub fn new(entity_type: &str, entity_id: &str, state_version: i64) -> Self {
        Self {
            entity_type: entity_type.into(),
            entity_id: entity_id.into(),
            state_version,
            from_state: None,
            to_state: None,
            details: None,
        }
    }

    pub fn transition(mut self, from: &str, to: &str) -> Self {
        self.from_state = Some(from.into());
        self.to_state = Some(to.into());
        self
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }
}

/// What the `apply` step of a mutation returns: the result value and the event (or events) to ledger.
pub struct MutationOutput {
    pub value: Value,
    pub event: MutationEvent,
    /// Further events of the same mutation, written after `event` with the same state version.
    pub extra_events: Vec<MutationEvent>,
}

impl MutationOutput {
    pub fn new(value: Value, event: MutationEvent) -> Self {
        Self {
            value,
            event,
            extra_events: Vec::new(),
        }
    }
}
