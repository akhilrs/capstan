//! The operator proposals (src/controller/operator-proposals.ts).
//!
//! Every public function returns `Err(KernelError::Unported("operatorProposals.<method>"))` until the package that owns this file fills
//! its body. The signatures and `dispatch` are fixed by the kernel foundation: fill the bodies, do not change them.

#![allow(unused_imports)]

use crate::dispatch::{arg, call, Args, ErrorHandler, RestartFilter};
use crate::errors::{KernelError, KernelResult};
use crate::kernel::Kernel;
use crate::types::MutationContext;
use serde_json::Value;

/// `operatorProposal`.
#[allow(unused_variables)]
pub fn operator_proposal(kernel: &Kernel, proposal_id: &str) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "operatorProposals.operatorProposal".into(),
    ))
}

/// `listOperatorProposals`.
#[allow(unused_variables)]
pub fn list_operator_proposals(kernel: &Kernel, filter: Option<&Value>) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "operatorProposals.listOperatorProposals".into(),
    ))
}

/// `pendingOperatorProposalCount`.
#[allow(unused_variables)]
pub fn pending_operator_proposal_count(kernel: &Kernel, agent_id: &str) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "operatorProposals.pendingOperatorProposalCount".into(),
    ))
}

/// `approvedOperatorProposals`.
#[allow(unused_variables)]
pub fn approved_operator_proposals(kernel: &Kernel) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "operatorProposals.approvedOperatorProposals".into(),
    ))
}

/// `runningOperatorProposal`.
#[allow(unused_variables)]
pub fn running_operator_proposal(kernel: &Kernel) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "operatorProposals.runningOperatorProposal".into(),
    ))
}

/// `proposeOperatorAction`.
#[allow(unused_variables)]
pub fn propose_operator_action(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "operatorProposals.proposeOperatorAction".into(),
    ))
}

/// `decideOperatorProposal`.
#[allow(unused_variables)]
pub fn decide_operator_proposal(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "operatorProposals.decideOperatorProposal".into(),
    ))
}

/// `cancelOperatorProposal`.
#[allow(unused_variables)]
pub fn cancel_operator_proposal(
    kernel: &Kernel,
    context: &MutationContext,
    proposal_id: &str,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "operatorProposals.cancelOperatorProposal".into(),
    ))
}

/// `dueOperatorExpiries`.
#[allow(unused_variables)]
pub fn due_operator_expiries(kernel: &Kernel, limits: &Value) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "operatorProposals.dueOperatorExpiries".into(),
    ))
}

/// `expireOperatorProposals`.
#[allow(unused_variables)]
pub fn expire_operator_proposals(
    kernel: &Kernel,
    context: &MutationContext,
    limits: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "operatorProposals.expireOperatorProposals".into(),
    ))
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "operatorProposal" => {
            let proposal_id = arg!(a.str(0, "proposalId"));
            Some(call(operator_proposal(kernel, proposal_id)))
        }
        "listOperatorProposals" => {
            let filter = a.opt_value(0);
            Some(call(list_operator_proposals(kernel, filter)))
        }
        "pendingOperatorProposalCount" => {
            let agent_id = arg!(a.str(0, "agentId"));
            Some(call(pending_operator_proposal_count(kernel, agent_id)))
        }
        "approvedOperatorProposals" => Some(call(approved_operator_proposals(kernel))),
        "runningOperatorProposal" => Some(call(running_operator_proposal(kernel))),
        "proposeOperatorAction" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(propose_operator_action(kernel, &context, input)))
        }
        "decideOperatorProposal" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(decide_operator_proposal(kernel, &context, input)))
        }
        "cancelOperatorProposal" => {
            let context = arg!(a.ctx(0));
            let proposal_id = arg!(a.str(1, "proposalId"));
            Some(call(cancel_operator_proposal(
                kernel,
                &context,
                proposal_id,
            )))
        }
        "dueOperatorExpiries" => {
            let limits = a.value(0);
            Some(call(due_operator_expiries(kernel, limits)))
        }
        "expireOperatorProposals" => {
            let context = arg!(a.ctx(0));
            let limits = a.value(1);
            Some(call(expire_operator_proposals(kernel, &context, limits)))
        }
        _ => None,
    }
}
