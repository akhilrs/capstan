//! A project whose in-project socket path is too long for a Unix socket address: the daemon binds the resolved socket in
//! the per-user runtime directory (`/tmp/capstan-<uid>`), guards it, keeps it fresh and rebinds it when it is lost.
//!
//! The project lock (taken when the kernel opens, before the server starts) already excludes a second daemon of the same
//! project, so the start-up probe only guards against a foreign listener: a hash collision or another program.

mod common;

use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::server::{DaemonServer, ServerOptions};
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::{Core, SystemEnv};
use common::project::{write_project, Identity};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The tests share the per-user runtime directory (one has it removed, one changes its mode), so they run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|p| p.into_inner())
}

fn identity(id: &str) -> Identity {
    Identity {
        project_id: id.into(),
        name: "Long Path".into(),
        credential: format!("operator-{}", "l".repeat(40)),
    }
}

/// A scratch project whose in-project socket path is longer than the limit. Its path is unique, so its hash is.
fn long_project() -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::Builder::new()
        .prefix("lp")
        .tempdir_in(capstan_testkit::short_temp_root())
        .unwrap();
    let mut dir = std::fs::canonicalize(root.path()).unwrap();
    while dir.join(".capstan/state/control.sock").as_os_str().len()
        <= capstan_wire::socket_path_limit()
    {
        dir.push("nested-directory-name");
    }
    std::fs::create_dir_all(&dir).unwrap();
    (root, dir)
}

struct Daemon {
    deps: Deps,
    server: Option<DaemonServer>,
    socket: PathBuf,
    credential: String,
    logs: Arc<Mutex<Vec<LogEntry>>>,
    state: PathBuf,
}

