//! The agent records and lifecycle (src/controller/agents.ts).
//!
//! Ending, replacing and restarting an agent reach into the notices, pauses, findings, operator, messages, links, plans
//! and integrations areas.

use crate::auth::{
    authenticate_actor, credential_hash, issue_credential, new_actor_id, AuthenticatedActor,
};
use crate::canonical::sha256;
use crate::dispatch::{arg, call, Args};
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{
    cut_at_characters, execute, is_lower_name, objective_of, query_all, query_opt, safe_id,
    safe_id_str,
};
use crate::json::stringify;
use crate::kernel::Kernel;
use crate::records::{
    is_final_state, AgentFindingRow, MutationEvent, MutationOutput, MAX_SUMMARY_BODY,
    MAX_SUMMARY_BYTES, MAX_SUMMARY_LINKS, MAX_SUMMARY_MESSAGES, MAX_SUMMARY_WORK,
    TRUNCATION_MARKER,
};
use crate::types::MutationContext;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

use super::{
    actors, findings, integrations, links, message_notices, messages, operator_grants,
    operator_proposals, pauses, plans, status,
};

const HERDR_STATES: [&str; 5] = ["idle", "working", "blocked", "done", "unknown"];

fn field<'a>(input: &'a Value, key: &str) -> &'a Value {
    input.get(key).unwrap_or(&Value::Null)
}

