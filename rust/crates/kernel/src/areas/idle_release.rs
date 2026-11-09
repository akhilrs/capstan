//! Automatic release of idle workers: the ledger side. It answers three questions for the daemon loop and records two
//! facts; it never ends a session itself.
//!
//! * [`open_work`]: why an agent must not be released (empty means nothing holds it).
//! * [`idle_candidates`]: the workers that are idle by the ledger's own evidence, with the time they went idle.
//! * [`keep_until`]: the end of the newest keep record of an agent (`agent_keep` events; the write side is elsewhere).
//! * [`record_idle_warning`] and [`record_auto_release`]: the ledger rows of the warning and of the release.
//!
//! An agent counts as idle when its latest Herdr state is idle, done or unknown. It has been idle since the latest of its
//! last authenticated call, the last time Herdr saw it working or blocked, its last child-process activity (given by the
//! daemon, which holds the process history in memory) and its latest message delivery or acknowledgement.

use super::messaging::parse_iso_ms;
use super::plan_packages::{self, PackageRow};
use super::{pauses, warm_reviewers};
use crate::errors::KernelResult;
use crate::helpers::{query_all, query_opt, safe_id_str};
use crate::kernel::Kernel;
use crate::records::{MutationEvent, MutationOutput};
use crate::types::MutationContext;
use capstan_ledger::iso_from_millis;
use serde_json::{json, Value};

/// The kinds of agent the release may end. A PM and a Supervisor are never released this way.
const RELEASABLE_KINDS: [&str; 2] = ["Developer", "Verifier"];

/// Message states that still wait for delivery or an answer.
const PENDING_MESSAGE_STATES: &str = "('queued', 'deferred', 'sent', 'unacked')";

fn pid(kernel: &Kernel) -> &str {
    kernel.project_id.as_str()
}

fn texts(kernel: &Kernel, sql: &str, params: impl rusqlite::Params) -> KernelResult<Vec<String>> {
    query_all(&kernel.database, sql, params, |row| row.get::<_, String>(0))
}

