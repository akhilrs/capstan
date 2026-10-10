//! `ensure_daemon`, `stop_daemon` and the calls against a scratch daemon (`client-test-daemon`) and hand-made sockets.
use capstan_client::{
    call_daemon, ensure_daemon, ping_daemon, stop_daemon, ClientError, EnsureOptions, PingOutcome,
    Reason, Response,
};
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

const DAEMON: &str = env!("CARGO_BIN_EXE_client-test-daemon");
const CREDENTIAL: &str = "operator-key";

struct Project {
    dir: tempfile::TempDir,
}

impl Project {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".capstan/state")).unwrap();
        Self { dir }
    }
    fn socket(&self) -> PathBuf {
        self.dir.path().join(".capstan/state/control.sock")
    }
    fn log(&self) -> PathBuf {
        self.dir.path().join(".capstan/daemon.log")
    }
    fn options(&self, mode: &str) -> EnsureOptions {
        let mut env: Vec<(OsString, OsString)> = std::env::vars_os().collect();
        for (name, value) in [
            ("CLIENT_TEST_SOCKET", self.socket().into_os_string()),
            ("CLIENT_TEST_CREDENTIAL", CREDENTIAL.into()),
            ("CLIENT_TEST_MODE", mode.into()),
            ("CAPSTAN_LAUNCH", "off".into()),
            ("CAPSTAN_TOKEN", "leaked-token".into()),
            ("CAPSTAN_SOCKET", "/leaked.sock".into()),
        ] {
            env.push((name.into(), value));
        }
        EnsureOptions {
            socket_path: self.socket(),
            credential: CREDENTIAL.into(),
            project_root: self.dir.path().to_path_buf(),
            log_path: self.log(),
            cli_path: DAEMON.into(),
            env,
            timeout_ms: Some(10_000),
        }
    }
}

fn unavailable(error: ClientError) -> (Reason, String) {
    match error {
        ClientError::Unavailable(e) => (e.reason, e.message),
        other => panic!("not an unavailable error: {other:?}"),
    }
}

#[test]
fn a_fresh_start_then_already_running_then_stop() {
    let project = Project::new();
    let options = project.options("serve");
    let first = ensure_daemon(&options).unwrap();
    assert!(first.started && first.pid > 0);
    let again = ensure_daemon(&options).unwrap();
    assert!(!again.started);
    assert_eq!(again.pid, first.pid);

    // The daemon ran in the project, detached, with its output in a private log and without the agent variables.
    let log = std::fs::read_to_string(project.log()).unwrap();
    assert_eq!(
        log, "started launch=off token=none socket_env=none\n",
        "CAPSTAN_LAUNCH is passed on, CAPSTAN_TOKEN and CAPSTAN_SOCKET are not"
    );
    let mode = std::fs::metadata(project.log())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);

    assert!(stop_daemon(&project.socket(), CREDENTIAL, Some(10_000)).unwrap());
    assert_eq!(
        ping_daemon(&project.socket(), CREDENTIAL, 1000),
        PingOutcome::Down
    );
    assert!(!stop_daemon(&project.socket(), CREDENTIAL, Some(10_000)).unwrap());
}

#[test]
fn a_stale_socket_and_pid_file_do_not_stop_a_start() {
    let project = Project::new();
    // A socket file nobody listens on, and the pid file of a daemon that died.
    drop(UnixListener::bind(project.socket()).unwrap());
    std::fs::write(
        project.dir.path().join(".capstan/state/controller.pid"),
        "999999\n",
    )
    .unwrap();
    assert_eq!(
        ping_daemon(&project.socket(), CREDENTIAL, 1000),
        PingOutcome::Down
    );
    let started = ensure_daemon(&project.options("serve")).unwrap();
    assert!(started.started);
    stop_daemon(&project.socket(), CREDENTIAL, Some(10_000)).unwrap();
}

fn foreign_socket(path: &Path, reply: &'static [u8]) {
    let listener = UnixListener::bind(path).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            let _ = BufReader::new(&stream).read_line(&mut line);
            let _ = (&stream).write_all(reply);
        }
    });
}

