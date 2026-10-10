//! The control socket server against a real kernel: concurrency (a blocking launcher call holds no other connection), the
//! long poll (mail from a concurrent send, the timeout, a superseded wait, a drain), the connection limit, half-close,
//! the response limit, the socket file and the order of the run function. The exact codes and texts are Node's
//! (src/daemon.ts, src/commands/wait.ts).

mod common;

use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger, Loops};
use capstan_daemon::run::{run_daemon_with, Built, Wiring};
use capstan_daemon::server::{DaemonServer, ServerOptions};
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::{Core, SystemEnv};
use capstan_launcher::api::{
    BranchRenameResult, LaunchResult, LauncherResult, LauncherService, LauncherStatus,
    ObserveResult, ReleaseResult, ReplaceResult, SetupOutcome, SpawnOptions, SpawnResult,
    StubLauncher,
};
use capstan_operator::api::{OperatorService, StubOperator};
use common::private_tempdir;
use common::project::{write_project, Identity};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ------------------------------------------------------------------------------------------------ the fixture

fn identity() -> Identity {
    Identity {
        project_id: "psrv".into(),
        name: "Server Test".into(),
        credential: format!("operator-{}", "s".repeat(40)),
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

struct Fixture {
    _dir: tempfile::TempDir,
    deps: Deps,
    server: Option<DaemonServer>,
    socket: PathBuf,
    owner: String,
    agents: HashMap<String, String>,
    logs: Arc<Mutex<Vec<LogEntry>>>,
}

#[derive(Default)]
struct Setup {
    wait_seconds: Option<i64>,
    launcher: Option<Arc<dyn LauncherService>>,
    max_response_bytes: Option<usize>,
}

impl Fixture {
    fn start(setup: Setup) -> Fixture {
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
            .expect("the daemon's kernel opens")
        };
        let logs: Arc<Mutex<Vec<LogEntry>>> = Arc::default();
        let sink: Logger = {
            let logs = Arc::clone(&logs);
            Arc::new(move |entry: &LogEntry| logs.lock().unwrap().push(entry.clone()))
        };
        let mut options = DaemonOptions::new(
            state.clone(),
            identity.initial_project(),
            dir.path().to_path_buf(),
        );
        if let Some(seconds) = setup.wait_seconds {
            options.capstan = Some(Arc::new(role_config(dir.path(), seconds)));
        }
        let mut deps = Deps::new(kernel, options, sink);
        deps.launcher = setup.launcher;
        let mut server_options = ServerOptions::new(state.join("control.sock"));
        if let Some(limit) = setup.max_response_bytes {
            server_options.max_response_bytes = limit;
        }
        let server = DaemonServer::start(deps.clone(), server_options).expect("the server starts");
        Fixture {
            socket: state.join("control.sock"),
            _dir: dir,
            deps,
            server: Some(server),
            owner: identity.credential,
            agents,
            logs,
        }
    }

    fn agent(&self, id: &str) -> String {
        self.agents[id].clone()
    }

    /// The response to one command, parsed.
    fn call(&self, credential: &str, command: &str, args: &[&str]) -> Value {
        call(&self.socket, credential, command, args)
    }

    fn operator(&self, command: &str, args: &[&str]) -> Value {
        self.call(&self.owner.clone(), command, args)
    }

    fn log_codes(&self, command: &str) -> Vec<String> {
        self.logs
            .lock()
            .unwrap()
            .iter()
            .filter(|entry| entry.command == command)
            .map(|entry| entry.code.clone())
            .collect()
    }

    /// Waits until the log has `count` entries for `command`.
    fn wait_for_log_count(&self, command: &str, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if self.log_codes(command).len() >= count {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "fewer than {count} log entries for {command}: {:?}",
            self.logs.lock().unwrap()
        );
    }

    /// Waits until the log has an entry for `command` with `code`.
    fn wait_for_log(&self, command: &str, code: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if self.log_codes(command).iter().any(|c| c == code) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "no log entry {command}/{code}: {:?}",
            self.logs.lock().unwrap()
        );
    }

    fn stop(&mut self) {
        if let Some(server) = self.server.take() {
            server.close().expect("the server closes");
        }
        self.deps.kernel.close();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A role configuration whose only host waits `seconds` seconds (read from a `capstan.toml` like the daemon's).
fn role_config(dir: &Path, seconds: i64) -> capstan_config::RoleConfig {
    let file = dir.join("capstan.toml");
    std::fs::write(
        &file,
        format!(
            "schema_version = 1\n\n[hosts.claude]\nkind = \"claude\"\nwait_timeout_seconds = {seconds}\n\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n\n[roles.developer]\nkind = \"Developer\"\nhost = \"claude\"\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    capstan_config::load_role_config(dir).expect("the configuration loads")
}

fn frame(credential: &str, command: &str, args: &[&str]) -> Vec<u8> {
    format!(
        "{}\n",
        json!({"v": 1, "credential": credential, "command": command, "args": args})
    )
    .into_bytes()
}

/// Sends `bytes` and reads until the daemon closes; (the text that came back, whether it was closed).
fn exchange(socket: &Path, bytes: &[u8], read_for: Duration) -> (Option<String>, bool) {
    let mut stream = UnixStream::connect(socket).expect("connects");
    stream.set_read_timeout(Some(read_for)).unwrap();
    let _ = stream.write_all(bytes);
    read_to_close(&mut stream)
}

fn read_to_close(stream: &mut UnixStream) -> (Option<String>, bool) {
    let mut data = Vec::new();
    let mut buffer = [0u8; 8192];
    let mut closed = true;
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => data.extend_from_slice(&buffer[..n]),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                closed = false;
                break;
            }
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&data).trim_end().to_string();
    ((!text.is_empty()).then_some(text), closed)
}

fn call(socket: &Path, credential: &str, command: &str, args: &[&str]) -> Value {
    let (text, _) = exchange(
        socket,
        &frame(credential, command, args),
        Duration::from_secs(20),
    );
    serde_json::from_str(&text.unwrap_or_else(|| panic!("{command} got no answer"))).unwrap()
}

// ------------------------------------------------------------------------------------------------ concurrency

/// A launcher whose `spawn` blocks until the test opens the gate, as a herdr call that blocks does. The test decides
/// when it ends, so nothing here depends on how long anything takes.
struct SlowLauncher {
    inner: StubLauncher,
    gate: (Mutex<bool>, std::sync::Condvar),
    started: Mutex<Option<Instant>>,
}

impl SlowLauncher {
    fn open_gate(&self) {
        *self.gate.0.lock().unwrap() = true;
        self.gate.1.notify_all();
    }
}

impl LauncherService for SlowLauncher {
    fn launch_pm(&self) -> LauncherResult<LaunchResult> {
        self.inner.launch_pm()
    }
    fn restart_pm(&self) -> LauncherResult<LaunchResult> {
        self.inner.restart_pm()
    }
    fn spawn(&self, role_name: &str, options: &SpawnOptions) -> LauncherResult<SpawnResult> {
        *self.started.lock().unwrap() = Some(Instant::now());
        // The cap only ends a test that never opens the gate.
        let (open, changed) = &self.gate;
        let guard = open.lock().unwrap();
        drop(
            changed
                .wait_timeout_while(guard, Duration::from_secs(30), |open| !*open)
                .unwrap(),
        );
        self.inner.spawn(role_name, options)
    }
    fn rename_branch_for_task(
        &self,
        agent_id: &str,
        task: &str,
    ) -> LauncherResult<BranchRenameResult> {
        self.inner.rename_branch_for_task(agent_id, task)
    }
    fn release(&self, agent_id: &str) -> LauncherResult<ReleaseResult> {
        self.inner.release(agent_id)
    }
    fn replace(&self, agent_id: &str) -> LauncherResult<ReplaceResult> {
        self.inner.replace(agent_id)
    }
    fn observe(&self, agent_id: &str, lines: usize) -> LauncherResult<ObserveResult> {
        self.inner.observe(agent_id, lines)
    }
    fn capture_prompt(&self, agent_id: &str) -> LauncherResult<capstan_herdr::api::CaptureOutcome> {
        self.inner.capture_prompt(agent_id)
    }
    fn answer_prompt(
        &self,
        agent_id: &str,
        request: capstan_launcher::api::AnswerPromptRequest<'_>,
    ) -> LauncherResult<capstan_herdr::api::RelayOutcome> {
        self.inner.answer_prompt(agent_id, request)
    }
    fn interrupt(&self, agent_id: &str) -> LauncherResult<bool> {
        self.inner.interrupt(agent_id)
    }
    fn status(&self) -> LauncherStatus {
        self.inner.status()
    }
    fn operator_environment(&self) -> BTreeMap<String, String> {
        self.inner.operator_environment()
    }
    fn in_flight_operations(&self) -> usize {
        self.inner.in_flight_operations()
    }
    fn adopt_all(&self) -> LauncherResult<()> {
        self.inner.adopt_all()
    }
    fn teardown(&self, agent_id: &str, worktree_path: &str) -> LauncherResult<SetupOutcome> {
        self.inner.teardown(agent_id, worktree_path)
    }
}

#[test]
fn a_launcher_call_that_is_blocked_holds_no_other_connection() {
    let launcher = Arc::new(SlowLauncher {
        inner: StubLauncher::new(),
        gate: (Mutex::new(false), std::sync::Condvar::new()),
        started: Mutex::new(None),
    });
    let fixture = Fixture::start(Setup {
        launcher: Some(launcher.clone()),
        ..Setup::default()
    });
    let spawn = {
        let (socket, owner) = (fixture.socket.clone(), fixture.owner.clone());
        std::thread::spawn(move || call(&socket, &owner, "spawn", &["developer"]))
    };
    // The launcher call is in progress and stays so until the gate opens.
    let deadline = Instant::now() + Duration::from_secs(5);
    while launcher.started.lock().unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "the spawn never reached the launcher"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    // Other connections are answered while it is blocked: a blocked server would leave these without an answer.
    for round in 0..3 {
        let status = fixture.operator("status", &[]);
        assert_eq!(status["ok"], true, "round {round}");
        let sent = fixture.operator("send", &["dev-1", &format!("while spawning {round}")]);
        assert_eq!(sent["ok"], true, "{sent}");
    }
    launcher.open_gate();
    let response = spawn.join().unwrap();
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["result"]["state"], "started");
    let spawns: Vec<String> = launcher
        .inner
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("spawn"))
        .collect();
    assert_eq!(spawns.len(), 1, "the launcher spawned once: {spawns:?}");
    assert!(spawns[0].starts_with("spawn developer"), "{spawns:?}");
}

#[test]
fn a_long_poll_answers_with_the_mail_a_concurrent_send_queues() {
    let fixture = Fixture::start(Setup::default());
    let credential = fixture.agent("dev-1");
    let wait = {
        let socket = fixture.socket.clone();
        std::thread::spawn(move || call(&socket, &credential, "wait", &[]))
    };
    fixture.wait_for_open_wait();
    let sent = fixture.operator("send", &["--action", "dev-1", "wake up"]);
    assert_eq!(sent["ok"], true, "{sent}");
    let response = wait.join().unwrap();
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["result"]["timedOut"], false);
    assert_eq!(response["result"]["count"], 1);
    assert_eq!(response["result"]["actionNeededCount"], 1);
    assert_eq!(response["result"]["messages"][0]["body"], "wake up");
    assert_eq!(response["result"]["messages"][0]["state"], "sent");
    assert_eq!(response["result"]["messages"][0]["from"], "operator");
    // The wait row was ended.
    let owner = fixture.owner.clone();
    let open = fixture
        .deps
        .kernel
        .run(move |core| core.open_waits(&owner))
        .unwrap();
    assert_eq!(open, json!([]));
    assert_eq!(fixture.log_codes("wait"), vec!["ok".to_string()]);
}