/// The reasons an agent still has work, in plain words; empty when it has none.
pub fn open_work(kernel: &Kernel, agent_id: &str) -> KernelResult<Vec<String>> {
    kernel.assert_open()?;
    safe_id_str(agent_id, "agent id")?;
    let mut reasons = Vec::new();
    let Some(actor_id) = query_opt(
        &kernel.database,
        "SELECT actor_id FROM agents WHERE project_id = ? AND agent_id = ?",
        [pid(kernel), agent_id],
        |row| row.get::<_, String>(0),
    )?
    else {
        return Ok(reasons);
    };

    // A plan package assigned to it whose progress is not integrated and that was not cancelled.
    let packages = query_all(
        &kernel.database,
        "SELECT p.plan_id, p.package_id, p.assigned_at, p.cancelled_at, pl.cancelled_at
         FROM plan_packages p JOIN plans pl ON pl.project_id = p.project_id AND pl.plan_id = p.plan_id
         WHERE p.project_id = ? AND p.assignee_agent_id = ? AND pl.state <> 'superseded'
         ORDER BY p.plan_id, p.package_id",
        [pid(kernel), agent_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                PackageRow {
                    package_id: row.get(1)?,
                    assignee_agent_id: Some(agent_id.to_string()),
                    assigned_at: row.get(2)?,
                    cancelled_at: row.get(3)?,
                },
                row.get::<_, Option<String>>(4)?.is_some(),
            ))
        },
    )?;
    for (plan_id, row, plan_cancelled) in packages {
        let facts = plan_packages::package_facts(kernel, &plan_id, &row, plan_cancelled)?;
        if !facts.cancelled && facts.progress != "integrated" {
            reasons.push(format!(
                "package {plan_id}/{} is {} (not yet integrated)",
                row.package_id, facts.progress
            ));
        }
    }

    // Its newest accepted report is not covered by a merged or confirmed integration.
    if let Some(report) = query_opt(
        &kernel.database,
        "SELECT report_id FROM agent_reports WHERE project_id = ? AND agent_id = ? AND state = 'accepted'
         ORDER BY sequence DESC LIMIT 1",
        [pid(kernel), agent_id],
        |row| row.get::<_, String>(0),
    )? {
        let covered = query_opt(
            &kernel.database,
            "SELECT 1 FROM integration_reports ir
               JOIN integrations i ON i.project_id = ir.project_id AND i.integration_id = ir.integration_id
              WHERE ir.project_id = ?1 AND ir.report_id = ?2 AND i.state IN ('merged', 'confirmed')
             UNION ALL
             SELECT 1 FROM integration_covered_reports c
               JOIN integrations i ON i.project_id = c.project_id AND i.integration_id = c.integration_id
              WHERE c.project_id = ?1 AND c.report_id = ?2 AND i.state IN ('merged', 'confirmed')
             LIMIT 1",
            [pid(kernel), report.as_str()],
            |_| Ok(()),
        )?;
        if covered.is_none() {
            reasons.push(format!(
                "its report {report} is not yet merged into an integration"
            ));
        }
    }

    // A verify run queued or running for its report, or that it requested.
    for run in texts(
        kernel,
        "SELECT v.run_id FROM verify_runs v
         WHERE v.project_id = ?1 AND v.state IN ('queued', 'running')
           AND (v.requested_by_actor_id = ?3
                OR v.subject_report_id IN (SELECT report_id FROM agent_reports WHERE project_id = ?1 AND agent_id = ?2))
         ORDER BY v.sequence",
        [pid(kernel), agent_id, actor_id.as_str()],
    )? {
        reasons.push(format!("verify run {run} is queued or running"));
    }

    // A review running where it is the reviewer or the report's author.
    for review in texts(
        kernel,
        "SELECT review_id FROM reviews
         WHERE project_id = ?1 AND state = 'started' AND (reviewer_agent_id = ?2 OR author_agent_id = ?2)
         ORDER BY sequence",
        rusqlite::params![pid(kernel), agent_id],
    )? {
        reasons.push(format!("review {review} is running"));
    }

    // A plan it is the architect of that is in draft or review, or approved with a live package not integrated.
    let plans = query_all(
        &kernel.database,
        "SELECT plan_id, state FROM plans
         WHERE project_id = ? AND architect_agent_id = ? AND cancelled_at IS NULL AND state <> 'superseded'
         ORDER BY sequence",
        [pid(kernel), agent_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    for (plan_id, state) in plans {
        if state == "approved" {
            let mut live = None;
            for row in plan_packages::plan_package_rows(kernel, &plan_id)? {
                let facts = plan_packages::package_facts(kernel, &plan_id, &row, false)?;
                if !facts.cancelled && facts.progress != "integrated" {
                    live = Some(row.package_id.clone());
                    break;
                }
            }
            if let Some(package) = live {
                reasons.push(format!(
                    "plan {plan_id} is approved and package {package} is not yet integrated"
                ));
            }
        } else {
            reasons.push(format!("plan {plan_id} is in {state}"));
        }
    }

    // An integration it created that is running or under review.
    for integration in texts(
        kernel,
        "SELECT i.integration_id FROM integrations i
         WHERE i.project_id = ?1 AND i.requested_by IN (?2, ?3)
           AND (i.state = 'running'
                OR EXISTS (SELECT 1 FROM reviews r WHERE r.project_id = i.project_id
                             AND r.subject_integration_id = i.integration_id AND r.state = 'started'))
         ORDER BY i.sequence",
        [pid(kernel), agent_id, actor_id.as_str()],
    )? {
        reasons.push(format!("integration {integration} is running or under review"));
    }

    // Mail waiting for it.
    let queued = texts(
        kernel,
        &format!(
            "SELECT message_id FROM messages WHERE project_id = ? AND recipient_agent_id = ?
             AND state IN {PENDING_MESSAGE_STATES} ORDER BY sequence"
        ),
        [pid(kernel), agent_id],
    )?;
    if !queued.is_empty() {
        reasons.push(format!(
            "{} message(s) to it are not yet delivered or acknowledged",
            queued.len()
        ));
    }

    // A question or other message it sent that nobody has acknowledged.
    let asked = texts(
        kernel,
        &format!(
            "SELECT message_id FROM messages WHERE project_id = ? AND sender_actor_id = ? AND class <> 'info'
             AND state IN {PENDING_MESSAGE_STATES} ORDER BY sequence"
        ),
        [pid(kernel), actor_id.as_str()],
    )?;
    if !asked.is_empty() {
        reasons.push(format!(
            "{} message(s) it sent are not yet acknowledged",
            asked.len()
        ));
    }

    // A prompt relay that waits for an answer.
    let relays = texts(
        kernel,
        "SELECT relay_id FROM prompt_relays WHERE project_id = ? AND agent_id = ? AND state IN ('captured', 'typing')
         ORDER BY sequence",
        [pid(kernel), agent_id],
    )?;
    if !relays.is_empty() {
        reasons.push("a prompt relay for it is pending".to_string());
    }
    Ok(reasons)
}

/// The end of the newest keep record of an agent (an `agent_keep` event with `until`), or `None` without one.
pub fn keep_until(kernel: &Kernel, agent_id: &str) -> KernelResult<Option<String>> {
    kernel.assert_open()?;
    safe_id_str(agent_id, "agent id")?;
    Ok(query_opt(
        &kernel.database,
        "SELECT COALESCE(json_extract(payload_json, '$.details.until'), json_extract(payload_json, '$.until'))
         FROM controller_events WHERE project_id = ? AND entity_type = 'agent_keep' AND entity_id = ?
         ORDER BY sequence DESC LIMIT 1",
        [pid(kernel), agent_id],
        |row| row.get::<_, Option<String>>(0),
    )?
    .flatten())
}

fn max_ms(current: &mut i64, text: Option<&str>) {
    if let Some(text) = text {
        *current = (*current).max(parse_iso_ms(text));
    }
}

/// When the agent went idle by the ledger's evidence (see the module text), or `None` while its latest Herdr state is
/// not idle, done or unknown.
fn idle_since_ms(
    kernel: &Kernel,
    agent_id: &str,
    last_activity: &str,
    created: &str,
    input: &Value,
) -> KernelResult<Option<i64>> {
    let latest_state = query_opt(
        &kernel.database,
        "SELECT herdr_state FROM agent_state_history WHERE project_id = ? AND agent_id = ?
         ORDER BY sequence DESC LIMIT 1",
        [pid(kernel), agent_id],
        |row| row.get::<_, String>(0),
    )?
    .unwrap_or_else(|| "unknown".to_string());
    if !["idle", "done", "unknown"].contains(&latest_state.as_str()) {
        return Ok(None);
    }
    let mut since = parse_iso_ms(created);
    max_ms(&mut since, Some(last_activity));
    let busy = query_opt(
        &kernel.database,
        "SELECT observed_at FROM agent_state_history WHERE project_id = ? AND agent_id = ?
         AND herdr_state IN ('working', 'blocked') ORDER BY sequence DESC LIMIT 1",
        [pid(kernel), agent_id],
        |row| row.get::<_, String>(0),
    )?;
    max_ms(&mut since, busy.as_deref());
    let mail = query_opt(
        &kernel.database,
        "SELECT MAX(sent_at), MAX(acked_at) FROM messages WHERE project_id = ? AND recipient_agent_id = ?",
        [pid(kernel), agent_id],
        |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<String>>(1)?)),
    )?;
    if let Some((sent, acked)) = mail {
        max_ms(&mut since, sent.as_deref());
        max_ms(&mut since, acked.as_deref());
    }
    if let Some(ms) = input["childActivity"][agent_id].as_i64() {
        since = since.max(ms);
    }
    Ok(Some(since))
}

