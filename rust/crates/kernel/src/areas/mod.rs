//! One file per area of the controller; each exposes its functions and a `dispatch` by operation name.

pub mod actors;
pub mod agents;
pub mod findings;
pub mod integrations;
pub mod links;
pub mod message_notices;
pub mod messages;
pub mod messaging;
pub mod operator_grants;
pub mod operator_proposals;
pub mod operator_runs;
pub mod panes;
pub mod pauses;
pub mod plan_packages;
pub mod plans;
pub mod prompt_relay;
pub mod reconcile;
pub mod reports;
pub mod reviews;
pub mod status;
