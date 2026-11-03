//! The cstan operator socket protocol, as `src/client.ts` speaks it, and the JavaScript-compatible JSON values the
//! front ends print. Everything here is checked byte for byte against Node by `tests/parity`.
pub mod js;
mod response;
mod socket_path;
mod transport;

pub use response::{frame, response, FrameError, Response};
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
/// The default per-call timeout.
pub const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