/// `registerAgent`.
pub fn register_agent(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let agent_id = safe_id(field(input, "agentId"), "agent id")?;
    let seat_id = safe_id(field(input, "seatId"), "seat id")?;
    let actor_id = safe_id(field(input, "actorId"), "actor id")?;
    let role_name = match field(input, "roleName").as_str() {
        Some(name) if is_lower_name(name) => name,
        _ => {
            return Err(KernelError::type_error(
                "role name must be a lowercase configured role name",
            ))
        }
    };
    kernel.mutate(
        context,
        "agent.register",
        "actor:manage",
        &json!({"agentId": agent_id, "roleName": role_name, "seatId": seat_id, "actorId": actor_id}),
        |_| {
            let definitions = actors::role_definitions(kernel)?;
            let definition = definitions
                .as_array()
                .into_iter()
                .flatten()
                .find(|candidate| candidate["name"] == role_name)
                .filter(|candidate| candidate["state"] == "active");
            let Some(definition) = definition else {
                return Err(KernelError::controller(
                    "agent role is not an active configured role definition",
                ));
            };
            let kind = definition["kind"].as_str().unwrap_or("").to_string();
            let seat = query_opt(
                &kernel.database,
                "SELECT role, state FROM seats WHERE project_id = ? AND seat_id = ?",
                [&kernel.project_id, seat_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?;
            if !seat.is_some_and(|(role, state)| state == "active" && role == kind) {
                return Err(KernelError::controller(
                    "agent seat must be active and match the role kind",
                ));
            }
            let actor_row = query_opt(
                &kernel.database,
                "SELECT role, seat_id, active, revoked_at, is_internal FROM actors WHERE project_id = ? AND actor_id = ?",
                [&kernel.project_id, actor_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )?;
            let usable = actor_row.is_some_and(|(role, seat, active, revoked_at, internal)| {
                active == 1
                    && revoked_at.is_none()
                    && internal == 0
                    && role == kind
                    && seat.as_deref() == Some(seat_id)
            });
            if !usable {
                return Err(KernelError::controller(
                    "agent actor must be active, of the role kind and attached to the agent seat",
                ));
            }
            let taken = query_opt(
                &kernel.database,
                "SELECT 1 AS present FROM agents WHERE project_id = ? AND (actor_id = ? OR agent_id = ? OR (seat_id = ? AND state = 'active')) LIMIT 1",
                [&kernel.project_id, actor_id, agent_id, seat_id],
                |_| Ok(()),
            )?;
            if taken.is_some() {
                return Err(KernelError::conflict(
                    "agent id, actor or seat is already bound to an agent",
                ));
            }
            let now = kernel.now();
            execute(
                &kernel.database,
                "INSERT INTO agents(project_id, agent_id, role_name, kind, seat_id, actor_id, generation, state, last_activity_at, created_at) VALUES (?, ?, ?, ?, ?, ?, 1, 'active', ?, ?)",
                rusqlite::params![kernel.project_id, agent_id, role_name, kind, seat_id, actor_id, now, now],
            )?;
            let record = kernel
                .agent_record(agent_id)?
                .ok_or_else(|| KernelError::controller("the registered agent is missing"))?;
            let mut event = MutationEvent::new("agent", agent_id, 1);
            event.to_state = Some("active".into());
            Ok(MutationOutput::new(serde_json::to_value(record)?, event))
        },
    )
}

/// `identify`.
pub fn identify(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    let actor = authenticate_actor(&kernel.database, &kernel.project_id, credential)?;
    Ok(json!({
        "actorId": actor.actor_id,
        "role": actor.role,
        "capabilities": actor.capabilities.iter().cloned().collect::<Vec<_>>(),
        "agent": kernel.agent_by_actor_record(&actor.actor_id)?,
    }))
}

/// `listAgents`.
pub fn list_agents(kernel: &Kernel) -> KernelResult<Value> {
    kernel.assert_open()?;
    Ok(serde_json::to_value(kernel.agent_records(false)?)?)
}

/// `activeAgents`.
pub fn active_agents(kernel: &Kernel) -> KernelResult<Value> {
    kernel.assert_open()?;
    Ok(serde_json::to_value(kernel.agent_records(true)?)?)
}

/// `agentRecord`.
pub fn agent_record(kernel: &Kernel, agent_id: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    Ok(match kernel.agent_record(agent_id)? {
        Some(record) => serde_json::to_value(record)?,
        None => Value::Null,
    })
}

/// `recordAgentObservation`.
pub fn record_agent_observation(
    kernel: &Kernel,
    context: &MutationContext,
    agent_id: &str,
    state: &Value,
) -> KernelResult<Value> {
    safe_id_str(agent_id, "agent id")?;
    let state = match state.as_str() {
        Some(state) if HERDR_STATES.contains(&state) => state,
        _ => return Err(KernelError::type_error("unknown Herdr state")),
    };
    kernel.authorize(&context.credential, "controller:reconcile")?;
    let latest = query_opt(
        &kernel.database,
        "SELECT herdr_state FROM agent_state_history WHERE project_id = ? AND agent_id = ? ORDER BY sequence DESC LIMIT 1",
        [&kernel.project_id, agent_id],
        |row| row.get::<_, String>(0),
    )?;
    if latest.as_deref() == Some(state) && !kernel.has_stored_request(context)? {
        return Ok(json!({"recorded": false}));
    }
    kernel.mutate(
        context,
        "agent.observe",
        "controller:reconcile",
        &json!({"agentId": agent_id, "state": state}),
        |_| {
            if !kernel.agent_row(agent_id)?.is_some_and(|agent| agent.state == "active") {
                return Err(KernelError::controller("agent is not active"));
            }
            let sequence = query_opt(
                &kernel.database,
                "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM agent_state_history WHERE project_id = ? AND agent_id = ?",
                [&kernel.project_id, agent_id],
                |row| row.get::<_, i64>(0),
            )?
            .unwrap_or(1);
            execute(
                &kernel.database,
                "INSERT INTO agent_state_history(project_id, agent_id, sequence, herdr_state, observed_at) SELECT ?, ?, ?, ?, MAX(?, COALESCE((SELECT MAX(observed_at) FROM agent_state_history WHERE project_id = ? AND agent_id = ?), ?))",
                rusqlite::params![
                    kernel.project_id,
                    agent_id,
                    sequence,
                    state,
                    kernel.now(),
                    kernel.project_id,
                    agent_id,
                    ""
                ],
            )?;
            let mut event = MutationEvent::new("agent_observation", agent_id, sequence);
            event.to_state = Some(state.into());
            Ok(MutationOutput::new(json!({"recorded": true}), event))
        },
    )
}

/// `endAgent`: an active agent's seat actors are revoked, its waits closed, a started review cancelled and its messages
/// cancelled except `failed` ones. `options.lost` also records the loss and tells the PM in the same transaction.
pub fn end_agent(
    kernel: &Kernel,
    context: &MutationContext,
    agent_id: &str,
    options: Option<&Value>,
) -> KernelResult<Value> {
    safe_id_str(agent_id, "agent id")?;
    let null = Value::Null;
    let options = options.unwrap_or(&null);
    let lost = options
        .get("lost")
        .filter(|v| !v.is_null())
        .and_then(Value::as_str);
    let given_branch = options
        .get("branch")
        .filter(|v| !v.is_null())
        .and_then(Value::as_str);
    kernel.mutate(
        context,
        "agent.end",
        "actor:manage",
        &json!({"agentId": agent_id, "lost": lost, "branch": given_branch}),
        |actor| {
            let agent = match kernel.agent_row(agent_id)? {
                Some(agent) if agent.state == "active" => agent,
                _ => return Err(KernelError::controller("agent is not active")),
            };
            actors::assert_seat_free_of_authority(kernel, &agent.seat_id)?;
            let now = kernel.now();
            actors::revoke_seat_actors(kernel, &agent.seat_id, &now)?;
            message_notices::close_waits(kernel, agent_id, &now)?;
            execute(
                &kernel.database,
                "UPDATE agents SET state = 'ended', ended_at = ? WHERE project_id = ? AND agent_id = ?",
                [&now, &kernel.project_id, agent_id],
            )?;
            // An agent that is no longer active cannot stay paused: close its pause in the same transaction.
            let open_pause = pauses::pause_rows(kernel)?.into_iter().find(|pause| {
                pause.scope == "agent" && pause.agent_id.as_deref() == Some(agent_id)
            });
            if let Some(open_pause) = open_pause {
                let reason = "agent ended";
                pauses::close_pause(
                    kernel,
                    "agent",
                    Some(agent_id),
                    reason,
                    &kernel.internal_principal()?.actor_id,
                    &now,
                )?;
                kernel.append_event(
                    actor,
                    context,
                    "agent",
                    agent_id,
                    "paused",
                    "active",
                    "agent.resume",
                    agent.generation,
                    &json!({"reason": reason, "scope": "agent", "pausedReason": open_pause.reason}),
                )?;
            }
            let ended_plan_reviews = query_all(
                &kernel.database,
                "SELECT subject_plan_id FROM reviews WHERE project_id = ? AND reviewer_agent_id = ? AND state = 'started' AND subject_plan_id IS NOT NULL",
                [&kernel.project_id, agent_id],
                |row| row.get::<_, String>(0),
            )?;
            execute(
                &kernel.database,
                "UPDATE reviews SET state = 'cancelled', failure_reason = 'agent_ended', completed_at = ? WHERE project_id = ? AND reviewer_agent_id = ? AND state = 'started'",
                [&now, &kernel.project_id, agent_id],
            )?;
            for plan_id in &ended_plan_reviews {
                execute(
                    &kernel.database,
                    "UPDATE plans SET state = 'draft', updated_at = ? WHERE project_id = ? AND plan_id = ? AND state = 'in_review'",
                    [&now, &kernel.project_id, plan_id],
                )?;
            }
            let findings = query_all(
                &kernel.database,
                "SELECT * FROM agent_findings WHERE project_id = ? AND state = 'open' AND (target_agent_id = ? OR raised_by_agent_id = ?) ORDER BY sequence",
                [&kernel.project_id, agent_id, agent_id],
                AgentFindingRow::from_row,
            )?;
            for finding in &findings {
                let reason = if finding.target_agent_id == agent_id {
                    "target_ended"
                } else {
                    "raiser_ended"
                };
                findings::close_finding(kernel, actor, context, finding, "cancelled", Some(reason), &now)?;
            }
            operator_proposals::cancel_unstarted_operator_proposals_of(kernel, agent_id, &now)?;
            operator_grants::end_operator_grants_of(kernel, agent_id)?;
            let unacknowledged: Vec<String> = messages::message_rows_for(kernel, agent_id)?
                .into_iter()
                .filter(|row| !is_final_state(&row.state))
                .map(|row| row.message_id)
                .collect();
            let cancelled = messages::cancel_messages_of(
                kernel, actor, context, agent_id, "agent_ended", &now, true,
            )?;
            let branch: Option<String> = match given_branch {
                Some(branch) => Some(branch.to_string()),
                None => query_opt(
                    &kernel.database,
                    "SELECT branch FROM agent_panes WHERE project_id = ? AND agent_id = ?",
                    [&kernel.project_id, agent_id],
                    |row| row.get::<_, Option<String>>(0),
                )?
                .flatten(),
            };
            if let Some(reason) = lost {
                actors::record_lost(
                    kernel,
                    actor,
                    context,
                    &agent,
                    reason,
                    &unacknowledged,
                    &now,
                    Some(branch.as_deref()),
                )?;
            }
            Ok(MutationOutput::new(
                json!({"cancelledMessageIds": cancelled}),
                MutationEvent::new("agent", agent_id, agent.generation)
                    .transition("active", "ended")
                    .with_details(json!({"cancelledMessageIds": cancelled, "branch": branch})),
            ))
        },
    )
}

/// `replaceAgentGeneration`.
pub fn replace_agent_generation(
    kernel: &Kernel,
    context: &MutationContext,
    agent_id: &str,
) -> KernelResult<Value> {
    safe_id_str(agent_id, "agent id")?;
    kernel.mutate(
        context,
        "agent.replace",
        "actor:manage",
        &json!({"agentId": agent_id}),
        |actor| replace_generation(kernel, actor, context, agent_id),
    )
}

/// `#replaceGeneration`: a new actor and credential for the agent's seat, the old actors revoked.
fn replace_generation(
    kernel: &Kernel,
    actor: &AuthenticatedActor,
    context: &MutationContext,
    agent_id: &str,
) -> KernelResult<MutationOutput> {
    let agent = match kernel.agent_row(agent_id)? {
        Some(agent) if agent.state == "active" => agent,
        _ => return Err(KernelError::controller("agent is not active")),
    };
    actors::assert_seat_free_of_authority(kernel, &agent.seat_id)?;
    let now = kernel.now();
    actors::revoke_seat_actors(kernel, &agent.seat_id, &now)?;
    let actor_id = new_actor_id(kernel.env.as_ref());
    let credential = issue_credential(kernel.env.as_ref());
    actors::insert_actor(
        kernel,
        &actor.actor_id,
        &actor_id,
        &format!("{} generation {}", agent.role_name, agent.generation + 1),
        &agent.kind,
        Some(&agent.seat_id),
        &credential_hash(&credential)?,
        &now,
    )?;
    let generation = agent.generation + 1;
    execute(
        &kernel.database,
        "UPDATE agents SET actor_id = ?, generation = ?, last_activity_at = ? WHERE project_id = ? AND agent_id = ?",
        rusqlite::params![actor_id, generation, now, kernel.project_id, agent_id],
    )?;
    message_notices::close_waits(kernel, agent_id, &now)?;
    operator_proposals::cancel_unstarted_operator_proposals_of(kernel, agent_id, &now)?;
    operator_grants::end_operator_grants_of(kernel, agent_id)?;
    execute(
        &kernel.database,
        "INSERT INTO agent_state_history(project_id, agent_id, sequence, herdr_state, observed_at) SELECT ?, ?, COALESCE(MAX(sequence), 0) + 1, 'unknown', MAX(?, COALESCE(MAX(observed_at), '')) FROM agent_state_history WHERE project_id = ? AND agent_id = ?",
        rusqlite::params![kernel.project_id, agent_id, now, kernel.project_id, agent_id],
    )?;
    let cancelled = messages::cancel_messages_of(
        kernel,
        actor,
        context,
        agent_id,
        "generation_replaced",
        &now,
        false,
    )?;
    Ok(MutationOutput::new(
        json!({
            "agentId": agent_id,
            "generation": generation,
            "actorId": actor_id,
            "credential": credential,
            "cancelledMessageIds": cancelled,
        }),
        MutationEvent::new("agent", agent_id, generation)
            .transition(&agent.generation.to_string(), &generation.to_string())
            .with_details(json!({"cancelledMessageIds": cancelled})),
    ))
}

/// The listed fields of `source` that it has, in the listed order (an object literal naming each; `undefined` is left out).
fn pick(source: &Value, keys: &[&str]) -> Value {
    let mut picked = Map::new();
    for key in keys {
        if let Some(value) = source.get(*key) {
            picked.insert((*key).to_string(), value.clone());
        }
    }
    Value::Object(picked)
}

/// A message of a restart summary: the four fields Node writes for a new one.
fn summary_message(message_id: &str, from: &str, body: &str, state: &str) -> Value {
    json!({"messageId": message_id, "from": from, "body": body, "state": state})
}

fn message_id(message: &Value) -> &str {
    message["messageId"].as_str().unwrap_or("")
}

fn message_body(message: &Value) -> &str {
    message["body"].as_str().unwrap_or("")
}

/// The newest summary a restart may carry (`#restartSummary`) and the JSON text Node stores and hashes: bounded so neither
/// the table nor a stored result grows without limit. A carried message keeps whatever fields it was stored with.
fn restart_summary(
    kernel: &Kernel,
    agent_id: &str,
    generation: i64,
    carried: &[String],
) -> KernelResult<(String, Value)> {
    let brief = query_opt(
        &kernel.database,
        "SELECT content_json FROM project_revisions WHERE project_id = ? AND kind = 'task_brief' ORDER BY revision DESC LIMIT 1",
        [&kernel.project_id],
        |row| row.get::<_, String>(0),
    )?;
    let snapshot = status::status_snapshot(kernel)?;
    let work: Vec<&Value> = snapshot["work"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| {
            !matches!(
                item["state"].as_str(),
                Some("accepted" | "canceled" | "failed")
            )
        })
        .collect();
    let mut seen = BTreeSet::new();
    let mut summary_messages: Vec<Value> = Vec::new();
    let mut truncated = work.len() > MAX_SUMMARY_WORK;
    let mut carried_ids = BTreeSet::new();
    let mut add = |message: Value, messages: &mut Vec<Value>| {
        if seen.insert(message_id(&message).to_string()) {
            messages.push(message);
        }
    };
    for row in messages::message_rows_for(kernel, agent_id)? {
        if matches!(row.state.as_str(), "acked" | "acked_late" | "cancelled") {
            continue;
        }
        let sender = messages::sender_of(kernel, &row.sender_actor_id)?;
        let from = sender["agentId"]
            .as_str()
            .or(sender["role"].as_str())
            .unwrap_or("");
        add(
            summary_message(&row.message_id, from, &row.body, &row.state),
            &mut summary_messages,
        );
    }
    for text in carried {
        let summary: Value = serde_json::from_str(text)?;
        if summary["truncated"].as_bool() == Some(true) {
            truncated = true;
        }
        for message in summary["messages"].as_array().into_iter().flatten() {
            carried_ids.insert(message_id(message).to_string());
            add(message.clone(), &mut summary_messages);
        }
    }
    if summary_messages.len() > MAX_SUMMARY_MESSAGES {
        truncated = true;
    }
    summary_messages.truncate(MAX_SUMMARY_MESSAGES);
    let mut bounded: Vec<Value> = Vec::new();
    for mut message in summary_messages {
        // A body carried from an earlier summary was cut once already.
        if (carried_ids.contains(message_id(&message))
            && message_body(&message).ends_with(TRUNCATION_MARKER))
            || message_body(&message).chars().count() <= MAX_SUMMARY_BODY
        {
            bounded.push(message);
            continue;
        }
        truncated = true;
        let body = format!(
            "{}{}",
            cut_at_characters(message_body(&message), MAX_SUMMARY_BODY),
            TRUNCATION_MARKER
        );
        message["body"] = Value::String(body);
        bounded.push(message);
    }
    let mut all_links = links::link_records(kernel)?;
    all_links.sort_by_key(|link| std::cmp::Reverse(link["drift"].as_bool() == Some(true)));
    if all_links.len() > MAX_SUMMARY_LINKS {
        truncated = true;
    }
    let mut links: Vec<Value> = all_links
        .iter()
        .take(MAX_SUMMARY_LINKS)
        .map(|link| {
            pick(
                link,
                &[
                    "refKind",
                    "refId",
                    "externalId",
                    "syncedState",
                    "wanted",
                    "drift",
                    "boundAgentId",
                ],
            )
        })
        .collect();
    let mut open_work: Vec<Value> = work
        .iter()
        .take(MAX_SUMMARY_WORK)
        .map(|item| {
            pick(
                item,
                &["workItemId", "title", "role", "state", "owner", "blockers"],
            )
        })
        .collect();
    let mut messages_out = bounded;
    let mut plans = plans::open_plans_for_summary(kernel)?;
    let mut integrations = integrations::merged_integrations_for_summary(kernel)?;
    let objective = objective_of(brief.as_deref())?;
    let generated_at = kernel.now();
    let render = |open_work: &[Value],
                  messages: &[Value],
                  plans: &[Value],
                  integrations: &[Value],
                  links: &[Value],
                  truncated: bool| {
        json!({
            "objective": objective,
            "openWork": open_work,
            "messages": messages,
            "plans": plans,
            "integrations": integrations,
            "links": links,
            "truncated": truncated,
            "summarizedGeneration": generation,
            "generatedAt": generated_at,
        })
    };
    // A summary must fit the prompt it is rendered into: shed messages, then work items, from the end until its JSON is
    // small enough.
    loop {
        let value = render(
            &open_work,
            &messages_out,
            &plans,
            &integrations,
            &links,
            truncated,
        );
        let text = stringify(&value);
        let empty = messages_out.is_empty()
            && open_work.is_empty()
            && plans.is_empty()
            && integrations.is_empty()
            && links.is_empty();
        if text.len() <= MAX_SUMMARY_BYTES || empty {
            return Ok((text, value));
        }
        if !messages_out.is_empty() {
            messages_out.pop();
        } else if !open_work.is_empty() {
            open_work.pop();
        } else if !plans.is_empty() {
            plans.pop();
        } else if !integrations.is_empty() {
            integrations.pop();
        } else {
            links.pop();
        }
        truncated = true;
    }
}

/// `restartAgentGeneration`: a PM restart in one transaction. The summary is built from the ledger first (the replace
/// cancels the PM's open messages), recorded, and then the generation is replaced.
pub fn restart_agent_generation(
    kernel: &Kernel,
    context: &MutationContext,
    agent_id: &str,
) -> KernelResult<Value> {
    safe_id_str(agent_id, "agent id")?;
    kernel.mutate(
        context,
        "agent.restart",
        "actor:manage",
        &json!({"agentId": agent_id}),
        |actor| {
            let agent = match kernel.agent_row(agent_id)? {
                Some(agent) if agent.state == "active" => agent,
                _ => return Err(KernelError::controller("agent is not active")),
            };
            if agent.kind != "PM" {
                return Err(KernelError::controller("only a PM is restarted with a summary"));
            }
            let carried = query_all(
                &kernel.database,
                "SELECT summary_json FROM pm_restarts WHERE project_id = ? AND agent_id = ? AND consumed = 0 ORDER BY sequence",
                [&kernel.project_id, agent_id],
                |row| row.get::<_, String>(0),
            )?;
            let (summary_json, summary) =
                restart_summary(kernel, agent_id, agent.generation, &carried)?;
            let summary_hash = sha256(&summary_json);
            let restart_id = kernel.env.uuid();
            let sequence = query_opt(
                &kernel.database,
                "SELECT COALESCE(MAX(sequence), 0) AS latest FROM pm_restarts WHERE project_id = ? AND agent_id = ?",
                [&kernel.project_id, agent_id],
                |row| row.get::<_, i64>(0),
            )?
            .unwrap_or(0)
                + 1;
            // The replace can refuse (the seat still holds authority); it runs before the row is written so a refusal
            // leaves no attempt behind.
            let mut replaced = replace_generation(kernel, actor, context, agent_id)?;
            execute(
                &kernel.database,
                "INSERT INTO pm_restarts(project_id, restart_id, agent_id, sequence, summarized_generation, summary_json, summary_hash, consumed, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?)",
                rusqlite::params![
                    kernel.project_id,
                    restart_id,
                    agent_id,
                    sequence,
                    agent.generation,
                    summary_json,
                    summary_hash,
                    kernel.now()
                ],
            )?;
            replaced.value["restartId"] = json!(restart_id);
            replaced.value["summary"] = summary;
            if let Some(Value::Object(details)) = replaced.event.details.as_mut() {
                details.insert("restartId".into(), json!(restart_id));
                details.insert("summaryHash".into(), json!(summary_hash));
            }
            Ok(replaced)
        },
    )
}

/// `markPmRestartsConsumed`: marks every restart summary of an agent up to a sequence as used by a started PM.
pub fn mark_pm_restarts_consumed(
    kernel: &Kernel,
    context: &MutationContext,
    agent_id: &str,
    up_to_sequence: i64,
) -> KernelResult<Value> {
    safe_id_str(agent_id, "agent id")?;
    if up_to_sequence < 1 {
        return Err(KernelError::type_error(
            "sequence must be a positive integer",
        ));
    }
    kernel.mutate(
        context,
        "pm_restart.consume",
        "controller:reconcile",
        &json!({"agentId": agent_id, "upToSequence": up_to_sequence}),
        |_| {
            let marked = execute(
                &kernel.database,
                "UPDATE pm_restarts SET consumed = 1 WHERE project_id = ? AND agent_id = ? AND consumed = 0 AND sequence <= ?",
                rusqlite::params![kernel.project_id, agent_id, up_to_sequence],
            )?;
            Ok(MutationOutput::new(
                json!({"marked": marked}),
                MutationEvent::new("pm_restart", agent_id, up_to_sequence)
                    .with_details(json!({"marked": marked})),
            ))
        },
    )
}

/// `pmRestarts`.
pub fn pm_restarts(kernel: &Kernel, credential: &str, agent_id: &str) -> KernelResult<Value> {
    kernel.authorize(credential, "controller:reconcile")?;
    safe_id_str(agent_id, "agent id")?;
    let rows = query_all(
        &kernel.database,
        "SELECT restart_id, sequence, summarized_generation, consumed, summary_hash, summary_json FROM pm_restarts WHERE project_id = ? AND agent_id = ? ORDER BY sequence",
        [&kernel.project_id, agent_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        },
    )?;
    let mut out = Vec::new();
    for (restart_id, sequence, generation, consumed, hash, summary_json) in rows {
        out.push(json!({
            "restartId": restart_id,
            "sequence": sequence,
            "summarizedGeneration": generation,
            "consumed": consumed == 1,
            "summaryHash": hash,
            "summary": serde_json::from_str::<Value>(&summary_json)?,
        }));
    }
    Ok(Value::Array(out))
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "registerAgent" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(register_agent(kernel, &context, input)))
        }
        "identify" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(identify(kernel, credential)))
        }
        "listAgents" => Some(call(list_agents(kernel))),
        "activeAgents" => Some(call(active_agents(kernel))),
        "agentRecord" => {
            let agent_id = arg!(a.str(0, "agentId"));
            Some(call(agent_record(kernel, agent_id)))
        }
        "recordAgentObservation" => {
            let context = arg!(a.ctx(0));
            let agent_id = arg!(a.str(1, "agentId"));
            let state = a.value(2);
            Some(call(record_agent_observation(
                kernel, &context, agent_id, state,
            )))
        }
        "endAgent" => {
            let context = arg!(a.ctx(0));
            let agent_id = arg!(a.str(1, "agentId"));
            let options = a.opt_value(2);
            Some(call(end_agent(kernel, &context, agent_id, options)))
        }
        "replaceAgentGeneration" => {
            let context = arg!(a.ctx(0));
            let agent_id = arg!(a.str(1, "agentId"));
            Some(call(replace_agent_generation(kernel, &context, agent_id)))
        }
        "restartAgentGeneration" => {
            let context = arg!(a.ctx(0));
            let agent_id = arg!(a.str(1, "agentId"));
            Some(call(restart_agent_generation(kernel, &context, agent_id)))
        }
        "markPmRestartsConsumed" => {
            let context = arg!(a.ctx(0));
            let agent_id = arg!(a.str(1, "agentId"));
            let up_to_sequence = arg!(a.i64(2, "upToSequence"));
            Some(call(mark_pm_restarts_consumed(
                kernel,
                &context,
                agent_id,
                up_to_sequence,
            )))
        }
        "pmRestarts" => {
            let credential = arg!(a.str(0, "credential"));
            let agent_id = arg!(a.str(1, "agentId"));
            Some(call(pm_restarts(kernel, credential, agent_id)))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_carried_message_keeps_its_stored_fields_and_order() {
        let stored = r#"{"messages":[{"state":"sent","note":{"b":1,"a":{"z":1,"y":[{"q":1,"p":2}]}},"messageId":"m1","body":"hello","from":"pm-1","extra":7,"3":"x"}]}"#;
        let summary: Value = serde_json::from_str(stored).unwrap();
        let mut message = summary["messages"][0].clone();
        assert_eq!(message_id(&message), "m1");
        let expected = r#"{"3":"x","state":"sent","note":{"b":1,"a":{"z":1,"y":[{"q":1,"p":2}]}},"messageId":"m1","body":"hello","from":"pm-1","extra":7}"#;
        assert_eq!(stringify(&message), expected);
        message["body"] = json!("cut[truncated]");
        assert_eq!(
            stringify(&message),
            expected.replace("\"hello\"", "\"cut[truncated]\"")
        );
    }

    #[test]
    fn a_new_message_has_the_four_fields_in_node_order() {
        assert_eq!(
            stringify(&summary_message("m2", "dev-1", "hi", "queued")),
            r#"{"messageId":"m2","from":"dev-1","body":"hi","state":"queued"}"#
        );
    }
}