#[test]
fn a_wait_times_out_after_the_hosts_limit() {
    let fixture = Fixture::start(Setup {
        wait_seconds: Some(1),
        ..Setup::default()
    });
    let response = fixture.call(&fixture.agent("dev-1"), "wait", &[]);
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(
        response["result"],
        json!({"messages": [], "timedOut": true, "count": 0, "actionNeededCount": 0})
    );
}

#[test]
fn a_newer_wait_supersedes_the_older_one() {
    let fixture = Fixture::start(Setup::default());
    let credential = fixture.agent("dev-1");
    let first = {
        let (socket, credential) = (fixture.socket.clone(), credential.clone());
        std::thread::spawn(move || call(&socket, &credential, "wait", &[]))
    };
    fixture.wait_for_open_wait();
    let second = {
        let (socket, credential) = (fixture.socket.clone(), credential.clone());
        std::thread::spawn(move || call(&socket, &credential, "wait", &[]))
    };
    let superseded = first.join().unwrap();
    assert_eq!(
        superseded,
        json!({"ok": false, "code": "superseded", "message": "a newer wait replaced this one"})
    );
    // The newer wait goes on and gets the mail.
    fixture.wait_for_open_wait();
    assert_eq!(
        fixture.operator("send", &["dev-1", "for the newer wait"])["ok"],
        true
    );
    let answer = second.join().unwrap();
    assert_eq!(
        answer["result"]["messages"][0]["body"],
        "for the newer wait"
    );
    assert_eq!(answer["result"]["timedOut"], false);
    let mut codes = fixture.log_codes("wait");
    codes.sort();
    assert_eq!(codes, vec!["ok".to_string(), "superseded".to_string()]);
}

