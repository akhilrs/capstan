//! The prompt relay command (src/commands/relay.ts: prompt). A stub until its package: every route it serves answers `not_implemented` (marked unported).

use super::{register_stubs, HandlerMap};

/// Registers the handlers of the routes this module serves (prompt).
pub fn register(map: &mut HandlerMap) {
    register_stubs(map, "relay");
}
