//! `ensureDaemon` and `stopDaemon`.

use crate::log::{log_tail, open_daemon_log};
use crate::{
    call_daemon, ping_daemon, unavailable, unavailable_reason, ClientError, PingOutcome, Reason,
    DEFAULT_TIMEOUT_MS,
};
use capstan_wire::Response;
use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A cap, not a delay: a start returns the moment the daemon answers a ping, or fails the moment the child exits.
const START_TIMEOUT_MS: u64 = 30_000;
const POLL_MS: u64 = 100;
const NOT_EXITED: i64 = i64::MIN;
/// Long enough for the winning daemon to finish starting on a loaded machine.
const LOST_RACE_GRACE_MS: u64 = 20_000;

pub struct EnsureOptions {
    pub socket_path: PathBuf,
    pub credential: String,
    pub project_root: PathBuf,
    pub log_path: PathBuf,
    /// The `cstan` executable: it is started as `<cli_path> daemon`.
    pub cli_path: PathBuf,
    /// The environment the daemon gets (less `CAPSTAN_TOKEN` and `CAPSTAN_SOCKET`): `CAPSTAN_LAUNCH`, `CSTAN_NODE_CLI`
    /// and the rest of the caller's.
    pub env: Vec<(OsString, OsString)>,
    pub timeout_ms: Option<u64>,
}

/// `{pid, started}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Started {
    pub pid: i64,
    pub started: bool,
}

fn errno_name(error: &std::io::Error) -> Option<&'static str> {
    Some(match error.raw_os_error()? {
        1 => "EPERM",
        2 => "ENOENT",
        7 => "E2BIG",
        8 => "ENOEXEC",
        12 => "ENOMEM",
        13 => "EACCES",
        20 => "ENOTDIR",
        26 => "ETXTBSY",
        36 => "ENAMETOOLONG",
        40 => "ELOOP",
        _ => return None,
    })
}

fn spawn_text(program: &Path, error: &std::io::Error) -> String {
    match errno_name(error) {
        Some(code) => format!("spawn {} {code}", program.display()),
        None => error.to_string(),
    }
}

fn spawn_daemon(options: &EnsureOptions, log: std::fs::File) -> Result<Child, std::io::Error> {
    let second = log.try_clone()?;
    let mut command = Command::new(&options.cli_path);
    command
        .arg("daemon")
        .current_dir(&options.project_root)
        .env_clear()
        .envs(
            options
                .env
                .iter()
                .filter(|(name, _)| name != "CAPSTAN_TOKEN" && name != "CAPSTAN_SOCKET")
                .map(|(name, value)| (name, value)),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(second));
    // Detached, as Node's `detached: true`: its own session, so the daemon outlives this process and its terminal.
    // SAFETY: setsid is async-signal-safe and touches no memory of this process.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    command.spawn()
}

fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// The daemon of the project, started if it does not answer: `{pid, started}`.
pub fn ensure_daemon(options: &EnsureOptions) -> Result<Started, ClientError> {
    let socket = options.socket_path.as_path();
    let ping = || ping_daemon(socket, &options.credential, DEFAULT_TIMEOUT_MS);
    let first = ping();
    if let PingOutcome::Running { pid } = first {
        return Ok(Started {
            pid,
            started: false,
        });
    }
    if let Some(blocked) = unavailable(&first) {
        return Err(blocked.into());
    }

    let log = open_daemon_log(&options.log_path).map_err(ClientError::Io)?;
    let log_offset = log.metadata().map(|m| m.len()).unwrap_or(0);
    let mut spawn_error: Option<String> = None;
    // The child is waited for on its own thread, so a daemon that ends is never left a zombie in a long-lived caller.
    let exited = Arc::new(AtomicI64::new(NOT_EXITED));
    match spawn_daemon(options, log) {
        Ok(mut child) => {
            let exited = Arc::clone(&exited);
            std::thread::spawn(move || {
                let code = child.wait().map_or(1, |status| status.code().unwrap_or(1));
                exited.store(i64::from(code), Ordering::SeqCst);
            });
        }
        Err(error) => spawn_error = Some(spawn_text(&options.cli_path, &error)),
    }
    let mut exit_code: Option<i32> = None;
    let mut lost_race_at: Option<Instant> = None;
    let timeout_ms = options.timeout_ms.unwrap_or(START_TIMEOUT_MS);
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        sleep_ms(POLL_MS);
        let outcome = ping();
        if let PingOutcome::Running { pid } = outcome {
            return Ok(Started { pid, started: true });
        }
        if let Some(message) = &spawn_error {
            return Err(unavailable_reason(
                Reason::StartFailed,
                format!("the daemon could not be started: {message}"),
            )
            .into());
        }
        match exited.load(Ordering::SeqCst) {
            NOT_EXITED => {}
            code => exit_code = Some(code as i32),
        }
        // Exit code 4 means the child lost the lock race: another controller is starting, so keep polling for a short
        // grace period. If nobody answers by then, the lock holder is not a starting daemon and waiting is futile.
        if exit_code == Some(4) {
            let since = *lost_race_at.get_or_insert_with(Instant::now);
            if since.elapsed() > Duration::from_millis(LOST_RACE_GRACE_MS) {
                return Err(unavailable_reason(
                    Reason::StartFailed,
                    "another controller holds the project lock but does not answer; stop it or check the daemon log",
                )
                .into());
            }
        }
        if let Some(code) = exit_code.filter(|code| *code != 4) {
            return Err(unavailable_reason(
                Reason::StartFailed,
                format!(
                    "the daemon exited during startup (exit code {code}): {}",
                    log_tail(&options.log_path, log_offset)
                ),
            )
            .into());
        }
        if let Some(failed) = unavailable(&outcome) {
            if !matches!(outcome, PingOutcome::Unreachable { .. }) {
                return Err(failed.into());
            }
        }
    }
    Err(unavailable_reason(
        Reason::StartTimeout,
        format!(
            "the daemon did not answer within {} seconds; see {} (another controller may be starting)",
            timeout_ms as f64 / 1000.0,
            options.log_path.display()
        ),
    )
    .into())
}

fn process_alive(pid: i64) -> bool {
    if pid <= 0 || pid > i64::from(i32::MAX) {
        return false;
    }
    // SAFETY: signal 0 only checks that the process exists.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Asks the daemon to shut down and waits until its process is gone: `true` for stopped, `false` for not running.
pub fn stop_daemon(
    socket: &Path,
    credential: &str,
    timeout_ms: Option<u64>,
) -> Result<bool, ClientError> {
    let first = ping_daemon(socket, credential, DEFAULT_TIMEOUT_MS);
    if first == PingOutcome::Down {
        return Ok(false);
    }
    if let Some(blocked) = unavailable(&first) {
        return Err(blocked.into());
    }
    let pid = match first {
        PingOutcome::Running { pid } => pid,
        _ => 0,
    };
    match call_daemon(socket, credential, "shutdown", &[], DEFAULT_TIMEOUT_MS)? {
        Response::Ok { .. } => {}
        _ => {
            return Err(unavailable_reason(
                Reason::Refused,
                "the daemon did not accept the shutdown request",
            )
            .into())
        }
    }
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.unwrap_or(START_TIMEOUT_MS));
    while Instant::now() < deadline {
        sleep_ms(POLL_MS);
        // Stopped means the process is gone: the kernel then releases the project lock. A closed listener alone is not
        // enough, because the daemon frees the lock a moment after it stops accepting connections.
        let gone = if pid > 0 {
            !process_alive(pid)
        } else {
            ping_daemon(socket, credential, DEFAULT_TIMEOUT_MS) == PingOutcome::Down
        };
        if gone {
            return Ok(true);
        }
    }
    Err(unavailable_reason(
        Reason::StartTimeout,
        "the daemon did not exit in time; check the pid file in the state directory",
    )
    .into())
}
