//! The agent commands (src/commands/agents.ts, observe and peek of relay.ts, and the inline shutdown, ask and assign of src/daemon.ts). A stub until its package: every route it serves answers `not_implemented` (marked unported).

use super::{register_stubs, HandlerMap};

/// Registers the handlers of the routes this module serves (spawn, release, replace, launch, pm-restart, pause, resume, peek, observe, shutdown, assign, ask).
pub fn register(map: &mut HandlerMap) {
    register_stubs(map, "agents");
}