/// The workers that are idle by the ledger's evidence, as `{agentId, role, kind, idleSinceMs, idleSince}` in agent order.
/// `input` holds `idleMinutes` (the grace after a spawn or the start of the daemon), `graceFloorMs` (when the daemon
/// started; child-process history before it is unknown) and `childActivity` (agent id to the last child activity in ms).
/// Warm reviewers, paused agents (or a paused run), kept agents, agents already replaced and agents still in their grace
/// are left out; so is every agent that is not a Developer or Verifier.
pub fn idle_candidates(kernel: &Kernel, now_ms: i64, input: &Value) -> KernelResult<Vec<Value>> {
    kernel.assert_open()?;
    let idle_ms = input["idleMinutes"].as_i64().unwrap_or(0).max(0) * 60_000;
    let grace_floor = input["graceFloorMs"].as_i64().unwrap_or(0);
    let agents = query_all(
        &kernel.database,
        "SELECT agent_id, role_name, kind, generation, last_activity_at, created_at FROM agents
         WHERE project_id = ? AND state = 'active' ORDER BY created_at, agent_id",
        [pid(kernel)],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        },
    )?;
    let mut out = Vec::new();
    for (agent_id, role, kind, last_activity, created) in agents {
        if !RELEASABLE_KINDS.contains(&kind.as_str()) {
            continue;
        }
        let grace_until = parse_iso_ms(&created).max(grace_floor) + idle_ms;
        if now_ms < grace_until {
            continue;
        }
        if warm_reviewers::warm_of_agent(kernel, &agent_id)?.is_some()
            || pauses::delivery_paused(kernel, &agent_id)?
        {
            continue;
        }
        if keep_until(kernel, &agent_id)?.is_some_and(|until| parse_iso_ms(&until) > now_ms) {
            continue;
        }
        let replaced = crate::helpers::exists(
            &kernel.database,
            super::IS_AGENT_REPLACED_SQL,
            [pid(kernel), agent_id.as_str()],
        )?;
        if replaced {
            continue;
        }
        let Some(since) = idle_since_ms(kernel, &agent_id, &last_activity, &created, input)? else {
            continue;
        };
        out.push(json!({
            "agentId": agent_id,
            "role": role,
            "kind": kind,
            "idleSinceMs": since,
            "idleSince": iso_from_millis(since),
        }));
    }
    Ok(out)
}

