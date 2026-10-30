//! The commands the daemon answers (`runRouted` and `handleWire` of the Node CLI): agent commands over the agent's own
//! socket and token, operator commands over the project's socket with the operator credential, and `inbox --hook`.
use std::time::Duration;

use capstan_wire::js::{self, JsStr, Value};
use capstan_wire::{call, frame, response, AfterSend, Response, Timeout, WireError};

use crate::agent::{socket_verdict, var, Verdict};
use crate::jsops::{self, as_number, concat, JsError, EXIT_BLOCKED, EXIT_INVALID, EXIT_RUNTIME};
use crate::{blocked, compact, invalid, output, render, Cli, Fail, Flow};

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
    ("cleanup", Access::Any),
    ("cache", Access::Any),
    ("verify", Access::Any),
    ("design", Access::Any),
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
        "cleanup" => 120_000,
        "cache" => 600_000,
        "replace" => 1_800_000,
        "spawn" | "request-review" | "plan" | "integrate" | "release" | "pm-restart" | "verify" => {
            600_000
        }
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
        if command == "verify" {
            let shape_ok = match args.first().map(String::as_str) {
                Some("show" | "cancel") => args.len() == 2,
                Some("list") => args.len() <= 2,
                Some(_) => args.len() == 1,
                None => false,
            };
            if !shape_ok {
                return Err(Fail::Usage);
            }
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
        if command == "wait"
            && args
                .first()
                .is_some_and(|a| a == "--until" || a.starts_with("--until="))
        {
            let until = parse_until(args)?;
            return self.wait_until(&socket, &credential, until, json);
        }
        // An agent's inbox and wait ask for the digest: info mail apart and acknowledged. Without it the daemon answers
        // as it always did.
        let digest_args = ["--digest".to_string()];
        let args = if use_agent && matches!(command, "inbox" | "wait") && args.is_empty() {
            &digest_args[..]
        } else {
            args
        };
        let request = frame(&credential, command, args)
            .map_err(|_| invalid("command request is too large"))?;
        let timeout = Timeout::Idle(Duration::from_millis(client_timeout_ms(command)));
        let mut reply = call(&socket, &request, timeout);
        // A daemon older than the client rejects the digest argument it never heard of; ask again the way it understands.
        if args == digest_args && refused_as_invalid(&reply) {
            let request = frame(&credential, command, &[])
                .map_err(|_| invalid("command request is too large"))?;
            reply = call(&socket, &request, timeout);
        }
        // A daemon older than the client rejects an `auto=` window it never heard of: say so, and do not retry it as a
        // timed grant. A current daemon's refusal of a bad window names `auto`, and is shown as it is.
        if is_window_request(command, args) && refused_as_unaware_of_windows(&reply) {
            return Err(invalid(
                "this controller predates auto-approve windows; restart it (cstan restart) or ask the user with Approve once",
            ));
        }
        if let Err(WireError::NotSent { code, .. }) = &reply {
            if capstan_wire::nothing_listens(code) {
                return Err(blocked(if agent.is_some() {
                    "the controller is not running; ask the operator to run cstan start"
                } else {
                    "the controller stopped answering; run the command again to restart it"
                }));
            }
        }
        let now_ms = self.ctx.now_ms;
        let view = View {
            all: self.status_all,
            brief: command == "plan"
                && args.first().is_some_and(|a| a == "show")
                && args.iter().any(|a| a == "--brief"),
            verify_sub: match args.first().map(String::as_str) {
                Some("show") => "show",
                Some("list") => "list",
                Some("cancel") => "cancel",
                _ => "",
            },
        };
        handle_wire(reply, &socket, json, command, now_ms, view, &mut self.io)?;
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
        let Some(waiting) = render::waiting(result.as_ref()).filter(|w| w.count > 0.0) else {
            return Ok(Flow::Code(0));
        };
        let count = waiting.count;
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

/// The longest `cstan wait --until` may block.
const UNTIL_MAX_SECONDS: u64 = 600;

/// `--until <duration>` of `cstan wait`: whole seconds, or a number with a unit s, m or h (90, 90s, 5m).
fn parse_until(args: &[String]) -> Result<Duration, Fail> {
    let usage = "usage: cstan wait [--until <duration>]  (for example 90s or 5m, at most 10m)";
    let value = match args {
        [flag, value] if flag == "--until" => value.as_str(),
        [one] if one.starts_with("--until=") => &one["--until=".len()..],
        _ => return Err(invalid(usage)),
    };
    let (digits, unit) = match value.char_indices().last() {
        Some((at, 's' | 'm' | 'h')) => (&value[..at], &value[at..]),
        _ => (value, "s"),
    };
    let count: u64 = digits.parse().map_err(|_| invalid(usage))?;
    let seconds = count.saturating_mul(match unit {
        "m" => 60,
        "h" => 3600,
        _ => 1,
    });
    if seconds == 0 || seconds > UNTIL_MAX_SECONDS {
        return Err(invalid(format!(
            "wait --until must be between 1s and {}m",
            UNTIL_MAX_SECONDS / 60
        )));
    }
    Ok(Duration::from_secs(seconds))
}

/// How long `wait --until` pauses before asking again after a round that ended early with no mail.
fn early_backoff(left: Duration) -> Duration {
    left.min(Duration::from_secs(1))
}

#[cfg(test)]
mod early_backoff_tests {
    use super::*;

    #[test]
    fn pauses_up_to_a_second_and_never_past_the_deadline() {
        assert_eq!(
            early_backoff(Duration::from_secs(60)),
            Duration::from_secs(1)
        );
        assert_eq!(
            early_backoff(Duration::from_millis(300)),
            Duration::from_millis(300)
        );
        assert_eq!(early_backoff(Duration::ZERO), Duration::ZERO);
    }
}

impl Cli<'_> {
    /// `wait --until`: repeats the daemon's wait (each round ends at the host's limit) until mail arrives or `until` has
    /// passed. A round that is already due returns at once with the unacked messages, as plain `wait` does.
    fn wait_until(
        &mut self,
        socket: &std::path::Path,
        credential: &str,
        until: Duration,
        json: bool,
    ) -> Result<Flow, Fail> {
        let deadline = std::time::Instant::now() + until;
        // Set once an older daemon rejected the digest and limit arguments: plain waits from then on.
        let mut legacy = false;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let limit = left.as_secs() + u64::from(left.subsec_nanos() > 0);
            let wait_args = if legacy {
                Vec::new()
            } else {
                vec![
                    "--digest".to_string(),
                    "--limit".to_string(),
                    limit.max(1).to_string(),
                ]
            };
            let request = frame(credential, "wait", &wait_args)
                .map_err(|_| invalid("command request is too large"))?;
            let timeout = Timeout::Idle(Duration::from_millis(client_timeout_ms("wait")));
            let reply = call(socket, &request, timeout);
            if !legacy && refused_as_invalid(&reply) {
                legacy = true;
                continue;
            }
            match &reply {
                Err(WireError::NotSent { code, .. }) if capstan_wire::nothing_listens(code) => {
                    return Err(blocked(
                        "the controller is not running; ask the operator to run cstan start",
                    ));
                }
                Err(WireError::AfterSend(AfterSend::Closed)) => {
                    return Err(JsError::runtime(
                        "the controller stopped or restarted during the wait; run cstan wait --until again",
                    )
                    .into());
                }
                Ok(line) => {
                    if let Response::Ok { result } = response(line) {
                        let result = result.as_ref();
                        let empty = jsops::read_opt(result, "timedOut")
                            .is_some_and(|v| matches!(v, Value::Bool(true)))
                            && as_number(jsops::read_opt(result, "count")) == Some(0.0);
                        if empty && std::time::Instant::now() < deadline {
                            // A daemon that answers early with no mail must not make this loop spin.
                            std::thread::sleep(early_backoff(
                                deadline.saturating_duration_since(std::time::Instant::now()),
                            ));
                            continue;
                        }
                        if empty {
                            if json {
                                output(&mut self.io.out, result, true);
                            } else {
                                text(
                                    &mut self.io.out,
                                    &JsStr::from(
                                        format!("no new messages (waited {}s)", until.as_secs())
                                            .as_str(),
                                    ),
                                );
                            }
                            return Ok(Flow::Code(0));
                        }
                    }
                }
                _ => {}
            }
            let now_ms = self.ctx.now_ms;
            handle_wire(
                reply,
                socket,
                json,
                "wait",
                now_ms,
                View {
                    all: false,
                    brief: false,
                    verify_sub: "",
                },
                &mut self.io,
            )?;
            return Ok(Flow::Code(0));
        }
    }
}

