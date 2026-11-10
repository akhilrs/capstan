//! One file per area of the controller; each exposes its functions and a `dispatch` by operation name.

pub mod actors;
pub mod agents;
pub mod approval_windows;
pub mod cleanups;
pub mod design_approvals;
pub mod event_feed;
pub mod feed_cursor;
pub mod findings;
pub mod id_prefix;
pub mod idle_release;
pub mod integrations;
pub mod links;
pub mod message_notices;
pub mod messages;
pub mod messaging;
pub mod model_overrides;
pub mod operator_grants;
pub mod operator_proposals;
pub mod operator_runs;
pub mod panes;
pub mod pauses;
pub mod plan_archive;
pub mod plan_handover;
pub mod plan_landing;
pub mod plan_packages;
pub mod plan_status;
pub mod plans;
pub mod pm_card;
pub mod pm_stuck;
pub mod prompt_relay;
pub mod push_events;
pub mod reconcile;
pub mod remote_decisions;
pub mod reports;
pub mod retention;
pub mod reviews;
pub mod snapshot_feed;
pub mod status;
pub mod user_messages;
pub mod verify_runs;
pub mod warm_reviewers;
pub mod write_once;

// The hot statements, held as constants so the query plan test reads the same text the code runs.
pub use actors::{
    BRANCH_AT_END_SQL, IS_AGENT_REPLACED_SQL, LOST_RECORDED_SQL, RECORDED_SUCCESSOR_SQL,
};
pub use plans::{NOTICE_SENT_SQL, PLAN_ROUNDS_SQL};
pub use reports::{ACCEPTED_REPORT_FOR_SQL, RECORD_AGENT_REPORT_SQL};
pub use reviews::{COMPLETE_REVIEW_SQL, NOTICE_PACKAGE_REVIEWED_SQL};
pub use warm_reviewers::{
    LAST_VERDICT_AT_SQL, REPLACEMENT_FAMILY_ENTITY_SQL, REPLACEMENT_FAMILY_SUCCESSOR_SQL,
};
