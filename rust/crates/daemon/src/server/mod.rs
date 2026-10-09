//! The control socket server (src/daemon.ts `startDaemonServer`): listener, connections, frame limits, timeouts, half-close
//! and drain. `handlers::handle_frame_with` is what a connection runs per frame (its hook arms a limited command's timer).
//!
//! One thread accepts, one thread serves each connection (at most 64 at a time; the 65th is closed at once, as Node's
//! `maxConnections` does), and one thread enforces the connection deadlines. A connection thread blocks in its handler;
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

pub use socket::{bind, remove_if_ours, remove_stale_socket, SocketIdentity};

use crate::deps::{Deps, LogEntry};
use crate::handlers::shared::{fail, AbortSignal, ErrorCode};
use crate::handlers::{
    handle_frame_with, DRAIN_FLUSH_MS, MAX_CONNECTIONS, MAX_FRAME_BYTES, MAX_RESPONSE_BYTES,
    REQUEST_TIMEOUT_MS,
};
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

/// What the server is started with.
#[derive(Clone, Debug)]
pub struct ServerOptions {
    pub socket_path: PathBuf,
    /// The largest response line; `MAX_RESPONSE_BYTES` by default.
    pub max_response_bytes: usize,
}

impl ServerOptions {
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            max_response_bytes: MAX_RESPONSE_BYTES,
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

struct Shared {
    deps: Deps,
    response_limit: usize,
    draining: AtomicBool,
    next_id: AtomicU64,
    conns: Mutex<HashMap<u64, Arc<Conn>>>,
    conns_changed: Condvar,
    /// Connections whose request is being answered (Node's `running`).
    running: Mutex<usize>,
    running_changed: Condvar,
    reaper_stop: AtomicBool,
}

impl Shared {
    fn log(&self, entry: LogEntry) {
        (self.deps.log)(&entry);
    }
}

/// The running server. `stop_accepting`, `drain` and `cleanup` are the ordered stop of `runDaemon`'s `finally`; `close` does
/// all three.
pub struct DaemonServer {
    shared: Arc<Shared>,
    socket_path: PathBuf,
    identity: SocketIdentity,
    accept: Mutex<Option<JoinHandle<()>>>,
    reaper: Mutex<Option<JoinHandle<()>>>,
    /// Writing a byte here wakes the accept thread.
    wake: Mutex<Option<std::os::fd::OwnedFd>>,
}

impl DaemonServer {
    /// `startDaemonServer`: removes a stale socket, binds with mode 0600 and starts accepting.
    pub fn start(deps: Deps, options: ServerOptions) -> io::Result<DaemonServer> {
        remove_stale_socket(&options.socket_path)?;
        let (listener, identity) = bind(&options.socket_path)?;
        listener.set_nonblocking(true)?;
        let (wake_read, wake_write) = make_pipe()?;
        let shared = Arc::new(Shared {
            deps,
            response_limit: options.max_response_bytes,
            draining: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
            conns: Mutex::new(HashMap::new()),
            conns_changed: Condvar::new(),
            running: Mutex::new(0),
            running_changed: Condvar::new(),
            reaper_stop: AtomicBool::new(false),
        });
        let accept = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("accept".into())
                .spawn(move || accept_loop(shared, listener, wake_read))?
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
            identity,
            accept: Mutex::new(Some(accept)),
            reaper: Mutex::new(Some(reaper)),
            wake: Mutex::new(Some(wake_write)),
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
        let connections: Vec<Arc<Conn>> = lock(&shared.conns).values().cloned().collect();
        for conn in &connections {
            conn.signal.abort("shutdown");
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
            if !lock(&conn.state).answered {
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
    }

    /// Removes the socket file if it is still ours.
    pub fn cleanup(&self) -> io::Result<()> {
        remove_if_ours(&self.socket_path, self.identity)
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
    // SAFETY: `fds` is a valid array of two ints; the descriptors it receives are owned below.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: pipe2 just returned these two descriptors and nothing else owns them.
    Ok(unsafe {
        (
            std::os::fd::OwnedFd::from_raw_fd(fds[0]),
            std::os::fd::OwnedFd::from_raw_fd(fds[1]),
        )
    })
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

fn accept_loop(shared: Arc<Shared>, listener: UnixListener, wake: std::os::fd::OwnedFd) {
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
            continue;
        }
        loop {
            match listener.accept() {
                Ok((stream, _)) => admit(&shared, stream),
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

/// Registers a new connection and starts its thread; the 65th open connection is closed at once.
fn admit(shared: &Arc<Shared>, stream: UnixStream) {
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
        if conns.len() >= MAX_CONNECTIONS {
            drop(conns);
            drop(stream);
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

/// Enforces the deadlines (`arm`): a connection still open when its time is up is destroyed.
fn reaper_loop(shared: Arc<Shared>) {
    while !shared.reaper_stop.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(20));
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
        if state.destroyed {
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
        let text = fail(ErrorCode::ShuttingDown, "the daemon is shutting down").to_text();
        respond(conn, &mut stream, &text);
        wait_for_close(&mut stream);
        return;
    }
    conn.arm(Duration::from_millis(REQUEST_TIMEOUT_MS));
    let Some(frame) = read_frame(&mut stream) else {
        // Closed before a frame, or a frame beyond the limit: dropped without an answer.
        conn.destroy();
        return;
    };
    lock(&conn.state).dispatched = true;
    *lock(&shared.running) += 1;
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
        handle_frame_with(
            &shared.deps,
            &frame,
            &conn.signal,
            shared.response_limit,
            &mut on_limit,
        )
    };
    if let Some(text) = &outcome.response {
        respond(conn, &mut stream, text);
    }
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

/// Reads the request frame: the bytes up to the first newline, at most `MAX_FRAME_BYTES`. None when the client closed
/// first or the frame is too long. Bytes after the newline are discarded.
fn read_frame(stream: &mut UnixStream) -> Option<Vec<u8>> {
    let mut frame: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let n = match stream.read(&mut chunk) {
            Ok(0) => return None,
            Ok(n) => n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        };
        match chunk[..n].iter().position(|&b| b == b'\n') {
            None => {
                frame.extend_from_slice(&chunk[..n]);
                if frame.len() > MAX_FRAME_BYTES {
                    return None;
                }
            }
            Some(newline) => {
                if frame.len() + newline > MAX_FRAME_BYTES {
                    return None;
                }
                frame.extend_from_slice(&chunk[..newline]);
                return Some(frame);
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
