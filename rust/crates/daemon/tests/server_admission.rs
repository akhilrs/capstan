//! Admission on a real socket: the busy answer instead of a silent close, the per-caller and wait shares, the lingering
//! refusals, the boot id and the oversize answer. Clients are raw sockets that send a full frame first, as `cstan` does;
//! slow and blocking handlers come through the `ServerOptions.handler` seam. Assertions count lines, with generous time
//! bounds, never latency.

mod common;

use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::{handle_frame_with, FrameOutcome};
use capstan_daemon::server::{DaemonServer, FrameHandler, ServerOptions};
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::{Core, SystemEnv};
use common::private_tempdir;
use common::project::{write_project, Identity};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const BOUND: Duration = Duration::from_secs(10);

fn identity() -> Identity {
    Identity {
        project_id: "padm".into(),
        name: "Admission Test".into(),
        credential: format!("operator-{}", "a".repeat(40)),
    }
}

/// One ledger step of the setup, run before the daemon opens the project.
fn setup_step(core: &Core, owner: &str, op: &str, args: Value) -> Value {
    let context = json!({
        "credential": owner,
        "requestId": format!("req-{op}-{}", core.state_version().unwrap()),
        "idempotencyKey": format!("idem-{op}-{}", core.state_version().unwrap()),
        "expectedVersion": core.state_version().unwrap(),
        "inputRevision": core.input_revision().unwrap(),
    });
    let mut all = vec![context];
    all.extend(args.as_array().cloned().unwrap());
    core.dispatch(op, &all)
        .unwrap_or_else(|e| panic!("setup {op}: {e}"))
}

