//! The control socket server (src/daemon.ts `startDaemonServer`): listener, connections, frame limits, timeouts, half-close
//! and drain. `handlers::handle_frame_with` is what a connection runs per frame (its hook arms a limited command's timer).
//!
//! One thread accepts, one thread serves each connection (at most `[daemon] max_connections` at a time, 64 by default),
//! and one thread enforces the connection deadlines and drains the lingering refusals. A connection over a limit gets
//! one `controller_busy` line, never a silent close. A connection thread blocks in its handler;
//! the handlers take the kernel one ledger step at a time, so a slow launcher call or a long poll never holds anything
//! another connection needs.
//!
//! Per connection, as in Node:
//!  - a request frame is the bytes up to the first newline, at most 64 KiB; a longer one drops the connection without an
//!    answer, as does a close before the newline;
//!  - the 5 second timer is armed at the start and never cleared (a connection that is still open then is destroyed); a
//!    command with a time limit re-arms it to `max(6 s, limit + 5 s)`;
//!  - a client that half-closes after its frame still gets its reply, except for a command with a time limit (wait, the
//!    launcher commands), which the client cannot outlive: its handler is aborted with `closed`;
//!  - the reply is written and the connection's write side ends.

mod socket;

pub use socket::{bind, rebind, remove_if_ours, remove_stale_socket, SocketIdentity};
use socket::{is_ours, is_runtime_socket, touch_runtime_socket};

use crate::deps::{Deps, LogEntry};
use crate::handlers::shared::{fail, AbortSignal, ErrorCode};
use crate::handlers::{
    handle_frame_with, replay, FrameOutcome, DRAIN_FLUSH_MS, MAX_FRAME_BYTES, MAX_RESPONSE_BYTES,
    REQUEST_TIMEOUT_MS,
};
use capstan_wire::js;
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

/// How many refused connections wait for their client to read the busy line (before the fd clamp).
pub const LINGER_MAX: usize = 1024;
/// How long a refused connection waits for its client's end.
pub const LINGER_MS: u64 = 1000;
/// How long a connection refused over a share waits for its client's end.
const BUSY_CLOSE_MS: u64 = 1000;
/// The server_busy log line is written at most this often.
const BUSY_LOG_MS: u64 = 10_000;
/// Descriptors kept for the rest of the daemon.
const FD_HEADROOM: usize = 256;

/// The type a frame handler has: the arguments of `handle_frame_with`.
type FrameFn = dyn Fn(&Deps, &[u8], &AbortSignal, usize, &mut dyn FnMut(u64, bool)) -> FrameOutcome
    + Send
    + Sync;

/// A replacement for `handle_frame_with` (a test seam): it runs after admission, with the same arguments.
#[derive(Clone)]
pub struct FrameHandler(Arc<FrameFn>);

impl FrameHandler {
    pub fn new(
        handler: impl Fn(&Deps, &[u8], &AbortSignal, usize, &mut dyn FnMut(u64, bool)) -> FrameOutcome
            + Send
            + Sync
            + 'static,
    ) -> Self {
        FrameHandler(Arc::new(handler))
    }
}

impl std::fmt::Debug for FrameHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FrameHandler(..)")
    }
}

/// What the server is started with.
#[derive(Clone, Debug)]
pub struct ServerOptions {
    pub socket_path: PathBuf,
    /// The largest response line; `MAX_RESPONSE_BYTES` by default.
    pub max_response_bytes: usize,
    /// Runs instead of `handle_frame_with` once a request is admitted. Tests only.
    #[doc(hidden)]
    pub handler: Option<FrameHandler>,
}

impl ServerOptions {
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            max_response_bytes: MAX_RESPONSE_BYTES,
            handler: None,
        }
    }
}

#[derive(Default)]
struct ConnState {
    deadline: Option<Instant>,
    dispatched: bool,
    /// A command with a time limit runs: the client cannot half-close.
    limited: bool,
    /// The reply was started (Node's `writableEnded`).
    answered: bool,
    destroyed: bool,
}

struct Conn {
    id: u64,
    /// A handle to the same socket, to end it from another thread.
    killer: UnixStream,
    signal: AbortSignal,
    state: Mutex<ConnState>,
}

impl Conn {
    fn arm(&self, after: Duration) {
        lock(&self.state).deadline = Some(Instant::now() + after);
    }

    /// Drain's answer to a connection whose request has not been dispatched: the line goes out exactly once (the state
    /// lock is held across the check and the write), and the serving thread never dispatches the request afterwards.
    fn answer_shutting_down(&self) {
        let mut state = lock(&self.state);
        if state.destroyed || state.answered || state.dispatched {
            return;
        }
        state.answered = true;
        let mut line = fail(ErrorCode::ShuttingDown, "the daemon is shutting down").to_text();
        line.push('\n');
        let mut writer = &self.killer;
        let _ = writer.write_all(line.as_bytes());
        let _ = self.killer.shutdown(std::net::Shutdown::Write);
    }

