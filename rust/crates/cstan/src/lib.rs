//! The cstan front end: the commands an agent runs over the operator socket, done natively and byte for byte as the Node
//! CLI (`src/cli.ts`) does them; every other command, and every case where the Node CLI would word a refusal, is handed to
//! Node unchanged (`Outcome::Fallback`). The front end speaks the wire protocol only: it knows no daemon internals.
pub mod agent;
pub mod jsops;
pub mod render;

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use capstan_wire::js::{self, JsStr, Value};
use capstan_wire::{call, frame, response, AfterSend, Response, Timeout, WireError};

use agent::{agent_environment, socket_verdict, var, Verdict};
use jsops::{as_number, concat, JsError, EXIT_BLOCKED, EXIT_INVALID, EXIT_RUNTIME};

/// What `cstan __front-version` prints, for the installer.
pub const VERSION: &str = env!("CSTAN_FRONT_VERSION");

/// Everything a run depends on, so a test can replay it without touching the process.
pub struct Context {
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub cwd: PathBuf,
    /// `Date.now()`.
    pub now_ms: f64,
}

/// Why a command goes to Node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// Not one of the native commands (or its arguments are not the plain form).
    NotNative,
    /// The agent environment is missing, half set, malformed or points at another project.
    NoAgentEnvironment,
    /// An argument is empty, not UTF-8 or holds U+FFFD.
    BadArgument,
    /// The request is over the frame limit.
    FrameTooLarge,
    /// Nothing reached the daemon.
    NotSent,
}

/// The end of a run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Done {
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        exit: i32,
    },
    Fallback(Fallback),
}

const WAIT_SECONDS: u64 = 3600 + 30;
const HOOK_TIMEOUT: Duration = Duration::from_secs(2);

/// The commands the front end serves natively, in an agent environment.
const NATIVE: &[&str] = &[
    "status",
    "ping",
    "inbox",
    "ack",
    "wait",
    "report",
    "ask",
    "request-review",
    "integrate",
    "plan",
    "op",
    "link",
    "review",
    "finding",
    "observe",
    "prompt",
    "send",
    "pause",
    "resume",
    "spawn",
    "release",
    "replace",
];

fn client_timeout(command: &str) -> Timeout {
    Timeout::Idle(match command {
        "wait" => Duration::from_secs(WAIT_SECONDS),
        "replace" => Duration::from_secs(1800),
        "spawn" | "request-review" | "plan" | "integrate" | "release" => Duration::from_secs(600),
        _ => capstan_wire::DEFAULT_TIMEOUT,
    })
}

/// `parseOptions`: `--json` before a `--` is a flag, everything after it is literal.
fn parse_options(args: &[String]) -> (Vec<String>, bool) {
    let (options, literal) = match args.iter().position(|a| a == "--") {
        Some(at) => (&args[..at], &args[at + 1..]),
        None => (args, &args[..0]),
    };
    let positional = options
        .iter()
        .filter(|a| *a != "--json")
        .chain(literal)
        .cloned()
        .collect();
    (positional, options.iter().any(|a| a == "--json"))
}

fn done(stdout: Vec<u8>, stderr: Vec<u8>, exit: i32) -> Outcome {
    Outcome::Done {
        stdout,
        stderr,
        exit,
    }
}

