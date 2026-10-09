//! The Rust adapter against a real Herdr server in a throwaway session (`capstan-test-*`), run by hand and in the
//! shadow run: `cargo test -p capstan-herdr --test live -- --ignored`. It never touches the operator's own session: the
//! server is started with the scratch home below, every command names the session, and HERDR_ variables are removed.
//!
//! Tested with Herdr 0.9.3 (the Node live test accepts any 0.9.x from 0.9.1 on; so does this one).

use capstan_herdr::adapter::{Adapter, AdapterOptions};
use capstan_herdr::api::*;
use capstan_herdr::runner::{ProcessRunner, RunnerOptions};
use std::collections::HashMap;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Live {
    session: String,
    environment: HashMap<String, String>,
    server: Option<Child>,
    root: PathBuf,
}

impl Drop for Live {
    fn drop(&mut self) {
        let herdr = |args: &[&str]| {
            let _ = Command::new("herdr")
                .args(args)
                .env_clear()
                .envs(&self.environment)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        };
        herdr(&["--session", &self.session, "server", "stop"]);
        if let Some(mut server) = self.server.take() {
            std::thread::sleep(Duration::from_millis(500));
            // The server is a process this test started.
            let _ = server.kill();
            let _ = server.wait();
        }
        herdr(&["session", "delete", &self.session]);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn random_suffix() -> String {
    let mut bytes = [0u8; 4];
    getrandom::fill(&mut bytes).unwrap();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn start() -> (Live, Arc<ProcessRunner>) {
    let root = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("cph-rs-{}", random_suffix()));
    assert!(
        root.to_string_lossy().len() <= 70,
        "the temporary path is short enough for a socket"
    );
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    for file in [".bashrc", ".bash_profile"] {
        std::fs::write(home.join(file), "PS1='❯ '\n").unwrap();
    }
    let mut environment: HashMap<String, String> = std::env::vars()
        .filter(|(name, _)| !name.starts_with("HERDR_") && !name.starts_with("XDG_"))
        .collect();
    environment.insert("HOME".into(), home.to_string_lossy().into_owned());
    environment.insert("TERM".into(), "xterm-256color".into());
    // The scratch home's rc files set the prompt the adapter waits for, so panes must run bash.
    environment.insert("SHELL".into(), "/bin/bash".into());
    let session = format!("capstan-test-{}", random_suffix());
    let server = Command::new("herdr")
        .args(["--session", &session, "server"])
        .env_clear()
        .envs(&environment)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("herdr starts");
    let mut live = Live {
        session: session.clone(),
        environment: environment.clone(),
        server: Some(server),
        root,
    };
    let runner = Arc::new(
        ProcessRunner::new(RunnerOptions {
            session,
            binary: None,
            timeout_ms: Some(30_000),
            env: Some(environment),
        })
        .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let ready = runner
            .run(
                &["workspace".to_string(), "list".to_string()],
                &RunOptions::default(),
            )
            .is_ok_and(|o| o.code == 0);
        if ready {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the isolated Herdr server did not start"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    live.environment.shrink_to_fit();
    (live, runner)
}

fn until<T>(what: &str, mut action: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(value) = action() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
#[ignore = "drives a real Herdr server; run by hand: cargo test -p capstan-herdr --test live -- --ignored"]
fn the_adapter_drives_a_real_session_and_removes_it() {
    if Command::new("herdr")
        .arg("--version")
        .stdout(Stdio::null())
        .status()
        .is_err()
    {
        panic!("herdr is not available");
    }
    let (live, runner) = start();
    assert!(live.session.starts_with("capstan-test-"));
    let mut options = AdapterOptions::new(runner.clone());
    options.temp_root = Some(live.root.clone());
    let adapter = Adapter::new(options);

    // Tested on 0.9.3; any 0.9.x from 0.9.1 keeps the CLI surface the adapter uses.
    let version = adapter.version().unwrap();
    let minor: u32 = version
        .strip_prefix("herdr 0.9.")
        .and_then(|rest| rest.parse().ok())
        .unwrap_or_else(|| panic!("unsupported Herdr version: {version}"));
    assert!(minor >= 1, "the adapter needs 0.9.x with x >= 1");

    // A fresh isolated session has notifications disabled: Herdr answers exit 0 with shown:false.
    let error = adapter
        .notify("Capstan: PM message waiting", "Message m-1 is waiting")
        .unwrap_err();
    assert_eq!(
        error.herdr_code(),
        Some("notification_not_shown"),
        "{error}"
    );

    // Create a pane.
    let cwd = live.root.to_string_lossy().into_owned();
    let created = adapter
        .create_workspace(CreateWorkspaceInput {
            cwd: &cwd,
            label: "live",
            role: PaneRole::Worker,
        })
        .unwrap();
    assert!(adapter.pane_entry(&created.pane_id).is_some());
    let listed = adapter.panes_at_path(&cwd).unwrap();
    assert!(listed.iter().any(|p| p.pane_id == created.pane_id));

    // Type into it once its shell shows the prompt, then read what the shell printed.
    until("the shell prompt", || {
        let screen = adapter.read_screen(&created.pane_id, false, None).ok()?;
        screen.trim_end().ends_with('❯').then_some(())
    });
    adapter
        .run_in_pane(&created.pane_id, "echo capstan-live-$((20+22))")
        .unwrap();
    let screen = until("the command output", || {
        let screen = adapter.read_screen(&created.pane_id, false, None).ok()?;
        screen
            .lines()
            .any(|l| l == "capstan-live-42")
            .then_some(screen)
    });
    assert!(screen.contains("capstan-live-42"));
    assert_eq!(
        adapter.pane_entry(&created.pane_id).unwrap().phase,
        PanePhase::Started
    );
    let state = adapter.pane_state(&created.pane_id).unwrap();
    assert!(!state.status.is_empty());
    let identity = adapter.pane_identity(&created.pane_id).unwrap();
    assert!(identity.is_some(), "Herdr knows the pane");

    // Close it.
    adapter.close_pane(&created.pane_id).unwrap();
    assert!(adapter.pane_entry(&created.pane_id).is_none());
    adapter.close();

    // The session goes away with this test: nothing of it is left in Herdr.
    let session = live.session.clone();
    drop(live);
    let listing = Command::new("herdr")
        .args(["session", "list"])
        .output()
        .expect("herdr runs");
    let listing = String::from_utf8_lossy(&listing.stdout);
    assert!(
        !listing.contains(&session),
        "the throwaway session is deleted: {listing}"
    );
}
