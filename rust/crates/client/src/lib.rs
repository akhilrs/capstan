//! The client of the controller daemon: the Rust port of `callDaemon`, `pingDaemon`, `ensureDaemon` and `stopDaemon` of
//! src/client.ts, with the same socket protocol, timings, error codes and texts. `ensure_daemon` starts `<cstan> daemon`
//! detached, with its output appended to `daemon.log`, and waits for the daemon to answer a ping on the socket.

mod ensure;
mod log;

pub use capstan_wire::Response;
pub use ensure::{ensure_daemon, stop_daemon, EnsureOptions, Started};
pub use log::{log_tail, open_daemon_log};

use capstan_wire::{AfterSend, Timeout, WireError};
use std::fmt;
use std::path::Path;
use std::time::Duration;

/// A call without a response: Node's error with its errno-style `code` (`ENOENT`, `ECONNREFUSED`, `ETIMEDOUT`,
/// `ECONNRESET`, `EMSGSIZE`, `EBADMSG`, ...).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallError {
    pub code: String,
    pub message: String,
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CallError {}

fn call_error(code: &str, message: &str) -> CallError {
    CallError {
        code: code.to_string(),
        message: message.to_string(),
    }
}

/// How long a call may go without a byte moving.
pub const DEFAULT_TIMEOUT_MS: u64 = 5_000;

/// One command on a fresh connection to the daemon at `socket`. A response with `ok` false is `Response::Refused`, not
/// an error; only a call that gets no response line is.
pub fn call_daemon(
    socket: &Path,
    credential: &str,
    command: &str,
    args: &[String],
    timeout_ms: u64,
) -> Result<Response, CallError> {
    let frame = capstan_wire::frame(credential, command, args)
        .map_err(|_| call_error("ERR_INVALID_ARG_VALUE", "command request is too large"))?;
    let timeout = Timeout::Idle(Duration::from_millis(timeout_ms));
    let line = capstan_wire::call(socket, &frame, timeout).map_err(|error| match error {
        WireError::NotSent { code, .. } => {
            let message = if code.is_empty() {
                "connect failed".to_string()
            } else {
                format!("connect {code} {}", socket.display())
            };
            CallError { code, message }
        }
        WireError::AfterSend(AfterSend::TimedOut) => call_error("ETIMEDOUT", "timed out"),
        WireError::AfterSend(AfterSend::Closed) => call_error("ECONNRESET", "connection closed"),
        WireError::AfterSend(AfterSend::TooLarge) => call_error("EMSGSIZE", "response too large"),
        WireError::AfterSend(AfterSend::Malformed) => call_error("EBADMSG", "malformed reply"),
        WireError::AfterSend(AfterSend::Io(code)) => CallError {
            message: code.clone(),
            code,
        },
    })?;
    match capstan_wire::response(&line) {
        Response::Malformed => Err(call_error("EBADMSG", "malformed reply")),
        response => Ok(response),
    }
}

/// What a ping found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PingOutcome {
    Running {
        pid: i64,
    },
    /// The daemon answered with `ok` false; `code` is its error code.
    Refused {
        code: String,
    },
    Down,
    Unreachable {
        reason: String,
    },
}

pub fn ping_daemon(socket: &Path, credential: &str, timeout_ms: u64) -> PingOutcome {
    match call_daemon(socket, credential, "ping", &[], timeout_ms) {
        Ok(Response::Ok { result }) => {
            let pid = match result {
                Some(capstan_wire::js::Value::Object(members)) => members
                    .iter()
                    .find(|(key, _)| key.to_utf8_lossy() == "pid")
                    .and_then(|(_, value)| match value {
                        capstan_wire::js::Value::Number(n) => Some(*n as i64),
                        _ => None,
                    }),
                _ => None,
            };
            PingOutcome::Running {
                pid: pid.unwrap_or(0),
            }
        }
        Ok(Response::Refused { code, .. }) => PingOutcome::Refused {
            code: code.to_utf8_lossy(),
        },
        Ok(Response::Malformed) => PingOutcome::Unreachable {
            reason: "EBADMSG".to_string(),
        },
        Err(error) if error.code == "ENOENT" || error.code == "ECONNREFUSED" => PingOutcome::Down,
        Err(error) => PingOutcome::Unreachable {
            reason: if error.code.is_empty() {
                "unknown".to_string()
            } else {
                error.code
            },
        },
    }
}

/// Why the controller is not usable (`ControllerUnavailableError`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Refused,
    Unreachable,
    StartTimeout,
    StartFailed,
}

impl Reason {
    /// The reason as Node names it.
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Refused => "refused",
            Reason::Unreachable => "unreachable",
            Reason::StartTimeout => "start_timeout",
            Reason::StartFailed => "start_failed",
        }
    }
}

/// `ControllerUnavailableError`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControllerUnavailable {
    pub reason: Reason,
    pub message: String,
}

impl fmt::Display for ControllerUnavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ControllerUnavailable {}

pub(crate) fn unavailable_reason(
    reason: Reason,
    message: impl Into<String>,
) -> ControllerUnavailable {
    ControllerUnavailable {
        reason,
        message: message.into(),
    }
}

/// Everything `ensure_daemon` and `stop_daemon` can fail with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientError {
    Unavailable(ControllerUnavailable),
    /// The daemon log or another file operation failed, as Node's raw error text.
    Io(String),
    Call(CallError),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::Unavailable(error) => error.fmt(f),
            ClientError::Io(message) => f.write_str(message),
            ClientError::Call(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<ControllerUnavailable> for ClientError {
    fn from(error: ControllerUnavailable) -> Self {
        ClientError::Unavailable(error)
    }
}

impl From<CallError> for ClientError {
    fn from(error: CallError) -> Self {
        ClientError::Call(error)
    }
}

/// The outcome of a ping that is not a running daemon, as an error; `None` for down.
pub(crate) fn unavailable(outcome: &PingOutcome) -> Option<ControllerUnavailable> {
    match outcome {
        PingOutcome::Refused { code } => Some(unavailable_reason(
            Reason::Refused,
            format!("the daemon refused the request ({code}); check the operator credential"),
        )),
        PingOutcome::Unreachable { .. } => Some(unavailable_reason(
            Reason::Unreachable,
            "the controller did not answer; check the daemon log or retry",
        )),
        _ => None,
    }
}
