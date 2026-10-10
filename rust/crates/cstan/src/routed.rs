//! The commands the daemon answers (`runRouted` and `handleWire` of the Node CLI): agent commands over the agent's own
//! socket and token, operator commands over the project's socket with the operator credential, and `inbox --hook`.
use std::time::Duration;

use capstan_wire::js::{self, JsStr, Value};
use capstan_wire::{call, frame, response, AfterSend, Response, Timeout, WireError};

use crate::agent::{socket_verdict, var, Verdict};
use crate::jsops::{self, as_number, concat, JsError, EXIT_BLOCKED, EXIT_INVALID, EXIT_RUNTIME};
use crate::{blocked, invalid, output, render, Cli, Fail, Flow};

const WAIT_SECONDS: u64 = 3600 + 30;
const HOOK_TIMEOUT: Duration = Duration::from_secs(2);

/// How a route is reached: with the agent's token, with the operator's, or with whichever the shell has.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
    Read,
    Any,
    Agent,
    Operator,
}

/// The `ROUTES` of src/daemon.ts the CLI forwards (all but `status`, `ping`, `shutdown`, `cancel`, `peek`, `pm-restart`
/// and `launch`, which the CLI words itself).
const ROUTED: &[(&str, Access)] = &[
    ("inbox", Access::Any),
    ("ack", Access::Agent),
    ("wait", Access::Agent),
    ("report", Access::Agent),
    ("ask", Access::Agent),
    ("request-review", Access::Agent),
    ("integrate", Access::Any),
    ("plan", Access::Any),
    ("op", Access::Any),
    ("link", Access::Any),
    ("review", Access::Agent),
    ("finding", Access::Agent),
    ("observe", Access::Agent),
    ("prompt", Access::Agent),
    ("assign", Access::Operator),
    ("send", Access::Any),
    ("pause", Access::Any),
    ("resume", Access::Any),
    ("spawn", Access::Any),
    ("release", Access::Any),
    ("replace", Access::Any),
    ("resolve", Access::Operator),
];

pub fn is_routed(command: &str) -> bool {
    ROUTED.iter().any(|(name, _)| *name == command)
}

fn access_of(command: &str) -> Access {
    match command {
        "status" | "ping" => Access::Read,
        "cancel" | "pm-restart" => Access::Operator,
        _ => ROUTED
            .iter()
            .find(|(name, _)| *name == command)
            .map_or(Access::Any, |(_, access)| *access),
    }
}

/// `callDaemon`'s timeout for the command, in milliseconds.
fn client_timeout_ms(command: &str) -> u64 {
    match command {
        "wait" => WAIT_SECONDS * 1000,
        "replace" => 1_800_000,
        "spawn" | "request-review" | "plan" | "integrate" | "release" | "pm-restart" => 600_000,
        _ => capstan_wire::DEFAULT_TIMEOUT.as_millis() as u64,
    }
}

fn after_send_message(error: &AfterSend) -> String {
    match error {
        AfterSend::TimedOut => "timed out".into(),
        AfterSend::Closed => "connection closed".into(),
        AfterSend::TooLarge => "response too large".into(),
        AfterSend::Malformed => "malformed reply".into(),
        AfterSend::Io(code) if code == "EPIPE" => format!("write {code}"),
        AfterSend::Io(code) => format!("read {code}"),
    }
}

/// What `handleWire` throws for a refusal: `invalid_request` and `error` carry the daemon's message (as `new Error(message)`
/// makes it: absent is empty), any other code is `code: message` with each part as a template literal shows it.
fn refusal(line: &[u8]) -> JsError {
    let members = std::str::from_utf8(line)
        .ok()
        .and_then(|text| js::parse(text).ok())
        .unwrap_or(Value::Null);
    let (code, message) = (members.get("code"), members.get("message"));
    let message_text = |absent: &str| match message {
        None => JsStr::from(absent),
        Some(v) => jsops::to_js_string(v),
    };
    match code {
        Some(Value::String(c)) if c.to_utf8_lossy() == "invalid_request" => JsError {
            exit: EXIT_INVALID,
            message: message_text(""),
        },
        Some(Value::String(c)) if c.to_utf8_lossy() == "error" => JsError {
            exit: EXIT_RUNTIME,
            message: message_text(""),
        },
        _ => JsError {
            exit: EXIT_BLOCKED,
            message: concat(&[
                &jsops::template(code),
                &": ".into(),
                &jsops::template(message),
            ]),
        },
    }
}

fn text(out: &mut Vec<u8>, line: &JsStr) {
    out.extend_from_slice(line.to_utf8_lossy().as_bytes());
    out.push(b'\n');
}

