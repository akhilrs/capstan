//! Request framing and the decision on a response line.
use crate::js::{self, JsStr, Value};
use crate::MAX_FRAME_BYTES;

/// The request is longer than `MAX_FRAME_BYTES`; nothing is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameError;

/// `JSON.stringify({v: 1, credential, command, args}) + "\n"`, refused above `MAX_FRAME_BYTES` (the newline not counted).
pub fn frame(credential: &str, command: &str, args: &[String]) -> Result<Vec<u8>, FrameError> {
    let value = Value::Object(vec![
        (JsStr::from("v"), Value::Number(1.0)),
        (JsStr::from("credential"), Value::String(credential.into())),
        (JsStr::from("command"), Value::String(command.into())),
        (
            JsStr::from("args"),
            Value::Array(
                args.iter()
                    .map(|a| Value::String(a.as_str().into()))
                    .collect(),
            ),
        ),
    ]);
    let mut bytes = js::stringify(&value, 0).to_utf8_lossy().into_bytes();
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(FrameError);
    }
    bytes.push(b'\n');
    Ok(bytes)
}

/// What a response line means.
#[derive(Clone, Debug, PartialEq)]
pub enum Response {
    /// `ok` is truthy; `result` is the member of that name (`None` when absent).
    Ok { result: Option<Value> },
    /// `ok` is falsy; `code` and `message` are the members as text (strings as they are, other values as
    /// the CLI renders them, absent or null as empty).
    Refused { code: JsStr, message: JsStr },
    /// Not UTF-8, not JSON, not an object, or an object without an `ok` member.
    Malformed,
}

/// Decides a response line exactly as `callDaemon` does: strict UTF-8 (a BOM is kept, so it is not JSON), `JSON.parse`,
/// a plain object with an `ok` member.
pub fn response(line: &[u8]) -> Response {
    let Ok(text) = std::str::from_utf8(line) else {
        return Response::Malformed;
    };
    let Ok(Value::Object(members)) = js::parse(text) else {
        return Response::Malformed;
    };
    let get = |key: &str| {
        members
            .iter()
            .find(|(k, _)| k.as_units() == JsStr::from(key).as_units())
            .map(|(_, v)| v)
    };
    let Some(ok) = get("ok") else {
        return Response::Malformed;
    };
    if ok.truthy() {
        return Response::Ok {
            result: get("result").cloned(),
        };
    }
    let text_of = |key: &str| match get(key) {
        None | Some(Value::Null) => JsStr::default(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => js::render(other),
    };
    Response::Refused {
        code: text_of("code"),
        message: text_of("message"),
    }
}
