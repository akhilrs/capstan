//! The run and agent pauses (src/controller/pauses.ts).
//!
//! Every public function returns `Err(KernelError::Unported("pauses.<method>"))` until the package that owns this file fills
//! its body. The signatures and `dispatch` are fixed by the kernel foundation: fill the bodies, do not change them.

use crate::auth::AuthenticatedActor;
use crate::canonical::canonical_json;
use crate::dispatch::{arg, call, Args};
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{execute, exists, pause_reason_text, query_all, query_opt, safe_id};
use crate::kernel::Kernel;
use crate::records::{MutationEvent, MutationOutput};
use crate::types::MutationContext;
use serde_json::{json, Value};

use super::message_notices;
use super::messaging::parse_iso_ms;

/// `PauseRecord`: an open pause of the run or of one agent.
#[derive(Clone, Debug)]
pub struct PauseRecord {
    pub scope: String,
    pub agent_id: Option<String>,
    pub reason: String,
    pub actor_id: String,
    pub paused_at: String,
}

impl PauseRecord {
    pub fn to_value(&self) -> Value {
        json!({
            "scope": self.scope,
            "agentId": self.agent_id,
            "reason": self.reason,
            "actorId": self.actor_id,
            "pausedAt": self.paused_at,
        })
    }
}

/// `transitionRun`.
pub fn transition_run(
    kernel: &Kernel,
    context: &MutationContext,
    to_state: &Value,
    reason: Option<&str>,
) -> KernelResult<Value> {
    let state_text = to_state.as_str().unwrap_or("").to_string();
    let pause_reason = match reason {
        None => {
            if state_text == "paused" {
                "no reason given".to_string()
            } else {
                format!("run {state_text}")
            }
        }
        Some(text) => pause_reason_text(&Value::String(text.to_string()))?,
    };
    let payload = json!({"toState": to_state});
    let apply = |actor: &AuthenticatedActor,
                 caller: &AuthenticatedActor|
     -> KernelResult<MutationOutput> {
        let run = query_opt(
            &kernel.database,
            "SELECT state, state_version FROM run_controls WHERE project_id = ?",
            [&kernel.project_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )?;
        let Some((run_state, run_version)) = run else {
            return Err(KernelError::controller("run control record is missing"));
        };
        if state_text == "paused" && run_state == "paused" {
            let open = open_run_pause(kernel)?;
            return Err(KernelError::conflict(format!(
                "run is already paused: {}",
                open.map_or("no reason recorded".to_string(), |p| p.reason)
            )));
        }
        if state_text == "active" && run_state == "active" {
            return Err(KernelError::conflict("run is not paused"));
        }
        if state_text == "completed" {
            if exists(
                &kernel.database,
                "SELECT 1 AS present FROM findings WHERE project_id = ? AND state <> 'resolved' LIMIT 1",
                [&kernel.project_id],
            )? {
                return Err(KernelError::conflict(
                    "run cannot complete with unresolved Supervisor findings",
                ));
            }
            let unfinished_work = exists(
                &kernel.database,
                "SELECT 1 AS present FROM work_items WHERE project_id = ? AND state NOT IN ('accepted', 'canceled') LIMIT 1",
                [&kernel.project_id],
            )?;
            let uncertain_authority = exists(
                &kernel.database,
                "SELECT 1 AS present FROM assignments WHERE project_id = ? AND authority_state IN ('active', 'unknown') LIMIT 1",
                [&kernel.project_id],
            )?;
            if unfinished_work || uncertain_authority {
                return Err(KernelError::conflict(
                    "run cannot complete while work is open or assignment authority is not contained",
                ));
            }
            if exists(
                &kernel.database,
                "SELECT 1 AS present FROM work_items WHERE project_id = ? AND state = 'accepted' AND input_revision <> ? LIMIT 1",
                rusqlite::params![kernel.project_id, context.input_revision],
            )? {
                return Err(KernelError::conflict(
                    "run cannot complete with work accepted against a stale input revision",
                ));
            }
        }
        if !kernel.is_transition_allowed("run_control", &run_state, &state_text, actor)? {
            return Err(KernelError::TransitionAuthorization(format!(
                "transition table rejects run state {run_state} -> {state_text}"
            )));
        }
        let now = kernel.now();
        execute(
            &kernel.database,
            "UPDATE run_controls SET state = ?, state_version = state_version + 1, updated_at = ? WHERE project_id = ? AND state = ?",
            [&state_text, &now, &kernel.project_id, &run_state],
        )?;
        if caller.role == "operator" {
            let action_payload = if reason.is_none() {
                json!({"from": run_state, "to": state_text})
            } else {
                json!({"from": run_state, "to": state_text, "reason": pause_reason})
            };
            execute(
                &kernel.database,
                "INSERT INTO operator_actions(project_id, action_id, action_type, target_id, actor_id, request_id,
                   input_revision, payload_json, created_at)
                 VALUES (?, ?, 'run.transition', ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    kernel.project_id,
                    kernel.env.uuid(),
                    kernel.project_id,
                    caller.actor_id,
                    context.request_id,
                    context.input_revision,
                    canonical_json(&action_payload),
                    now
                ],
            )?;
        }
        if state_text == "paused" {
            open_pause(kernel, "run", None, &pause_reason, &caller.actor_id, &now)?;
        } else if run_state == "paused" {
            close_pause(kernel, "run", None, &pause_reason, &caller.actor_id, &now)?;
        }
        if caller.role == "operator" && state_text == "paused" {
            message_notices::notice_to_pm(
                kernel,
                &format!("The operator paused the run: {pause_reason}. Workers are held and spawn, plan assign, request-review and integrate are refused until `cstan resume --reason \"<text>\"`. You keep receiving messages."),
                &now,
                false,
            )?;
        } else if caller.role == "operator" && run_state == "paused" && state_text == "active" {
            message_notices::notice_to_pm(
                kernel,
                &format!("The operator resumed the run: {pause_reason}. Held messages are delivered in order."),
                &now,
                false,
            )?;
        }
        let mut event = MutationEvent::new("run_control", &kernel.project_id, run_version + 1)
            .transition(&run_state, &state_text);
        if reason.is_some() {
            event = event.with_details(json!({"reason": pause_reason}));
        }
        Ok(MutationOutput::new(json!({"state": state_text}), event))
    };
    if matches!(state_text.as_str(), "canceled" | "completed" | "failed") {
        kernel.mutate_as_controller(context, "run.transition", "run:control", &payload, apply)
    } else {
        kernel.mutate(
            context,
            "run.transition",
            "run:control",
            &payload,
            |actor| apply(actor, actor),
        )
    }
}