/// Whether the daemon answered `invalid_request`: what a daemon older than the client says to an argument it lacks.
fn refused_as_invalid(reply: &Result<Vec<u8>, WireError>) -> bool {
    matches!(reply, Ok(line) if matches!(response(line), Response::Refused { code, .. } if code.to_utf8_lossy() == "invalid_request"))
}

/// Whether the daemon answered `invalid_request` with text that does not mention `auto`: an older daemon, which has
/// never heard of the window; a current daemon's own refusal of the window names it.
fn refused_as_unaware_of_windows(reply: &Result<Vec<u8>, WireError>) -> bool {
    matches!(reply, Ok(line) if matches!(response(line), Response::Refused { code, message }
        if code.to_utf8_lossy() == "invalid_request"
            && !message.to_utf8_lossy().to_lowercase().contains("auto")))
}

/// Whether the request opens an auto-approve window: `op decide` or `prompt answer` with `--session auto=...`.
fn is_window_request(command: &str, args: &[String]) -> bool {
    matches!(command, "op" | "prompt")
        && args
            .iter()
            .any(|a| a.starts_with("auto=") || a.starts_with("--session=auto="))
}

/// What the text output of a reply is trimmed to.
#[derive(Clone, Copy)]
struct View {
    /// `status --all`.
    all: bool,
    /// `plan show --brief`.
    brief: bool,
    /// The subcommand of `verify`: `show`, `list`, `cancel`, or empty for a queue request.
    verify_sub: &'static str,
}