/// The newest idle warning of an agent: `{warnedAt, idleSince, releaseAt}`, or `None`. A keep record written after the
/// warning (a keep or its clearing) ends that idle episode, so the warning no longer counts.
pub fn last_idle_warning(kernel: &Kernel, agent_id: &str) -> KernelResult<Option<Value>> {
    kernel.assert_open()?;
    safe_id_str(agent_id, "agent id")?;
    query_opt(
        &kernel.database,
        "SELECT w.created_at, json_extract(w.payload_json, '$.details.idleSince'), json_extract(w.payload_json, '$.details.releaseAt')
         FROM controller_events w WHERE w.project_id = ?1 AND w.entity_type = 'agent_idle_warning' AND w.entity_id = ?2
           AND NOT EXISTS (SELECT 1 FROM controller_events k WHERE k.project_id = w.project_id
                             AND k.entity_type IN ('agent_keep', 'agent_idle_episode_end') AND k.entity_id = w.entity_id
                             AND k.sequence > w.sequence)
         ORDER BY w.sequence DESC LIMIT 1",
        [pid(kernel), agent_id],
        |row| {
            Ok(json!({
                "warnedAt": row.get::<_, String>(0)?,
                "idleSince": row.get::<_, Option<String>>(1)?,
                "releaseAt": row.get::<_, Option<String>>(2)?,
            }))
        },
    )
}

/// Records that the PM was warned about an idle agent (`controller:reconcile`); `input` holds `agentId`, `idleSince` and
/// `releaseAt`.
pub fn record_idle_warning(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let agent_id = safe_id_str(input["agentId"].as_str().unwrap_or(""), "agent id")?.to_string();
    kernel.mutate(
        context,
        "agent.idle_warning",
        "controller:reconcile",
        input,
        |_| {
            let generation = kernel
                .agent_row(&agent_id)?
                .map_or(0, |agent| agent.generation);
            Ok(MutationOutput::new(
                json!({"recorded": true}),
                MutationEvent::new("agent_idle_warning", &agent_id, generation).with_details(
                    json!({
                        "idleSince": input["idleSince"],
                        "releaseAt": input["releaseAt"],
                        "role": input["role"],
                    }),
                ),
            ))
        },
    )
}

/// Records that an idle episode ended without a release (open work, activity, a keep): a warning older than this row no
/// longer counts. `input` holds `agentId`.
pub fn record_idle_episode_end(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let agent_id = safe_id_str(input["agentId"].as_str().unwrap_or(""), "agent id")?.to_string();
    kernel.mutate(
        context,
        "agent.idle_episode_end",
        "controller:reconcile",
        input,
        |_| {
            let generation = kernel
                .agent_row(&agent_id)?
                .map_or(0, |agent| agent.generation);
            Ok(MutationOutput::new(
                json!({"recorded": true}),
                MutationEvent::new("agent_idle_episode_end", &agent_id, generation)
                    .with_details(json!({})),
            ))
        },
    )
}

