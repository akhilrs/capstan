//! `call` against fake servers on a real Unix socket.
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use capstan_wire::{call, AfterSend, Timeout, WireError, MAX_RESPONSE_BYTES};

fn socket_path(name: &str) -> (tempfile_dir::Dir, PathBuf) {
    let dir = tempfile_dir::Dir::new(name);
    let path = dir.path().join("s.sock");
    (dir, path)
}

/// A scratch directory removed on drop (no dependencies in this crate).
mod tempfile_dir {
    use std::path::{Path, PathBuf};

    pub struct Dir(PathBuf);

    impl Dir {
        pub fn new(name: &str) -> Dir {
            let path =
                std::env::temp_dir().join(format!("cstan-wire-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Dir(path)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

fn read_request(stream: &mut impl Read) -> Vec<u8> {
    let mut request = Vec::new();
    let mut byte = [0u8; 1];
    while stream.read(&mut byte).unwrap_or(0) == 1 {
        request.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
    }
    request
}

fn serve(
    path: &Path,
    handler: impl FnOnce(std::os::unix::net::UnixStream) + Send + 'static,
) -> thread::JoinHandle<()> {
    let listener = UnixListener::bind(path).unwrap();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        handler(stream);
    })
}

const FRAME: &[u8] = b"{\"v\":1}\n";

#[test]
fn a_line_comes_back_without_its_newline() {
    let (_dir, path) = socket_path("line");
    let server = serve(&path, |mut stream| {
        assert_eq!(read_request(&mut stream), FRAME);
        stream.write_all(b"{\"ok\":true}\nignored").unwrap();
    });
    let line = call(&path, FRAME, Timeout::Deadline(Duration::from_secs(5))).unwrap();
    assert_eq!(line, b"{\"ok\":true}");
    server.join().unwrap();
}

#[test]
fn a_missing_or_dead_socket_was_not_sent() {
    let (_dir, path) = socket_path("missing");
    let error = call(&path, FRAME, Timeout::Idle(Duration::from_secs(1))).unwrap_err();
    assert!(
        matches!(&error, WireError::NotSent { kind: std::io::ErrorKind::NotFound, code } if code == "ENOENT"),
        "{error:?}"
    );
    // A socket file nobody listens on refuses the connection.
    drop(UnixListener::bind(&path).unwrap());
    let error = call(&path, FRAME, Timeout::Idle(Duration::from_secs(1))).unwrap_err();
    assert!(
        matches!(&error, WireError::NotSent { kind: std::io::ErrorKind::ConnectionRefused, code } if code == "ECONNREFUSED"),
        "{error:?}"
    );
}

#[test]
fn anything_after_the_connect_is_after_send() {
    let (_dir, path) = socket_path("closed");
    let server = serve(&path, |mut stream| {
        read_request(&mut stream);
        stream.write_all(b"half a line").unwrap();
    });
    let error = call(&path, FRAME, Timeout::Idle(Duration::from_secs(2))).unwrap_err();
    assert_eq!(error, WireError::AfterSend(AfterSend::Closed));
    server.join().unwrap();
}

#[test]
fn idle_rearms_on_every_read_and_deadline_does_not() {
    // The server drips a byte every 150 ms for about a second, then ends the line.
    let drip = |stream: &mut std::os::unix::net::UnixStream| {
        read_request(stream);
        for _ in 0..7 {
            if stream.write_all(b" ").is_err() {
                return;
            }
            thread::sleep(Duration::from_millis(150));
        }
        let _ = stream.write_all(b"\n");
    };

    let (_dir, path) = socket_path("idle");
    let server = serve(&path, move |mut stream| drip(&mut stream));
    let started = Instant::now();
    let line = call(&path, FRAME, Timeout::Idle(Duration::from_millis(500))).unwrap();
    assert_eq!(line, b"       ");
    assert!(
        started.elapsed() > Duration::from_millis(900),
        "ran past one idle period"
    );
    server.join().unwrap();

    let (_dir, path) = socket_path("deadline");
    let server = serve(&path, move |mut stream| drip(&mut stream));
    let started = Instant::now();
    let error = call(&path, FRAME, Timeout::Deadline(Duration::from_millis(500))).unwrap_err();
    assert_eq!(error, WireError::AfterSend(AfterSend::TimedOut));
    assert!(
        started.elapsed() < Duration::from_millis(900),
        "gave up at the deadline"
    );
    server.join().unwrap();
}

#[test]
fn silence_times_out_in_both_modes() {
    for timeout in [
        Timeout::Idle(Duration::from_millis(200)),
        Timeout::Deadline(Duration::from_millis(200)),
    ] {
        let (_dir, path) = socket_path("silent");
        let server = serve(&path, |mut stream| {
            read_request(&mut stream);
            thread::sleep(Duration::from_millis(600));
        });
        let error = call(&path, FRAME, timeout).unwrap_err();
        assert_eq!(
            error,
            WireError::AfterSend(AfterSend::TimedOut),
            "{timeout:?}"
        );
        server.join().unwrap();
    }
}

fn big_line(length: usize, newline: bool) -> Vec<u8> {
    let mut line = vec![b'x'; length];
    if newline {
        line.push(b'\n');
    }
    line
}

#[test]
fn the_response_limit_is_one_mebibyte() {
    for (length, newline, expected) in [
        (MAX_RESPONSE_BYTES, true, Ok(MAX_RESPONSE_BYTES)),
        (MAX_RESPONSE_BYTES + 1, true, Err(AfterSend::TooLarge)),
        (MAX_RESPONSE_BYTES + 1, false, Err(AfterSend::TooLarge)),
        (3 * MAX_RESPONSE_BYTES, false, Err(AfterSend::TooLarge)),
    ] {
        let (_dir, path) = socket_path("big");
        let data = big_line(length, newline);
        let server = serve(&path, move |mut stream| {
            read_request(&mut stream);
            // The client may hang up as soon as it has seen enough.
            let _ = stream.write_all(&data);
        });
        let got = call(&path, FRAME, Timeout::Deadline(Duration::from_secs(10)));
        match expected {
            Ok(n) => assert_eq!(got.unwrap().len(), n),
            Err(kind) => assert_eq!(
                got.unwrap_err(),
                WireError::AfterSend(kind),
                "{length} {newline}"
            ),
        }
        server.join().unwrap();
    }
}
