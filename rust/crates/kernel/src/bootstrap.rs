//! First-time setup of a project: src/controller/bootstrap.ts.

use crate::auth::{base64url, credential_hash};
use crate::canonical::{canonical_json, sha256};
use crate::env::Env;
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{acceptance_criteria_from_content, execute, js_trim};
use crate::types::{InitialProject, INPUT_KINDS};
use capstan_ledger::{iso_from_millis, Database};
use serde_json::{json, Value};

pub fn initialize_project(
    database: &Database,
    project: &InitialProject,
    env: &dyn Env,
) -> KernelResult<()> {
    let initial_inputs: Vec<(&str, String)> = project
        .initial_inputs
        .iter()
        .map(|input| (input.kind.as_str(), canonical_json(&input.content)))
        .collect();
    let complete = initial_inputs.len() == INPUT_KINDS.len()
        && INPUT_KINDS
            .iter()
            .all(|kind| initial_inputs.iter().filter(|(k, _)| k == kind).count() == 1);
    if !complete {
        return Err(KernelError::type_error(
            "initial project requires exactly one revision of each durable input kind",
        ));
    }
    if js_trim(&project.name).is_empty() {
        return Err(KernelError::type_error("project name must not be empty"));
    }
    let owner_hash = credential_hash(&project.owner_credential)?;
    let internal_hash = credential_hash(&base64url(&env.random_bytes(32)))?;
    let now = iso_from_millis(env.now());
    let owner_id = env.uuid();
    let internal_id = env.uuid();
    database.exec("BEGIN IMMEDIATE")?;
    let result = (|| -> KernelResult<()> {
        let id = project.project_id.as_str();
        execute(
            database,
            "INSERT INTO projects(project_id, name, current_input_revision, state_version, created_at) VALUES (?, ?, 1, 1, ?)",
            rusqlite::params![id, project.name, now],
        )?;
        execute(
            database,
            "INSERT INTO supervision_control(project_id, enabled, health, target_epoch, updated_at) VALUES (?, 0, 'degraded', 0, ?)",
            rusqlite::params![id, now],
        )?;
        let insert_actor = "INSERT INTO actors(actor_id, project_id, display_name, role, seat_id, credential_hash, active, is_internal, created_at)
             VALUES (?, ?, ?, ?, NULL, ?, 1, ?, ?)";
        execute(
            database,
            insert_actor,
            rusqlite::params![
                owner_id,
                id,
                "project operator",
                "operator",
                owner_hash,
                0,
                now
            ],
        )?;
        execute(
            database,
            insert_actor,
            rusqlite::params![
                internal_id,
                id,
                "capstan internal controller",
                "controller",
                internal_hash,
                1,
                now
            ],
        )?;
        let grant_role = "INSERT INTO capability_grants(project_id, actor_id, capability, granted_by, granted_at)
             SELECT ?, ?, capability, ?, ? FROM role_capabilities WHERE role = ?";
        execute(
            database,
            grant_role,
            rusqlite::params![id, owner_id, owner_id, now, "operator"],
        )?;
        execute(
            database,
            grant_role,
            rusqlite::params![id, internal_id, owner_id, now, "controller"],
        )?;
        for (kind, content) in &initial_inputs {
            if *kind == "acceptance_criteria" {
                let parsed: Value = serde_json::from_str(content)?;
                acceptance_criteria_from_content(&parsed)?;
            }
            execute(
                database,
                "INSERT INTO project_revisions(project_id, revision, kind, content_json, content_hash, created_by, request_id, created_at)
                 VALUES (?, 1, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    id,
                    kind,
                    content,
                    sha256(content),
                    owner_id,
                    format!("bootstrap:{id}"),
                    now
                ],
            )?;
        }
        execute(
            database,
            "INSERT INTO run_controls(project_id, state, state_version, updated_at) VALUES (?, 'active', 0, ?)",
            rusqlite::params![id, now],
        )?;
        execute(
            database,
            "INSERT INTO controller_events(project_id, sequence, event_id, entity_type, entity_id, from_state, to_state,
               state_version, actor_id, request_id, input_revision, payload_json, created_at)
             VALUES (?, 1, ?, 'project', ?, NULL, 'initialized', 1, ?, ?, 1, ?, ?)",
            rusqlite::params![
                id,
                env.uuid(),
                id,
                owner_id,
                format!("bootstrap:{id}"),
                canonical_json(&json!({"inputKinds": INPUT_KINDS})),
                now
            ],
        )?;
        database.exec("COMMIT")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = database.exec("ROLLBACK");
    }
    result
}
