//! The agent panes (src/controller/panes.ts).
//!
//! Every public function returns `Err(KernelError::Unported("panes.<method>"))` until the package that owns this file fills
//! its body. The signatures and `dispatch` are fixed by the kernel foundation: fill the bodies, do not change them.

#![allow(unused_imports)]

use crate::dispatch::{arg, call, Args, ErrorHandler, RestartFilter};
use crate::errors::{KernelError, KernelResult};
use crate::kernel::Kernel;
use crate::types::MutationContext;
use serde_json::Value;

/// `recordAgentPane`.
#[allow(unused_variables)]
pub fn record_agent_pane(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported("panes.recordAgentPane".into()))
}

/// `clearAgentPane`.
#[allow(unused_variables)]
pub fn clear_agent_pane(
    kernel: &Kernel,
    context: &MutationContext,
    agent_id: &str,
) -> KernelResult<Value> {
    Err(KernelError::Unported("panes.clearAgentPane".into()))
}

/// `recordFallbackPane`.
#[allow(unused_variables)]
pub fn record_fallback_pane(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported("panes.recordFallbackPane".into()))
}

/// `clearFallbackPane`.
#[allow(unused_variables)]
pub fn clear_fallback_pane(kernel: &Kernel, context: &MutationContext) -> KernelResult<Value> {
    Err(KernelError::Unported("panes.clearFallbackPane".into()))
}

/// `recordOrphanPane`.
#[allow(unused_variables)]
pub fn record_orphan_pane(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    Err(KernelError::Unported("panes.recordOrphanPane".into()))
}

/// `clearOrphanPane`.
#[allow(unused_variables)]
pub fn clear_orphan_pane(
    kernel: &Kernel,
    context: &MutationContext,
    pane_id: &str,
) -> KernelResult<Value> {
    Err(KernelError::Unported("panes.clearOrphanPane".into()))
}

/// `orphanPanes`.
#[allow(unused_variables)]
pub fn orphan_panes(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("panes.orphanPanes".into()))
}

/// `orphanPaneTerminals`.
#[allow(unused_variables)]
pub fn orphan_pane_terminals(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("panes.orphanPaneTerminals".into()))
}

/// `agentPanes`.
#[allow(unused_variables)]
pub fn agent_panes(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("panes.agentPanes".into()))
}

/// `paneTerminalId`.
#[allow(unused_variables)]
pub fn pane_terminal_id(kernel: &Kernel, credential: &str, agent_id: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("panes.paneTerminalId".into()))
}

/// `fallbackPane`.
#[allow(unused_variables)]
pub fn fallback_pane(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    Err(KernelError::Unported("panes.fallbackPane".into()))
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "recordAgentPane" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(record_agent_pane(kernel, &context, input)))
        }
        "clearAgentPane" => {
            let context = arg!(a.ctx(0));
            let agent_id = arg!(a.str(1, "agentId"));
            Some(call(clear_agent_pane(kernel, &context, agent_id)))
        }
        "recordFallbackPane" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(record_fallback_pane(kernel, &context, input)))
        }
        "clearFallbackPane" => {
            let context = arg!(a.ctx(0));
            Some(call(clear_fallback_pane(kernel, &context)))
        }
        "recordOrphanPane" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(record_orphan_pane(kernel, &context, input)))
        }
        "clearOrphanPane" => {
            let context = arg!(a.ctx(0));
            let pane_id = arg!(a.str(1, "paneId"));
            Some(call(clear_orphan_pane(kernel, &context, pane_id)))
        }
        "orphanPanes" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(orphan_panes(kernel, credential)))
        }
        "orphanPaneTerminals" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(orphan_pane_terminals(kernel, credential)))
        }
        "agentPanes" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(agent_panes(kernel, credential)))
        }
        "paneTerminalId" => {
            let credential = arg!(a.str(0, "credential"));
            let agent_id = arg!(a.str(1, "agentId"));
            Some(call(pane_terminal_id(kernel, credential, agent_id)))
        }
        "fallbackPane" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(fallback_pane(kernel, credential)))
        }
        _ => None,
    }
}