#[test]
fn a_foreign_socket_is_not_started_over() {
    let project = Project::new();
    foreign_socket(&project.socket(), b"hello there\n");
    let (reason, message) = unavailable(ensure_daemon(&project.options("serve")).unwrap_err());
    assert_eq!(reason, Reason::Unreachable);
    assert_eq!(
        message,
        "the controller did not answer; check the daemon log or retry"
    );
    assert!(!project.log().exists(), "no daemon was started");

    let other = Project::new();
    foreign_socket(
        &other.socket(),
        b"{\"ok\":false,\"code\":\"unauthorized\"}\n",
    );
    let (reason, message) = unavailable(ensure_daemon(&other.options("serve")).unwrap_err());
    assert_eq!(reason, Reason::Refused);
    assert_eq!(
        message,
        "the daemon refused the request (unauthorized); check the operator credential"
    );
}

#[test]
fn a_start_that_fails_says_why() {
    // The program cannot be run.
    let project = Project::new();
    let mut options = project.options("serve");
    options.cli_path = project.dir.path().join("no-such-cstan");
    let (reason, message) = unavailable(ensure_daemon(&options).unwrap_err());
    assert_eq!(reason, Reason::StartFailed);
    assert_eq!(
        message,
        format!(
            "the daemon could not be started: spawn {} ENOENT",
            options.cli_path.display()
        )
    );

    // It exits while starting: its exit code and the end of what it wrote.
    let project = Project::new();
    let (reason, message) = unavailable(ensure_daemon(&project.options("exit:3")).unwrap_err());
    assert_eq!(reason, Reason::StartFailed);
    assert_eq!(
        message,
        "the daemon exited during startup (exit code 3): started launch=off token=none socket_env=none | cstan: the project lock is held"
    );

    // Exit code 4 (the lock race lost) keeps waiting, up to the timeout.
    let project = Project::new();
    let mut options = project.options("exit:4");
    options.timeout_ms = Some(400);
    let (reason, message) = unavailable(ensure_daemon(&options).unwrap_err());
    assert_eq!(reason, Reason::StartTimeout);
    assert_eq!(
        message,
        format!(
            "the daemon did not answer within 0.4 seconds; see {} (another controller may be starting)",
            project.log().display()
        )
    );

    // It never answers.
    let project = Project::new();
    let mut options = project.options("idle");
    options.timeout_ms = Some(300);
    let (reason, message) = unavailable(ensure_daemon(&options).unwrap_err());
    assert_eq!(reason, Reason::StartTimeout);
    assert!(message.starts_with("the daemon did not answer within 0.3 seconds; see "));
}

#[test]
fn a_log_that_is_not_private_is_refused() {
    let project = Project::new();
    std::fs::write(project.log(), "").unwrap();
    std::fs::set_permissions(project.log(), std::fs::Permissions::from_mode(0o644)).unwrap();
    let error = ensure_daemon(&project.options("serve")).unwrap_err();
    assert_eq!(
        error,
        ClientError::Io(
            "daemon log must be a regular file owned by the current user with mode 0600".into()
        )
    );
    // A symbolic link is not followed.
    let project = Project::new();
    std::os::unix::fs::symlink("/dev/null", project.log()).unwrap();
    assert!(matches!(
        ensure_daemon(&project.options("serve")).unwrap_err(),
        ClientError::Io(_)
    ));
}

#[test]
fn calls_return_the_response_or_the_error_code() {
    let project = Project::new();
    let missing = call_daemon(&project.socket(), CREDENTIAL, "ping", &[], 1000).unwrap_err();
    assert_eq!(missing.code, "ENOENT");
    drop(UnixListener::bind(project.socket()).unwrap());
    let refused = call_daemon(&project.socket(), CREDENTIAL, "ping", &[], 1000).unwrap_err();
    assert_eq!(refused.code, "ECONNREFUSED");

    let other = Project::new();
    foreign_socket(&other.socket(), b"[1]\n");
    let malformed = call_daemon(&other.socket(), CREDENTIAL, "ping", &[], 1000).unwrap_err();
    assert_eq!(
        (malformed.code.as_str(), malformed.message.as_str()),
        ("EBADMSG", "malformed reply")
    );

    let third = Project::new();
    foreign_socket(
        &third.socket(),
        b"{\"ok\":false,\"code\":\"nope\",\"message\":\"m\"}\n",
    );
    assert!(matches!(
        call_daemon(&third.socket(), CREDENTIAL, "ping", &[], 1000),
        Ok(Response::Refused { .. })
    ));

    // A request over the frame limit is not sent.
    let big = vec!["x".repeat(70_000)];
    let error = call_daemon(&third.socket(), CREDENTIAL, "send", &big, 1000).unwrap_err();
    assert_eq!(error.message, "command request is too large");
}