impl Cli<'_> {
    /// `runRouted`.
    pub fn run_routed(&mut self, command: &str, args: &[String], json: bool) -> Result<Flow, Fail> {
        if args.iter().any(String::is_empty) {
            return Err(invalid("command arguments must not be empty"));
        }
        if args.iter().any(|a| a.contains('\u{fffd}')) {
            return Err(invalid(
                "a command argument holds a replacement character, so its text was not valid UTF-8",
            ));
        }
        let access = access_of(command);
        let agent = if access == Access::Operator {
            None
        } else {
            self.agent_environment()?
        };
        let use_agent = access == Access::Agent
            || (matches!(access, Access::Read | Access::Any) && agent.is_some());
        let (socket, credential) = if use_agent {
            let Some(agent) = &agent else {
                return Err(invalid(
                    "this command must be run by an agent (CAPSTAN_TOKEN and CAPSTAN_SOCKET are not set)",
                ));
            };
            (agent.socket.clone(), agent.token.clone())
        } else {
            let operator = self.ensure_running()?;
            (operator.socket_path, operator.credential)
        };
        let request = frame(&credential, command, args)
            .map_err(|_| invalid("command request is too large"))?;
        let timeout = Timeout::Idle(Duration::from_millis(client_timeout_ms(command)));
        let reply = call(&socket, &request, timeout);
        if let Err(WireError::NotSent { code, .. }) = &reply {
            if code == "ENOENT" || code == "ECONNREFUSED" {
                return Err(blocked(if agent.is_some() {
                    "the controller is not running; ask the operator to run cstan start"
                } else {
                    "the controller stopped answering; run the command again to restart it"
                }));
            }
        }
        let now_ms = self.ctx.now_ms;
        handle_wire(reply, &socket, json, command, now_ms, &mut self.io)?;
        Ok(Flow::Code(0))
    }

    /// `runInboxHook`: silent and exit 0 whatever happens, but for the one line of context when messages wait.
    pub fn inbox_hook(&mut self) -> Result<Flow, Fail> {
        let env = &self.ctx.env;
        let text = |name: &str| {
            var(env, name)
                .map(|v| v.to_string_lossy().into_owned())
                .filter(|v| !v.is_empty())
        };
        let (Some(token), Some(socket), Some(_)) = (
            text("CAPSTAN_TOKEN"),
            text("CAPSTAN_SOCKET"),
            text("CAPSTAN_AGENT_ID"),
        ) else {
            return Ok(Flow::Code(0));
        };
        if !socket.starts_with('/') {
            return Ok(Flow::Code(0));
        }
        let socket = std::path::Path::new(&socket);
        if matches!(
            socket_verdict(&self.ctx.cwd, socket),
            Verdict::Foreign { .. }
        ) {
            return Ok(Flow::Code(0));
        }
        let Ok(request) = frame(&token, "inbox", &["--hook".to_string()]) else {
            return Ok(Flow::Code(0));
        };
        let Ok(line) = call(socket, &request, Timeout::Idle(HOOK_TIMEOUT)) else {
            return Ok(Flow::Code(0));
        };
        let Response::Ok { result } = response(&line) else {
            return Ok(Flow::Code(0));
        };
        let Some(count) = as_number(jsops::read_opt(result.as_ref(), "count")).filter(|n| *n > 0.0)
        else {
            return Ok(Flow::Code(0));
        };
        let minutes = render::oldest_minutes(
            jsops::read_opt(result.as_ref(), "oldestQueuedAt"),
            self.ctx.now_ms,
        );
        let message = format!(
            "{} Capstan message(s) are waiting for you (oldest {} min). Run cstan inbox now and ack each one before you continue or report.",
            js::number_to_string(count),
            js::number_to_string(minutes),
        );
        self.io.out_line(&format!(
            "{{\"hookSpecificOutput\":{{\"hookEventName\":\"PostToolUse\",\"additionalContext\":\"{message}\"}}}}"
        ));
        Ok(Flow::Code(0))
    }
}

/// `handleWire` and the error mapping of `runRouted`: what is printed, or the error the CLI would print.
fn handle_wire(
    reply: Result<Vec<u8>, WireError>,
    socket: &std::path::Path,
    json: bool,
    command: &str,
    now_ms: f64,
    io: &mut crate::Io,
) -> Result<(), JsError> {
    let line = match reply {
        Ok(line) => line,
        Err(WireError::AfterSend(error)) => {
            return Err(JsError::runtime(after_send_message(&error)));
        }
        Err(WireError::NotSent { code, .. }) => {
            return Err(JsError::runtime(if code.is_empty() {
                "connect failed".to_string()
            } else {
                format!("connect {code} {}", socket.display())
            }));
        }
    };
    let result = match response(&line) {
        Response::Malformed => return Err(JsError::runtime("malformed reply")),
        Response::Refused { .. } => return Err(refusal(&line)),
        Response::Ok { result } => result,
    };
    let result = result.as_ref();
    let (stdout, stderr) = (&mut io.out, &mut io.err);
    if !json && (command == "inbox" || command == "wait") {
        text(stdout, &render::render_messages(result)?);
    } else {
        if !json && (command == "status" || command == "ping") {
            for line in render::controller_lines(result) {
                text(stdout, &line);
            }
        }
        if !json && command == "status" {
            let mut lines = render::pause_lines(result, now_ms)?;
            lines.extend(render::pm_mail_lines(result));
            for line in &lines {
                text(stdout, line);
            }
        }
        output(stdout, result, json);
    }
    if !json {
        if let Some(notice) = render::unread_notice(result, now_ms) {
            stderr.extend_from_slice(notice.as_bytes());
            stderr.push(b'\n');
        }
    }
    if let Some(Value::String(warning)) = jsops::read_opt(result, "warning") {
        text(stderr, &concat(&[&"warning: ".into(), warning]));
    }
    Ok(())
}
