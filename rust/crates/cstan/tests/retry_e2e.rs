//! `cstan` against a fake controller that is busy, slow or drops answers: busy waits, lost-reply retries and the
//! final error texts. The fake keeps the real daemon's rules where they matter here: a connection over its limit gets
//! one `controller_busy` line and a close, and `<socket>.boot` holds the daemon's boot id. Assertions are on exit
//! codes, output and counts, never on timing.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::{handle_frame_with, FrameOutcome};
use capstan_daemon::server::{DaemonServer, FrameHandler, ServerOptions};
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::{Core, SystemEnv};
use serde_json::json;

#[allow(dead_code)]
#[path = "../../daemon/tests/common/project.rs"]
mod project;
use project::{write_project, Identity};

const FRONT: &str = env!("CARGO_BIN_EXE_cstan");
const BUSY: &str = r#"{"ok":false,"code":"controller_busy","message":"controller busy: all connection slots are in use","retryAfterMs":100}"#;
const OK: &str = r#"{"ok":true,"result":{"state":"fine"}}"#;
const RETRIED_TEXT: &str = "connection closed; retried 2 times, the request may have been applied: check with cstan inbox or cstan status before running it again";

struct Fake {
    dir: PathBuf,
    connections: Arc<AtomicUsize>,
    rids: Arc<Mutex<Vec<String>>>,
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// How the fake treats one connection: its index, the request line and the stream.
type Behavior = dyn Fn(usize, &str, UnixStream, &PathBuf) + Send + Sync;

fn fake(label: &str, boot: Option<&str>, behavior: Arc<Behavior>) -> Fake {
    let dir =
        std::env::temp_dir().join(format!("capstan-retry-e2e-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let socket = dir.join("control.sock");
    if let Some(id) = boot {
        std::fs::write(dir.join("control.sock.boot"), id).unwrap();
    }
    let listener = UnixListener::bind(&socket).unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let rids = Arc::new(Mutex::new(Vec::new()));
    let (count, seen, boot_dir) = (connections.clone(), rids.clone(), dir.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let index = count.fetch_add(1, Ordering::SeqCst);
            let (behavior, seen, boot_dir) = (behavior.clone(), seen.clone(), boot_dir.clone());
            std::thread::spawn(move || {
                let mut line = String::new();
                let _ = BufReader::new(&stream).read_line(&mut line);
                if let Some(at) = line.find("\"rid\":\"") {
                    let rest = &line[at + 7..];
                    seen.lock()
                        .unwrap()
                        .push(rest[..rest.find('"').unwrap()].to_string());
                }
                behavior(index, &line, stream, &boot_dir);
            });
        }
    });
    Fake {
        dir,
        connections,
        rids,
    }
}

fn reply(mut stream: UnixStream, text: &str) {
    let _ = writeln!(stream, "{text}");
}

fn command(f: &Fake, args: &[&str]) -> Command {
    let mut c = Command::new(FRONT);
    c.args(args)
        .current_dir(&f.dir)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("CAPSTAN_TOKEN", "agent-token")
        .env("CAPSTAN_SOCKET", f.dir.join("control.sock"))
        .env("CAPSTAN_AGENT_ID", "developer-1")
        .stdin(Stdio::null());
    c
}

fn run(f: &Fake, args: &[&str]) -> Output {
    command(f, args).output().unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn sixty_five_status_processes_against_eight_slots_all_succeed() {
    let active = Arc::new(AtomicUsize::new(0));
    let behavior: Arc<Behavior> = {
        let active = active.clone();
        Arc::new(move |_, _, stream, _| {
            if active.fetch_add(1, Ordering::SeqCst) >= 8 {
                active.fetch_sub(1, Ordering::SeqCst);
                reply(stream, BUSY);
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
            reply(stream, OK);
            active.fetch_sub(1, Ordering::SeqCst);
        })
    };
    let f = fake("load", Some("boot-1"), behavior);
    let children: Vec<_> = (0..65)
        .map(|_| {
            command(&f, &["status", "--json"])
                .env("CAPSTAN_BUSY_BUDGET_MS", "120000")
                .env("CAPSTAN_RETRY_LOG", "1")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let outputs: Vec<Output> = children
        .into_iter()
        .map(|c| c.wait_with_output().unwrap())
        .collect();
    let quiet = run(&f, &["status", "--json"]);
    assert_eq!(quiet.status.code(), Some(0));
    for o in &outputs {
        assert_eq!(o.status.code(), Some(0), "{}", stderr(o));
        assert!(!stderr(o).contains("connection closed"), "{}", stderr(o));
        assert_eq!(o.stdout, quiet.stdout);
    }
    assert!(
        outputs
            .iter()
            .any(|o| stderr(o).contains("controller busy, retrying in ")),
        "at least one process met a busy answer"
    );
    // One request id per process; every retry of one process used the same id.
    let rids = f.rids.lock().unwrap();
    let mut distinct: Vec<&String> = rids.iter().collect();
    distinct.sort();
    distinct.dedup();
    assert_eq!(distinct.len(), 66);
}

#[test]
fn retry_lines_stay_off_stderr_by_default() {
    let first = Arc::new(AtomicUsize::new(0));
    let behavior: Arc<Behavior> = {
        let first = first.clone();
        Arc::new(move |_, _, stream, _| {
            reply(
                stream,
                if first.fetch_add(1, Ordering::SeqCst) == 0 {
                    BUSY
                } else {
                    OK
                },
            )
        })
    };
    let f = fake("quiet", Some("boot-1"), behavior);
    let o = run(&f, &["status", "--json"]);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(stderr(&o), "");
    assert_eq!(f.connections.load(Ordering::SeqCst), 2);
}

#[test]
fn the_inbox_hook_gives_up_after_its_small_budget_and_exits_zero() {
    let behavior: Arc<Behavior> =
        Arc::new(|_, _, stream, _| reply(stream, &BUSY.replace("100", "500")));
    let f = fake("hook", Some("boot-1"), behavior);
    let o = run(&f, &["inbox", "--hook"]);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&o.stdout), "");
    assert_eq!(stderr(&o), "");
    // A 2 s budget in waits of at least 500 ms: five attempts at the most.
    let attempts = f.connections.load(Ordering::SeqCst);
    assert!((2..=5).contains(&attempts), "{attempts}");
}

#[test]
fn without_a_boot_file_a_lost_reply_is_not_retried() {
    let behavior: Arc<Behavior> = Arc::new(|_, _, stream, _| drop(stream));
    let f = fake("noboot", None, behavior);
    let o = run(&f, &["status"]);
    assert_eq!(o.status.code(), Some(5), "{}", stderr(&o));
    assert!(stderr(&o).contains("connection closed"), "{}", stderr(&o));
    assert!(!stderr(&o).contains("retried"), "{}", stderr(&o));
    assert_eq!(f.connections.load(Ordering::SeqCst), 1);
}

#[test]
fn a_reply_lost_twice_is_asked_again_with_one_request_id() {
    let behavior: Arc<Behavior> = Arc::new(|i, _, stream, _| {
        if i < 2 {
            drop(stream)
        } else {
            reply(stream, OK)
        }
    });
    let f = fake("twice", Some("boot-1"), behavior);
    let o = run(&f, &["status", "--json"]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(f.connections.load(Ordering::SeqCst), 3);
    let rids = f.rids.lock().unwrap();
    assert_eq!(rids.len(), 3);
    assert!(rids.iter().all(|r| *r == rids[0]));
}

#[test]
fn a_reply_that_is_always_lost_ends_with_the_retried_text() {
    let behavior: Arc<Behavior> = Arc::new(|_, _, stream, _| drop(stream));
    let f = fake("always", Some("boot-1"), behavior);
    let o = run(&f, &["send", "pm", "hello"]);
    assert_eq!(o.status.code(), Some(5), "{}", stderr(&o));
    assert_eq!(stderr(&o).trim_end(), format!("cstan: {RETRIED_TEXT}"));
    assert_eq!(f.connections.load(Ordering::SeqCst), 3);
}

#[test]
fn a_boot_id_that_changes_adds_the_restart_note() {
    let behavior: Arc<Behavior> = Arc::new(|_, _, stream, dir| {
        std::fs::write(dir.join("control.sock.boot"), "boot-2").unwrap();
        drop(stream)
    });
    let f = fake("restart", Some("boot-1"), behavior);
    let o = run(&f, &["status"]);
    assert_eq!(o.status.code(), Some(5), "{}", stderr(&o));
    assert!(
        stderr(&o)
            .trim_end()
            .ends_with("connection closed; the controller restarted during the request"),
        "{}",
        stderr(&o)
    );
    assert_eq!(f.connections.load(Ordering::SeqCst), 1);
}

// ---- Through the daemon's handler seam (`ServerOptions.handler`): the real command handlers, with a policy that
// ---- answers busy or drops the answer of chosen attempts.

#[derive(Clone, Copy)]
enum Mode {
    Pass,
    /// The first attempt gets a busy line without running.
    BusyOnce,
    /// The command runs on every attempt; the first `n` answers are never written (the connection just closes).
    DropFirst(usize),
}

struct Seam {
    dir: PathBuf,
    socket: PathBuf,
    agents: std::collections::HashMap<String, String>,
    mode: Arc<Mutex<(Mode, usize)>>,
    _server: DaemonServer,
}

impl Drop for Seam {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn dispatch(core: &Core, owner: &str, op: &str, args: serde_json::Value) -> serde_json::Value {
    let version = core.state_version().unwrap();
    let context = json!({
        "credential": owner,
        "requestId": format!("req-{op}-{version}"),
        "idempotencyKey": format!("idem-{op}-{version}"),
        "expectedVersion": version,
        "inputRevision": core.input_revision().unwrap(),
    });
    let mut all = vec![context];
    all.extend(args.as_array().cloned().unwrap());
    core.dispatch(op, &all)
        .unwrap_or_else(|e| panic!("setup {op}: {e}"))
}

impl Seam {
    fn start(label: &str) -> Seam {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::fs::canonicalize("/tmp")
            .unwrap()
            .join(format!("cre-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let identity = Identity {
            project_id: "pretry".into(),
            name: "Retry Test".into(),
            credential: format!("operator-{}", "a".repeat(40)),
        };
        let state = write_project(&dir, &identity);
        let options = KernelOptions {
            workspace_root: Some(dir.clone()),
            ..KernelOptions::default()
        };
        let core = Core::open(
            &state,
            &identity.initial_project(),
            &options,
            Box::new(SystemEnv),
        )
        .expect("the setup controller opens");
        let owner = identity.credential.as_str();
        dispatch(
            &core,
            owner,
            "syncRoleDefinitions",
            json!([[
                {"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)},
                {"name": "pm", "kind": "PM", "host": "claude", "configHash": "b".repeat(64)},
            ]]),
        );
        let mut agents = std::collections::HashMap::new();
        for (agent, role_name, kind) in [("pm-1", "pm", "PM"), ("dev-1", "developer", "Developer")]
        {
            let seat = format!("seat-{agent}");
            dispatch(
                &core,
                owner,
                "createSeat",
                json!([{"seatId": seat, "name": seat, "role": kind}]),
            );
            let actor = dispatch(
                &core,
                owner,
                "createActor",
                json!([{"displayName": agent, "role": kind, "seatId": seat}]),
            );
            dispatch(
                &core,
                owner,
                "registerAgent",
                json!([{"agentId": agent, "roleName": role_name, "seatId": seat, "actorId": actor["actorId"]}]),
            );
            agents.insert(
                agent.to_string(),
                actor["credential"].as_str().unwrap().to_string(),
            );
        }
        core.close();
        let kernel = {
            let (state, project, directory) =
                (state.clone(), identity.initial_project(), dir.clone());
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
        let deps = Deps::new(
            kernel,
            DaemonOptions::new(state.clone(), identity.initial_project(), dir.clone()),
            sink,
        );
        let socket = state.join("control.sock");
        let mode = Arc::new(Mutex::new((Mode::Pass, 0usize)));
        let policy = mode.clone();
        let mut options = ServerOptions::new(socket.clone());
        options.handler = Some(FrameHandler::new(
            move |deps, frame, signal, limit, on_limit| {
                let (mode, attempt) = {
                    let mut guard = policy.lock().unwrap();
                    let seen = guard.1;
                    guard.1 += 1;
                    (guard.0, seen)
                };
                if let (Mode::BusyOnce, 0) = (mode, attempt) {
                    return FrameOutcome {
                        response: Some(BUSY.to_string()),
                        log: LogEntry::new("busy", "controller_busy"),
                        identified: true,
                    };
                }
                let mut outcome = handle_frame_with(deps, frame, signal, limit, on_limit);
                if matches!(mode, Mode::DropFirst(n) if attempt < n) {
                    outcome.response = None;
                }
                outcome
            },
        ));
        let server = DaemonServer::start(deps, options).expect("the server starts");
        Seam {
            dir,
            socket,
            agents,
            mode,
            _server: server,
        }
    }

    fn set(&self, mode: Mode) {
        *self.mode.lock().unwrap() = (mode, 0);
    }

    fn attempts(&self) -> usize {
        self.mode.lock().unwrap().1
    }

    fn cstan(&self, agent: &str, args: &[&str]) -> Output {
        self.cstan_with(agent, args, true)
    }

    fn cstan_with(&self, agent: &str, args: &[&str], log: bool) -> Output {
        let mut command = Command::new(FRONT);
        command
            .args(args)
            .current_dir(&self.dir)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("CAPSTAN_TOKEN", &self.agents[agent])
            .env("CAPSTAN_SOCKET", &self.socket)
            .env("CAPSTAN_AGENT_ID", agent)
            .stdin(Stdio::null());
        if log {
            command.env("CAPSTAN_RETRY_LOG", "1");
        }
        command.output().unwrap()
    }
}

#[test]
fn seam_status_json_after_a_busy_wait_is_byte_identical() {
    let seam = Seam::start("status");
    let plain = seam.cstan("dev-1", &["status", "--json"]);
    assert_eq!(plain.status.code(), Some(0), "{}", stderr(&plain));
    seam.set(Mode::BusyOnce);
    let retried = seam.cstan("dev-1", &["status", "--json"]);
    assert_eq!(retried.status.code(), Some(0), "{}", stderr(&retried));
    assert!(
        stderr(&retried).contains("controller busy, retrying in "),
        "{}",
        stderr(&retried)
    );
    assert_eq!(seam.attempts(), 2);
    assert_eq!(retried.stdout, plain.stdout);
}

#[test]
fn seam_inbox_after_a_lost_reply_is_byte_identical() {
    let seam = Seam::start("inbox");
    let sent = seam.cstan("pm-1", &["send", "dev-1", "hello there"]);
    assert_eq!(sent.status.code(), Some(0), "{}", stderr(&sent));
    let plain = seam.cstan("dev-1", &["inbox"]);
    assert_eq!(plain.status.code(), Some(0), "{}", stderr(&plain));
    assert!(String::from_utf8_lossy(&plain.stdout).contains("hello there"));
    seam.set(Mode::DropFirst(1));
    let retried = seam.cstan("dev-1", &["inbox"]);
    assert_eq!(retried.status.code(), Some(0), "{}", stderr(&retried));
    assert!(
        stderr(&retried).contains("controller did not answer, retrying"),
        "{}",
        stderr(&retried)
    );
    assert_eq!(seam.attempts(), 2);
    assert_eq!(retried.stdout, plain.stdout);
}

#[test]
fn seam_send_whose_reply_is_always_lost_ends_with_the_retried_text() {
    let seam = Seam::start("send");
    seam.set(Mode::DropFirst(usize::MAX));
    let o = seam.cstan_with("pm-1", &["send", "dev-1", "once only"], false);
    assert_eq!(o.status.code(), Some(5), "{}", stderr(&o));
    let err = stderr(&o);
    assert_eq!(err.trim_end(), format!("cstan: {RETRIED_TEXT}"));
    assert_eq!(seam.attempts(), 3);
    // The send ran once: the repeats were answered from the reply cache.
    seam.set(Mode::Pass);
    let inbox = seam.cstan("dev-1", &["inbox"]);
    assert_eq!(
        String::from_utf8_lossy(&inbox.stdout)
            .matches("once only")
            .count(),
        1
    );
}