    /// `socket.destroy()`: ends both directions, which wakes a read in progress, and aborts the handler with `closed`.
    fn destroy(&self) {
        {
            let mut state = lock(&self.state);
            if state.destroyed {
                return;
            }
            state.destroyed = true;
        }
        let _ = self.killer.shutdown(std::net::Shutdown::Both);
        self.signal.abort("closed");
    }
}

/// The connection limits of a running server.
struct Limits {
    max: usize,
    caller: usize,
    wait: usize,
    wait_per_caller: usize,
    retry_ms: u64,
    linger_max: usize,
}

/// A refused connection kept open until its client has read the busy line.
struct Lingering {
    stream: UnixStream,
    since: Instant,
}

#[derive(Default)]
struct Bucket {
    other: usize,
    waits: usize,
}

#[derive(Default)]
struct Admission {
    buckets: HashMap<[u8; 32], Bucket>,
    waits: usize,
}

const REASONS: [&str; 5] = ["slots", "caller", "unverified", "waits", "linger_full"];
const SLOTS: usize = 0;
const CALLER: usize = 1;
const UNVERIFIED: usize = 2;
const WAITS: usize = 3;
const LINGER_FULL: usize = 4;

#[derive(Default)]
struct BusyLog {
    counts: [u64; 5],
    last: Option<Instant>,
}

struct Shared {
    deps: Deps,
    limits: Limits,
    handler: Option<FrameHandler>,
    linger: Mutex<Vec<Lingering>>,
    admission: Mutex<Admission>,
    busy_log: Mutex<BusyLog>,
    response_limit: usize,
    draining: AtomicBool,
    next_id: AtomicU64,
    conns: Mutex<HashMap<u64, Arc<Conn>>>,
    conns_changed: Condvar,
    /// Connections whose request is being answered (Node's `running`).
    running: Mutex<usize>,
    running_changed: Condvar,
    reaper_stop: AtomicBool,
    /// Connections drain destroyed without giving them a line.
    drain_lineless: std::sync::atomic::AtomicUsize,
    /// The identity of the socket file this server created; the accept loop updates it when it rebinds.
    identity: Mutex<SocketIdentity>,
}

impl Shared {
    fn log(&self, entry: LogEntry) {
        (self.deps.log)(&entry);
    }

    /// The busy line, with its newline.
    fn busy_line(&self, reason: usize) -> String {
        let text = match reason {
            SLOTS => "all connection slots are in use",
            CALLER => "too many open connections for this caller",
            WAITS => "too many waits are open",
            _ => "the controller is busy",
        };
        let mut line = serde_json::json!({
            "ok": false,
            "code": capstan_wire::CONTROLLER_BUSY,
            "message": format!("controller busy: {text}"),
            "retryAfterMs": self.limits.retry_ms,
        })
        .to_string();
        line.push('\n');
        line
    }

    /// Counts a refusal; the server_busy line is written at most once per `BUSY_LOG_MS`.
    fn note_busy(&self, reason: usize) {
        lock(&self.busy_log).counts[reason] += 1;
        self.flush_busy();
    }

    fn flush_busy(&self) {
        let counts = {
            let mut log = lock(&self.busy_log);
            let due = log
                .last
                .is_none_or(|at| at.elapsed() >= Duration::from_millis(BUSY_LOG_MS));
            if !due || log.counts.iter().all(|&n| n == 0) {
                return;
            }
            log.last = Some(Instant::now());
            std::mem::take(&mut log.counts)
        };
        let mut detail = serde_json::Map::new();
        for (name, count) in REASONS.iter().zip(counts) {
            detail.insert((*name).to_string(), count.into());
        }
        let mut entry = LogEntry::new("server", "server_busy");
        entry.detail = Some(serde_json::Value::Object(detail));
        self.log(entry);
    }

    /// Takes a place in the caller's share, or says which limit is reached.
    fn acquire(self: &Arc<Self>, key: [u8; 32], wait: bool) -> Result<Share, usize> {
        let mut admission = lock(&self.admission);
        let (waits_open, bucket_waits, bucket_other) = (
            admission.waits,
            admission.buckets.get(&key).map_or(0, |b| b.waits),
            admission.buckets.get(&key).map_or(0, |b| b.other),
        );
        if wait {
            if waits_open >= self.limits.wait {
                return Err(WAITS);
            }
            if bucket_waits >= self.limits.wait_per_caller {
                return Err(CALLER);
            }
            admission.waits += 1;
            admission.buckets.entry(key).or_default().waits += 1;
        } else {
            if bucket_other >= self.limits.caller {
                return Err(CALLER);
            }
            admission.buckets.entry(key).or_default().other += 1;
        }
        Ok(Share {
            shared: Arc::clone(self),
            key,
            wait,
        })
    }
}

