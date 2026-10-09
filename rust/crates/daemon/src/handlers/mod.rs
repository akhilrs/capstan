//! The command layer: `ROUTES` (src/daemon.ts), the access classification, the unread-notice wrapper of src/commands.ts
//! and `dispatch`, which takes an identified caller's request to the handler of its route. One `register` function per
//! handler module fills the handler table; a module that is not ported yet registers stubs that answer `not_implemented`
//! (`shared::unported`).
//!
//! `handle_frame` is everything the daemon does between the bytes of one request frame and the bytes of its answer
//! (parse, authenticate, classify, dispatch, log), without a socket: the server of d2b-srv calls it per connection and
//! the in-process replay of the transcripts calls it per frame, so both run the same code. Connection-level behavior
//! (frame size, timeouts, half-close, drain) belongs to the server.

pub mod agents;
pub mod env;
pub mod findings;
pub mod links;
pub mod messages;
pub mod operator;
pub mod plans;
pub mod relay;
pub mod reports;
pub mod shared;
pub mod status;
pub mod wait;

use crate::deps::{Deps, LogEntry};
use capstan_kernel::types::Identity;
use capstan_wire::js::{self, JsStr};
use serde_json::{json, Value};
use shared::{
    fail, host_wait_seconds, ok, AbortSignal, CommandCall, CommandEnv, CommandResponse, ErrorCode,
    Handler, LAUNCHER_LIMIT_MS,
};
use std::collections::HashMap;
use std::sync::LazyLock;

pub const MAX_FRAME_BYTES: usize = 65_536;
pub const MAX_RESPONSE_BYTES: usize = 1_048_576;
pub const MAX_ARGS: usize = 16;
pub const REQUEST_TIMEOUT_MS: u64 = 5_000;
pub const MAX_CONNECTIONS: usize = 64;
pub const DRAIN_FLUSH_MS: u64 = 1_000;
pub const STUB_STAGE: &str = "Stage 2e";

// ------------------------------------------------------------------------------------------------ routes

/// `CommandAccess`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Agent,
    Operator,
    Any,
}

impl Access {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Agent => "agent",
            Self::Operator => "operator",
            Self::Any => "any",
        }
    }
}

/// One entry of `ROUTES`: who may call it, the stage a stub waits for (the route answers `not_implemented` itself), and the
/// handler module that serves it.
#[derive(Clone, Copy, Debug)]
pub struct Route {
    pub name: &'static str,
    pub access: Access,
    pub stub: Option<&'static str>,
    pub owner: &'static str,
}

const fn route(name: &'static str, access: Access, owner: &'static str) -> Route {
    Route {
        name,
        access,
        stub: None,
        owner,
    }
}

const fn stub_route(name: &'static str, access: Access, owner: &'static str) -> Route {
    Route {
        name,
        access,
        stub: Some(STUB_STAGE),
        owner,
    }
}

/// `ROUTES` in the order of src/daemon.ts, each with the module that serves it (a test checks names, access and stubs
/// against src/daemon.ts).
pub const ROUTES: [Route; 29] = [
    route("status", Access::Read, "status"),
    route("ping", Access::Read, "status"),
    route("inbox", Access::Any, "messages"),
    route("ack", Access::Agent, "messages"),
    route("wait", Access::Agent, "wait"),
    route("report", Access::Agent, "reports"),
    stub_route("ask", Access::Agent, "agents"),
    route("request-review", Access::Agent, "reports"),
    route("integrate", Access::Any, "reports"),
    route("plan", Access::Any, "plans"),
    route("op", Access::Any, "operator"),
    route("link", Access::Any, "links"),
    route("review", Access::Agent, "findings"),
    route("finding", Access::Agent, "findings"),
    route("observe", Access::Agent, "agents"),
    route("prompt", Access::Agent, "relay"),
    route("peek", Access::Operator, "agents"),
    stub_route("assign", Access::Operator, "agents"),
    route("cancel", Access::Operator, "messages"),
    route("send", Access::Any, "messages"),
    route("pm-restart", Access::Operator, "agents"),
    route("launch", Access::Operator, "agents"),
    route("pause", Access::Any, "agents"),
    route("resume", Access::Any, "agents"),
    route("spawn", Access::Any, "agents"),
    route("release", Access::Any, "agents"),
    route("replace", Access::Any, "agents"),
    route("resolve", Access::Operator, "messages"),
    route("shutdown", Access::Operator, "agents"),
];

