//! The integration pipeline (src/integration.ts): merging reports into an integration branch and settling it. It shells
//! out to `git`; the integrate package fills these bodies. Each returns `Err(KernelError::Unported("integrate.<fn>"))`
//! until then.

use crate::errors::{KernelError, KernelResult};
use crate::kernel::Kernel;
use serde_json::Value;

/// `squashMessage`: the squash commit message of an integration (subject and one entry per report).
#[allow(unused_variables)]
pub fn squash_message(kernel: &Kernel, input: &Value) -> KernelResult<Value> {
    Err(KernelError::Unported("integrate.squashMessage".into()))
}

/// `integrate`: merges the reports of an integration into a branch off the base.
#[allow(unused_variables)]
pub fn integrate(kernel: &Kernel, input: &Value) -> KernelResult<Value> {
    Err(KernelError::Unported("integrate.integrate".into()))
}

/// `settleIntegration`: confirms or discards a merged integration.
#[allow(unused_variables)]
pub fn settle_integration(kernel: &Kernel, input: &Value) -> KernelResult<Value> {
    Err(KernelError::Unported("integrate.settleIntegration".into()))
}

/// `recoverIntegrations`: settles the integrations a restart interrupted.
#[allow(unused_variables)]
pub fn recover_integrations(kernel: &Kernel, input: &Value) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "integrate.recoverIntegrations".into(),
    ))
}
