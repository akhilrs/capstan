//! One request on a fresh Unix-socket connection: write a frame, read one line.
use std::io::{self, ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::MAX_RESPONSE_BYTES;

/// How long a call may take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timeout {
    /// Fails after this long without a byte moving; every read re-arms it (Node's `socket.setTimeout`).
    Idle(Duration),
    /// Fails this long after the call began, however much arrives.
    Deadline(Duration),
}

/// What went wrong after the request was (at least partly) sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AfterSend {
    TimedOut,
    /// The peer closed before a full line arrived.
    Closed,
    /// The response line is longer than `MAX_RESPONSE_BYTES`.
    TooLarge,
    /// The line is not a response (see `response`); `call` itself never returns this.
    Malformed,
    /// Any other socket error, as Node words it (`ECONNRESET`, `EPIPE`, ...).
    Io(String),
}

/// Why a call has no response line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WireError {
    /// Nothing reached the daemon: the connect or the first write failed. `code` is the errno name
    /// (`ENOENT`, `ECONNREFUSED`, `EACCES`, ...), empty if unknown.
    NotSent {
        kind: ErrorKind,
        code: String,
    },
    AfterSend(AfterSend),
}

fn errno_name(error: &io::Error) -> String {
    let name = match error.raw_os_error() {
        Some(libc_errno) => match libc_errno {
            libc::EPERM => "EPERM",
            libc::ENOENT => "ENOENT",
            libc::EINTR => "EINTR",
            libc::EAGAIN => "EAGAIN",
            libc::ENOMEM => "ENOMEM",
            libc::EACCES => "EACCES",
            libc::ENOTDIR => "ENOTDIR",
            libc::EMFILE => "EMFILE",
            libc::EPIPE => "EPIPE",
            libc::ENAMETOOLONG => "ENAMETOOLONG",
            libc::ELOOP => "ELOOP",
            libc::ENOTSOCK => "ENOTSOCK",
            libc::EMSGSIZE => "EMSGSIZE",
            libc::EADDRINUSE => "EADDRINUSE",
            libc::ECONNRESET => "ECONNRESET",
            libc::ETIMEDOUT => "ETIMEDOUT",
            libc::ECONNREFUSED => "ECONNREFUSED",
            _ => "",
        },
        None => "",
    };
    name.to_string()
}

/// Whether the errno name of a `NotSent` means nothing listens at the socket path: no file there, a socket nobody
/// accepts on, or a file that is not a socket (Linux refuses that connection, macOS says ENOTSOCK).
pub fn nothing_listens(code: &str) -> bool {
    matches!(code, "ENOENT" | "ECONNREFUSED" | "ENOTSOCK")
}

fn timed_out(error: &io::Error) -> bool {
    matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

fn after_send(error: &io::Error) -> AfterSend {
    if timed_out(error) {
        return AfterSend::TimedOut;
    }
    let name = errno_name(error);
    AfterSend::Io(if name.is_empty() {
        error.to_string()
    } else {
        name
    })
}

/// The longest socket path this system can bind or connect to, in bytes (`sun_path` less its terminating NUL).
pub fn socket_path_limit() -> usize {
    // SAFETY: sockaddr_un is plain data; an all-zero value is valid and only its field size is read.
    let address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    address.sun_path.len() - 1
}

/// Refuses a socket path the system cannot bind or connect to, with a message that names the path and the limit.
pub fn check_socket_path(socket: &Path) -> Result<(), String> {
    use std::os::unix::ffi::OsStrExt;
    let length = socket.as_os_str().as_bytes().len();
    let limit = socket_path_limit();
    if length <= limit {
        return Ok(());
    }
    Err(format!(
        "control socket path is {length} bytes, longer than the {limit}-byte limit of this system: {}; move the project to a shorter path",
        socket.display()
    ))
}

#[derive(Clone, Copy)]
enum Direction {
    Read,
    Write,
}

/// Runs one read or write on `stream`, failing with a timeout after `allowed`. macOS refuses a socket timeout with
/// EINVAL once the peer has closed; the bound is then kept with poll and a non-blocking attempt instead, so the
/// operation reports the peer's real state (buffered bytes, EOF, EPIPE) without ever waiting longer than `allowed`.
fn bounded<T>(
    stream: &mut UnixStream,
    direction: Direction,
    allowed: Duration,
    mut operation: impl FnMut(&mut UnixStream) -> io::Result<T>,
) -> io::Result<T> {
    let armed = match direction {
        Direction::Read => stream.set_read_timeout(Some(allowed)),
        Direction::Write => stream.set_write_timeout(Some(allowed)),
    };
    match armed {
        Ok(()) => return operation(stream),
        Err(e) if e.raw_os_error() == Some(libc::EINVAL) => {}
        Err(e) => return Err(e),
    }
    let deadline = Instant::now() + allowed;
    stream.set_nonblocking(true)?;
    let result = poll_then(stream, direction, deadline, &mut operation);
    let restored = stream.set_nonblocking(false);
    let value = result?;
    restored?;
    Ok(value)
}

fn poll_then<T>(
    stream: &mut UnixStream,
    direction: Direction,
    deadline: Instant,
    operation: &mut impl FnMut(&mut UnixStream) -> io::Result<T>,
) -> io::Result<T> {
    use std::os::fd::AsRawFd;
    let events = match direction {
        Direction::Read => libc::POLLIN,
        Direction::Write => libc::POLLOUT,
    };
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::from(ErrorKind::TimedOut));
        }
        let wait_ms = left
            .as_micros()
            .div_ceil(1000)
            .min(libc::c_int::MAX as u128) as libc::c_int;
        let mut descriptor = libc::pollfd {
            fd: stream.as_raw_fd(),
            events,
            revents: 0,
        };
        // SAFETY: one valid pollfd for an open descriptor this function borrows.
        let ready = unsafe { libc::poll(&mut descriptor, 1, wait_ms) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if ready == 0 {
            continue;
        }
        match operation(stream) {
            Err(e) if e.kind() == ErrorKind::WouldBlock => continue,
            other => return other,
        }
    }
}