/// The open pause row of the run, or none.
fn open_run_pause(kernel: &Kernel) -> KernelResult<Option<PauseRecord>> {
    Ok(pause_rows(kernel)?.into_iter().find(|p| p.scope == "run"))
}

/// `pauseRows`: every open pause, oldest first.
pub fn pause_rows(kernel: &Kernel) -> KernelResult<Vec<PauseRecord>> {
    query_all(
        &kernel.database,
        "SELECT scope, agent_id, reason, actor_id, paused_at FROM pauses WHERE project_id = ? AND resumed_at IS NULL ORDER BY paused_at, pause_id",
        [&kernel.project_id],
        |row| {
            Ok(PauseRecord {
                scope: row.get(0)?,
                agent_id: row.get(1)?,
                reason: row.get(2)?,
                actor_id: row.get(3)?,
                paused_at: row.get(4)?,
            })
        },
    )
}

fn open_pause(
    kernel: &Kernel,
    scope: &str,
    agent_id: Option<&str>,
    reason: &str,
    actor_id: &str,
    now: &str,
) -> KernelResult<()> {
    execute(
        &kernel.database,
        "INSERT INTO pauses(project_id, pause_id, scope, agent_id, reason, actor_id, paused_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        rusqlite::params![
            kernel.project_id,
            kernel.env.uuid(),
            scope,
            agent_id,
            reason,
            actor_id,
            now
        ],
    )?;
    Ok(())
}

