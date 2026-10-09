//! Status and ping (src/commands/status.ts and the inline ping of src/daemon.ts). A stub until its package: every route it serves answers `not_implemented` (marked unported).

use super::{register_stubs, HandlerMap};

/// Registers the handlers of the routes this module serves (status, ping).
pub fn register(map: &mut HandlerMap) {
    register_stubs(map, "status");
}
