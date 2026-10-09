//! The message state machine (src/controller/messaging.ts): pure functions over message and agent facts, ported by the
//! messages package. The foundation keeps only the constants it needs in `records.rs` (`FINAL_MESSAGE_STATES`,
//! `UNRESOLVED_MESSAGE_STATES`, `TIMER_NAMES`).

/// The state machine has no operation of its own; nothing is dispatched by name.
pub(crate) fn dispatch(
    _kernel: &crate::kernel::Kernel,
    _op: &str,
    _args: &[serde_json::Value],
) -> Option<crate::errors::KernelResult<serde_json::Value>> {
    None
}
