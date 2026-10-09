//! The operator grants (src/controller/operator-grants.ts).
//!
//! Every public function returns `Err(KernelError::Unported("operatorGrants.<method>"))` until the package that owns this file fills
//! its body. The signatures and `dispatch` are fixed by the kernel foundation: fill the bodies, do not change them.

#![allow(unused_imports)]

use crate::dispatch::{arg, call, Args, ErrorHandler, RestartFilter};
use crate::errors::{KernelError, KernelResult};
use crate::kernel::Kernel;
use crate::types::MutationContext;
use serde_json::Value;

/// `activeOperatorGrants`.
#[allow(unused_variables)]
pub fn active_operator_grants(kernel: &Kernel) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "operatorGrants.activeOperatorGrants".into(),
    ))
}

/// `listOperatorGrants`.
#[allow(unused_variables)]
pub fn list_operator_grants(kernel: &Kernel, limit: Option<i64>) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "operatorGrants.listOperatorGrants".into(),
    ))
}

/// `dueOperatorGrantExpiries`.
#[allow(unused_variables)]
pub fn due_operator_grant_expiries(
    kernel: &Kernel,
    also_due: Option<&[String]>,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "operatorGrants.dueOperatorGrantExpiries".into(),
    ))
}

/// `expireOperatorGrants`.
#[allow(unused_variables)]
pub fn expire_operator_grants(
    kernel: &Kernel,
    context: &MutationContext,
    also_due: Option<&[String]>,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "operatorGrants.expireOperatorGrants".into(),
    ))
}

/// `endOperatorGrantsForRestart`.
#[allow(unused_variables)]
pub fn end_operator_grants_for_restart(
    kernel: &Kernel,
    context: &MutationContext,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "operatorGrants.endOperatorGrantsForRestart".into(),
    ))
}

/// `revokeOperatorGrant`.
#[allow(unused_variables)]
pub fn revoke_operator_grant(
    kernel: &Kernel,
    context: &MutationContext,
    grant_id: &str,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "operatorGrants.revokeOperatorGrant".into(),
    ))
}

/// `recordOperatorFullAuto`.
#[allow(unused_variables)]
pub fn record_operator_full_auto(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "operatorGrants.recordOperatorFullAuto".into(),
    ))
}

/// `endFullAutoProposal`.
#[allow(unused_variables)]
pub fn end_full_auto_proposal(
    kernel: &Kernel,
    context: &MutationContext,
    proposal_id: &str,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "operatorGrants.endFullAutoProposal".into(),
    ))
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "activeOperatorGrants" => Some(call(active_operator_grants(kernel))),
        "listOperatorGrants" => {
            let limit = arg!(a.opt_i64(0, "limit"));
            Some(call(list_operator_grants(kernel, limit)))
        }
        "dueOperatorGrantExpiries" => {
            let also = arg!(a.opt_str_list(0));
            Some(call(due_operator_grant_expiries(kernel, also.as_deref())))
        }
        "expireOperatorGrants" => {
            let context = arg!(a.ctx(0));
            let also = arg!(a.opt_str_list(1));
            Some(call(expire_operator_grants(
                kernel,
                &context,
                also.as_deref(),
            )))
        }
        "endOperatorGrantsForRestart" => {
            let context = arg!(a.ctx(0));
            Some(call(end_operator_grants_for_restart(kernel, &context)))
        }
        "revokeOperatorGrant" => {
            let context = arg!(a.ctx(0));
            let grant_id = arg!(a.str(1, "grantId"));
            Some(call(revoke_operator_grant(kernel, &context, grant_id)))
        }
        "recordOperatorFullAuto" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(record_operator_full_auto(kernel, &context, input)))
        }
        "endFullAutoProposal" => {
            let context = arg!(a.ctx(0));
            let proposal_id = arg!(a.str(1, "proposalId"));
            Some(call(end_full_auto_proposal(kernel, &context, proposal_id)))
        }
        _ => None,
    }
}
