//! The Operator command (src/commands/operator.ts: op). A stub until its package: every route it serves answers `not_implemented` (marked unported).

use super::{register_stubs, HandlerMap};

/// Registers the handlers of the routes this module serves (op).
pub fn register(map: &mut HandlerMap) {
    register_stubs(map, "operator");
}