/// One place in a caller's share; the bucket is removed when it is empty.
struct Share {
    shared: Arc<Shared>,
    key: [u8; 32],
    wait: bool,
}

impl Drop for Share {
    fn drop(&mut self) {
        let mut admission = lock(&self.shared.admission);
        if self.wait {
            admission.waits = admission.waits.saturating_sub(1);
        }
        if let Some(bucket) = admission.buckets.get_mut(&self.key) {
            if self.wait {
                bucket.waits = bucket.waits.saturating_sub(1);
            } else {
                bucket.other = bucket.other.saturating_sub(1);
            }
            if bucket.waits == 0 && bucket.other == 0 {
                admission.buckets.remove(&self.key);
            }
        }
    }
}

/// The caller key and wait class of a frame, from the frame alone. None for what is not counted: a frame that is not
/// an object, has no string credential, or is a legacy `action` request.
fn classify(frame: &[u8]) -> Option<([u8; 32], bool)> {
    let value = js::parse(std::str::from_utf8(frame).ok()?).ok()?;
    if !matches!(value, js::Value::Object(_)) {
        return None;
    }
    let credential = match value.get("credential") {
        Some(js::Value::String(text)) => text.to_utf8_lossy(),
        _ => return None,
    };
    if value.get("action").is_some() && value.get("command").is_none() {
        return None;
    }
    let wait = matches!(value.get("command"), Some(js::Value::String(text)) if text.to_utf8_lossy() == "wait");
    Some((replay::caller_of(&credential), wait))
}

/// The linger count that fits the descriptor limit, and whether the limit is low.
fn fd_plan(limit: u64, max_connections: usize, linger_max: usize) -> (usize, bool) {
    // A connection holds three descriptors: its stream, the killer clone and the reader clone.
    let need = (3 * max_connections + linger_max + FD_HEADROOM) as u64;
    if limit >= need {
        (linger_max, false)
    } else {
        let room = limit.saturating_sub((3 * max_connections + FD_HEADROOM) as u64);
        (
            usize::try_from(room).unwrap_or(linger_max).min(linger_max),
            true,
        )
    }
}

fn soft_fd_limit() -> Option<u64> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a valid rlimit struct the call fills.
    let rc = unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) };
    #[allow(clippy::unnecessary_cast)] // rlim_t is not u64 on every platform
    (rc == 0).then_some(limit.rlim_cur as u64)
}

fn fd_limit_entry(limit: u64, max_connections: usize, linger_max: usize) -> LogEntry {
    let mut entry = LogEntry::new("server", "fd_limit_low");
    entry.detail = Some(serde_json::json!({
        "limit": limit,
        "maxConnections": max_connections,
        "lingerMax": linger_max,
    }));
    entry
}

/// A new random 128-bit id as hex.
fn new_boot_id() -> Option<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).ok()?;
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn boot_path(socket: &std::path::Path) -> PathBuf {
    let mut name = socket.as_os_str().to_os_string();
    name.push(".boot");
    PathBuf::from(name)
}