/// The route named `command` (exact match, like `Object.hasOwn`).
pub fn route_of(command: &str) -> Option<&'static Route> {
    ROUTES.iter().find(|route| route.name == command)
}

/// The handler modules, in the order their `register` functions run.
pub const MODULES: [&str; 10] = [
    "messages", "wait", "status", "agents", "plans", "reports", "links", "relay", "findings",
    "operator",
];

/// The table of handlers by command name.
pub type HandlerMap = HashMap<&'static str, Handler>;

/// What a not yet ported route answers: `not_implemented`, marked so a replay reports it as pending.
fn stub_handler(_env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let owner = route_of(call.command).map_or("?", |route| route.owner);
    Some(shared::unported(call.command, owner))
}

/// Registers the stub for every route that `owner` serves. A module that ports its routes replaces this call with its
/// own handlers.
pub fn register_stubs(map: &mut HandlerMap, owner: &str) {
    for route in ROUTES.iter().filter(|route| route.owner == owner) {
        map.insert(route.name, stub_handler as Handler);
    }
}

static HANDLERS: LazyLock<HandlerMap> = LazyLock::new(|| {
    let mut map = HandlerMap::new();
    messages::register(&mut map);
    wait::register(&mut map);
    status::register(&mut map);
    agents::register(&mut map);
    plans::register(&mut map);
    reports::register(&mut map);
    links::register(&mut map);
    relay::register(&mut map);
    findings::register(&mut map);
    operator::register(&mut map);
    map
});

/// The handler of `command`, when one is registered.
pub fn handler_of(command: &str) -> Option<Handler> {
    HANDLERS.get(command).copied()
}

// ------------------------------------------------------------------------------------------------ classification

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Actor {
    Operator,
    Agent,
    Other,
}

/// `classify`.
pub fn classify(identity: &Identity) -> Actor {
    if identity.role == "operator" {
        Actor::Operator
    } else if identity.agent.is_some() {
        Actor::Agent
    } else {
        Actor::Other
    }
}

/// `allowed`: `read` and `any` are open to every operator and agent, the others to their own kind.
pub fn allowed(access: Access, actor: Actor) -> bool {
    match access {
        Access::Read | Access::Any => true,
        Access::Operator => actor == Actor::Operator,
        Access::Agent => actor == Actor::Agent,
    }
}

/// `limitMs` of `createCommandHandlers`: how long a command may run, or None for the daemon's default limit.
pub fn command_limit_ms(deps: &Deps, command: &str, identity: &Identity) -> Option<u64> {
    match command {
        "wait" => {
            let role_name = identity
                .agent
                .as_ref()
                .map(|agent| agent.role_name.as_str());
            let seconds = host_wait_seconds(deps.options.capstan.as_deref(), role_name);
            Some(u64::try_from(seconds).unwrap_or(0) * 1000)
        }
        "launch" | "spawn" | "request-review" | "plan" | "integrate" | "prompt" | "release"
        | "pm-restart" => Some(LAUNCHER_LIMIT_MS),
        // A replacement is a release and a spawn, each with its own budget, behind whatever else the launcher runs.
        "replace" => Some(3 * LAUNCHER_LIMIT_MS),
        _ => None,
    }
}

// ------------------------------------------------------------------------------------------------ dispatch

/// A request after its frame was parsed and its credential accepted: `args` is the member as sent (it is checked here).
pub struct Request<'a> {
    pub command: &'a str,
    pub credential: &'a str,
    pub args: Option<&'a js::Value>,
}

/// What `dispatch` did: the response (None when the client is gone and nothing may be written) and, for the commands that
/// get that far, the argument counts the log entry carries.
pub struct Dispatched {
    pub response: Option<CommandResponse>,
    pub arg_count: Option<usize>,
    pub arg_bytes: Option<usize>,
}

impl Dispatched {
    fn plain(response: CommandResponse) -> Self {
        Self {
            response: Some(response),
            arg_count: None,
            arg_bytes: None,
        }
    }
}

