//! The notify command (`cstan notify test`): sends one test notification through the ntfy sender and answers how it went.
//! The operator is the only caller; agents get `forbidden` from the route table.

use super::shared::{fail, ok, CommandCall, CommandEnv, CommandResponse, ErrorCode};
use super::HandlerMap;

/// How long a test may take: the longest curl run (`timeout_seconds` is at most 60) plus its kill grace, and a margin.
pub const NOTIFY_LIMIT_MS: u64 = 70_000;

/// Registers the handler of the route this module serves (notify).
pub fn register(map: &mut HandlerMap) {
    map.insert("notify", notify);
}

fn notify(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    if call.args != ["test"] {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "notify takes one word: test",
        ));
    }
    let Some(sender) = env.deps.ntfy() else {
        return Some(fail(
            ErrorCode::NotConfigured,
            "ntfy is not enabled in capstan.toml",
        ));
    };
    Some(match sender.send_test() {
        Ok(answer) => ok(answer.to_value()),
        Err(_) => fail(ErrorCode::Busy, "a test notification is already running"),
    })
}
