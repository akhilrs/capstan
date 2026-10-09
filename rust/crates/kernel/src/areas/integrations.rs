//! The integrations (src/controller/integrations.ts).
//!
//! Every public function returns `Err(KernelError::Unported("integrations.<method>"))` until the package that owns this file fills
//! its body. The signatures and `dispatch` are fixed by the kernel foundation: fill the bodies, do not change them.

#![allow(unused_imports)]

use crate::dispatch::{arg, call, Args, ErrorHandler, RestartFilter};
use crate::errors::{KernelError, KernelResult};
use crate::kernel::Kernel;
use crate::types::MutationContext;
use serde_json::Value;

/// `beginIntegration`.
#[allow(unused_variables)]
pub fn begin_integration(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "integrations.beginIntegration".into(),
    ))
}

/// `finishIntegration`.
#[allow(unused_variables)]
pub fn finish_integration(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "integrations.finishIntegration".into(),
    ))
}

/// `settleIntegration`.
#[allow(unused_variables)]
pub fn settle_integration(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "integrations.settleIntegration".into(),
    ))
}

/// `integration`.
#[allow(unused_variables)]
pub fn integration(kernel: &Kernel, integration_id: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("integrations.integration".into()))
}

/// `integrationCommitInfo`.
#[allow(unused_variables)]
pub fn integration_commit_info(kernel: &Kernel, integration_id: &str) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "integrations.integrationCommitInfo".into(),
    ))
}

/// `plannedCommitInfo`.
#[allow(unused_variables)]
pub fn planned_commit_info(kernel: &Kernel, report_ids: &Value) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "integrations.plannedCommitInfo".into(),
    ))
}

/// `integrationBranchRecorded`.
#[allow(unused_variables)]
pub fn integration_branch_recorded(kernel: &Kernel, branch: &str) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "integrations.integrationBranchRecorded".into(),
    ))
}

/// `integrations`.
#[allow(unused_variables)]
pub fn integrations(kernel: &Kernel, credential: &str, limit: Option<i64>) -> KernelResult<Value> {
    Err(KernelError::Unported("integrations.integrations".into()))
}

/// `settledIntegrations`.
#[allow(unused_variables)]
pub fn settled_integrations(
    kernel: &Kernel,
    credential: &str,
    limit: Option<i64>,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "integrations.settledIntegrations".into(),
    ))
}

/// `coverageCandidates`.
#[allow(unused_variables)]
pub fn coverage_candidates(
    kernel: &Kernel,
    credential: &str,
    integration_id: &str,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "integrations.coverageCandidates".into(),
    ))
}

/// `recordCoveredReports`.
#[allow(unused_variables)]
pub fn record_covered_reports(
    kernel: &Kernel,
    credential: &str,
    integration_id: &str,
    covered: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "integrations.recordCoveredReports".into(),
    ))
}

/// `runningIntegrations`.
#[allow(unused_variables)]
pub fn running_integrations(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    Err(KernelError::Unported(
        "integrations.runningIntegrations".into(),
    ))
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "beginIntegration" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(begin_integration(kernel, &context, input)))
        }
        "finishIntegration" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(finish_integration(kernel, &context, input)))
        }
        "settleIntegration" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(settle_integration(kernel, &context, input)))
        }
        "integration" => {
            let integration_id = arg!(a.str(0, "integrationId"));
            Some(call(integration(kernel, integration_id)))
        }
        "integrationCommitInfo" => {
            let integration_id = arg!(a.str(0, "integrationId"));
            Some(call(integration_commit_info(kernel, integration_id)))
        }
        "plannedCommitInfo" => {
            let report_ids = a.value(0);
            Some(call(planned_commit_info(kernel, report_ids)))
        }
        "integrationBranchRecorded" => {
            let branch = arg!(a.str(0, "branch"));
            Some(call(integration_branch_recorded(kernel, branch)))
        }
        "integrations" => {
            let credential = arg!(a.str(0, "credential"));
            let limit = arg!(a.opt_i64(1, "limit"));
            Some(call(integrations(kernel, credential, limit)))
        }
        "settledIntegrations" => {
            let credential = arg!(a.str(0, "credential"));
            let limit = arg!(a.opt_i64(1, "limit"));
            Some(call(settled_integrations(kernel, credential, limit)))
        }
        "coverageCandidates" => {
            let credential = arg!(a.str(0, "credential"));
            let integration_id = arg!(a.str(1, "integrationId"));
            Some(call(coverage_candidates(
                kernel,
                credential,
                integration_id,
            )))
        }
        "recordCoveredReports" => {
            let credential = arg!(a.str(0, "credential"));
            let integration_id = arg!(a.str(1, "integrationId"));
            let covered = a.value(2);
            Some(call(record_covered_reports(
                kernel,
                credential,
                integration_id,
                covered,
            )))
        }
        "runningIntegrations" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(running_integrations(kernel, credential)))
        }
        _ => None,
    }
}