/// The unread-notice wrapper of src/commands.ts: an agent's command other than inbox and wait (and the operator agent's
/// op, whose answers are exact) also tells the caller when mail waits for it, the PM included.
fn with_notice(
    env: &CommandEnv<'_>,
    call: &CommandCall<'_>,
    response: CommandResponse,
) -> CommandResponse {
    let CommandResponse::Ok(Value::Object(result)) = &response else {
        return response;
    };
    if call.identity.agent.is_none() {
        return response;
    }
    let credential = call.credential.to_string();
    match env
        .deps
        .kernel
        .run(move |core| core.unread_summary(&credential))
    {
        Ok(summary) => {
            if summary["count"].as_i64().unwrap_or(0) == 0 {
                return response;
            }
            let mut merged = result.clone();
            merged.insert(
                "unread".into(),
                json!({
                    "count": summary["count"],
                    "oldestQueuedAt": summary["oldestQueuedAt"],
                    "actionNeeded": summary["actionNeeded"],
                }),
            );
            ok(Value::Object(merged))
        }
        Err(error) => {
            env.log(
                "unread_notice_failed",
                json!({"error": format!("{}: {}", error.name(), error.message())}),
            );
            response
        }
    }
}

/// `dispatch` with a hook: `on_limit(limit_ms, is_wait)` runs right before a handler that has a time limit starts, which is
/// where the server arms the connection's timer.
pub fn dispatch_with(
    deps: &Deps,
    identity: &Identity,
    request: &Request<'_>,
    signal: &AbortSignal,
    on_limit: &mut dyn FnMut(u64, bool),
) -> Dispatched {
    let command = request.command;
    let actor = classify(identity);
    if actor == Actor::Other {
        return Dispatched::plain(fail(
            ErrorCode::Forbidden,
            "the caller is not an operator or an agent",
        ));
    }
    let Some(route) = route_of(command) else {
        return Dispatched::plain(fail(ErrorCode::UnknownCommand, "unknown command"));
    };
    if !allowed(route.access, actor) {
        return Dispatched::plain(fail(
            ErrorCode::Forbidden,
            if route.access == Access::Operator {
                "this command needs the operator credential"
            } else {
                "this command needs an agent token"
            },
        ));
    }
    let args: Vec<String> = match request.args {
        None => Vec::new(),
        Some(js::Value::Array(items)) if items.len() <= MAX_ARGS => {
            let mut args = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    js::Value::String(text) if text.len_units() > 0 => {
                        args.push(text.to_utf8_lossy())
                    }
                    _ => {
                        return Dispatched::plain(args_refusal());
                    }
                }
            }
            args
        }
        Some(_) => return Dispatched::plain(args_refusal()),
    };
    let arg_bytes: usize = args.iter().map(String::len).sum();
    let arg_count = args.len();
    let counted = |response: Option<CommandResponse>| Dispatched {
        response,
        arg_count: Some(arg_count),
        arg_bytes: Some(arg_bytes),
    };
    if let Some(stage) = route.stub {
        return counted(Some(fail(
            ErrorCode::NotImplemented,
            format!("{command} is not implemented yet ({stage})"),
        )));
    }
    let Some(handler) = handler_of(command) else {
        return counted(Some(fail(ErrorCode::Error, "command has no handler")));
    };
    let limit_ms = command_limit_ms(deps, command, identity);
    if let Some(limit) = limit_ms {
        on_limit(limit, command == "wait");
    }
    let env = CommandEnv::new(deps);
    let call = CommandCall {
        command,
        credential: request.credential,
        identity,
        args: &args,
        signal,
        limit_ms: if command == "wait" { limit_ms } else { None },
    };
    let response = handler(&env, &call).map(|response| {
        if matches!(command, "inbox" | "wait" | "op") {
            response
        } else {
            with_notice(&env, &call, response)
        }
    });
    counted(response)
}

fn args_refusal() -> CommandResponse {
    fail(
        ErrorCode::InvalidRequest,
        format!("args must be at most {MAX_ARGS} non-empty strings"),
    )
}

/// `dispatch(&Deps, identity, request)`: the response of the route's handler, or the refusal that stops the request
/// before it (a caller that is neither operator nor agent, an unknown command, access, bad arguments, a stub route).
pub fn dispatch(
    deps: &Deps,
    identity: &Identity,
    request: &Request<'_>,
    signal: &AbortSignal,
) -> Dispatched {
    dispatch_with(deps, identity, request, signal, &mut |_, _| {})
}

// ------------------------------------------------------------------------------------------------ frames

/// What one frame produced: the response line (without its newline; None when nothing may be written) and the log entry.
pub struct FrameOutcome {
    pub response: Option<String>,
    pub log: LogEntry,
}

fn limited(text: String, limit: usize) -> String {
    if text.len() + 1 > limit {
        fail(ErrorCode::Error, "response exceeds the size limit").to_text()
    } else {
        text
    }
}

fn utf8_units(text: &JsStr, max: usize) -> String {
    text.to_utf8_lossy().chars().take(max).collect()
}

