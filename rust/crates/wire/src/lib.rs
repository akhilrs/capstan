//! The cstan operator socket protocol, as `src/client.ts` speaks it, and the JavaScript-compatible JSON values the
//! front ends print. Everything here is checked byte for byte against Node by `tests/parity`.
pub mod js;
mod response;
mod retry;
mod socket_path;
mod transport;

pub use response::{busy_retry_after, frame, frame_with_rid, response, FrameError, Response};
pub use retry::{call_retrying, new_request_id, RetryFailed, RetryPolicy, Retried};
pub use socket_path::{
    check_private_dir, control_socket, control_socket_in, ensure_private_dir, runtime_dir_name,
    socket_matches, SOCKET_NAME,
};
pub use transport::{
    call, call_in, check_socket_path, nothing_listens, socket_path_limit, AfterSend, Timeout,
    WireError,
};

/// The longest request frame, without its newline.
pub const MAX_FRAME_BYTES: usize = 65_536;
/// The longest response line, without its newline.
pub const MAX_RESPONSE_BYTES: usize = 1_048_576;
/// The error code of a connection the daemon refused for being over a limit; the client retries on it.
pub const CONTROLLER_BUSY: &str = "controller_busy";
/// Commands whose answer may carry a credential or a nonce: the reply cache keeps them as `Kept`, never as text.
pub const KEPT_COMMANDS: [&str; 7] = [
    "spawn",
    "launch",
    "replace",
    "pm-restart",
    "op",
    "decide",
    "decide-merge",
];
/// Commands whose lost reply the client never retries: [`KEPT_COMMANDS`] (no replayable answer) plus `wait` and
/// `notify` (a long wait). Built from [`KEPT_COMMANDS`], so a command added there is never retried either.
pub const NO_LOST_REPLY_RETRY: [&str; 9] = {
    let mut all = [""; 9];
    let mut i = 0;
    while i < KEPT_COMMANDS.len() {
        all[i] = KEPT_COMMANDS[i];
        i += 1;
    }
    all[7] = "wait";
    all[8] = "notify";
    all
};
/// The default per-call timeout.
pub const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