/// Writes the boot id atomically with mode 0600, replacing what a crashed daemon left.
fn write_boot_id(path: &std::path::Path, id: &str) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut temp = path.as_os_str().to_os_string();
    temp.push(format!(".{}.tmp", std::process::id()));
    let temp = PathBuf::from(temp);
    let _ = std::fs::remove_file(&temp);
    let written = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(id.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// The running server. `stop_accepting`, `drain` and `cleanup` are the ordered stop of `runDaemon`'s `finally`; `close` does
/// all three.
pub struct DaemonServer {
    shared: Arc<Shared>,
    socket_path: PathBuf,
    accept: Mutex<Option<JoinHandle<()>>>,
    reaper: Mutex<Option<JoinHandle<()>>>,
    /// Writing a byte here wakes the accept thread.
    wake: Mutex<Option<std::os::fd::OwnedFd>>,
    /// The boot file and the id this server wrote to it.
    boot: Option<(PathBuf, String)>,
}

impl DaemonServer {
    /// `startDaemonServer`: removes a stale socket, binds with mode 0600 and starts accepting.
    pub fn start(deps: Deps, options: ServerOptions) -> io::Result<DaemonServer> {
        let (listener, identity) = rebind(&options.socket_path)?;
        listener.set_nonblocking(true)?;
        let (wake_read, wake_write) = make_pipe()?;
        let tuning = capstan_config::daemon_limits(deps.options.capstan.as_deref());
        let to_usize = |n: i64| usize::try_from(n).unwrap_or(0).max(1);
        let max = to_usize(tuning.max_connections);
        let mut linger_max = LINGER_MAX;
        let mut fd_entry = None;
        if let Some(limit) = soft_fd_limit() {
            let (clamped, low) = fd_plan(limit, max, LINGER_MAX);
            if low {
                linger_max = clamped;
                fd_entry = Some(fd_limit_entry(limit, max, clamped));
            }
        }
        let limits = Limits {
            max,
            caller: to_usize(tuning.caller_connections),
            wait: to_usize(tuning.wait_connections),
            wait_per_caller: to_usize(tuning.wait_per_caller),
            retry_ms: u64::try_from(tuning.busy_retry_ms).unwrap_or(500),
            linger_max,
        };
        if let Some(entry) = fd_entry {
            (deps.log)(&entry);
        }
        // The id is written only after the bind, so a second daemon that fails to bind leaves the running one's file.
        let boot = new_boot_id().and_then(|id| {
            let path = boot_path(&options.socket_path);
            match write_boot_id(&path, &id) {
                Ok(()) => Some((path, id)),
                Err(error) => {
                    let mut entry = LogEntry::new("server", "boot_id_unwritten");
                    entry.detail = Some(serde_json::json!({ "error": error.to_string() }));
                    (deps.log)(&entry);
                    None
                }
            }
        });
        let shared = Arc::new(Shared {
            deps,
            limits,
            handler: options.handler.clone(),
            linger: Mutex::new(Vec::new()),
            admission: Mutex::new(Admission::default()),
            busy_log: Mutex::new(BusyLog::default()),
            response_limit: options.max_response_bytes,
            draining: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
            conns: Mutex::new(HashMap::new()),
            conns_changed: Condvar::new(),
            running: Mutex::new(0),
            running_changed: Condvar::new(),
            reaper_stop: AtomicBool::new(false),
            drain_lineless: std::sync::atomic::AtomicUsize::new(0),
            identity: Mutex::new(identity),
        });
        let accept = {
            let shared = Arc::clone(&shared);
            let path = options.socket_path.clone();
            std::thread::Builder::new()
                .name("accept".into())
                .spawn(move || accept_loop(shared, listener, wake_read, path))?
        };
        let reaper = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("reaper".into())
                .spawn(move || reaper_loop(shared))?
        };
        Ok(DaemonServer {
            shared,
            socket_path: options.socket_path,
            accept: Mutex::new(Some(accept)),
            reaper: Mutex::new(Some(reaper)),
            wake: Mutex::new(Some(wake_write)),
            boot,
        })
    }

    /// Step 1 of an ordered stop: no new connections (one that is already in is answered `shutting_down`).
    pub fn stop_accepting(&self) {
        self.shared.draining.store(true, Ordering::SeqCst);
        if let Some(wake) = lock(&self.wake).take() {
            let _ = write_byte(&wake);
        }
        if let Some(accept) = lock(&self.accept).take() {
            let _ = accept.join();
        }
    }

    /// Step 3 of an ordered stop: abort handlers, await them, destroy what is left.
    pub fn drain(&self) {
        let shared = &self.shared;
        lock(&shared.linger).clear();
        let connections: Vec<Arc<Conn>> = lock(&shared.conns).values().cloned().collect();
        for conn in &connections {
            conn.signal.abort("shutdown");
        }
        // A connection that was accepted but not dispatched gets the shutting_down line (unstored, so safe to retry),
        // and can no longer be dispatched: dispatch and this check take the same state lock.
        for conn in &connections {
            conn.answer_shutting_down();
        }
        {
            let mut running = lock(&shared.running);
            while *running > 0 {
                running = shared
                    .running_changed
                    .wait(running)
                    .unwrap_or_else(|p| p.into_inner());
            }
        }
        // A socket that was already answered is ended and must flush its reply (a shutting_down or superseded code)
        // before it closes; only a socket that never got a request is destroyed at once.
        for conn in lock(&shared.conns).values() {
            let state = lock(&conn.state);
            if !state.answered && !state.destroyed {
                drop(state);
                shared.drain_lineless.fetch_add(1, Ordering::SeqCst);
                conn.destroy();
            }
        }
        let deadline = Instant::now() + Duration::from_millis(DRAIN_FLUSH_MS);
        {
            let mut conns = lock(&shared.conns);
            while !conns.is_empty() {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                conns = shared
                    .conns_changed
                    .wait_timeout(conns, left)
                    .unwrap_or_else(|p| p.into_inner())
                    .0;
            }
        }
        for conn in lock(&shared.conns).values() {
            conn.destroy();
        }
        // The connection threads end once their sockets are gone.
        let settle = Instant::now() + Duration::from_secs(2);
        {
            let mut conns = lock(&shared.conns);
            while !conns.is_empty() && Instant::now() < settle {
                conns = shared
                    .conns_changed
                    .wait_timeout(conns, Duration::from_millis(20))
                    .unwrap_or_else(|p| p.into_inner())
                    .0;
            }
        }
        shared.reaper_stop.store(true, Ordering::SeqCst);
        if let Some(reaper) = lock(&self.reaper).take() {
            let _ = reaper.join();
        }
        lock(&shared.linger).clear();
    }

    /// Removes the socket file if it is still ours.
    pub fn cleanup(&self) -> io::Result<()> {
        if let Some((path, id)) = &self.boot {
            if std::fs::read_to_string(path).is_ok_and(|text| text == *id) {
                let _ = std::fs::remove_file(path);
            }
        }
        remove_if_ours(&self.socket_path, *lock(&self.shared.identity))
    }

    pub fn close(&self) -> io::Result<()> {
        self.stop_accepting();
        self.drain();
        self.cleanup()
    }

    /// How many connections are open now.
    pub fn open_connections(&self) -> usize {
        lock(&self.shared.conns).len()
    }

    /// Connections drain destroyed without a line (the architecture wants 0).
    #[doc(hidden)]
    pub fn drain_lineless(&self) -> usize {
        self.shared.drain_lineless.load(Ordering::SeqCst)
    }

    /// Refused connections still waiting for their client to read the busy line.
    #[doc(hidden)]
    pub fn lingering_connections(&self) -> usize {
        lock(&self.shared.linger).len()
    }

    /// Caller buckets held now (never more than the open connections).
    #[doc(hidden)]
    pub fn admission_buckets(&self) -> usize {
        lock(&self.shared.admission).buckets.len()
    }

    /// The boot file of this server and its id, when it could be written.
    pub fn boot_file(&self) -> Option<(&std::path::Path, &str)> {
        self.boot.as_ref().map(|(p, id)| (p.as_path(), id.as_str()))
    }

    pub fn socket_path(&self) -> &std::path::Path {
        &self.socket_path
    }
}