/// Handles one complete request frame (the bytes before the newline), as `handle` and `handleLegacy` of src/daemon.ts:
/// a legacy `action` request, a malformed one, an unauthorized one and a command that is dispatched. Reads the kernel's
/// clock once at the start and once for the `ms` of the log entry, as Node reads `Date.now()`.
pub fn handle_frame(
    deps: &Deps,
    frame: &[u8],
    signal: &AbortSignal,
    response_limit: usize,
) -> FrameOutcome {
    let started = deps.kernel.now_ms();
    let elapsed = |deps: &Deps| deps.kernel.now_ms() - started;
    let invalid = |deps: &Deps, message: &str| FrameOutcome {
        response: Some(limited(
            fail(ErrorCode::InvalidRequest, message).to_text(),
            response_limit,
        )),
        log: {
            let mut entry = LogEntry::new("?", "invalid_request");
            entry.ms = elapsed(deps);
            entry
        },
    };
    let parsed = std::str::from_utf8(frame)
        .ok()
        .and_then(|text| js::parse(text).ok());
    let Some(request) = parsed else {
        return invalid(deps, "request is not valid JSON");
    };
    let is_record = matches!(request, js::Value::Object(_));
    if is_record && request.get("action").is_some() && request.get("command").is_none() {
        return handle_legacy(deps, &request, started, response_limit);
    }
    let command = match request.get("command") {
        Some(js::Value::String(text)) if text.len_units() >= 1 && text.len_units() <= 64 => {
            Some(text.to_utf8_lossy())
        }
        _ => None,
    };
    let version_ok = matches!(request.get("v"), Some(js::Value::Number(n)) if *n == 1.0);
    let (true, true, Some(command)) = (is_record, version_ok, command) else {
        return invalid(deps, "malformed command request");
    };
    let unauthorized = |deps: &Deps| FrameOutcome {
        response: Some(limited(
            fail(ErrorCode::Unauthorized, "credential not accepted").to_text(),
            response_limit,
        )),
        log: {
            let mut entry = LogEntry::new(command.clone(), "unauthorized");
            entry.ms = elapsed(deps);
            entry
        },
    };
    let credential = match request.get("credential") {
        Some(js::Value::String(text)) => text.to_utf8_lossy(),
        _ => return unauthorized(deps),
    };
    let identified = {
        let credential = credential.clone();
        deps.kernel.run(move |core| core.identify(&credential))
    };
    let identity: Identity = match identified {
        Ok(value) => match serde_json::from_value(value) {
            Ok(identity) => identity,
            Err(_) => return internal_error(deps, response_limit),
        },
        Err(capstan_kernel::KernelError::Authentication(_)) => return unauthorized(deps),
        Err(_) => return internal_error(deps, response_limit),
    };
    let dispatched = dispatch(
        deps,
        &identity,
        &Request {
            command: &command,
            credential: &credential,
            args: request.get("args"),
        },
        signal,
    );
    let mut entry = LogEntry::new(command, "closed");
    entry.actor_id = Some(identity.actor_id.clone());
    entry.role = Some(identity.role.clone());
    entry.arg_count = dispatched.arg_count;
    entry.arg_bytes = dispatched.arg_bytes;
    let response = dispatched.response.map(|response| {
        entry.code = response.code_str().to_string();
        limited(response.to_text(), response_limit)
    });
    entry.ms = elapsed(deps);
    FrameOutcome {
        response,
        log: entry,
    }
}

/// `.catch(() => send(error "internal error"); log(...))` of the connection handler.
fn internal_error(_deps: &Deps, response_limit: usize) -> FrameOutcome {
    FrameOutcome {
        response: Some(limited(
            fail(ErrorCode::Error, "internal error").to_text(),
            response_limit,
        )),
        log: LogEntry::new("?", "error"),
    }
}

