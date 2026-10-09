//! The finding and review commands (src/commands/findings.ts). A stub until its package: every route it serves answers `not_implemented` (marked unported).

use super::{register_stubs, HandlerMap};

/// Registers the handlers of the routes this module serves (finding, review).
pub fn register(map: &mut HandlerMap) {
    register_stubs(map, "findings");
}
