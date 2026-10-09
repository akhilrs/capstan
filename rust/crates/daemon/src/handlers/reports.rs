//! The report commands (src/commands/reports.ts: report, request-review, integrate). A stub until its package: every route it serves answers `not_implemented` (marked unported).

use super::{register_stubs, HandlerMap};

/// Registers the handlers of the routes this module serves (report, request-review, integrate).
pub fn register(map: &mut HandlerMap) {
    register_stubs(map, "reports");
}
