//! The cstan operator socket protocol, as `src/client.ts` speaks it, and the JavaScript-compatible JSON values the
//! front ends print. Everything here is checked byte for byte against Node by `tests/parity`.
pub mod js;
mod response;
mod transport;

pub use response::{frame, response, FrameError, Response};
pub use transport::{call, AfterSend, Timeout, WireError};

/// The longest request frame, without its newline.
pub const MAX_FRAME_BYTES: usize = 65_536;
/// The longest response line, without its newline.
pub const MAX_RESPONSE_BYTES: usize = 1_048_576;
/// The default per-call timeout.
pub const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
