//! The agent records and lifecycle (src/controller/agents.ts).
//!
//! Every public function returns `Err(KernelError::Unported("agents.<method>"))` until the package that owns this file fills
//! its body. The signatures and `dispatch` are fixed by the kernel foundation: fill the bodies, do not change them.

#![allow(unused_imports)]

use crate::auth::authenticate_actor;
use crate::dispatch::{arg, call, Args};
use crate::errors::{KernelError, KernelResult};
use crate::kernel::Kernel;
use crate::types::MutationContext;
use serde_json::{json, Value};

/// `registerAgent`.
#[allow(unused_variables)]
pub fn register_agent(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported("agents.registerAgent".into()))
}

/// `identify`.
#[allow(unused_variables)]
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
#[allow(unused_variables)]
pub fn list_agents(kernel: &Kernel) -> KernelResult<Value> {
    kernel.assert_open()?;
    Ok(serde_json::to_value(kernel.agent_records(false)?)?)
}

/// `activeAgents`.
#[allow(unused_variables)]
pub fn active_agents(kernel: &Kernel) -> KernelResult<Value> {
    kernel.assert_open()?;
    Ok(serde_json::to_value(kernel.agent_records(true)?)?)
}

/// `agentRecord`.
#[allow(unused_variables)]
pub fn agent_record(kernel: &Kernel, agent_id: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    Ok(match kernel.agent_record(agent_id)? {
        Some(record) => serde_json::to_value(record)?,
        None => Value::Null,
    })
}

/// `recordAgentObservation`.
#[allow(unused_variables)]
pub fn record_agent_observation(
    kernel: &Kernel,
    context: &MutationContext,
    agent_id: &str,
    state: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "agents.recordAgentObservation".into(),
    ))
}

/// `endAgent`.
#[allow(unused_variables)]
pub fn end_agent(
    kernel: &Kernel,
    context: &MutationContext,
    agent_id: &str,
    options: Option<&Value>,
) -> KernelResult<Value> {
    Err(KernelError::Unported("agents.endAgent".into()))
}

/// `replaceAgentGeneration`.
#[allow(unused_variables)]
pub fn replace_agent_generation(
    kernel: &Kernel,
    context: &MutationContext,
    agent_id: &str,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "agents.replaceAgentGeneration".into(),
    ))
}

/// `restartAgentGeneration`.
#[allow(unused_variables)]
pub fn restart_agent_generation(
    kernel: &Kernel,
    context: &MutationContext,
    agent_id: &str,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "agents.restartAgentGeneration".into(),
    ))
}

/// `markPmRestartsConsumed`.
#[allow(unused_variables)]
pub fn mark_pm_restarts_consumed(
    kernel: &Kernel,
    context: &MutationContext,
    agent_id: &str,
    up_to_sequence: i64,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "agents.markPmRestartsConsumed".into(),
    ))
}

/// `pmRestarts`.
#[allow(unused_variables)]
pub fn pm_restarts(kernel: &Kernel, credential: &str, agent_id: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("agents.pmRestarts".into()))
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