impl Drop for DaemonServer {
    fn drop(&mut self) {
        // A server dropped without an ordered stop still releases its threads and the socket file.
        if lock(&self.accept).is_some() || lock(&self.reaper).is_some() {
            let _ = self.close();
        }
    }
}

// ------------------------------------------------------------------------------------------------ the listener

fn make_pipe() -> io::Result<(std::os::fd::OwnedFd, std::os::fd::OwnedFd)> {
    use std::os::fd::FromRawFd;
    let mut fds = [0 as libc::c_int; 2];
    #[cfg(target_os = "linux")]
    // SAFETY: `fds` is a valid array of two ints; the descriptors it receives are owned below.
    let created = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) };
    #[cfg(not(target_os = "linux"))]
    // SAFETY: `fds` is a valid array of two ints; the descriptors it receives are owned below.
    let created = unsafe { libc::pipe(fds.as_mut_ptr()) };
    if created != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the pipe call just returned these two descriptors and nothing else owns them.
    let ends = unsafe {
        (
            std::os::fd::OwnedFd::from_raw_fd(fds[0]),
            std::os::fd::OwnedFd::from_raw_fd(fds[1]),
        )
    };
    // Without pipe2 the flags are set after creation; a child spawned by another thread in between can inherit the
    // wake pipe, which only keeps it open (std accepts the same window for its own descriptors on these systems).
    #[cfg(not(target_os = "linux"))]
    for end in [&ends.0, &ends.1] {
        set_cloexec_nonblocking(end)?;
    }
    Ok(ends)
}