/// `handleWire` and the error mapping of `runRouted`: what is printed, or the error the CLI would print.
fn handle_wire(
    reply: Result<Vec<u8>, WireError>,
    socket: &std::path::Path,
    json: bool,
    command: &str,
    now_ms: f64,
    view: View,
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
    } else if let (false, "ack", Some(lines)) = (json, command, render::ack_lines(result)) {
        for line in lines {
            text(stdout, &JsStr::from(line.as_str()));
        }
    } else if !json && command == "cleanup" {
        for line in render::cleanup_lines(result) {
            text(stdout, &JsStr::from(line.as_str()));
        }
    } else if !json && command == "verify" {
        for line in render::verify_lines(view.verify_sub, result, now_ms) {
            text(stdout, &JsStr::from(line.as_str()));
        }
    } else if !json && command == "cache" {
        for line in render::cache_lines(result, now_ms) {
            text(stdout, &JsStr::from(line.as_str()));
        }
    } else {
        if !json && (command == "status" || command == "ping") {
            for line in render::controller_lines(result) {
                text(stdout, &line);
            }
        }
        if !json && command == "status" {
            let mut lines = render::pause_lines(result, now_ms)?;
            lines.extend(render::pm_mail_lines(result));
            lines.extend(
                crate::watch::window_line(result.and_then(|r| r.get("operator")), true)
                    .map(|line| JsStr::from(line.as_str())),
            );
            lines.extend(
                render::verify_status_lines(result, now_ms)
                    .iter()
                    .map(|line| JsStr::from(line.as_str())),
            );
            lines.extend(
                render::cache_status_line(result, now_ms)
                    .iter()
                    .map(|line| JsStr::from(line.as_str())),
            );
            for line in &lines {
                text(stdout, line);
            }
        }
        if !json && command == "status" {
            let compact = result.map(|r| compact::status(r, view.all));
            output(stdout, compact.as_ref(), false);
        } else if let Some(lines) = result
            .filter(|_| !json && view.brief)
            .and_then(compact::plan_brief)
        {
            for line in lines {
                text(stdout, &JsStr::from(line.as_str()));
            }
        } else {
            output(stdout, result, json);
        }
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
