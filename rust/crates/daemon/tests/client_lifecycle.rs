//! `capstan-client` against the real daemon: `ensure_daemon` starts it detached with its output in `daemon.log`, a second
//! call finds it, and `stop_daemon` ends it. The program started is a scratch script that runs `cstan-daemon` whatever it
//! is asked (the client starts `<cstan> daemon`).

mod common;

use capstan_client::{ensure_daemon, ping_daemon, stop_daemon, EnsureOptions, PingOutcome};
use common::private_tempdir;
use common::project::{write_project, Identity};
use std::os::unix::fs::PermissionsExt;

#[test]
fn the_client_starts_finds_and_stops_the_real_daemon() {
    let dir = private_tempdir();
    let identity = Identity {
        project_id: "pclientlife".into(),
        name: "Client Lifecycle".into(),
        credential: format!("operator-{}", "y".repeat(40)),
    };
    let state = write_project(dir.path(), &identity);
    let script = dir.path().join("cstan-as-daemon");
    std::fs::write(
        &script,
        format!("#!/bin/sh\nexec '{}'\n", common::process::binary()),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = state.join("control.sock");
    let options = EnsureOptions {
        socket_path: socket.clone(),
        credential: identity.credential.clone(),
        project_root: dir.path().to_path_buf(),
        log_path: dir.path().join(".capstan").join("daemon.log"),
        cli_path: script,
        env: vec![
            ("PATH".into(), std::env::var_os("PATH").unwrap_or_default()),
            ("CAPSTAN_LAUNCH".into(), "off".into()),
        ],
        timeout_ms: Some(60_000),
    };

    // A stale socket file and a stale pid file from a daemon that died do not matter.
    std::fs::write(state.join("controller.pid"), "999999\n").unwrap();
    let first = ensure_daemon(&options).expect("the daemon starts");
    assert!(first.started && first.pid > 0);
    let found = ensure_daemon(&options).unwrap();
    assert!(!found.started);
    assert_eq!(found.pid, first.pid);
    let log = std::fs::read_to_string(&options.log_path).unwrap();
    assert!(log.contains("\"event\":\"ready\""), "{log}");

    assert!(stop_daemon(&socket, &identity.credential, Some(30_000)).unwrap());
    assert_eq!(
        ping_daemon(&socket, &identity.credential, 1000),
        PingOutcome::Down
    );
}