/// Sends `frame` (a request line with its newline) and returns the first response line without the newline.
pub fn call(socket: &Path, frame: &[u8], timeout: Timeout) -> Result<Vec<u8>, WireError> {
    let started = Instant::now();
    let left = |limit: Duration| {
        limit
            .checked_sub(started.elapsed())
            .filter(|d| !d.is_zero())
    };
    if check_socket_path(socket).is_err() {
        return Err(WireError::NotSent {
            kind: ErrorKind::InvalidInput,
            code: "ENAMETOOLONG".to_string(),
        });
    }
    let mut stream = UnixStream::connect(socket).map_err(|e| WireError::NotSent {
        kind: e.kind(),
        code: errno_name(&e),
    })?;
    let not_sent = |e: io::Error| WireError::NotSent {
        kind: e.kind(),
        code: errno_name(&e),
    };

    let mut written = 0;
    while written < frame.len() {
        let allowed = match timeout {
            Timeout::Idle(d) => Some(d),
            Timeout::Deadline(d) => left(d),
        };
        let fail = |e: io::Error, written: usize| {
            if written == 0 && !timed_out(&e) {
                not_sent(e)
            } else {
                WireError::AfterSend(after_send(&e))
            }
        };
        let Some(allowed) = allowed else {
            return Err(if written == 0 {
                not_sent(io::Error::from(ErrorKind::TimedOut))
            } else {
                WireError::AfterSend(AfterSend::TimedOut)
            });
        };
        match bounded(&mut stream, Direction::Write, allowed, |s| {
            s.write(&frame[written..])
        }) {
            Ok(0) => return Err(WireError::AfterSend(AfterSend::Closed)),
            Ok(n) => written += n,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(fail(e, written)),
        }
    }

    let mut bytes: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    let mut searched = 0;
    loop {
        if let Some(offset) = bytes[searched..].iter().position(|b| *b == b'\n') {
            let newline = searched + offset;
            if newline > MAX_RESPONSE_BYTES {
                return Err(WireError::AfterSend(AfterSend::TooLarge));
            }
            bytes.truncate(newline);
            return Ok(bytes);
        }
        searched = bytes.len();
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(WireError::AfterSend(AfterSend::TooLarge));
        }
        let allowed = match timeout {
            Timeout::Idle(d) => d,
            Timeout::Deadline(d) => left(d).ok_or(WireError::AfterSend(AfterSend::TimedOut))?,
        };
        match bounded(&mut stream, Direction::Read, allowed, |s| {
            s.read(&mut chunk)
        }) {
            Ok(0) => return Err(WireError::AfterSend(AfterSend::Closed)),
            Ok(n) => bytes.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(WireError::AfterSend(after_send(&e))),
        }
    }
}