#[cfg(not(target_os = "linux"))]
fn set_cloexec_nonblocking(fd: &std::os::fd::OwnedFd) -> io::Result<()> {
    let raw = fd.as_raw_fd();
    // SAFETY: fcntl on an open descriptor this process owns; F_GETFL/F_SETFL/F_SETFD take integer arguments.
    unsafe {
        let flags = libc::fcntl(raw, libc::F_GETFL);
        if flags < 0
            || libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0
            || libc::fcntl(raw, libc::F_SETFD, libc::FD_CLOEXEC) < 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn write_byte(fd: &std::os::fd::OwnedFd) -> io::Result<()> {
    let byte = [1u8];
    // SAFETY: writes one byte from a valid buffer to an open descriptor.
    let written = unsafe { libc::write(fd.as_raw_fd(), byte.as_ptr().cast(), 1) };
    if written < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// How often a runtime-fallback socket and its directory are touched, and how long a failed rebind waits to be retried.
const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(3600);

/// Keeps a runtime-fallback socket alive from the accept loop's one second wakeup: an hourly touch against age-based
/// cleaners, and a fresh listener when the file is gone or no longer ours. Never panics; failures are logged.
struct Upkeep {
    path: PathBuf,
    enabled: bool,
    next_touch: Instant,
    next_retry: Instant,
}

impl Upkeep {
    fn new(path: PathBuf) -> Self {
        let now = Instant::now();
        Self {
            enabled: is_runtime_socket(&path),
            path,
            next_touch: now + MAINTENANCE_INTERVAL,
            next_retry: now,
        }
    }

    fn wakeup(&mut self, shared: &Shared, listener: &mut UnixListener) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        let identity = *lock(&shared.identity);
        if !is_ours(&self.path, identity) {
            if now >= self.next_retry {
                match rebind(&self.path) {
                    Ok((fresh, fresh_identity)) => {
                        if fresh.set_nonblocking(true).is_ok() {
                            // Only the listening descriptor changes; accepted connections keep running.
                            *listener = fresh;
                            *lock(&shared.identity) = fresh_identity;
                            self.next_touch = now + MAINTENANCE_INTERVAL;
                            shared.log(LogEntry::new("server", "socket_rebound"));
                        } else {
                            self.failed(shared, now, "cannot make the new listener non-blocking");
                        }
                    }
                    Err(error) => self.failed(shared, now, &error.to_string()),
                }
            }
            return;
        }
        if now >= self.next_touch {
            self.next_touch = now + MAINTENANCE_INTERVAL;
            if let Err(error) = touch_runtime_socket(&self.path) {
                let mut entry = LogEntry::new("server", "socket_touch_failed");
                entry.detail = Some(serde_json::json!({ "error": error.to_string() }));
                shared.log(entry);
            }
        }
    }

    fn failed(&mut self, shared: &Shared, now: Instant, error: &str) {
        self.next_retry = now + MAINTENANCE_INTERVAL;
        let mut entry = LogEntry::new("server", "socket_rebind_failed");
        entry.detail = Some(serde_json::json!({ "error": error }));
        shared.log(entry);
    }
}

fn accept_loop(
    shared: Arc<Shared>,
    mut listener: UnixListener,
    wake: std::os::fd::OwnedFd,
    socket_path: PathBuf,
) {
    let mut upkeep = Upkeep::new(socket_path);
    let mut scratch = vec![0u8; MAX_FRAME_BYTES];
    loop {
        let mut fds = [
            libc::pollfd {
                fd: listener.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: wake.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: `fds` is a valid array of two pollfd structs.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), 2, 1000) };
        if ready < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        if fds[1].revents != 0 || shared.draining.load(Ordering::SeqCst) {
            // Nothing more is accepted; the pending ones are left to the kernel, as `server.close()` leaves them.
            return;
        }
        if fds[0].revents == 0 {
            upkeep.wakeup(&shared, &mut listener);
            continue;
        }
        loop {
            match listener.accept() {
                Ok((stream, _)) => admit(&shared, stream, &mut scratch),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    let mut entry = LogEntry::new("server", "server_error");
                    entry.ms = 0;
                    shared.log(entry);
                    std::thread::sleep(Duration::from_millis(10));
                    break;
                }
            }
        }
    }
}

/// Registers a new connection and starts its thread; one over `max_connections` gets the busy line.
fn admit(shared: &Arc<Shared>, stream: UnixStream, scratch: &mut [u8]) {
    if lock(&shared.conns).len() >= shared.limits.max {
        refuse_at_cap(shared, stream, scratch);
        return;
    }
    let _ = stream.set_nonblocking(false);
    let Ok(killer) = stream.try_clone() else {
        return;
    };
    let conn = Arc::new(Conn {
        id: shared.next_id.fetch_add(1, Ordering::SeqCst),
        killer,
        signal: AbortSignal::new(),
        state: Mutex::new(ConnState::default()),
    });
    {
        let mut conns = lock(&shared.conns);
        if conns.len() >= shared.limits.max {
            drop(conns);
            refuse_at_cap(shared, stream, scratch);
            return;
        }
        conns.insert(conn.id, Arc::clone(&conn));
    }
    let spawned = {
        let (shared, conn) = (Arc::clone(shared), Arc::clone(&conn));
        std::thread::Builder::new()
            .name(format!("conn-{}", conn.id))
            .spawn(move || {
                serve_connection(&shared, &conn, stream);
                conn.destroy();
                lock(&shared.conns).remove(&conn.id);
                shared.conns_changed.notify_all();
            })
    };
    if spawned.is_err() {
        lock(&shared.conns).remove(&conn.id);
        shared.conns_changed.notify_all();
    }
}

/// The accept thread's refusal: one non-blocking read to discard what the client sent, the busy line, the end of the
/// write side, and the stream goes to the lingering list. Nothing here can block, and no thread is started.
fn refuse_at_cap(shared: &Shared, stream: UnixStream, scratch: &mut [u8]) {
    shared.note_busy(SLOTS);
    if lock(&shared.linger).len() >= shared.limits.linger_max {
        shared.note_busy(LINGER_FULL);
        return;
    }
    if stream.set_nonblocking(true).is_err() {
        return;
    }
    let _ = (&stream).read(scratch);
    let _ = (&stream).write(shared.busy_line(SLOTS).as_bytes());
    let _ = stream.shutdown(std::net::Shutdown::Write);
    lock(&shared.linger).push(Lingering {
        stream,
        since: Instant::now(),
    });
}

