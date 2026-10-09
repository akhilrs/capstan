//! The message commands (src/commands/messages.ts: inbox, ack, send, cancel, resolve). A stub until its package: every route it serves answers `not_implemented` (marked unported).

use super::{register_stubs, HandlerMap};

/// Registers the handlers of the routes this module serves (inbox, ack, send, cancel, resolve).
pub fn register(map: &mut HandlerMap) {
    register_stubs(map, "messages");
}