/// `handleLegacy`: the old `{action, token}` control requests (`status` and `inspect`, for the operator only).
fn handle_legacy(
    deps: &Deps,
    request: &js::Value,
    started: i64,
    response_limit: usize,
) -> FrameOutcome {
    let identity: Option<Identity> = match request.get("token") {
        Some(js::Value::String(token)) => {
            let token = token.to_utf8_lossy();
            deps.kernel
                .run(move |core| core.identify(&token))
                .ok()
                .and_then(|value| serde_json::from_value(value).ok())
        }
        _ => None,
    };
    let action = match request.get("action") {
        Some(js::Value::String(text)) => text
            .as_units()
            .iter()
            .take(32)
            .copied()
            .collect::<Vec<u16>>(),
        _ => vec![u16::from(b'?')],
    };
    let action_text = utf8_units(&JsStr::from(action), 32);
    let done = |deps: &Deps, body: Value, code: &str| {
        let mut entry = LogEntry::new(format!("control:{action_text}"), code);
        entry.actor_id = identity.as_ref().map(|i| i.actor_id.clone());
        entry.role = identity.as_ref().map(|i| i.role.clone());
        entry.ms = deps.kernel.now_ms() - started;
        FrameOutcome {
            response: Some(limited(body.to_string(), response_limit)),
            log: entry,
        }
    };
    if !identity.as_ref().is_some_and(|i| i.role == "operator") {
        return done(deps, json!({"error": "unauthorized"}), "unauthorized");
    }
    let id = request.get("id");
    if action_text == "status" && id.is_none() {
        return match deps.kernel.run(|core| {
            let request_id = core.kernel().env.uuid();
            Ok((request_id, core.status_snapshot()?))
        }) {
            Ok((request_id, snapshot)) => done(
                deps,
                json!({"requestId": request_id, "result": snapshot}),
                "ok",
            ),
            Err(_) => internal_error(deps, response_limit),
        };
    }
    if let (true, Some(js::Value::String(id))) = (action_text == "inspect", id) {
        if id.len_units() > 0 {
            let id = id.to_utf8_lossy();
            let answer = deps.kernel.run(move |core| {
                let request_id = core.kernel().env.uuid();
                Ok(core.inspect(&id).map(|result| (request_id, result)))
            });
            return match answer {
                Ok(Ok((request_id, result))) => done(
                    deps,
                    json!({"requestId": request_id, "result": result}),
                    "ok",
                ),
                Ok(Err(error)) => done(deps, json!({"error": error.message()}), "error"),
                Err(_) => internal_error(deps, response_limit),
            };
        }
    }
    done(
        deps,
        json!({"error": "invalid control request"}),
        "invalid_request",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The routes of src/daemon.ts: `name: { access: "x" }` and `name: { access: "x", stub: STUB_STAGE }`.
    fn node_routes() -> Vec<(String, String, bool)> {
        let source = include_str!("../../../../../src/daemon.ts");
        let start = source
            .find("export const ROUTES")
            .expect("ROUTES in daemon.ts");
        let body = &source[start..];
        let body = &body[body.find('{').unwrap()..];
        let end = body.find("\n};").expect("the end of ROUTES");
        let mut routes = Vec::new();
        for line in body[..end].lines().skip(1) {
            let line = line.trim();
            let Some((name, rest)) = line.split_once(": { access: \"") else {
                continue;
            };
            let name = name.trim_matches('"').to_string();
            let (access, tail) = rest.split_once('"').expect("an access");
            routes.push((name, access.to_string(), tail.contains("stub: STUB_STAGE")));
        }
        routes
    }

    #[test]
    fn routes_equal_the_routes_of_daemon_ts() {
        let node = node_routes();
        let rust: Vec<(String, String, bool)> = ROUTES
            .iter()
            .map(|r| {
                (
                    r.name.to_string(),
                    r.access.as_str().to_string(),
                    r.stub.is_some(),
                )
            })
            .collect();
        assert_eq!(rust, node, "ROUTES differs from src/daemon.ts");
        assert!(include_str!("../../../../../src/daemon.ts")
            .contains("const STUB_STAGE = \"Stage 2e\""));
        assert_eq!(STUB_STAGE, "Stage 2e");
    }

    #[test]
    fn every_route_has_an_owner_module_that_registers_it() {
        for route in ROUTES.iter() {
            assert!(
                MODULES.contains(&route.owner),
                "{}: unknown owner {}",
                route.name,
                route.owner
            );
            if route.stub.is_none() {
                assert!(
                    handler_of(route.name).is_some(),
                    "{} has no handler",
                    route.name
                );
            }
        }
        assert_eq!(MODULES.len(), 10);
    }

    #[test]
    fn access_classification_follows_the_node_rules() {
        assert!(allowed(Access::Read, Actor::Agent));
        assert!(allowed(Access::Any, Actor::Operator));
        assert!(allowed(Access::Operator, Actor::Operator));
        assert!(!allowed(Access::Operator, Actor::Agent));
        assert!(allowed(Access::Agent, Actor::Agent));
        assert!(!allowed(Access::Agent, Actor::Operator));
    }
}
