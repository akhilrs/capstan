//! The operator socket: one connection per call, one JSON line each way.
use std::path::PathBuf;
use std::time::Duration;

use capstan_wire::{AfterSend, Timeout, WireError};
use serde_json::Value;

use crate::model::{to_wire_call, DashAction};
use crate::view::Link;

pub const MAX_FRAME_BYTES: usize = capstan_wire::MAX_FRAME_BYTES;
pub const MAX_RESPONSE_BYTES: usize = capstan_wire::MAX_RESPONSE_BYTES;
pub const TIMEOUT: Duration = capstan_wire::DEFAULT_TIMEOUT;

/// Why a call produced no daemon answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientError {
    /// The response line is longer than `MAX_RESPONSE_BYTES`.
    TooLarge,
    /// The request is longer than `MAX_FRAME_BYTES`; nothing was sent.
    RequestTooLarge,
    /// Not connected, closed early, timed out or not a reply.
    Unreachable,
}

impl ClientError {
    /// The link state the dashboard shows after a failed poll.
    pub fn link(self) -> Link {
        match self {
            ClientError::TooLarge => Link::Toolarge,
            _ => Link::Down,
        }
    }
}

/// The daemon's answer: `ok` with a result, or refused with a code and a message.
#[derive(Clone, Debug, PartialEq)]
pub enum Response {
    Ok(Value),
    Refused { code: String, message: String },
}

/// What a dashboard action got back, as the dashboard shows it.
#[derive(Clone, Debug, PartialEq)]
pub enum CallResult {
    Ok(Value),
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct Client {
    pub socket: PathBuf,
    credential: String,
    pub timeout: Duration,
}

pub const UNANSWERED: &str = "the controller did not answer";

impl Client {
    pub fn new(socket: PathBuf, credential: String) -> Self {
        Client {
            socket,
            credential,
            timeout: TIMEOUT,
        }
    }

    /// One request on a fresh connection.
    pub fn call(&self, command: &str, args: &[String]) -> Result<Response, ClientError> {
        let frame = capstan_wire::frame(&self.credential, command, args)
            .map_err(|_| ClientError::RequestTooLarge)?;
        let line = capstan_wire::call(&self.socket, &frame, Timeout::Deadline(self.timeout))
            .map_err(|error| match error {
                WireError::AfterSend(AfterSend::TooLarge) => ClientError::TooLarge,
                _ => ClientError::Unreachable,
            })?;
        parse_response(&line)
    }

    /// The operator `status` result; any refusal is "unreachable" for the link.
    pub fn status(&self) -> Result<Value, ClientError> {
        match self.call("status", &[])? {
            Response::Ok(result) if result.is_object() => Ok(result),
            _ => Err(ClientError::Unreachable),
        }
    }

    /// An action's daemon call with the failure texts the dashboard shows.
    pub fn run_action(&self, action: &DashAction) -> CallResult {
        let (command, args) = to_wire_call(action);
        self.call_text(&command, &args)
    }

    pub fn call_text(&self, command: &str, args: &[String]) -> CallResult {
        match self.call(command, args) {
            Ok(Response::Ok(result)) => CallResult::Ok(result),
            Ok(Response::Refused { message, .. }) => CallResult::Failed(message),
            Err(_) => CallResult::Failed(UNANSWERED.to_string()),
        }
    }
}

fn parse_response(line: &[u8]) -> Result<Response, ClientError> {
    let text = std::str::from_utf8(line).map_err(|_| ClientError::Unreachable)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let body: Value = serde_json::from_str(text).map_err(|_| ClientError::Unreachable)?;
    let Value::Object(map) = body else {
        return Err(ClientError::Unreachable);
    };
    match map.get("ok") {
        None => Err(ClientError::Unreachable),
        Some(Value::Bool(true)) => Ok(Response::Ok(
            map.get("result").cloned().unwrap_or(Value::Null),
        )),
        Some(_) => {
            let text_of = |key: &str| match map.get(key) {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Null) | None => String::new(),
                Some(other) => other.to_string(),
            };
            let code = text_of("code");
            let mut message = text_of("message");
            if message.is_empty() {
                message = code.clone();
            }
            Ok(Response::Refused { code, message })
        }
    }
}