/// Runs one invocation. A command that is not native, or that Node would refuse in its own words, is `Fallback`.
pub fn run(context: &Context) -> Outcome {
    let Some(command) = context.args.first().and_then(|a| a.to_str()) else {
        return Outcome::Fallback(Fallback::NotNative);
    };
    if command == "__front-version" {
        return if context.args.len() == 1 {
            done(format!("cstan-front {VERSION}\n").into_bytes(), vec![], 0)
        } else {
            Outcome::Fallback(Fallback::NotNative)
        };
    }
    if command == "inbox" && context.args.len() == 2 && context.args[1] == "--hook" {
        return run_hook(context);
    }
    if !NATIVE.contains(&command) {
        return Outcome::Fallback(Fallback::NotNative);
    }
    let mut rest = Vec::new();
    for arg in &context.args[1..] {
        match arg.to_str() {
            Some(text) => rest.push(text.to_string()),
            None => return Outcome::Fallback(Fallback::BadArgument),
        }
    }
    let (positional, json) = parse_options(&rest);
    match command {
        "status" => {
            let before = rest.iter().position(|a| a == "--").unwrap_or(rest.len());
            if rest[..before].iter().any(|a| a == "--watch") || !positional.is_empty() {
                return Outcome::Fallback(Fallback::NotNative);
            }
        }
        "ping" if !positional.is_empty() => return Outcome::Fallback(Fallback::NotNative),
        _ => {}
    }
    if positional
        .iter()
        .any(|a| a.is_empty() || a.contains('\u{fffd}'))
    {
        return Outcome::Fallback(Fallback::BadArgument);
    }
    let Ok(agent) = agent_environment(&context.env, &context.cwd) else {
        return Outcome::Fallback(Fallback::NoAgentEnvironment);
    };
    let Ok(request) = frame(&agent.token, command, &positional) else {
        return Outcome::Fallback(Fallback::FrameTooLarge);
    };
    let reply = match call(&agent.socket, &request, client_timeout(command)) {
        Err(WireError::NotSent { .. }) => return Outcome::Fallback(Fallback::NotSent),
        other => other,
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    match handle_wire(
        reply,
        json,
        command,
        context.now_ms,
        &mut stdout,
        &mut stderr,
    ) {
        Ok(()) => done(stdout, stderr, 0),
        Err(error) => {
            stderr.extend(
                concat(&[&"cstan: ".into(), &error.message, &"\n".into()])
                    .to_utf8_lossy()
                    .into_bytes(),
            );
            done(stdout, stderr, error.exit)
        }
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

/// `output(value, json)`; `None` is `undefined`.
fn output(out: &mut Vec<u8>, value: Option<&Value>, json: bool) {
    let rendered = match (value, json) {
        (None, _) => JsStr::from("undefined"),
        (Some(v), true) => js::stringify(v, 2),
        (Some(v), false) => js::render(v),
    };
    text(out, &rendered);
}

/// `handleWire` and the error mapping of `runRouted`: what is printed, or the error the CLI would print.
fn handle_wire(
    reply: Result<Vec<u8>, WireError>,
    json: bool,
    command: &str,
    now_ms: f64,
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
) -> Result<(), JsError> {
    let line = match reply {
        Ok(line) => line,
        Err(WireError::AfterSend(error)) => {
            return Err(JsError::runtime(after_send_message(&error)));
        }
        Err(WireError::NotSent { code, .. }) => {
            return Err(JsError::runtime(format!("connect {code}")));
        }
    };
    let result = match response(&line) {
        Response::Malformed => return Err(JsError::runtime("malformed reply")),
        Response::Refused { .. } => return Err(refusal(&line)),
        Response::Ok { result } => result,
    };
    let result = result.as_ref();
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

/// `runInboxHook`: silent and exit 0 whatever happens, but for the one line of context when messages wait.
fn run_hook(context: &Context) -> Outcome {
    let silent = || done(vec![], vec![], 0);
    let token = var(&context.env, "CAPSTAN_TOKEN").filter(|v| !v.is_empty());
    let socket = var(&context.env, "CAPSTAN_SOCKET").filter(|v| !v.is_empty());
    let agent = var(&context.env, "CAPSTAN_AGENT_ID").filter(|v| !v.is_empty());
    let (Some(token), Some(socket), Some(_)) = (token, socket, agent) else {
        return silent();
    };
    let (Some(token), Some(socket)) = (token.to_str(), socket.to_str()) else {
        return Outcome::Fallback(Fallback::BadArgument);
    };
    if !socket.starts_with('/') {
        return silent();
    }
    let socket = std::path::Path::new(socket);
    if socket_verdict(&context.cwd, socket) == Verdict::Foreign {
        return silent();
    }
    let Ok(request) = frame(token, "inbox", &["--hook".to_string()]) else {
        return silent();
    };
    let Ok(line) = call(socket, &request, Timeout::Idle(HOOK_TIMEOUT)) else {
        return silent();
    };
    let Response::Ok { result } = response(&line) else {
        return silent();
    };
    let Some(count) = as_number(jsops::read_opt(result.as_ref(), "count")).filter(|n| *n > 0.0)
    else {
        return silent();
    };
    let minutes = render::oldest_minutes(
        jsops::read_opt(result.as_ref(), "oldestQueuedAt"),
        context.now_ms,
    );
    let message = format!(
        "{} Capstan message(s) are waiting for you (oldest {} min). Run cstan inbox now and ack each one before you continue or report.",
        js::number_to_string(count),
        js::number_to_string(minutes),
    );
    done(
        format!(
            "{{\"hookSpecificOutput\":{{\"hookEventName\":\"PostToolUse\",\"additionalContext\":\"{message}\"}}}}\n"
        )
        .into_bytes(),
        vec![],
        0,
    )
}
