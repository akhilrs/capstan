//! The worker reports (src/controller/reports.ts).
//!
//! Every public function returns `Err(KernelError::Unported("reports.<method>"))` until the package that owns this file fills
//! its body. The signatures and `dispatch` are fixed by the kernel foundation: fill the bodies, do not change them.

#![allow(unused_imports)]

use crate::dispatch::{arg, call, Args, ErrorHandler, RestartFilter};
use crate::errors::{KernelError, KernelResult};
use crate::kernel::Kernel;
use crate::types::MutationContext;
use serde_json::Value;

/// `recordAgentReport`.
#[allow(unused_variables)]
pub fn record_agent_report(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported("reports.recordAgentReport".into()))
}

/// `acceptedReportFor`.
#[allow(unused_variables)]
pub fn accepted_report_for(
    kernel: &Kernel,
    agent_id: &str,
    generation: i64,
    commit_sha: &str,
) -> KernelResult<Value> {
    Err(KernelError::Unported("reports.acceptedReportFor".into()))
}

/// `unannouncedReports`.
#[allow(unused_variables)]
pub fn unannounced_reports(
    kernel: &Kernel,
    credential: &str,
    limit: Option<i64>,
) -> KernelResult<Value> {
    Err(KernelError::Unported("reports.unannouncedReports".into()))
}

/// `announceReport`.
#[allow(unused_variables)]
pub fn announce_report(
    kernel: &Kernel,
    context: &MutationContext,
    report_id: &str,
) -> KernelResult<Value> {
    Err(KernelError::Unported("reports.announceReport".into()))
}

/// `agentReports`.
#[allow(unused_variables)]
pub fn agent_reports(kernel: &Kernel, credential: &str, limit: Option<i64>) -> KernelResult<Value> {
    Err(KernelError::Unported("reports.agentReports".into()))
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "recordAgentReport" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(record_agent_report(kernel, &context, input)))
        }
        "acceptedReportFor" => {
            let agent_id = arg!(a.str(0, "agentId"));
            let generation = arg!(a.i64(1, "generation"));
            let commit_sha = arg!(a.str(2, "commitSha"));
            Some(call(accepted_report_for(
                kernel, agent_id, generation, commit_sha,
            )))
        }
        "unannouncedReports" => {
            let credential = arg!(a.str(0, "credential"));
            let limit = arg!(a.opt_i64(1, "limit"));
            Some(call(unannounced_reports(kernel, credential, limit)))
        }
        "announceReport" => {
            let context = arg!(a.ctx(0));
            let report_id = arg!(a.str(1, "reportId"));
            Some(call(announce_report(kernel, &context, report_id)))
        }
        "agentReports" => {
            let credential = arg!(a.str(0, "credential"));
            let limit = arg!(a.opt_i64(1, "limit"));
            Some(call(agent_reports(kernel, credential, limit)))
        }
        _ => None,
    }
}