/// Reads and discards what lingering clients still send; closes a stream on its client's end or after `LINGER_MS`.
fn drain_lingering(shared: &Shared) {
    let mut list = lock(&shared.linger);
    if list.is_empty() {
        return;
    }
    let limit = Duration::from_millis(LINGER_MS);
    let mut sink = [0u8; 4096];
    list.retain_mut(|item| {
        if item.since.elapsed() >= limit {
            return false;
        }
        for _ in 0..16 {
            match item.stream.read(&mut sink) {
                Ok(0) => return false,
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return true,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return false,
            }
        }
        true
    });
}

/// Enforces the deadlines (`arm`): a connection still open when its time is up is destroyed.
fn reaper_loop(shared: Arc<Shared>) {
    while !shared.reaper_stop.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(20));
        drain_lingering(&shared);
        shared.flush_busy();
        let now = Instant::now();
        let due: Vec<Arc<Conn>> = lock(&shared.conns)
            .values()
            .filter(|conn| lock(&conn.state).deadline.is_some_and(|at| at <= now))
            .cloned()
            .collect();
        for conn in due {
            conn.destroy();
        }
    }
}

// ------------------------------------------------------------------------------------------------ a connection

/// `respond`: the line and the end of the write side. A destroyed socket is not written.
fn respond(conn: &Conn, stream: &mut UnixStream, text: &str) {
    {
        let mut state = lock(&conn.state);
        if state.destroyed || state.answered {
            return;
        }
        state.answered = true;
    }
    let mut line = String::with_capacity(text.len() + 1);
    line.push_str(text);
    line.push('\n');
    let _ = stream.write_all(line.as_bytes());
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Write);
}

fn serve_connection(shared: &Arc<Shared>, conn: &Arc<Conn>, mut stream: UnixStream) {
    if shared.draining.load(Ordering::SeqCst) {
        // The same once-only path drain uses, so the line is written exactly once whichever side gets there first.
        conn.answer_shutting_down();
        wait_for_close(&mut stream);
        return;
    }
    conn.arm(Duration::from_millis(REQUEST_TIMEOUT_MS));
    let frame = match read_frame(&mut stream) {
        FrameRead::Frame(frame) => frame,
        FrameRead::Closed => {
            // Closed before a frame: nothing to answer.
            conn.destroy();
            return;
        }
        FrameRead::TooLarge => {
            let text = fail(ErrorCode::InvalidRequest, "request too large").to_text();
            respond(conn, &mut stream, &text);
            wait_for_close(&mut stream);
            return;
        }
    };
    let share = match classify(&frame) {
        Some((key, wait)) => match shared.acquire(key, wait) {
            Ok(share) => Some(share),
            Err(reason) => {
                shared.note_busy(reason);
                let line = shared.busy_line(reason);
                respond(conn, &mut stream, line.trim_end());
                conn.arm(Duration::from_millis(BUSY_CLOSE_MS));
                wait_for_close(&mut stream);
                return;
            }
        },
        None => None,
    };
    {
        // Drain takes this lock to answer connections that are not dispatched; a handler never starts after it.
        let mut state = lock(&conn.state);
        if state.answered || state.destroyed {
            drop(state);
            wait_for_close(&mut stream);
            return;
        }
        state.dispatched = true;
        *lock(&shared.running) += 1;
    }
    let reader = std::cell::RefCell::new(None::<JoinHandle<()>>);
    let outcome = {
        let mut on_limit = |limit_ms: u64, _is_wait: bool| {
            // Any command with a limit is one the client cannot outlive; a half-close ends it.
            {
                let mut state = lock(&conn.state);
                state.limited = true;
                state.deadline = Some(
                    Instant::now()
                        + Duration::from_millis(6_000.max(limit_ms + REQUEST_TIMEOUT_MS)),
                );
            }
            if let Ok(mut clone) = stream.try_clone() {
                let conn = Arc::clone(conn);
                let spawned = std::thread::Builder::new()
                    .name("conn-reader".into())
                    .spawn(move || watch_client(&conn, &mut clone));
                *reader.borrow_mut() = spawned.ok();
            }
        };
        match &shared.handler {
            Some(handler) => (handler.0)(
                &shared.deps,
                &frame,
                &conn.signal,
                shared.response_limit,
                &mut on_limit,
            ),
            None => handle_frame_with(
                &shared.deps,
                &frame,
                &conn.signal,
                shared.response_limit,
                &mut on_limit,
            ),
        }
    };
    if share.is_some() && !outcome.identified {
        shared.note_busy(UNVERIFIED);
    }
    if let Some(text) = &outcome.response {
        respond(conn, &mut stream, text);
    }
    drop(share);
    shared.log(outcome.log);
    {
        let mut running = lock(&shared.running);
        *running -= 1;
        shared.running_changed.notify_all();
    }
    match reader.into_inner() {
        Some(reader) => {
            let _ = reader.join();
        }
        None => wait_for_close(&mut stream),
    }
}