/// `closePause`.
pub fn close_pause(
    kernel: &Kernel,
    scope: &str,
    agent_id: Option<&str>,
    reason: &str,
    actor_id: &str,
    now: &str,
) -> KernelResult<()> {
    execute(
        &kernel.database,
        "UPDATE pauses SET resumed_at = ?, resume_reason = ?, resumed_by = ?
         WHERE project_id = ? AND scope = ? AND COALESCE(agent_id, '') = ? AND resumed_at IS NULL",
        rusqlite::params![
            now,
            reason,
            actor_id,
            kernel.project_id,
            scope,
            agent_id.unwrap_or("")
        ],
    )?;
    Ok(())
}

/// What is paused now: the run (while its state is paused) and each paused agent.
pub struct PauseState {
    pub run: Option<PauseRecord>,
    pub agents: Vec<PauseRecord>,
}

/// `pauseState`, typed.
pub fn pause_state_of(kernel: &Kernel) -> KernelResult<PauseState> {
    kernel.assert_open()?;
    let rows = pause_rows(kernel)?;
    let run_state = query_opt(
        &kernel.database,
        "SELECT state FROM run_controls WHERE project_id = ?",
        [&kernel.project_id],
        |row| row.get::<_, String>(0),
    )?;
    let run = if run_state.as_deref() == Some("paused") {
        rows.iter().find(|p| p.scope == "run").cloned()
    } else {
        None
    };
    Ok(PauseState {
        run,
        agents: rows.into_iter().filter(|p| p.scope == "agent").collect(),
    })
}

/// `pauseState`.
pub fn pause_state(kernel: &Kernel) -> KernelResult<Value> {
    let state = pause_state_of(kernel)?;
    Ok(json!({
        "run": state.run.as_ref().map_or(Value::Null, PauseRecord::to_value),
        "agents": state.agents.iter().map(PauseRecord::to_value).collect::<Vec<_>>(),
    }))
}

/// `isDeliveryPaused`, as a bool: messages to the agent are held when it is paused, or the run is paused and the agent is
/// not the PM (which coordinates and must see notices).
pub fn delivery_paused(kernel: &Kernel, agent_id: &str) -> KernelResult<bool> {
    kernel.assert_open()?;
    let state = pause_state_of(kernel)?;
    if state
        .agents
        .iter()
        .any(|p| p.agent_id.as_deref() == Some(agent_id))
    {
        return Ok(true);
    }
    if state.run.is_none() {
        return Ok(false);
    }
    Ok(!matches!(kernel.agent_row(agent_id)?, Some(agent) if agent.kind == "PM"))
}

/// `isDeliveryPaused`.
pub fn is_delivery_paused(kernel: &Kernel, agent_id: &str) -> KernelResult<Value> {
    delivery_paused(kernel, agent_id).map(Value::Bool)
}

/// `assertRunNotPaused`: refuses a step that must not start while the run is paused.
pub fn assert_run_not_paused(kernel: &Kernel, action: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    if let Some(run) = pause_state_of(kernel)?.run {
        return Err(KernelError::RunPaused(format!(
            "run_paused: {action} is refused while the run is paused: {}",
            run.reason
        )));
    }
    Ok(Value::Null)
}

/// `resumedMsFor`: the latest time a pause that covers the agent ended, in ms; 0 when none did.
pub fn resumed_ms_for(kernel: &Kernel, agent_id: &str) -> KernelResult<i64> {
    let at = query_opt(
        &kernel.database,
        "SELECT MAX(resumed_at) AS at FROM pauses WHERE project_id = ? AND resumed_at IS NOT NULL AND (scope = 'run' OR agent_id = ?)",
        [&kernel.project_id, agent_id],
        |row| row.get::<_, Option<String>>(0),
    )?
    .flatten();
    Ok(at.map_or(0, |text| parse_iso_ms(&text)))
}

