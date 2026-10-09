//! The external links (src/controller/links.ts).
//!
//! Every public function returns `Err(KernelError::Unported("links.<method>"))` until the package that owns this file fills
//! its body. The signatures and `dispatch` are fixed by the kernel foundation: fill the bodies, do not change them.

#![allow(unused_imports)]

use crate::dispatch::{arg, call, Args, ErrorHandler, RestartFilter};
use crate::errors::{KernelError, KernelResult};
use crate::kernel::Kernel;
use crate::types::MutationContext;
use serde_json::Value;

/// `linkExternal`.
#[allow(unused_variables)]
pub fn link_external(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported("links.linkExternal".into()))
}

/// `bindRequirement`.
#[allow(unused_variables)]
pub fn bind_requirement(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported("links.bindRequirement".into()))
}

/// `taskNaming`.
#[allow(unused_variables)]
pub fn task_naming(kernel: &Kernel, credential: &str, reference: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("links.taskNaming".into()))
}

/// `agentHasReports`.
#[allow(unused_variables)]
pub fn agent_has_reports(kernel: &Kernel, agent_id: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("links.agentHasReports".into()))
}

/// `activeBranchHolder`.
#[allow(unused_variables)]
pub fn active_branch_holder(
    kernel: &Kernel,
    branch: &str,
    except_agent_id: Option<&str>,
) -> KernelResult<Value> {
    Err(KernelError::Unported("links.activeBranchHolder".into()))
}

/// `renameAgentBranch`.
#[allow(unused_variables)]
pub fn rename_agent_branch(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported("links.renameAgentBranch".into()))
}

/// `externalLinks`.
#[allow(unused_variables)]
pub fn external_links(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("links.externalLinks".into()))
}

/// `wantedNexoraState`.
#[allow(unused_variables)]
pub fn wanted_nexora_state(
    kernel: &Kernel,
    credential: &str,
    ref_kind: &Value,
    ref_id: &str,
) -> KernelResult<Value> {
    Err(KernelError::Unported("links.wantedNexoraState".into()))
}

/// `syncDrift`.
#[allow(unused_variables)]
pub fn sync_drift(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("links.syncDrift".into()))
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "linkExternal" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(link_external(kernel, &context, input)))
        }
        "bindRequirement" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(bind_requirement(kernel, &context, input)))
        }
        "taskNaming" => {
            let credential = arg!(a.str(0, "credential"));
            let reference = arg!(a.str(1, "ref"));
            Some(call(task_naming(kernel, credential, reference)))
        }
        "agentHasReports" => {
            let agent_id = arg!(a.str(0, "agentId"));
            Some(call(agent_has_reports(kernel, agent_id)))
        }
        "activeBranchHolder" => {
            let branch = arg!(a.str(0, "branch"));
            let except_agent_id = arg!(a.opt_str(1, "exceptAgentId"));
            Some(call(active_branch_holder(kernel, branch, except_agent_id)))
        }
        "renameAgentBranch" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(rename_agent_branch(kernel, &context, input)))
        }
        "externalLinks" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(external_links(kernel, credential)))
        }
        "wantedNexoraState" => {
            let credential = arg!(a.str(0, "credential"));
            let ref_kind = a.value(1);
            let ref_id = arg!(a.str(2, "refId"));
            Some(call(wanted_nexora_state(
                kernel, credential, ref_kind, ref_id,
            )))
        }
        "syncDrift" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(sync_drift(kernel, credential)))
        }
        _ => None,
    }
}