/// Reads until the client's end or an error. Used after the reply: the connection stays until the client closes or the
/// deadline destroys it.
fn wait_for_close(stream: &mut UnixStream) {
    let mut sink = [0u8; 4096];
    loop {
        match stream.read(&mut sink) {
            Ok(0) => return,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

enum FrameRead {
    Frame(Vec<u8>),
    /// The client closed (or the read failed) before a whole frame.
    Closed,
    TooLarge,
}

/// Reads the request frame: the bytes up to the first newline, at most `MAX_FRAME_BYTES`. Bytes after the newline are
/// discarded.
fn read_frame(stream: &mut UnixStream) -> FrameRead {
    let mut frame: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let n = match stream.read(&mut chunk) {
            Ok(0) => return FrameRead::Closed,
            Ok(n) => n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return FrameRead::Closed,
        };
        match chunk[..n].iter().position(|&b| b == b'\n') {
            None => {
                frame.extend_from_slice(&chunk[..n]);
                if frame.len() > MAX_FRAME_BYTES {
                    return FrameRead::TooLarge;
                }
            }
            Some(newline) => {
                if frame.len() + newline > MAX_FRAME_BYTES {
                    return FrameRead::TooLarge;
                }
                frame.extend_from_slice(&chunk[..newline]);
                return FrameRead::Frame(frame);
            }
        }
    }
}

/// Runs while a command with a limit is handled: later bytes are discarded but read, so a closed client is noticed at once
/// and extra bytes cannot extend the deadline; the client's end of its writing is the end of the command.
fn watch_client(conn: &Arc<Conn>, stream: &mut UnixStream) {
    let mut sink = [0u8; 4096];
    loop {
        match stream.read(&mut sink) {
            Ok(0) => {
                if lock(&conn.state).limited {
                    conn.signal.abort("closed");
                    conn.destroy();
                }
                return;
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => {
                conn.destroy();
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn private_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }

    #[test]
    fn the_linger_count_is_clamped_to_the_descriptor_limit() {
        assert_eq!(fd_plan(1024, 64, LINGER_MAX), (576, true));
        assert_eq!(fd_plan(65_536, 64, LINGER_MAX), (LINGER_MAX, false));
        assert_eq!(fd_plan(100, 64, LINGER_MAX), (0, true));
        let entry = fd_limit_entry(1024, 64, 576);
        assert_eq!(entry.code, "fd_limit_low");
        assert_eq!(entry.detail.unwrap()["lingerMax"], 576);
    }

    #[test]
    fn a_stale_socket_is_removed_and_other_things_are_refused() {
        let dir = private_dir();
        let path = dir.path().join("control.sock");
        remove_stale_socket(&path).expect("nothing there is fine");
        let (listener, _) = bind(&path).unwrap();
        drop(listener);
        assert!(path.exists(), "the socket file outlives its listener");
        remove_stale_socket(&path).expect("a stale socket is removed");
        assert!(!path.exists());
        std::fs::write(&path, "x").unwrap();
        let error = remove_stale_socket(&path).unwrap_err();
        assert_eq!(
            error.to_string(),
            "control socket path exists and is not a socket"
        );
        let relative = remove_stale_socket(std::path::Path::new("control.sock")).unwrap_err();
        assert_eq!(relative.to_string(), "control socket path must be absolute");
    }

    #[test]
    fn a_socket_path_over_the_system_limit_is_refused_with_its_length() {
        let dir = private_dir();
        let mut name = String::from("c");
        while dir.path().join(&name).as_os_str().len() <= capstan_wire::socket_path_limit() {
            name.push('x');
        }
        let error = bind(&dir.path().join(&name)).unwrap_err();
        assert!(
            error.to_string().starts_with("control socket path is "),
            "{error}"
        );
        assert!(!dir.path().join(&name).exists());
    }

    #[test]
    fn a_directory_that_is_not_private_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let error = remove_stale_socket(&dir.path().join("control.sock")).unwrap_err();
        assert_eq!(
            error.to_string(),
            "control socket directory must be private and owned by the current user"
        );
    }

    #[test]
    fn the_socket_is_bound_with_mode_0600_and_cleanup_removes_only_ours() {
        let dir = private_dir();
        let path = dir.path().join("control.sock");
        let (listener, identity) = bind(&path).unwrap();
        let mode = std::fs::symlink_metadata(&path)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        drop(listener);
        remove_if_ours(&path, identity).unwrap();
        assert!(!path.exists());
        // A socket that is not ours stays.
        let (other, _) = bind(&path).unwrap();
        remove_if_ours(
            &path,
            SocketIdentity {
                dev: identity.dev,
                ino: identity.ino ^ 0xffff,
            },
        )
        .unwrap();
        assert!(path.exists());
        drop(other);
    }
}