/// Registers a PM (`pm-1`) and two developers (`dev-1`, `dev-2`); returns the credential of each agent by id.
fn register_agents(state: &Path, directory: &Path, identity: &Identity) -> HashMap<String, String> {
    let core = Core::open(
        state,
        &identity.initial_project(),
        &KernelOptions {
            workspace_root: Some(directory.to_path_buf()),
            ..KernelOptions::default()
        },
        Box::new(SystemEnv),
    )
    .expect("the setup controller opens");
    let owner = identity.credential.as_str();
    setup_step(
        &core,
        owner,
        "syncRoleDefinitions",
        json!([[
            {"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)},
            {"name": "pm", "kind": "PM", "host": "claude", "configHash": "b".repeat(64)},
        ]]),
    );
    let mut credentials = HashMap::new();
    for (agent, role_name, kind) in [
        ("pm-1", "pm", "PM"),
        ("dev-1", "developer", "Developer"),
        ("dev-2", "developer", "Developer"),
    ] {
        let seat = format!("seat-{agent}");
        setup_step(
            &core,
            owner,
            "createSeat",
            json!([{"seatId": seat, "name": seat, "role": kind}]),
        );
        let actor = setup_step(
            &core,
            owner,
            "createActor",
            json!([{"displayName": agent, "role": kind, "seatId": seat}]),
        );
        setup_step(
            &core,
            owner,
            "registerAgent",
            json!([{
                "agentId": agent,
                "roleName": role_name,
                "seatId": seat,
                "actorId": actor["actorId"],
            }]),
        );
        credentials.insert(
            agent.to_string(),
            actor["credential"].as_str().unwrap().to_string(),
        );
    }
    core.close();
    credentials
}

fn role_config(
    dir: &Path,
    daemon: &str,
    trust: &capstan_config::TrustStore,
) -> capstan_config::RoleConfig {
    let file = dir.join("capstan.toml");
    std::fs::write(
        &file,
        format!("schema_version = 1\n\n[hosts.claude]\nkind = \"claude\"\n\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n\n[review]\nwarm_reviewer = false\n\n[roles.developer]\nkind = \"Developer\"\nhost = \"claude\"\n\n[daemon]\n{daemon}\n"),
    )
    .unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    common::trusted_project(dir, trust);
    capstan_config::load_role_config_with(dir, trust).expect("the configuration loads")
}

/// The blocking commands `hold` and `wait` stay open until the gate opens or the connection is aborted.
#[derive(Default)]
struct Gate {
    open: AtomicBool,
}

fn handler(gate: Arc<Gate>) -> FrameHandler {
    FrameHandler::new(move |deps, frame, signal, limit, on_limit| {
        let request: Value = serde_json::from_slice(frame).unwrap_or(Value::Null);
        match request["command"].as_str() {
            Some(command @ ("hold" | "wait")) => {
                on_limit(60_000, command == "wait");
                while !gate.open.load(Ordering::SeqCst) && signal.reason().is_none() {
                    std::thread::sleep(Duration::from_millis(5));
                }
                FrameOutcome {
                    response: Some(json!({"ok": true, "result": {"held": true}}).to_string()),
                    log: LogEntry::new(command, "ok"),
                    identified: true,
                }
            }
            _ => handle_frame_with(deps, frame, signal, limit, on_limit),
        }
    })
}

struct Env {
    dir: common::PrivateDir,
    deps: Deps,
    socket: PathBuf,
    owner: String,
    agents: HashMap<String, String>,
    gate: Arc<Gate>,
    server: Option<DaemonServer>,
}

impl Env {
    fn start(daemon: &str) -> Env {
        Env::start_with(daemon, None)
    }

    fn start_with(daemon: &str, replace: Option<FrameHandler>) -> Env {
        let dir = private_tempdir();
        let identity = identity();
        let state = write_project(dir.path(), &identity);
        let agents = register_agents(&state, dir.path(), &identity);
        let kernel = {
            let (state, project, directory) = (
                state.clone(),
                identity.initial_project(),
                dir.path().to_path_buf(),
            );
            KernelHandle::spawn(move || {
                Core::open(
                    &state,
                    &project,
                    &KernelOptions {
                        workspace_root: Some(directory),
                        ..KernelOptions::default()
                    },
                    Box::new(SystemEnv),
                )
            })
            .expect("the kernel opens")
        };
        let sink: Logger = Arc::new(|_: &LogEntry| {});
        let mut options = DaemonOptions::new(
            state.clone(),
            identity.initial_project(),
            dir.path().to_path_buf(),
        );
        let trust = common::kept_trust_store();
        options.capstan = Some(Arc::new(role_config(dir.path(), daemon, &trust)));
        options.trust_store = trust;
        let deps = Deps::new(kernel, options, sink);
        let gate = Arc::new(Gate::default());
        let mut env = Env {
            socket: state.join("control.sock"),
            dir,
            deps,
            owner: identity.credential,
            agents,
            gate: Arc::clone(&gate),
            server: None,
        };
        env.server = Some(env.launch(Some(replace.unwrap_or_else(|| handler(gate)))));
        env
    }

    fn launch(&self, handler: Option<FrameHandler>) -> DaemonServer {
        let mut options = ServerOptions::new(self.socket.clone());
        options.handler = handler.or_else(|| Some(handler_of(&self.gate)));
        DaemonServer::start(self.deps.clone(), options).expect("the server starts")
    }

    fn server(&self) -> &DaemonServer {
        self.server.as_ref().unwrap()
    }

    fn agent(&self, id: &str) -> String {
        self.agents[id].clone()
    }

    fn release(&self) {
        self.gate.open.store(true, Ordering::SeqCst);
    }

    fn wait_buckets(&self, count: usize) {
        let end = Instant::now() + BOUND;
        while self.server().admission_buckets() != count {
            assert!(Instant::now() < end, "buckets remain");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_open(&self, count: usize) {
        let end = Instant::now() + BOUND;
        while self.server().open_connections() != count {
            assert!(
                Instant::now() < end,
                "open connections {} != {count}",
                self.server().open_connections()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Opens `count` connections that stay open in the blocking handler, and waits until the server holds them all.
    fn hold(&self, credential: &str, command: &str, count: usize) -> Vec<UnixStream> {
        let held: Vec<UnixStream> = (0..count)
            .map(|_| send(&self.socket, &frame(credential, command, &[], None)))
            .collect();
        let end = Instant::now() + BOUND;
        while self.server().open_connections() < count {
            assert!(Instant::now() < end, "the server never held {count}");
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(100));
        held
    }
}

fn handler_of(gate: &Arc<Gate>) -> FrameHandler {
    handler(Arc::clone(gate))
}

fn frame(credential: &str, command: &str, args: &[&str], rid: Option<&str>) -> Vec<u8> {
    let mut request = json!({"v": 1, "credential": credential, "command": command, "args": args});
    if let Some(rid) = rid {
        request["rid"] = json!(rid);
    }
    let mut bytes = request.to_string().into_bytes();
    bytes.push(b'\n');
    bytes
}

fn send(socket: &Path, bytes: &[u8]) -> UnixStream {
    let mut stream = UnixStream::connect(socket).expect("connects");
    stream.set_read_timeout(Some(BOUND)).unwrap();
    stream.write_all(bytes).expect("the frame is written");
    stream
}

/// Everything the server sends until it closes. A reset is a failure: the line must come before the end.
fn read_lines(stream: &mut UnixStream) -> Vec<String> {
    let mut bytes = Vec::new();
    stream
        .read_to_end(&mut bytes)
        .unwrap_or_else(|e| panic!("the connection ended with {e} after {} bytes", bytes.len()));
    String::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

fn call(socket: &Path, credential: &str, command: &str) -> Vec<String> {
    read_lines(&mut send(socket, &frame(credential, command, &[], None)))
}

fn code_of(line: &str) -> String {
    let value: Value = serde_json::from_str(line).unwrap();
    value["code"].as_str().unwrap_or("").to_string()
}

fn assert_busy(lines: &[String]) {
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(code_of(&lines[0]), "controller_busy", "{lines:?}");
}

fn assert_ok(lines: &[String]) {
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(
        serde_json::from_str::<Value>(&lines[0]).unwrap()["ok"],
        true,
        "{lines:?}"
    );
}

const SMALL: &str =
    "max_connections = 8\ncaller_connections = 8\nwait_connections = 8\nwait_per_caller = 4";

#[test]
fn at_max_connections_every_extra_connection_reads_the_busy_line_then_eof() {
    let env = Env::start(SMALL);
    let held = env.hold(&env.owner.clone(), "hold", 8);
    for _ in 0..100 {
        let lines = call(&env.socket, &env.owner, "status");
        assert_busy(&lines);
    }
    // The old-client view: the line parses as a refusal with the code and the message.
    let lines = call(&env.socket, &env.owner, "status");
    match capstan_wire::response(lines[0].as_bytes()) {
        capstan_wire::Response::Refused { code, message } => {
            assert_eq!(code.to_utf8_lossy(), capstan_wire::CONTROLLER_BUSY);
            assert!(message.to_utf8_lossy().starts_with("controller busy: "));
        }
        other => panic!("{other:?}"),
    }
    let value: Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(value["retryAfterMs"], 500);
    env.release();
    drop(held);
    env.wait_open(0);
    assert_ok(&call(&env.socket, &env.owner, "status"));
}

#[test]
fn the_fifth_connection_of_one_caller_is_busy_and_another_caller_is_served() {
    let env = Env::start(
        "max_connections = 16\ncaller_connections = 4\nwait_connections = 4\nwait_per_caller = 4",
    );
    let pm = env.agent("pm-1");
    let held = env.hold(&pm, "hold", 4);
    assert_busy(&call(&env.socket, &pm, "status"));
    assert_ok(&call(&env.socket, &env.agent("dev-1"), "status"));
    env.release();
    drop(held);
    env.wait_open(0);
    assert_ok(&call(&env.socket, &pm, "status"));
    // The share is released right after the reply is written, which the client can read first.
    env.wait_buckets(0);
}

#[test]
fn bogus_credentials_are_unauthorized_and_leave_no_bucket() {
    let env = Env::start(SMALL);
    for n in 0..200 {
        let lines = call(&env.socket, &format!("bogus-{n}"), "status");
        assert_eq!(lines.len(), 1);
        assert_eq!(code_of(&lines[0]), "unauthorized", "{lines:?}");
    }
    let end = Instant::now() + BOUND;
    while env.server().admission_buckets() != 0 {
        assert!(Instant::now() < end, "buckets remain");
        std::thread::sleep(Duration::from_millis(10));
    }
    // Frames that carry no credential are not counted at all.
    for bytes in [
        &b"[1,2]\n"[..],
        &b"{\"v\":1,\"credential\":7,\"command\":\"status\"}\n"[..],
    ] {
        let lines = read_lines(&mut send(&env.socket, bytes));
        assert_eq!(lines.len(), 1);
    }
    assert_eq!(env.server().admission_buckets(), 0);
}

#[test]
fn waits_are_counted_apart_and_a_caller_with_waits_is_still_served() {
    let env = Env::start(
        "max_connections = 16\ncaller_connections = 16\nwait_connections = 2\nwait_per_caller = 2",
    );
    let pm = env.agent("pm-1");
    let held = env.hold(&pm, "wait", 2);
    assert_ok(&call(&env.socket, &pm, "status"));
    // A third wait from anyone is busy: the wait share is used up.
    assert_busy(&call(&env.socket, &env.agent("dev-1"), "wait"));
    assert_busy(&call(&env.socket, &pm, "wait"));
    env.release();
    drop(held);
    env.wait_open(0);
    env.wait_buckets(0);
}

#[test]
fn one_caller_may_not_hold_more_waits_than_its_share() {
    let env = Env::start(
        "max_connections = 16\ncaller_connections = 16\nwait_connections = 4\nwait_per_caller = 1",
    );
    let pm = env.agent("pm-1");
    let held = env.hold(&pm, "wait", 1);
    assert_busy(&call(&env.socket, &pm, "wait"));
    let other = env.hold(&env.agent("dev-1"), "wait", 2);
    env.release();
    drop((held, other));
    env.wait_open(0);
}

#[test]
fn a_frame_sent_in_two_writes_while_at_the_cap_reads_the_busy_line_and_no_reset() {
    let env = Env::start(SMALL);
    let held = env.hold(&env.owner.clone(), "hold", 8);
    let bytes = frame(&env.owner, "status", &[], None);
    let mut stream = UnixStream::connect(&env.socket).unwrap();
    stream.set_read_timeout(Some(BOUND)).unwrap();
    stream.write_all(&bytes[..10]).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    stream
        .write_all(&bytes[10..])
        .expect("the second write is drained");
    assert_busy(&read_lines(&mut stream));
    env.release();
    drop(held);
}

#[test]
fn a_client_that_sends_nothing_is_closed_after_the_request_timeout() {
    let env = Env::start(SMALL);
    let started = Instant::now();
    let mut stream = UnixStream::connect(&env.socket).unwrap();
    stream.set_read_timeout(Some(BOUND)).unwrap();
    let mut bytes = Vec::new();
    let _ = stream.read_to_end(&mut bytes);
    assert!(bytes.is_empty());
    let waited = started.elapsed();
    assert!(waited >= Duration::from_millis(4_000), "{waited:?}");
    assert!(waited < BOUND, "{waited:?}");
}

#[test]
fn a_shutdown_closes_every_lingering_stream() {
    let mut env = Env::start(SMALL);
    let held = env.hold(&env.owner.clone(), "hold", 8);
    // Refused clients that read their line but keep the connection open.
    let mut open: Vec<UnixStream> = Vec::new();
    for _ in 0..5 {
        let mut stream = send(&env.socket, &frame(&env.owner, "status", &[], None));
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        while stream.read(&mut byte).unwrap_or(0) == 1 {
            line.push(byte[0]);
            if byte[0] == b'\n' {
                break;
            }
        }
        assert!(!line.is_empty());
        open.push(stream);
    }
    assert!(env.server().lingering_connections() > 0);
    env.release();
    env.server.take().unwrap().close().unwrap();
    drop((held, open));
    let server = env.launch(None);
    assert_eq!(server.lingering_connections(), 0);
}

#[test]
fn the_boot_file_holds_a_new_id_after_each_start_and_goes_with_a_clean_stop() {
    let mut env = Env::start(SMALL);
    let boot = PathBuf::from(format!("{}.boot", env.socket.display()));
    let first = std::fs::read_to_string(&boot).unwrap();
    assert_eq!(first.len(), 32);
    assert!(first.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(
        std::fs::metadata(&boot).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(env.server().boot_file().unwrap().1, first);
    env.server.take().unwrap().close().unwrap();
    assert!(!boot.exists(), "a clean stop removes its own file");
    // A crashed daemon's file is replaced.
    std::fs::write(&boot, "stale").unwrap();
    let server = env.launch(None);
    let second = std::fs::read_to_string(&boot).unwrap();
    assert_eq!(second.len(), 32);
    assert_ne!(second, first);
    server.close().unwrap();
    assert!(!boot.exists());
    let _ = &env.dir;
}

#[test]
fn an_oversize_frame_is_answered_before_the_close() {
    let env = Env::start(SMALL);
    let big = vec![b'a'; 70_000];
    let mut stream = UnixStream::connect(&env.socket).unwrap();
    stream.set_read_timeout(Some(BOUND)).unwrap();
    let _ = stream.write_all(&big);
    let lines = read_lines(&mut stream);
    assert_eq!(lines.len(), 1, "{lines:?}");
    let value: Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(value["code"], "invalid_request");
    assert_eq!(value["message"], "request too large");
}

#[test]
fn a_destroyed_connection_while_the_handler_runs_is_retried_to_the_stored_answer() {
    let runs = Arc::new(AtomicUsize::new(0));
    let first: Arc<Mutex<Option<String>>> = Arc::default();
    let seam = {
        let (runs, first) = (Arc::clone(&runs), Arc::clone(&first));
        FrameHandler::new(move |deps, frame, signal, limit, on_limit| {
            runs.fetch_add(1, Ordering::SeqCst);
            let outcome = handle_frame_with(deps, frame, signal, limit, on_limit);
            let mut seen = first.lock().unwrap();
            if seen.is_none() {
                *seen = outcome.response.clone();
                drop(seen);
                // Past the request deadline: the server destroys the connection while the handler is still running.
                while signal.reason().is_none() {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            outcome
        })
    };
    let env = Env::start_with(SMALL, Some(seam));
    let pm = env.agent("pm-1");
    let bytes = frame(&pm, "send", &["dev-1", "hello"], Some("lost-1"));
    let mut stream = send(&env.socket, &bytes);
    let mut sink = Vec::new();
    let _ = stream.read_to_end(&mut sink);
    assert!(sink.is_empty(), "the destroyed connection got no answer");
    let lines = read_lines(&mut send(&env.socket, &bytes));
    assert_eq!(lines.len(), 1);
    assert_eq!(Some(lines[0].clone()), *first.lock().unwrap());
    assert_eq!(
        runs.load(Ordering::SeqCst),
        2,
        "the retry reached the real handler, which replayed"
    );
    assert_ok(&lines);
    // The send itself ran once: dev-1 has exactly one message.
    let inbox = call(&env.socket, &env.agent("dev-1"), "inbox");
    assert_eq!(inbox.len(), 1, "{inbox:?}");
    assert_eq!(inbox[0].matches("hello").count(), 1, "{inbox:?}");
}

// ------------------------------------------------------------------------------------------------ load

fn tally(
    socket: &Path,
    credential: &str,
    answers: &AtomicU64,
    busy: &AtomicU64,
    silent: &AtomicU64,
) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return false;
    };
    stream.set_read_timeout(Some(BOUND)).unwrap();
    let _ = stream.write_all(&frame(credential, "status", &[], None));
    let mut bytes = Vec::new();
    let _ = stream.read_to_end(&mut bytes);
    let text = String::from_utf8_lossy(&bytes);
    match text.lines().next() {
        None => {
            silent.fetch_add(1, Ordering::SeqCst);
        }
        Some(line) if line.contains("controller_busy") => {
            busy.fetch_add(1, Ordering::SeqCst);
        }
        Some(_) => {
            answers.fetch_add(1, Ordering::SeqCst);
        }
    }
    true
}

#[test]
fn a_load_of_200_threads_ends_every_connection_with_a_line() {
    let env = Env::start(
        "max_connections = 16\ncaller_connections = 16\nwait_connections = 16\nwait_per_caller = 4",
    );
    let (answers, busy, silent) = (
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicU64::new(0)),
    );
    let started = Instant::now();
    let threads: Vec<_> = (0..200)
        .map(|_| {
            let (socket, owner) = (env.socket.clone(), env.owner.clone());
            let (a, b, s) = (Arc::clone(&answers), Arc::clone(&busy), Arc::clone(&silent));
            std::thread::spawn(move || {
                for _ in 0..20 {
                    assert!(tally(&socket, &owner, &a, &b, &s), "the daemon stayed up");
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    assert!(started.elapsed() < Duration::from_secs(120));
    assert_eq!(
        silent.load(Ordering::SeqCst),
        0,
        "connections without a line"
    );
    assert_eq!(
        answers.load(Ordering::SeqCst) + busy.load(Ordering::SeqCst),
        4000
    );
}

#[test]
fn a_restart_under_load_ends_every_clean_connection_with_a_line() {
    let mut env = Env::start(
        "max_connections = 16\ncaller_connections = 16\nwait_connections = 16\nwait_per_caller = 4",
    );
    // 0 = up, 1 = stopping or down, 2 = up again.
    let epoch = Arc::new(AtomicU64::new(0));
    let done = Arc::new(AtomicBool::new(false));
    let (answers, busy, silent_clean, unclean) = (
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicU64::new(0)),
    );
    let after = Arc::new(AtomicU64::new(0));
    let threads: Vec<_> = (0..40)
        .map(|_| {
            let (socket, owner) = (env.socket.clone(), env.owner.clone());
            let (epoch, done) = (Arc::clone(&epoch), Arc::clone(&done));
            let (a, b, s, u, after) = (
                Arc::clone(&answers),
                Arc::clone(&busy),
                Arc::clone(&silent_clean),
                Arc::clone(&unclean),
                Arc::clone(&after),
            );
            std::thread::spawn(move || {
                while !done.load(Ordering::SeqCst) {
                    let before = epoch.load(Ordering::SeqCst);
                    let silent = AtomicU64::new(0);
                    let connected = tally(&socket, &owner, &a, &b, &silent);
                    let end = epoch.load(Ordering::SeqCst);
                    if !connected {
                        continue;
                    }
                    if silent.load(Ordering::SeqCst) > 0 {
                        if before == end && before != 1 {
                            s.fetch_add(1, Ordering::SeqCst);
                        } else {
                            u.fetch_add(1, Ordering::SeqCst);
                        }
                    } else if before == 2 {
                        after.fetch_add(1, Ordering::SeqCst);
                    }
                }
            })
        })
        .collect();
    std::thread::sleep(Duration::from_millis(500));
    epoch.store(1, Ordering::SeqCst);
    let stopped = env.server.take().unwrap();
    stopped.close().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    env.server = Some(env.launch(None));
    epoch.store(2, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(1000));
    done.store(true, Ordering::SeqCst);
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(
        stopped.drain_lineless(),
        0,
        "drain destroyed accepted connections without a line"
    );
    assert_eq!(
        silent_clean.load(Ordering::SeqCst),
        0,
        "clean connections without a line"
    );
    assert!(
        after.load(Ordering::SeqCst) > 0,
        "connections after the restart got lines"
    );
    eprintln!(
        "answers {} busy {} unclean-without-line {}",
        answers.load(Ordering::SeqCst),
        busy.load(Ordering::SeqCst),
        unclean.load(Ordering::SeqCst)
    );
}