fn start_at(dir: &Path, state: &Path, identity: &Identity) -> (Deps, Arc<Mutex<Vec<LogEntry>>>) {
    let kernel = {
        let (state, project, directory) = (
            state.to_path_buf(),
            identity.initial_project(),
            dir.to_path_buf(),
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
    let logs: Arc<Mutex<Vec<LogEntry>>> = Arc::default();
    let sink: Logger = {
        let logs = Arc::clone(&logs);
        Arc::new(move |entry: &LogEntry| logs.lock().unwrap().push(entry.clone()))
    };
    let options = DaemonOptions::new(
        state.to_path_buf(),
        identity.initial_project(),
        dir.to_path_buf(),
    );
    (Deps::new(kernel, options, sink), logs)
}

impl Daemon {
    fn start(dir: &Path, id: &str) -> Daemon {
        let identity = identity(id);
        let state = write_project(dir, &identity);
        let (deps, logs) = start_at(dir, &state, &identity);
        let socket = deps.options.socket_path();
        let server = DaemonServer::start(deps.clone(), ServerOptions::new(socket.clone()))
            .expect("the server starts");
        Daemon {
            deps,
            server: Some(server),
            socket,
            credential: identity.credential,
            logs,
            state,
        }
    }

    fn ping(&self) -> Value {
        call(&self.socket, &self.credential, "ping")
    }

    fn stop(&mut self) {
        if let Some(server) = self.server.take() {
            server.close().expect("the server closes");
        }
        self.deps.kernel.close();
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.stop();
        let _ = std::fs::remove_file(&self.socket);
    }
}

fn call(socket: &Path, credential: &str, command: &str) -> Value {
    let mut stream = UnixStream::connect(socket).expect("connects");
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let frame = json!({"v": 1, "credential": credential, "command": command, "args": []});
    stream.write_all(format!("{frame}\n").as_bytes()).unwrap();
    let mut text = String::new();
    let _ = stream.read_to_string(&mut text);
    serde_json::from_str(text.trim_end()).unwrap_or_else(|e| panic!("{command}: {e}: {text:?}"))
}

fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o777
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if done() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn the_accessor_resolves_a_long_state_directory_to_the_runtime_path_and_a_short_one_to_the_project()
{
    let (_root, long) = long_project();
    let state = long.join(".capstan/state");
    let options = DaemonOptions::new(
        state.clone(),
        identity("pacc").initial_project(),
        long.clone(),
    );
    assert_eq!(options.socket_path(), capstan_wire::control_socket(&state));
    assert!(options
        .socket_path()
        .starts_with(Path::new("/tmp").join(capstan_wire::runtime_dir_name())));
    let short = tempfile::tempdir_in("/tmp").unwrap();
    let short_state = std::fs::canonicalize(short.path()).unwrap().join("state");
    let options = DaemonOptions::new(
        short_state.clone(),
        identity("pacc").initial_project(),
        short.path().to_path_buf(),
    );
    assert_eq!(options.socket_path(), short_state.join("control.sock"));
}

#[test]
fn a_long_project_binds_the_runtime_socket_serves_and_leaves_the_directory_on_stop() {
    let _serial = serial();
    let (_root, dir) = long_project();
    let mut daemon = Daemon::start(&dir, "plong");
    let runtime = Path::new("/tmp").join(capstan_wire::runtime_dir_name());
    assert_eq!(daemon.socket, capstan_wire::control_socket(&daemon.state));
    assert_eq!(daemon.socket.parent().unwrap(), runtime);
    assert_eq!(mode(&daemon.socket), 0o600);
    assert_eq!(mode(&runtime), 0o700);
    assert_eq!(daemon.ping()["ok"], true);
    let socket = daemon.socket.clone();
    daemon.stop();
    assert!(!socket.exists(), "stop removes the socket file");
    assert!(runtime.is_dir(), "stop leaves the runtime directory");
}

#[test]
fn the_start_guards_the_runtime_path() {
    let _serial = serial();
    // The project lock excludes a second daemon of the same project, so only a foreign listener is probed.
    let (_root, dir) = long_project();
    let identity = identity("pguard");
    let state = write_project(&dir, &identity);
    let (deps, _logs) = start_at(&dir, &state, &identity);
    let socket = deps.options.socket_path();
    let runtime = socket.parent().unwrap().to_path_buf();
    let start = || DaemonServer::start(deps.clone(), ServerOptions::new(socket.clone()));
    std::fs::create_dir_all(&runtime).unwrap();
    let _ = std::fs::remove_file(&socket);

    // A live foreign listener: the start fails and its socket stays.
    let listener = UnixListener::bind(&socket).unwrap();
    let error = start().err().expect("a live listener refuses the start");
    assert!(
        error.to_string().contains("another controller listens on"),
        "{error}"
    );
    assert!(socket.exists());
    // A stale socket is removed and the start succeeds.
    drop(listener);
    let server = start().expect("a stale socket is replaced");
    server.close().unwrap();
    assert!(!socket.exists());
    // A regular file refuses.
    std::fs::write(&socket, "x").unwrap();
    assert!(start()
        .err()
        .expect("refused")
        .to_string()
        .contains("not a socket"));
    std::fs::remove_file(&socket).unwrap();
    // A runtime directory that is not private refuses with ensure_private_dir's message.
    let original = mode(&runtime);
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o755)).unwrap();
    let error = start().err().expect("a 0755 directory refuses");
    assert!(
        error
            .to_string()
            .contains("not safe for the control socket"),
        "{error}"
    );
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(original)).unwrap();
    deps.kernel.close();
}

fn rebound(daemon: &Daemon) -> bool {
    daemon
        .logs
        .lock()
        .unwrap()
        .iter()
        .any(|entry| entry.command == "server" && entry.code == "socket_rebound")
}

#[test]
fn a_deleted_socket_is_rebound_within_seconds() {
    let _serial = serial();
    let (_root, dir) = long_project();
    let mut daemon = Daemon::start(&dir, "prebind");
    std::fs::remove_file(&daemon.socket).unwrap();
    wait_until("the socket_rebound log line", || rebound(&daemon));
    assert_eq!(daemon.ping()["ok"], true);
    let socket = daemon.socket.clone();
    daemon.stop();
    assert!(!socket.exists(), "stop removes the new socket");
}

#[test]
fn a_deleted_runtime_directory_is_recreated_and_the_socket_rebound() {
    let _serial = serial();
    let (_root, dir) = long_project();
    let mut daemon = Daemon::start(&dir, "pdirgone");
    let runtime = daemon.socket.parent().unwrap().to_path_buf();
    // Another test may share the directory; losing it only makes them rebind too.
    std::fs::remove_dir_all(&runtime).unwrap();
    wait_until("the socket_rebound log line", || rebound(&daemon));
    assert_eq!(mode(&runtime), 0o700);
    assert_eq!(daemon.ping()["ok"], true);
    let socket = daemon.socket.clone();
    daemon.stop();
    assert!(!socket.exists());
}