/// `pauseAgent`.
pub fn pause_agent(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    agent_pause_change(kernel, context, input, true)
}

/// `resumeAgent`.
pub fn resume_agent(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    agent_pause_change(kernel, context, input, false)
}

fn agent_pause_change(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
    pausing: bool,
) -> KernelResult<Value> {
    let agent_id = safe_id(input.get("agentId").unwrap_or(&Value::Null), "agent id")?.to_string();
    let reason = pause_reason_text(input.get("reason").unwrap_or(&Value::Null))?;
    kernel.mutate(
        context,
        if pausing { "agent.pause" } else { "agent.resume" },
        "run:control",
        &json!({"agentId": agent_id, "reason": reason}),
        |actor| {
            let caller = kernel.agent_by_actor(&actor.actor_id)?;
            if actor.role != "operator" && !matches!(&caller, Some(c) if c.kind == "PM") {
                return Err(KernelError::controller(
                    "only the operator or the active PM pauses or resumes an agent",
                ));
            }
            let target = match kernel.agent_row(&agent_id)? {
                Some(target) if target.state == "active" => target,
                _ => return Err(KernelError::controller("the agent is not active")),
            };
            if pausing && matches!(&caller, Some(c) if c.agent_id == target.agent_id) {
                return Err(KernelError::controller("an agent cannot pause itself"));
            }
            let open = pause_rows(kernel)?
                .into_iter()
                .find(|p| p.scope == "agent" && p.agent_id.as_deref() == Some(&target.agent_id));
            let now = kernel.now();
            if pausing {
                if let Some(open) = &open {
                    return Err(KernelError::conflict(format!(
                        "agent {} is already paused: {}",
                        target.agent_id, open.reason
                    )));
                }
                open_pause(kernel, "agent", Some(&target.agent_id), &reason, &actor.actor_id, &now)?;
                if actor.role == "operator" {
                    message_notices::notice_to_pm(
                        kernel,
                        &format!("The operator paused {}: {reason}. Its messages are held until `cstan resume {} --reason \"<text>\"`.", target.agent_id, target.agent_id),
                        &now,
                        false,
                    )?;
                }
            } else {
                if open.is_none() {
                    return Err(KernelError::conflict(format!(
                        "agent {} is not paused",
                        target.agent_id
                    )));
                }
                close_pause(kernel, "agent", Some(&target.agent_id), &reason, &actor.actor_id, &now)?;
                if actor.role == "operator" {
                    message_notices::notice_to_pm(
                        kernel,
                        &format!("The operator resumed {}: {reason}. Its held messages are delivered in order.", target.agent_id),
                        &now,
                        false,
                    )?;
                }
            }
            let record = open.unwrap_or(PauseRecord {
                scope: "agent".into(),
                agent_id: Some(target.agent_id.clone()),
                reason: reason.clone(),
                actor_id: actor.actor_id.clone(),
                paused_at: now,
            });
            let (from, to) = if pausing { ("active", "paused") } else { ("paused", "active") };
            Ok(MutationOutput::new(
                record.to_value(),
                MutationEvent::new("agent", &target.agent_id, target.generation)
                    .transition(from, to)
                    .with_details(json!({"reason": reason, "scope": "agent"})),
            ))
        },
    )
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "transitionRun" => {
            let context = arg!(a.ctx(0));
            let to_state = a.value(1);
            let reason = arg!(a.opt_str(2, "reason"));
            Some(call(transition_run(kernel, &context, to_state, reason)))
        }
        "pauseState" => Some(call(pause_state(kernel))),
        "isDeliveryPaused" => {
            let agent_id = arg!(a.str(0, "agentId"));
            Some(call(is_delivery_paused(kernel, agent_id)))
        }
        "assertRunNotPaused" => {
            let action = arg!(a.str(0, "action"));
            Some(call(assert_run_not_paused(kernel, action)))
        }
        "pauseAgent" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(pause_agent(kernel, &context, input)))
        }
        "resumeAgent" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(resume_agent(kernel, &context, input)))
        }
        _ => None,
    }
}