/// Records an automatic release (`controller:reconcile`) after the launcher ended the agent: an `agent_auto_release`
/// event (the launcher's release already wrote the one `agent` end event, so no consumer sees a second end) with the
/// reason `idle_auto_release`. `input` holds `agentId`, `idleSince`, `idleMinutes`,
/// `warnedAt` and `keptBranches`. The first kept branch is repeated as `branch`, where an ended agent's branch is read.
pub fn record_auto_release(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let agent_id = safe_id_str(input["agentId"].as_str().unwrap_or(""), "agent id")?.to_string();
    kernel.mutate(
        context,
        "agent.idle_auto_release",
        "controller:reconcile",
        input,
        |_| {
            let generation = kernel
                .agent_row(&agent_id)?
                .map_or(0, |agent| agent.generation);
            let kept = input["keptBranches"].clone();
            let kept = if kept.is_array() { kept } else { json!([]) };
            Ok(MutationOutput::new(
                json!({"recorded": true}),
                MutationEvent::new("agent_auto_release", &agent_id, generation).with_details(
                    json!({
                        "reason": "idle_auto_release",
                        "idleSince": input["idleSince"],
                        "idleMinutes": input["idleMinutes"],
                        "warnedAt": input["warnedAt"],
                        "keptBranches": kept,
                        "branch": kept.get(0).cloned().unwrap_or(Value::Null),
                    }),
                ),
            ))
        },
    )
}

/// Records a keep (`controller:reconcile`): `input` holds `agentId`, `hours` (0 clears the keep) and `by`. The newest
/// `agent_keep` record of an agent wins; a cleared keep has no `until`.
pub fn record_keep(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let agent_id = safe_id_str(input["agentId"].as_str().unwrap_or(""), "agent id")?.to_string();
    let hours = input["hours"].as_i64().unwrap_or(0);
    kernel.mutate(context, "agent.keep", "controller:reconcile", input, |_| {
        let generation = kernel
            .agent_row(&agent_id)?
            .map_or(0, |agent| agent.generation);
        let until = if hours > 0 {
            json!(iso_from_millis(kernel.env.now() + hours * 3_600_000))
        } else {
            Value::Null
        };
        Ok(MutationOutput::new(
            json!({"recorded": true, "until": until}),
            MutationEvent::new("agent_keep", &agent_id, generation)
                .with_details(json!({"until": until, "by": input["by"]})),
        ))
    })
}

/// What `cstan status` shows of every active worker: `{agentId, idleSince, warnedAt, autoReleaseAt, keptUntil, openWork}`
/// (times in ISO form or null; `openWork` holds at most five reasons). `input` is the one of [`idle_candidates`]. `autoReleaseAt` is the time the loop would release the agent, null while it would not.
pub fn idle_overview(kernel: &Kernel, now_ms: i64, input: &Value) -> KernelResult<Vec<Value>> {
    kernel.assert_open()?;
    let idle_ms = input["idleMinutes"].as_i64().unwrap_or(0).max(0) * 60_000;
    let grace_floor = input["graceFloorMs"].as_i64().unwrap_or(0);
    let agents = query_all(
        &kernel.database,
        "SELECT agent_id, kind, last_activity_at, created_at FROM agents
         WHERE project_id = ? AND state = 'active' ORDER BY created_at, agent_id",
        [pid(kernel)],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        },
    )?;
    let mut out = Vec::new();
    for (agent_id, kind, last_activity, created) in agents {
        if !RELEASABLE_KINDS.contains(&kind.as_str()) {
            continue;
        }
        let kept = keep_until(kernel, &agent_id)?.filter(|until| parse_iso_ms(until) > now_ms);
        let since = idle_since_ms(kernel, &agent_id, &last_activity, &created, input)?;
        let open = open_work(kernel, &agent_id)?;
        let in_grace = now_ms < parse_iso_ms(&created).max(grace_floor) + idle_ms;
        let held = in_grace
            || warm_reviewers::warm_of_agent(kernel, &agent_id)?.is_some()
            || pauses::delivery_paused(kernel, &agent_id)?;
        let warning = match since {
            Some(since_ms) => last_idle_warning(kernel, &agent_id)?
                .filter(|w| w["idleSince"].as_str() == Some(iso_from_millis(since_ms).as_str())),
            None => None,
        };
        let release_at = match since {
            Some(since_ms) if kept.is_none() && !held && open.is_empty() => Some(
                warning
                    .as_ref()
                    .and_then(|w| w["releaseAt"].as_str().map(str::to_string))
                    .unwrap_or_else(|| iso_from_millis(since_ms + idle_ms)),
            ),
            _ => None,
        };
        let warned = if release_at.is_some() {
            warning
                .as_ref()
                .and_then(|w| w["warnedAt"].as_str().map(str::to_string))
        } else {
            None
        };
        out.push(json!({
            "agentId": agent_id,
            "idleSince": since.map(iso_from_millis),
            "warnedAt": warned,
            "autoReleaseAt": release_at,
            "keptUntil": kept,
            "openWork": open.into_iter().take(5).collect::<Vec<_>>(),
        }));
    }
    Ok(out)
}
