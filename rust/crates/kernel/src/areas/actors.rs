//! The actors, seats and role definitions, and the agent facts that hang off them: src/controller/actors.ts.
//!
//! Filled in by the kernel foundation. The functions that read or write messages, notices or pauses (`record_agent_lost`,
//! `agent_seed`) call into those areas and stay `Unported` until the messages package fills them.

#![allow(unused_imports)]

use crate::auth::{
    authenticate_actor, credential_hash, issue_credential, require_capability, AuthenticatedActor,
};
use crate::canonical::sha256;
use crate::dispatch::{arg, call, Args};
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{
    execute, exists, is_lower_name, is_sha256_hex, lost_notice, query_all, query_opt, safe_id_str,
};
use crate::kernel::Kernel;
use crate::plan_body::package_of_body;
use crate::records::{
    is_final_state, AgentFindingRow, AgentReportRow, AgentRow, MutationEvent, MutationOutput,
    LOST_EVENT_IDS, SEED_FINDINGS, SEED_MESSAGES, SEED_REPORTS,
};
use crate::types::{
    MutationContext, RoleDefinition, RoleDefinitionInput, RoleSyncResult, WORKER_KINDS,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

use super::{message_notices, messages, pauses};

fn text_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// `createActor`.
pub fn create_actor(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let display_name = text_field(input, "displayName").unwrap_or("");
    let role = text_field(input, "role").unwrap_or("");
    let seat_id = text_field(input, "seatId").filter(|s| !s.is_empty());
    let payload = json!({
        "displayName": input.get("displayName").cloned().unwrap_or(Value::Null),
        "role": input.get("role").cloned().unwrap_or(Value::Null),
        "seatId": input.get("seatId").cloned().unwrap_or(Value::Null),
    });
    kernel.mutate(context, "actor.create", "actor:manage", &payload, |actor| {
        let actor_id = kernel.env.uuid();
        let credential = issue_credential(kernel.env.as_ref());
        let hash = credential_hash(&credential)?;
        let mut stored_seat: Option<&str> = None;
        if WORKER_KINDS.contains(&role) {
            let Some(seat) = seat_id else {
                return Err(KernelError::controller("role-scoped actors require a seat"));
            };
            let found = query_opt(
                &kernel.database,
                "SELECT role, state FROM seats WHERE project_id = ? AND seat_id = ?",
                [&kernel.project_id, seat],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?;
            if !matches!(&found, Some((r, s)) if r == role && s == "active") {
                return Err(KernelError::controller("actor seat must be active and match its role"));
            }
            stored_seat = Some(seat);
            if exists(
                &kernel.database,
                "SELECT 1 AS present FROM actors WHERE project_id = ? AND seat_id = ? AND active = 1 AND revoked_at IS NULL LIMIT 1",
                [&kernel.project_id, seat],
            )? {
                return Err(KernelError::conflict(
                    "seat already has an active actor; revoke it before issuing a replacement",
                ));
            }
        } else if seat_id.is_some() {
            return Err(KernelError::controller(
                "operator and controller actors cannot be attached to a worker seat",
            ));
        }
        let now = kernel.now();
        insert_actor(kernel, &actor.actor_id, &actor_id, display_name, role, stored_seat, &hash, &now)?;
        Ok(MutationOutput::new(
            json!({"actorId": actor_id, "credential": credential}),
            MutationEvent::new("actor", &actor_id, 0),
        ))
    })
}

/// `revokeActor`.
pub fn revoke_actor(
    kernel: &Kernel,
    context: &MutationContext,
    actor_id: &str,
) -> KernelResult<Value> {
    kernel.mutate(context, "actor.revoke", "actor:manage", &json!({"actorId": actor_id}), |_| {
        let target = query_opt(
            &kernel.database,
            "SELECT role, is_internal, active, seat_id FROM actors WHERE project_id = ? AND actor_id = ?",
            [&kernel.project_id, actor_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )?;
        let Some((role, is_internal, active, seat_id)) = target.filter(|t| t.1 == 0 && t.0 != "operator") else {
            return Err(KernelError::controller(
                "the internal controller and project operator cannot be revoked through this operation",
            ));
        };
        let _ = (role, is_internal);
        if active == 0 {
            return Ok(MutationOutput::new(
                json!({"revoked": true}),
                MutationEvent::new("actor", actor_id, 0),
            ));
        }
        if let Some(seat) = seat_id.as_deref().filter(|s| !s.is_empty()) {
            let other_active = exists(
                &kernel.database,
                "SELECT 1 AS present FROM actors
                 WHERE project_id = ? AND seat_id = ? AND actor_id <> ?
                   AND active = 1 AND revoked_at IS NULL LIMIT 1",
                [&kernel.project_id, seat, actor_id],
            )?;
            let active_authority = exists(
                &kernel.database,
                "SELECT 1 AS present FROM assignments
                 WHERE project_id = ? AND seat_id = ?
                   AND authority_state IN ('active', 'unknown') LIMIT 1",
                [&kernel.project_id, seat],
            )?;
            if !other_active && active_authority {
                return Err(KernelError::conflict(
                    "cannot revoke the last active actor for a seat with active or uncertain assignments",
                ));
            }
        }
        let now = kernel.now();
        execute(
            &kernel.database,
            "UPDATE actors SET active = 0, revoked_at = ? WHERE project_id = ? AND actor_id = ?",
            [&now, &kernel.project_id, actor_id],
        )?;
        execute(
            &kernel.database,
            "UPDATE capability_grants SET revoked_at = ? WHERE project_id = ? AND actor_id = ? AND revoked_at IS NULL",
            [&now, &kernel.project_id, actor_id],
        )?;
        Ok(MutationOutput::new(
            json!({"revoked": true}),
            MutationEvent::new("actor", actor_id, 1).transition("active", "revoked"),
        ))
    })
}

/// `createSeat`.
pub fn create_seat(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    kernel.mutate(context, "seat.create", "work:write", input, |_| {
        let seat_id = text_field(input, "seatId");
        execute(
            &kernel.database,
            "INSERT INTO seats(project_id, seat_id, name, role, state, state_version, created_at)
             VALUES (?, ?, ?, ?, 'active', 0, ?)",
            rusqlite::params![
                kernel.project_id,
                seat_id,
                text_field(input, "name"),
                text_field(input, "role"),
                kernel.now()
            ],
        )?;
        let seat_id = seat_id.unwrap_or("");
        Ok(MutationOutput::new(
            json!({"seatId": seat_id}),
            MutationEvent::new("seat", seat_id, 0),
        ))
    })
}

fn stored_role_definitions(kernel: &Kernel) -> KernelResult<Vec<RoleDefinition>> {
    kernel.assert_open()?;
    query_all(
        &kernel.database,
        "SELECT role_name, kind, host, config_hash, state FROM role_definitions WHERE project_id = ? ORDER BY role_name",
        [&kernel.project_id],
        |row| {
            Ok(RoleDefinition {
                name: row.get(0)?,
                kind: row.get(1)?,
                host: row.get(2)?,
                config_hash: row.get(3)?,
                state: row.get(4)?,
            })
        },
    )
}

/// `roleDefinitions`.
pub fn role_definitions(kernel: &Kernel) -> KernelResult<Value> {
    Ok(serde_json::to_value(stored_role_definitions(kernel)?)?)
}

/// `roleKind`.
pub fn role_kind(kernel: &Kernel, role_name: &str) -> KernelResult<Value> {
    let definition = stored_role_definitions(kernel)?
        .into_iter()
        .find(|candidate| candidate.name == role_name);
    match definition {
        Some(d) if d.state == "active" => Ok(Value::String(d.kind)),
        _ => Err(KernelError::controller(
            "role is not an active configured role definition",
        )),
    }
}

fn role_input(role: &Value) -> KernelResult<RoleDefinitionInput> {
    let ok = (|| {
        let name = role.get("name")?.as_str()?;
        let kind = role.get("kind")?.as_str()?;
        let host = role.get("host")?.as_str()?;
        let config_hash = role.get("configHash")?.as_str()?;
        (is_lower_name(name)
            && WORKER_KINDS.contains(&kind)
            && is_lower_name(host)
            && is_sha256_hex(config_hash))
        .then(|| RoleDefinitionInput {
            name: name.into(),
            kind: kind.into(),
            host: host.into(),
            config_hash: config_hash.into(),
        })
    })();
    ok.ok_or_else(|| {
        KernelError::type_error(
            "role definition needs a lowercase name and host, a role kind and a SHA-256 hex config hash",
        )
    })
}

/// `syncRoleDefinitions`.
pub fn sync_role_definitions(
    kernel: &Kernel,
    context: &MutationContext,
    desired: &Value,
) -> KernelResult<Value> {
    let mut sorted = desired
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .map(role_input)
        .collect::<KernelResult<Vec<_>>>()?;
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    if sorted
        .iter()
        .map(|r| &r.name)
        .collect::<BTreeSet<_>>()
        .len()
        != sorted.len()
    {
        return Err(KernelError::type_error("role names must be unique"));
    }
    kernel.assert_open()?;
    kernel.assert_writable()?;
    require_capability(
        &authenticate_actor(&kernel.database, &kernel.project_id, &context.credential)?,
        "actor:manage",
    )?;
    let is_replay = kernel.has_stored_request(context)?;
    let planned = if is_replay {
        None
    } else {
        Some(role_difference(kernel, &sorted)?)
    };
    if let Some(difference) = planned.as_ref().filter(|d| !d.changed) {
        return Ok(serde_json::to_value(difference)?);
    }
    // Every write to role_definitions and seats must bump projects.state_version: the caller creates the context
    // before this pre-check, so a matching expected version proves the rows read above are still current.
    let payload = json!({"roles": sorted});
    kernel.mutate(context, "role.sync", "actor:manage", &payload, |_| {
        let difference = match planned {
            Some(difference) => difference,
            None => role_difference(kernel, &sorted)?,
        };
        let now = kernel.now();
        let by_name: BTreeMap<&str, &RoleDefinitionInput> = sorted.iter().map(|r| (r.name.as_str(), r)).collect();
        for name in &difference.inserted {
            let role = by_name[name.as_str()];
            execute(
                &kernel.database,
                "INSERT INTO role_definitions(project_id, role_name, kind, host, config_hash, state, created_at, updated_at) VALUES (?, ?, ?, ?, ?, 'active', ?, ?)",
                rusqlite::params![kernel.project_id, name, role.kind, role.host, role.config_hash, now, now],
            )?;
        }
        for name in difference.updated.iter().chain(difference.reactivated.iter()) {
            let role = by_name[name.as_str()];
            execute(
                &kernel.database,
                "UPDATE role_definitions SET kind = ?, host = ?, config_hash = ?, state = 'active', updated_at = ? WHERE project_id = ? AND role_name = ?",
                rusqlite::params![role.kind, role.host, role.config_hash, now, kernel.project_id, name],
            )?;
        }
        for name in &difference.retired {
            execute(
                &kernel.database,
                "UPDATE role_definitions SET state = 'retired', updated_at = ? WHERE project_id = ? AND role_name = ?",
                rusqlite::params![now, kernel.project_id, name],
            )?;
        }
        let value = serde_json::to_value(&difference)?;
        Ok(MutationOutput::new(
            value.clone(),
            MutationEvent::new("role_definition", &kernel.project_id, 0).with_details(value),
        ))
    })
}

fn role_difference(
    kernel: &Kernel,
    desired: &[RoleDefinitionInput],
) -> KernelResult<RoleSyncResult> {
    let stored: BTreeMap<String, RoleDefinition> = stored_role_definitions(kernel)?
        .into_iter()
        .map(|d| (d.name.clone(), d))
        .collect();
    let mut seat_roles: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (name, role) in query_all(
        &kernel.database,
        "SELECT name, role FROM seats WHERE project_id = ?",
        [&kernel.project_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )? {
        seat_roles.entry(name).or_default().insert(role);
    }
    let (mut inserted, mut updated, mut reactivated, mut retired) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for role in desired {
        if seat_roles
            .get(&role.name)
            .is_some_and(|kinds| kinds.iter().any(|kind| *kind != role.kind))
        {
            return Err(KernelError::conflict(
                "a seat named after the role has a different kind than the role",
            ));
        }
        let Some(current) = stored.get(&role.name) else {
            inserted.push(role.name.clone());
            continue;
        };
        let differs = current.kind != role.kind
            || current.host != role.host
            || current.config_hash != role.config_hash;
        if current.state == "retired" {
            reactivated.push(role.name.clone());
        } else if differs {
            updated.push(role.name.clone());
        }
    }
    let wanted: BTreeSet<&str> = desired.iter().map(|r| r.name.as_str()).collect();
    for definition in stored.values() {
        if definition.state == "active" && !wanted.contains(definition.name.as_str()) {
            retired.push(definition.name.clone());
        }
    }
    Ok(RoleSyncResult {
        changed: inserted.len() + updated.len() + reactivated.len() + retired.len() > 0,
        inserted,
        updated,
        reactivated,
        retired,
    })
}

fn lost_recorded(kernel: &Kernel, agent_id: &str, generation: i64) -> KernelResult<bool> {
    exists(
        &kernel.database,
        "SELECT 1 AS present FROM controller_events WHERE project_id = ? AND entity_type = 'agent' AND entity_id = ?
           AND to_state = 'lost' AND json_extract(payload_json, '$.details.generation') = ?",
        rusqlite::params![kernel.project_id, agent_id, generation],
    )
}

/// Records that an agent is lost (once per agent and generation, whatever the reason) and tells the PM when exactly
/// one is active. The caller owns the transaction. Returns whether it recorded.
#[allow(clippy::too_many_arguments)]
pub fn record_lost(
    kernel: &Kernel,
    actor: &AuthenticatedActor,
    context: &MutationContext,
    agent: &AgentRow,
    reason: &str,
    unacknowledged_message_ids: &[String],
    now: &str,
    known_branch: Option<Option<&str>>,
) -> KernelResult<bool> {
    if lost_recorded(kernel, &agent.agent_id, agent.generation)? {
        return Ok(false);
    }
    let branch: Option<String> = match known_branch {
        Some(branch) => branch.map(str::to_string),
        None => query_opt(
            &kernel.database,
            "SELECT branch FROM agent_panes WHERE project_id = ? AND agent_id = ?",
            [&kernel.project_id, &agent.agent_id],
            |row| row.get::<_, Option<String>>(0),
        )?
        .flatten(),
    };
    let named: Vec<&String> = unacknowledged_message_ids
        .iter()
        .take(LOST_EVENT_IDS)
        .collect();
    kernel.append_event(
        actor,
        context,
        "agent",
        &agent.agent_id,
        "active",
        "lost",
        "agent.lost",
        0,
        &json!({
            "generation": agent.generation,
            "reason": reason,
            "branch": branch,
            "unacknowledgedMessageIds": named,
            "unacknowledgedCount": unacknowledged_message_ids.len(),
        }),
    )?;
    if let Some(parties) = message_notices::notice_parties(kernel)? {
        if parties.pm.agent_id != agent.agent_id {
            let paused = pauses::is_delivery_paused(kernel, &agent.agent_id)?
                .as_bool()
                .unwrap_or(false);
            let body = lost_notice(
                agent,
                reason,
                branch.as_deref(),
                unacknowledged_message_ids,
                paused,
            );
            messages::insert_queued_message(
                kernel,
                &parties.controller_actor_id,
                &parties.pm,
                &body,
                &sha256(&body),
                now,
                true,
            )?;
        }
    }
    Ok(true)
}

/// `recordAgentLost`: an active agent whose pane is gone. Written once per agent and generation; the agent stays
/// active, nothing is cancelled and nothing is replaced.
pub fn record_agent_lost(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let agent_id = safe_id_str(text_field(input, "agentId").unwrap_or(""), "agent id")?;
    kernel.mutate(
        context,
        "agent.lost",
        "controller:reconcile",
        &json!({"agentId": agent_id}),
        |actor| {
            let agent = kernel.agent_row(agent_id)?.filter(|a| a.state == "active");
            let Some(agent) = agent else {
                return Err(KernelError::controller("agent is not active"));
            };
            let now = kernel.now();
            let unacknowledged: Vec<String> = messages::message_rows_for(kernel, agent_id)?
                .into_iter()
                .filter(|row| !is_final_state(&row.state))
                .map(|row| row.message_id)
                .collect();
            let recorded = record_lost(
                kernel,
                actor,
                context,
                &agent,
                "pane_gone",
                &unacknowledged,
                &now,
                None,
            )?;
            Ok(MutationOutput::new(
                json!({"recorded": recorded}),
                MutationEvent::new("agent", agent_id, agent.generation)
                    .with_details(json!({"recorded": recorded})),
            ))
        },
    )
}

/// `isAgentReplaced`: whether the ledger holds a replacement of this agent.
pub fn is_agent_replaced(kernel: &Kernel, agent_id: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    safe_id_str(agent_id, "agent id")?;
    Ok(Value::Bool(exists(
        &kernel.database,
        "SELECT 1 AS present FROM controller_events WHERE project_id = ? AND entity_type = 'agent' AND entity_id = ? AND to_state = 'replaced'",
        [&kernel.project_id, agent_id],
    )?))
}

/// `recordAgentReplaced`: records that a started agent replaces an earlier one.
pub fn record_agent_replaced(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let predecessor_id = safe_id_str(
        text_field(input, "predecessorId").unwrap_or(""),
        "predecessor agent id",
    )?;
    let successor_id = safe_id_str(
        text_field(input, "successorId").unwrap_or(""),
        "successor agent id",
    )?;
    kernel.mutate(context, "agent.replaced", "controller:reconcile", input, |_| {
        let predecessor = kernel.agent_row(predecessor_id)?;
        let successor = kernel.agent_row(successor_id)?;
        let (Some(predecessor), Some(successor)) = (predecessor, successor) else {
            return Err(KernelError::controller("the predecessor and the successor must exist"));
        };
        if predecessor.kind == "PM" {
            return Err(KernelError::controller("a PM is restarted, not replaced"));
        }
        if predecessor.agent_id == successor.agent_id
            || predecessor.role_name != successor.role_name
            || successor.state != "active"
        {
            return Err(KernelError::controller(
                "the successor must be another active agent of the same role",
            ));
        }
        if is_agent_replaced(kernel, predecessor_id)?.as_bool() == Some(true) {
            return Err(KernelError::controller("the agent was already replaced"));
        }
        let rebound = execute(
            &kernel.database,
            "UPDATE plan_packages SET assignee_agent_id = ? WHERE project_id = ? AND assignee_agent_id = ? AND cancelled_at IS NULL",
            [successor_id, &kernel.project_id, predecessor_id],
        )?;
        execute(
            &kernel.database,
            "UPDATE external_links SET bound_agent_id = ? WHERE project_id = ? AND bound_agent_id = ?",
            [successor_id, &kernel.project_id, predecessor_id],
        )?;
        let mut details = json!({"successorId": successor_id, "packagesRebound": rebound});
        if let Some(branch) = input.get("branch").filter(|v| !v.is_null()) {
            details["branch"] = branch.clone();
        }
        if let Some(kept_ref) = input.get("keptRef").filter(|v| !v.is_null()) {
            details["keptRef"] = kept_ref.clone();
            details["keptTip"] = input.get("keptTip").filter(|v| !v.is_null()).cloned().unwrap_or(Value::Null);
        }
        Ok(MutationOutput::new(
            json!({"recorded": true}),
            MutationEvent::new("agent", predecessor_id, predecessor.generation)
                .transition(&predecessor.state, "replaced")
                .with_details(details),
        ))
    })
}

/// The branch an agent had when it ended, from the end event.
fn branch_at_end(kernel: &Kernel, agent_id: &str) -> KernelResult<Option<String>> {
    Ok(query_opt(
        &kernel.database,
        "SELECT json_extract(payload_json, '$.details.branch') AS branch FROM controller_events
         WHERE project_id = ? AND entity_type = 'agent' AND entity_id = ? AND to_state = 'ended' ORDER BY sequence DESC LIMIT 1",
        [&kernel.project_id, agent_id],
        |row| row.get::<_, Option<String>>(0),
    )?
    .flatten())
}

/// `agentSeed`: what a replacement of this agent is seeded with, newest entries only, in ledger order. A read; works
/// for an ended agent.
pub fn agent_seed(kernel: &Kernel, agent_id: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    safe_id_str(agent_id, "agent id")?;
    let Some(agent) = kernel.agent_row(agent_id)? else {
        return Err(KernelError::controller("the agent does not exist"));
    };
    let all_messages = messages::message_rows_for(kernel, agent_id)?;
    let shown = &all_messages[all_messages.len().saturating_sub(SEED_MESSAGES)..];
    let reports = query_all(
        &kernel.database,
        "SELECT * FROM agent_reports WHERE project_id = ? AND agent_id = ? AND state = 'accepted' ORDER BY sequence",
        [&kernel.project_id, agent_id],
        AgentReportRow::from_row,
    )?;
    let open_findings = query_all(
        &kernel.database,
        "SELECT * FROM agent_findings WHERE project_id = ? AND target_agent_id = ? AND state = 'open' ORDER BY sequence",
        [&kernel.project_id, agent_id],
        AgentFindingRow::from_row,
    )?;
    let findings = &open_findings[open_findings.len().saturating_sub(SEED_FINDINGS)..];
    let pane = query_opt(
        &kernel.database,
        "SELECT branch, base_sha, task_ref, task_title FROM agent_panes WHERE project_id = ? AND agent_id = ?",
        [&kernel.project_id, agent_id],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        },
    )?;
    let last = reports.last();
    let branch = match pane.as_ref().and_then(|p| p.0.clone()) {
        Some(branch) => Some(branch),
        None => match last.and_then(|r| r.branch.clone()) {
            Some(branch) => Some(branch),
            None => branch_at_end(kernel, agent_id)?,
        },
    };
    let mut message_values = Vec::new();
    for row in shown {
        let sender = messages::sender_of(kernel, &row.sender_actor_id)?;
        let who = sender
            .get("agentId")
            .filter(|v| !v.is_null())
            .or_else(|| sender.get("role"))
            .cloned()
            .unwrap_or(Value::Null);
        message_values.push(json!({
            "messageId": row.message_id,
            "sender": who,
            "state": row.state,
            "stateReason": row.state_reason,
            "body": row.body,
        }));
    }
    let package_rows = query_all(
        &kernel.database,
        "SELECT p.plan_id, p.package_id, pl.architect_agent_id, r.body_json FROM plan_packages p
         JOIN plans pl ON pl.project_id = p.project_id AND pl.plan_id = p.plan_id
         LEFT JOIN plan_revisions r ON r.project_id = pl.project_id AND r.plan_id = pl.plan_id AND r.revision = pl.approved_revision
         WHERE p.project_id = ? AND p.assignee_agent_id = ? AND p.cancelled_at IS NULL ORDER BY pl.sequence, p.package_id",
        [&kernel.project_id, agent_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        },
    )?;
    let packages: Vec<Value> = package_rows
        .iter()
        .map(|(plan_id, package_id, architect, body)| {
            let view = body
                .as_deref()
                .and_then(|b| package_of_body(b, package_id))
                .map_or(Value::Null, |v| v.to_value());
            json!({"planId": plan_id, "packageId": package_id, "architectAgentId": architect, "view": view})
        })
        .collect();
    Ok(json!({
        "agentId": agent_id,
        "roleName": agent.role_name,
        "kind": agent.kind,
        "state": agent.state,
        "generation": agent.generation,
        "branch": branch,
        "baseSha": pane.as_ref().and_then(|p| p.1.clone()),
        "taskRef": pane.as_ref().and_then(|p| p.2.clone()),
        "taskTitle": pane.as_ref().and_then(|p| p.3.clone()),
        "messages": message_values,
        "messagesOmitted": all_messages.len() - shown.len(),
        "reports": reports[reports.len().saturating_sub(SEED_REPORTS)..].iter().map(|r| json!({
            "reportId": r.report_id,
            "commitSha": r.commit_sha,
            "branch": r.branch,
            "summary": r.summary,
        })).collect::<Vec<_>>(),
        "reportsOmitted": reports.len().saturating_sub(SEED_REPORTS),
        "lastAcceptedCommit": last.map(|r| r.commit_sha.clone()),
        "findingsOmitted": open_findings.len() - findings.len(),
        "findings": findings.iter().map(|f| json!({
            "findingId": f.finding_id,
            "severity": f.severity,
            "requestedCorrection": f.requested_correction,
            "interventions": f.interventions,
        })).collect::<Vec<_>>(),
        "packages": packages,
    }))
}

/// `seatActorIds`: the active actors of a seat, so a crash between creating an actor and registering its agent can be undone.
pub fn seat_actor_ids(kernel: &Kernel, credential: &str, seat_id: &str) -> KernelResult<Value> {
    kernel.authorize(credential, "controller:reconcile")?;
    safe_id_str(seat_id, "seat id")?;
    let ids = query_all(
        &kernel.database,
        "SELECT actor_id FROM actors WHERE project_id = ? AND seat_id = ? AND active = 1 AND revoked_at IS NULL ORDER BY actor_id",
        [&kernel.project_id, seat_id],
        |row| row.get::<_, String>(0),
    )?;
    Ok(json!(ids))
}

pub fn assert_seat_free_of_authority(kernel: &Kernel, seat_id: &str) -> KernelResult<()> {
    if exists(
        &kernel.database,
        "SELECT 1 AS present FROM assignments WHERE project_id = ? AND seat_id = ? AND authority_state IN ('active', 'unknown') LIMIT 1",
        [&kernel.project_id, seat_id],
    )? {
        return Err(KernelError::conflict(
            "the agent seat has an assignment with active or uncertain authority; end or revoke it first",
        ));
    }
    Ok(())
}

pub fn revoke_seat_actors(kernel: &Kernel, seat_id: &str, now: &str) -> KernelResult<()> {
    let actors = query_all(
        &kernel.database,
        "SELECT actor_id FROM actors WHERE project_id = ? AND seat_id = ? AND active = 1 AND revoked_at IS NULL",
        [&kernel.project_id, seat_id],
        |row| row.get::<_, String>(0),
    )?;
    for actor_id in actors {
        execute(
            &kernel.database,
            "UPDATE actors SET active = 0, revoked_at = ? WHERE project_id = ? AND actor_id = ?",
            [now, &kernel.project_id, &actor_id],
        )?;
        execute(
            &kernel.database,
            "UPDATE capability_grants SET revoked_at = ? WHERE project_id = ? AND actor_id = ? AND revoked_at IS NULL",
            [now, &kernel.project_id, &actor_id],
        )?;
    }
    Ok(())
}

/// Inserts an actor with the capabilities of its role.
#[allow(clippy::too_many_arguments)]
pub fn insert_actor(
    kernel: &Kernel,
    granted_by: &str,
    actor_id: &str,
    display_name: &str,
    role: &str,
    seat_id: Option<&str>,
    credential_hash: &str,
    now: &str,
) -> KernelResult<()> {
    execute(
        &kernel.database,
        "INSERT INTO actors(actor_id, project_id, display_name, role, seat_id, credential_hash, active, is_internal, created_at)
         VALUES (?, ?, ?, ?, ?, ?, 1, 0, ?)",
        rusqlite::params![actor_id, kernel.project_id, display_name, role, seat_id, credential_hash, now],
    )?;
    execute(
        &kernel.database,
        "INSERT INTO capability_grants(project_id, actor_id, capability, granted_by, granted_at) SELECT ?, ?, capability, ?, ? FROM role_capabilities WHERE role = ?",
        rusqlite::params![kernel.project_id, actor_id, granted_by, now, role],
    )?;
    Ok(())
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "createActor" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(create_actor(kernel, &context, input)))
        }
        "revokeActor" => {
            let context = arg!(a.ctx(0));
            let actor_id = arg!(a.str(1, "actorId"));
            Some(call(revoke_actor(kernel, &context, actor_id)))
        }
        "createSeat" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(create_seat(kernel, &context, input)))
        }
        "roleDefinitions" => Some(call(role_definitions(kernel))),
        "roleKind" => {
            let role_name = arg!(a.str(0, "roleName"));
            Some(call(role_kind(kernel, role_name)))
        }
        "syncRoleDefinitions" => {
            let context = arg!(a.ctx(0));
            let desired = a.value(1);
            Some(call(sync_role_definitions(kernel, &context, desired)))
        }
        "recordAgentLost" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(record_agent_lost(kernel, &context, input)))
        }
        "isAgentReplaced" => {
            let agent_id = arg!(a.str(0, "agentId"));
            Some(call(is_agent_replaced(kernel, agent_id)))
        }
        "recordAgentReplaced" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(record_agent_replaced(kernel, &context, input)))
        }
        "agentSeed" => {
            let agent_id = arg!(a.str(0, "agentId"));
            Some(call(agent_seed(kernel, agent_id)))
        }
        "seatActorIds" => {
            let credential = arg!(a.str(0, "credential"));
            let seat_id = arg!(a.str(1, "seatId"));
            Some(call(seat_actor_ids(kernel, credential, seat_id)))
        }
        _ => None,
    }
}
