//! The shadow run (scripts/shadow-daemon.mjs) as tests: the Node daemon and `cstan-daemon` answer the same wire requests
//! over two copies of one ledger and nothing may differ. The small run is part of `cargo test` (and of check:rust-daemon);
//! the large one (50 agents, 5000 messages, 20 plans with reports and integrations) is run by hand:
//! `npm run build && cargo test -p capstan-daemon --test shadow -- --ignored`.
//!
//! Needs node and dist/ (npm run build); without them a test says so and passes.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const DAEMON: &str = env!("CARGO_BIN_EXE_cstan-daemon");

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn suffix() -> String {
    let mut bytes = [0u8; 4];
    getrandom::fill(&mut bytes).unwrap();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether the script can run here; otherwise why not, printed.
fn ready() -> bool {
    let node = Command::new("node").arg("--version").output().is_ok();
    let built = repository().join("dist/src/cli.js").exists();
    if !node || !built {
        eprintln!(
            "SKIPPED LOUDLY: shadow test did not run: node and dist/ (npm run build) are needed"
        );
    }
    node && built
}

fn script(args: &[&str]) -> Output {
    Command::new("node")
        .arg(repository().join("scripts/shadow-daemon.mjs"))
        .args(args)
        .current_dir(repository())
        .env("CSTAN_DAEMON_BIN", DAEMON)
        .env_remove("CAPSTAN_SOCKET")
        .env_remove("CAPSTAN_TOKEN")
        .output()
        .unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// A small generated project under /tmp, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn generate(sizes: &[&str]) -> Scratch {
        let directory = PathBuf::from(format!("/tmp/cph-shadow-{}", suffix()));
        let mut args = vec!["--generate", directory.to_str().unwrap()];
        args.extend_from_slice(sizes);
        let output = script(&args);
        assert!(output.status.success(), "{}", text(&output));
        Scratch(directory)
    }

    fn state(&self) -> PathBuf {
        self.0.join(".capstan/state")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_small_generated_ledger_shows_no_difference() {
    if !ready() {
        return;
    }
    let output = script(&["--self-test"]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("no response and no table differs"),
        "{}",
        text(&output)
    );
}

#[test]
fn a_source_whose_daemon_runs_is_refused_and_a_source_is_never_changed() {
    if !ready() {
        return;
    }
    let scratch = Scratch::generate(&["--agents", "5", "--messages", "10", "--plans", "1"]);
    let ledger = scratch.state().join("controller.sqlite");
    let before = std::fs::read(&ledger).unwrap();
    let wal_before = std::fs::read(scratch.state().join("controller.sqlite-wal")).ok();

    // The pid file names a live process (this test's own).
    std::fs::write(
        scratch.state().join("daemon.pid"),
        format!("{}\n", std::process::id()),
    )
    .unwrap();
    let refused = script(&[scratch.0.to_str().unwrap()]);
    assert_eq!(refused.status.code(), Some(2), "{}", text(&refused));
    assert!(text(&refused).contains("is running"), "{}", text(&refused));
    std::fs::remove_file(scratch.state().join("daemon.pid")).unwrap();

    // Something listens on the control socket.
    let socket = scratch.state().join("control.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let refused = script(&[scratch.0.to_str().unwrap()]);
    assert_eq!(refused.status.code(), Some(2), "{}", text(&refused));
    assert!(text(&refused).contains("listening"), "{}", text(&refused));
    drop(listener);
    std::fs::remove_file(&socket).unwrap();

    // A ledger without its project cannot be opened by a daemon at all.
    let lone = PathBuf::from(format!("/tmp/cph-shadow-lone-{}", suffix()));
    std::fs::create_dir_all(&lone).unwrap();
    std::fs::copy(&ledger, lone.join("controller.sqlite")).unwrap();
    let refused = script(&[lone.join("controller.sqlite").to_str().unwrap()]);
    let _ = std::fs::remove_dir_all(&lone);
    assert_eq!(refused.status.code(), Some(2), "{}", text(&refused));

    // A real run changes nothing in the source: the ledger and its log are byte for byte what they were.
    let shadowed = script(&[scratch.0.to_str().unwrap()]);
    assert!(shadowed.status.success(), "{}", text(&shadowed));
    assert_eq!(std::fs::read(&ledger).unwrap(), before);
    assert_eq!(
        std::fs::read(scratch.state().join("controller.sqlite-wal")).ok(),
        wal_before
    );
}

#[test]
#[ignore = "large: 50 agents, 5000 messages, 20 plans; run by hand: cargo test -p capstan-daemon --test shadow -- --ignored"]
fn a_large_generated_ledger_shows_no_difference() {
    if !ready() {
        return;
    }
    let scratch = Scratch::generate(&["--agents", "50", "--messages", "5000", "--plans", "20"]);
    let connection = rusqlite::Connection::open_with_flags(
        scratch.state().join("controller.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let count = |sql: &str| -> i64 { connection.query_row(sql, [], |row| row.get(0)).unwrap() };
    assert!(count("select count(*) from agents") >= 50);
    assert!(count("select count(*) from messages") >= 5000);
    assert!(count("select count(*) from plans") >= 20);
    assert!(count("select count(*) from agent_reports") >= 20);
    assert!(count("select count(*) from integrations") >= 20);
    drop(connection);
    let output = script(&[scratch.0.to_str().unwrap()]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("no response and no table differs"),
        "{}",
        text(&output)
    );
}

/// The socket replay of the transcript harness compares ids and times by shape because its daemon reads the system clock
/// and randomness. Only the groups that must run over the socket do that: scenarios with a concurrent step or a timeout
/// (wait.json). Every other scenario is replayed in process with a seeded clock and compared exactly.
#[test]
fn only_concurrent_and_timeout_scenarios_compare_by_shape() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/transcripts");
    let mut exact = 0;
    let mut by_shape = Vec::new();
    for entry in std::fs::read_dir(&directory).unwrap() {
        let path = entry.unwrap().path();
        let group: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        for scenario in group["scenarios"].as_array().unwrap() {
            let concurrent = scenario["steps"]
                .as_array()
                .unwrap()
                .iter()
                .any(|step| step["concurrent"] == true);
            if concurrent || scenario["mode"] == "socket" {
                by_shape.push((file.clone(), scenario["name"].as_str().unwrap().to_string()));
            } else {
                exact += 1;
            }
        }
    }
    assert!(exact >= 40, "{exact} scenarios compare exactly");
    assert!(
        by_shape.iter().all(|(file, _)| file == "wait.json"),
        "only the wait group is compared by shape: {by_shape:?}"
    );
    // The harness applies the shape comparison in one place, the judgement of a socket exchange.
    let replay = include_str!("common/replay.rs");
    assert_eq!(
        replay.matches("by_shape(").count(),
        3,
        "defined once, used twice in `judge`"
    );
    assert!(replay.contains("fn judge("));
}
