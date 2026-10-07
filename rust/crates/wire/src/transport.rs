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
            1 => "EPERM",
            2 => "ENOENT",
            4 => "EINTR",
            11 => "EAGAIN",
            12 => "ENOMEM",
            13 => "EACCES",
            20 => "ENOTDIR",
            24 => "EMFILE",
            32 => "EPIPE",
            36 => "ENAMETOOLONG",
            40 => "ELOOP",
            88 => "ENOTSOCK",
            90 => "EMSGSIZE",
            98 => "EADDRINUSE",
            104 => "ECONNRESET",
            110 => "ETIMEDOUT",
            111 => "ECONNREFUSED",
            _ => "",
        },
        None => "",
    };
    name.to_string()
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

/// Sends `frame` (a request line with its newline) and returns the first response line without the newline.
pub fn call(socket: &Path, frame: &[u8], timeout: Timeout) -> Result<Vec<u8>, WireError> {
    let started = Instant::now();
    let left = |limit: Duration| {
        limit
            .checked_sub(started.elapsed())
            .filter(|d| !d.is_zero())
    };
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
        stream
            .set_write_timeout(Some(allowed))
            .map_err(|e| fail(e, written))?;
        match stream.write(&frame[written..]) {
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
        stream
            .set_read_timeout(Some(allowed))
            .map_err(|e| WireError::AfterSend(after_send(&e)))?;
        match stream.read(&mut chunk) {
            Ok(0) => return Err(WireError::AfterSend(AfterSend::Closed)),
            Ok(n) => bytes.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(WireError::AfterSend(after_send(&e))),
        }
    }
}