#[test]
fn a_drain_answers_a_waiting_agent_shutting_down() {
    let mut fixture = Fixture::start(Setup::default());
    let credential = fixture.agent("dev-2");
    let wait = {
        let socket = fixture.socket.clone();
        std::thread::spawn(move || {
            exchange(
                &socket,
                &frame(&credential, "wait", &[]),
                Duration::from_secs(20),
            )
        })
    };
    fixture.wait_for_open_wait();
    let server = fixture.server.take().unwrap();
    server.stop_accepting();
    // No new connection is accepted once the listener is closed.
    assert!(UnixStream::connect(&fixture.socket).is_err());
    server.drain();
    let (text, closed) = wait.join().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&text.unwrap()).unwrap(),
        json!({"ok": false, "code": "shutting_down", "message": "the daemon is shutting down"})
    );
    assert!(closed);
    server.cleanup().unwrap();
    assert!(!fixture.socket.exists(), "the socket file is removed");
    assert_eq!(fixture.log_codes("wait"), vec!["shutting_down".to_string()]);
}

impl Fixture {
    fn wait_for_open_wait(&self) {
        let owner = self.owner.clone();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let open = self
                .deps
                .kernel
                .run({
                    let owner = owner.clone();
                    move |core| core.open_waits(&owner)
                })
                .unwrap();
            if open.as_array().is_some_and(|rows| !rows.is_empty()) {
                return;
            }
            assert!(Instant::now() < deadline, "the wait never began");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

// ------------------------------------------------------------------------------------------------ connections

#[test]
fn the_65th_connection_is_closed_at_once_and_a_freed_slot_is_used_again() {
    let fixture = Fixture::start(Setup::default());
    let mut held: Vec<UnixStream> = (0..64)
        .map(|_| UnixStream::connect(&fixture.socket).expect("connects"))
        .collect();
    // Let the server register all 64.
    let deadline = Instant::now() + Duration::from_secs(5);
    while fixture.server.as_ref().unwrap().open_connections() < 64 {
        assert!(
            Instant::now() < deadline,
            "the server never saw 64 connections"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut extra = UnixStream::connect(&fixture.socket).expect("the kernel accepts the connect");
    extra
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let _ = extra.write_all(&frame(&fixture.owner, "ping", &[]));
    let (text, closed) = read_to_close(&mut extra);
    assert_eq!((text, closed), (None, true), "no answer, and closed");
    // A slot frees when a held connection goes away.
    drop(held.pop());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut again = UnixStream::connect(&fixture.socket).unwrap();
        again
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let _ = again.write_all(&frame(&fixture.owner, "ping", &[]));
        let (text, _) = read_to_close(&mut again);
        if let Some(text) = text {
            assert_eq!(serde_json::from_str::<Value>(&text).unwrap()["ok"], true);
            break;
        }
        assert!(Instant::now() < deadline, "the freed slot was never used");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_client_that_half_closes_after_its_frame_still_gets_its_reply() {
    let fixture = Fixture::start(Setup::default());
    let mut stream = UnixStream::connect(&fixture.socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream
        .write_all(&frame(&fixture.owner, "ping", &[]))
        .unwrap();
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    let (text, closed) = read_to_close(&mut stream);
    let answer: Value = serde_json::from_str(&text.expect("an answer")).unwrap();
    assert_eq!(answer["ok"], true);
    assert_eq!(answer["result"]["pong"], true);
    assert!(closed);
}

#[test]
fn a_wait_cannot_outlive_a_client_that_half_closes() {
    let fixture = Fixture::start(Setup::default());
    let mut stream = UnixStream::connect(&fixture.socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream
        .write_all(&frame(&fixture.agent("dev-1"), "wait", &[]))
        .unwrap();
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    let (text, closed) = read_to_close(&mut stream);
    assert_eq!(
        (text, closed),
        (None, true),
        "the wait ends without an answer"
    );
    fixture.wait_for_log("wait", "closed");
    // Its wait row is gone too.
    let owner = fixture.owner.clone();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let open = fixture
            .deps
            .kernel
            .run({
                let owner = owner.clone();
                move |core| core.open_waits(&owner)
            })
            .unwrap();
        if open == json!([]) {
            break;
        }
        assert!(Instant::now() < deadline, "the wait row stayed open");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_client_that_closes_during_a_wait_ends_it() {
    let fixture = Fixture::start(Setup::default());
    let stream = {
        let mut stream = UnixStream::connect(&fixture.socket).unwrap();
        stream
            .write_all(&frame(&fixture.agent("dev-1"), "wait", &[]))
            .unwrap();
        stream
    };
    fixture.wait_for_open_wait();
    drop(stream);
    fixture.wait_for_log("wait", "closed");
}

#[test]
fn a_response_over_the_limit_is_replaced_by_the_size_error() {
    let fixture = Fixture::start(Setup {
        max_response_bytes: Some(200),
        ..Setup::default()
    });
    let response = fixture.operator("status", &[]);
    assert_eq!(
        response,
        json!({"ok": false, "code": "error", "message": "response exceeds the size limit"})
    );
    // A small answer still fits.
    assert_eq!(fixture.operator("ping", &[])["result"]["pong"], true);
}

#[test]
fn unanswerable_frames_get_the_node_answers() {
    let fixture = Fixture::start(Setup::default());
    let (text, closed) = exchange(&fixture.socket, b"{not json\n", Duration::from_secs(10));
    assert_eq!(
        serde_json::from_str::<Value>(&text.unwrap()).unwrap(),
        json!({"ok": false, "code": "invalid_request", "message": "request is not valid JSON"})
    );
    assert!(closed);
    let big = vec![b'a'; 65_537];
    let (text, closed) = exchange(&fixture.socket, &big, Duration::from_secs(10));
    assert_eq!(
        (text, closed),
        (None, true),
        "a frame over 64 KiB is dropped"
    );
    let mut exact = vec![b'a'; 65_536];
    exact.push(b'\n');
    let (text, _) = exchange(&fixture.socket, &exact, Duration::from_secs(10));
    assert_eq!(
        serde_json::from_str::<Value>(&text.unwrap()).unwrap()["code"],
        "invalid_request",
        "a frame of exactly 64 KiB is read"
    );
    // The server writes an answer before it logs the request (as Node's does), so the second entry can still be on its
    // way when the client has the answer: wait for it instead of reading the log at once.
    fixture.wait_for_log_count("?", 2);
    let codes = fixture.log_codes("?");
    assert_eq!(
        codes,
        vec!["invalid_request".to_string(), "invalid_request".to_string()]
    );
}

// ------------------------------------------------------------------------------------------------ the socket file

#[test]
fn the_socket_is_private_and_a_stale_one_is_replaced() {
    let dir = private_tempdir();
    let socket = dir.path().join("control.sock");
    // A socket left by a daemon that died.
    drop(UnixStream::connect(&socket).err());
    let stale = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    drop(stale);
    assert!(socket.exists());
    let identity = identity();
    let state = dir.path().join("state");
    std::fs::create_dir(&state).unwrap();
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
    let stale_in_state = state.join("control.sock");
    drop(std::os::unix::net::UnixListener::bind(&stale_in_state).unwrap());
    let kernel = {
        let (state, project) = (state.clone(), identity.initial_project());
        KernelHandle::spawn(move || {
            Core::open(
                &state,
                &project,
                &KernelOptions::default(),
                Box::new(SystemEnv),
            )
        })
        .unwrap()
    };
    let sink: Logger = Arc::new(|_| {});
    let deps = Deps::new(
        kernel.clone(),
        DaemonOptions::new(
            state.clone(),
            identity.initial_project(),
            dir.path().to_path_buf(),
        ),
        sink,
    );
    let server = DaemonServer::start(deps, ServerOptions::new(stale_in_state.clone())).unwrap();
    let mode = std::fs::symlink_metadata(&stale_in_state)
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    let answer = call(&stale_in_state, &identity.credential, "ping", &[]);
    assert_eq!(answer["ok"], true);
    server.close().unwrap();
    assert!(!stale_in_state.exists());
    kernel.close();
}

#[test]
fn something_that_is_not_a_socket_at_the_path_stops_the_server() {
    let dir = private_tempdir();
    let identity = identity();
    let state = write_project(dir.path(), &identity);
    std::fs::write(state.join("control.sock"), "not a socket").unwrap();
    let kernel = {
        let (state, project) = (state.clone(), identity.initial_project());
        KernelHandle::spawn(move || {
            Core::open(
                &state,
                &project,
                &KernelOptions::default(),
                Box::new(SystemEnv),
            )
        })
        .unwrap()
    };
    let sink: Logger = Arc::new(|_| {});
    let deps = Deps::new(
        kernel.clone(),
        DaemonOptions::new(
            state.clone(),
            identity.initial_project(),
            dir.path().to_path_buf(),
        ),
        sink,
    );
    let error = DaemonServer::start(deps, ServerOptions::new(state.join("control.sock")))
        .err()
        .expect("refused");
    assert_eq!(
        error.to_string(),
        "control socket path exists and is not a socket"
    );
    assert!(
        state.join("control.sock").exists(),
        "the file is left alone"
    );
    kernel.close();
}

// ------------------------------------------------------------------------------------------------ the run function

/// Records what the run function does to the loops, the launcher and the operator, in order.
struct Recorder {
    events: Arc<Mutex<Vec<String>>>,
    launcher: Arc<StubLauncher>,
}

impl Recorder {
    fn note(&self, event: &str) {
        self.events.lock().unwrap().push(event.to_string());
    }
}

impl Loops for Recorder {
    fn start_ticks(&self, _deps: &Deps) {
        let adopted = self.launcher.calls().contains(&"adopt_all".to_string());
        self.note(&format!("start_ticks after_adopt={adopted}"));
    }
    fn recover_and_relay(&self, _deps: &Deps) {
        self.note("recover_and_relay");
    }
    fn stop_relay(&self) {
        self.note("stop_relay");
    }
    fn stop_ticks(&self) {
        self.note("stop_ticks");
    }
}

#[test]
fn the_run_function_keeps_nodes_order_with_the_core_stubs() {
    let dir = private_tempdir();
    let identity = identity();
    let state = write_project(dir.path(), &identity);
    let events: Arc<Mutex<Vec<String>>> = Arc::default();
    let launcher = Arc::new(StubLauncher::new());
    let operator = Arc::new(StubOperator::new());
    let mut options = DaemonOptions::new(
        state.clone(),
        identity.initial_project(),
        dir.path().to_path_buf(),
    );
    {
        let events = Arc::clone(&events);
        options.announce = Some(Arc::new(move |event: &str, _pid: u32| {
            events.lock().unwrap().push(format!("announce {event}"));
        }));
    }
    let wiring = Wiring {
        sync_roles: None,
        build: Some({
            let (events, launcher, operator) = (
                Arc::clone(&events),
                Arc::clone(&launcher),
                Arc::clone(&operator),
            );
            Box::new(move |deps: &mut Deps| {
                events.lock().unwrap().push("build".into());
                deps.launcher = Some(launcher.clone() as Arc<dyn LauncherService>);
                deps.operator = Some(operator.clone() as Arc<dyn OperatorService>);
                deps.loops = Arc::new(Recorder {
                    events: Arc::clone(&events),
                    launcher: Arc::clone(&launcher),
                });
                Built {
                    before_operator_recover: Some(Box::new({
                        let events = Arc::clone(&events);
                        move || events.lock().unwrap().push("ingest restart results".into())
                    })),
                    skip_restarts_with_plan: None,
                    stop_restart_watch: Some(Box::new({
                        let events = Arc::clone(&events);
                        move || events.lock().unwrap().push("stop restart watch".into())
                    })),
                }
            })
        }),
    };
    let daemon = std::thread::spawn(move || run_daemon_with(options, wiring));
    // Ready: the socket answers, and the loops have started after the adoption.
    let socket = state.join("control.sock");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if events.lock().unwrap().iter().any(|e| e == "announce ready") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "not ready: {:?}",
            events.lock().unwrap()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let pong = call(&socket, &identity.credential, "ping", &[]);
    assert_eq!(pong["result"]["pong"], true);
    assert!(state.join("daemon.pid").exists(), "the pid file is written");
    let stopping = call(&socket, &identity.credential, "shutdown", &[]);
    assert_eq!(stopping, json!({"ok": true, "result": {"stopping": true}}));
    let outcome = daemon.join().unwrap();
    assert!(outcome.is_ok(), "{outcome:?}");
    let events = events.lock().unwrap().clone();
    let at = |name: &str| {
        events
            .iter()
            .position(|event| event == name)
            .unwrap_or_else(|| panic!("no {name} in {events:?}"))
    };
    // Node's order, the adoption of recorded panes first and the loops after it; the announcement of `ready` races the
    // adoption thread, as it does in Node (the adoption is not awaited).
    let chain = [
        "build",
        "ingest restart results",
        "start_ticks after_adopt=true",
        "recover_and_relay",
        "stop_relay",
        "stop_ticks",
        "stop restart watch",
        "announce lock_released",
        "announce socket_removed",
    ];
    let positions: Vec<usize> = chain.iter().map(|name| at(name)).collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "{events:?}"
    );
    assert!(
        at("ingest restart results") < at("announce ready"),
        "{events:?}"
    );
    assert!(at("announce ready") < at("stop_relay"), "{events:?}");
    // The launcher adopted its panes first, and the operator recovered, started and stopped.
    assert_eq!(launcher.calls(), vec!["adopt_all".to_string()]);
    let calls = operator.calls();
    assert_eq!(
        calls.first().map(String::as_str),
        Some("recover"),
        "{calls:?}"
    );
    assert!(calls.iter().any(|c| c.starts_with("start")), "{calls:?}");
    assert_eq!(calls.last().map(String::as_str), Some("stop"), "{calls:?}");
    assert!(!socket.exists(), "the socket file is gone");
    assert!(!state.join("daemon.pid").exists(), "the pid file is gone");
}

#[test]
fn a_daemon_with_a_configuration_syncs_its_roles_before_the_server_starts() {
    let dir = private_tempdir();
    let identity = identity();
    let state = write_project(dir.path(), &identity);
    let mut options = DaemonOptions::new(
        state.clone(),
        identity.initial_project(),
        dir.path().to_path_buf(),
    );
    options.capstan = Some(Arc::new(role_config(dir.path(), 5)));
    let ready: Arc<Mutex<bool>> = Arc::default();
    {
        let ready = Arc::clone(&ready);
        options.announce = Some(Arc::new(move |event: &str, _pid: u32| {
            if event == "ready" {
                *ready.lock().unwrap() = true;
            }
        }));
    }
    let daemon = std::thread::spawn(move || run_daemon_with(options, Wiring::default()));
    let deadline = Instant::now() + Duration::from_secs(20);
    while !*ready.lock().unwrap() {
        assert!(Instant::now() < deadline, "the daemon did not become ready");
        std::thread::sleep(Duration::from_millis(20));
    }
    let socket = state.join("control.sock");
    // Without a launcher the commands that need one say so, with the text of Node.
    let launch = call(&socket, &identity.credential, "launch", &[]);
    assert_eq!(
        launch,
        json!({"ok": false, "code": "not_configured", "message": "launching agents needs capstan.toml and Herdr"})
    );
    let stopping = call(&socket, &identity.credential, "shutdown", &[]);
    assert_eq!(stopping["result"]["stopping"], true);
    assert!(daemon.join().unwrap().is_ok());
    let core = Core::open(
        &state,
        &identity.initial_project(),
        &KernelOptions {
            workspace_root: Some(dir.path().to_path_buf()),
            ..KernelOptions::default()
        },
        Box::new(SystemEnv),
    )
    .unwrap();
    let roles = core.role_definitions().unwrap();
    let names: Vec<&str> = roles
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|role| role["name"].as_str())
        .collect();
    assert_eq!(names, vec!["developer", "pm"], "{roles}");
    core.close();
}
