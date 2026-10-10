//! A scratch daemon for the tests of `capstan-client`; it is started as `<this> daemon` by `ensure_daemon` and does what
//! `CLIENT_TEST_MODE` says: `serve` (the default) listens on `CLIENT_TEST_SOCKET` and answers `ping` and `shutdown` for the
//! credential `CLIENT_TEST_CREDENTIAL`; `exit:<code>` writes a line and exits; `idle` waits and never listens.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args != ["daemon"] {
        eprintln!("client-test-daemon: unexpected arguments {args:?}");
        std::process::exit(64);
    }
    let var = |name: &str| std::env::var(name).unwrap_or_default();
    println!(
        "started launch={} token={} socket_env={}",
        std::env::var("CAPSTAN_LAUNCH").unwrap_or_else(|_| "unset".into()),
        std::env::var("CAPSTAN_TOKEN").unwrap_or_else(|_| "none".into()),
        std::env::var("CAPSTAN_SOCKET").unwrap_or_else(|_| "none".into()),
    );
    let mode = var("CLIENT_TEST_MODE");
    if let Some(code) = mode.strip_prefix("exit:") {
        eprintln!("cstan: the project lock is held");
        std::process::exit(code.parse().unwrap_or(1));
    }
    if mode == "idle" {
        std::thread::sleep(std::time::Duration::from_secs(60));
        return;
    }
    let socket = var("CLIENT_TEST_SOCKET");
    let credential = var("CLIENT_TEST_CREDENTIAL");
    // A socket file left by an earlier daemon is replaced.
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).expect("the socket binds");
    for stream in listener.incoming().flatten() {
        let mut line = String::new();
        let mut reader = BufReader::new(&stream);
        if reader.read_line(&mut line).is_err() {
            continue;
        }
        let mut out = &stream;
        if !line.contains(&format!("\"credential\":\"{credential}\"")) {
            let _ = writeln!(
                out,
                r#"{{"ok":false,"code":"unauthorized","message":"no"}}"#
            );
            continue;
        }
        if line.contains("\"command\":\"shutdown\"") {
            let _ = writeln!(out, r#"{{"ok":true,"result":{{}}}}"#);
            let _ = std::fs::remove_file(&socket);
            std::process::exit(0);
        }
        let _ = writeln!(
            out,
            r#"{{"ok":true,"result":{{"pid":{}}}}}"#,
            std::process::id()
        );
    }
}
