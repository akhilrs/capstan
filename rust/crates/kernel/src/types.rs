//! The vocabulary of the controller: src/controller/types.ts (the shapes the foundation and the actors area use; an
//! area that owns a record type defines it in its own file).

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const ROLES: [&str; 6] = [
    "operator",
    "controller",
    "PM",
    "Developer",
    "Verifier",
    "Supervisor",
];

/// The kinds a seat or agent can have: every role except `operator` and `controller`.
pub const WORKER_KINDS: [&str; 4] = ["PM", "Developer", "Verifier", "Supervisor"];

pub const INPUT_KINDS: [&str; 5] = [
    "project_config",
    "task_brief",
    "acceptance_criteria",
    "policy",
    "plan",
];

pub const CAPABILITIES: [&str; 27] = [
    "project:inputs:write",
    "work:write",
    "work:assign",
    "work:report",
    "candidate:verify",
    "candidate:accept",
    "finding:write",
    "run:control",
    "usage:write",
    "actor:manage",
    "message:send",
    "message:receive",
    "message:resolve",
    "report:submit",
    "review:request",
    "review:submit",
    "plan:write",
    "plan:read",
    "operator:propose",
    "operator:decide",
    "operator:read",
    "prompt:relay",
    "finding:raise",
    "finding:check",
    "agent:observe",
    "recovery:write",
    "controller:reconcile",
];

/// `MutationContext`: the credential and the request identity of one mutation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MutationContext {
    pub credential: String,
    pub request_id: String,
    pub idempotency_key: String,
    pub expected_version: i64,
    pub input_revision: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectInput {
    pub kind: String,
    pub content: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitialProject {
    pub project_id: String,
    pub name: String,
    pub owner_credential: String,
    pub initial_inputs: Vec<ProjectInput>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatInput {
    pub seat_id: String,
    pub name: String,
    pub role: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RoleDefinitionInput {
    pub name: String,
    pub kind: String,
    pub host: String,
    pub config_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RoleDefinition {
    pub name: String,
    pub kind: String,
    pub host: String,
    pub config_hash: String,
    pub state: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RoleSyncResult {
    pub changed: bool,
    pub inserted: Vec<String>,
    pub updated: Vec<String>,
    pub reactivated: Vec<String>,
    pub retired: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ActorInput {
    pub display_name: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentRecord {
    pub agent_id: String,
    pub role_name: String,
    pub kind: String,
    pub seat_id: String,
    pub actor_id: String,
    pub generation: i64,
    pub state: String,
    pub last_activity_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub actor_id: String,
    pub role: String,
    pub capabilities: Vec<String>,
    pub agent: Option<AgentRecord>,
}
