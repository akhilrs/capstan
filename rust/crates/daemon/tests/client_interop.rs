//! The Rust `cstan` front end against a scratch `cstan-daemon`: `ping`, `status`, `send`, `inbox`, `ack` and `wait` run as
//! the agents and the operator do, over the daemon's socket. (The Node CLI half of this test is gone with the Node CLI: the
//! Node client's answers are the frozen transcripts of `tests/transcripts`, replayed by `cstan` itself.)
//! The front end is built with cargo when it is not beside the daemon binary.

mod common;

use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::{Core, SystemEnv};
use common::process::{binary, Daemon};
use common::project::{write_project, Identity};
use common::{private_tempdir, repo_root};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

/// The Rust front end beside the daemon binary; built with cargo when it is not there yet.
fn front_end() -> PathBuf {
    let beside = Path::new(binary()).with_file_name("cstan");
    if beside.exists() {
        return beside;
    }
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .args(["build", "-j", "2", "--locked", "-p", "cstan-front"])
        .current_dir(repo_root().join("rust"))
        .status()
        .expect("cargo runs");
    assert!(status.success(), "the front end did not build");
    assert!(beside.exists(), "no cstan at {}", beside.display());
    beside
}

struct Project {
    dir: tempfile::TempDir,
    socket: PathBuf,
    agents: HashMap<String, String>,
}

fn step(core: &Core, owner: &str, op: &str, args: Value) -> Value {
    let version = core.state_version().unwrap();
    let context = json!({
        "credential": owner,
        "requestId": format!("req-{op}-{version}"),
        "idempotencyKey": format!("idem-{op}-{version}"),
        "expectedVersion": version,
        "inputRevision": core.input_revision().unwrap(),
    });
    let mut all = vec![context];
    all.extend(args.as_array().cloned().unwrap());
    core.dispatch(op, &all)
        .unwrap_or_else(|e| panic!("setup {op}: {e}"))
}

/// A project with a PM (`pm-1`) and two developers; the ledger is set up before any daemon runs.
fn project() -> Project {
    let dir = private_tempdir();
    let identity = Identity {
        project_id: "pinterop".into(),
        name: "Interop".into(),
        credential: format!("operator-{}", "i".repeat(40)),
    };
    let state = write_project(dir.path(), &identity);
    let core = Core::open(
        &state,
        &identity.initial_project(),
        &KernelOptions {
            workspace_root: Some(dir.path().to_path_buf()),
            ..KernelOptions::default()
        },
        Box::new(SystemEnv),
    )
    .expect("the setup controller opens");
    let owner = identity.credential.clone();
    step(
        &core,
        &owner,
        "syncRoleDefinitions",
        json!([[
            {"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)},
            {"name": "pm", "kind": "PM", "host": "claude", "configHash": "b".repeat(64)},
        ]]),
    );
    let mut agents = HashMap::new();
    for (agent, role_name, kind) in [
        ("pm-1", "pm", "PM"),
        ("dev-1", "developer", "Developer"),
        ("dev-2", "developer", "Developer"),
    ] {
        let seat = format!("seat-{agent}");
        step(
            &core,
            &owner,
            "createSeat",
            json!([{"seatId": seat, "name": seat, "role": kind}]),
        );
        let actor = step(
            &core,
            &owner,
            "createActor",
            json!([{"displayName": agent, "role": kind, "seatId": seat}]),
        );
        step(
            &core,
            &owner,
            "registerAgent",
            json!([{"agentId": agent, "roleName": role_name, "seatId": seat, "actorId": actor["actorId"]}]),
        );
        agents.insert(
            agent.to_string(),
            actor["credential"].as_str().unwrap().to_string(),
        );
    }
    core.close();
    Project {
        socket: state.join("control.sock"),
        dir,
        agents,
    }
}

/// Runs `cstan <args>` as an agent (the agent environment) or, without `agent`, as the operator in the project directory.
fn run(front: &Path, project: &Project, agent: Option<&str>, args: &[&str]) -> Output {
    let mut command = Command::new(front);
    command
        .args(args)
        .current_dir(project.dir.path())
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", project.dir.path());
    if let Some(agent) = agent {
        command
            .env("CAPSTAN_TOKEN", &project.agents[agent])
            .env("CAPSTAN_SOCKET", &project.socket);
    }
    command.output().expect("the client runs")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn started(project: &Project) -> Daemon {
    let mut daemon = Daemon::start(project.dir.path());
    let ready = daemon.wait_for_line(Duration::from_secs(20), |line| {
        line.contains("\"event\":\"ready\"")
    });
    assert!(
        ready.is_some(),
        "the daemon did not start: {}",
        daemon.stderr()
    );
    daemon
}

#[test]
fn the_front_end_runs_against_the_rust_daemon() {
    let front = front_end();
    let project = project();
    let mut daemon = started(&project);

    // ping: the daemon says who it is.
    let ping = run(&front, &project, Some("dev-1"), &["ping"]);
    assert_eq!(ping.status.code(), Some(0), "{}", text(&ping.stderr));
    assert!(
        text(&ping.stdout).contains("pong"),
        "{}",
        text(&ping.stdout)
    );

    // status: JSON with the three agents.
    let output = run(&front, &project, Some("dev-1"), &["status", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let status: Value = serde_json::from_str(&text(&output.stdout))
        .unwrap_or_else(|e| panic!("status is not JSON ({e}): {}", text(&output.stdout)));
    assert_eq!(status["agents"].as_array().map(Vec::len), Some(3));

    // send, inbox, ack: dev-2 writes to the PM, the PM reads and acknowledges.
    let body = "hello from dev-2";
    let sent = run(&front, &project, Some("dev-2"), &["send", "@pm", body]);
    assert_eq!(sent.status.code(), Some(0), "{}", text(&sent.stderr));
    assert!(
        !text(&sent.stdout).trim().is_empty(),
        "send printed nothing"
    );
    let inbox = run(&front, &project, Some("pm-1"), &["inbox"]);
    assert_eq!(inbox.status.code(), Some(0), "{}", text(&inbox.stderr));
    assert!(
        text(&inbox.stdout).contains(body),
        "{}",
        text(&inbox.stdout)
    );
    let listed = run(&front, &project, Some("pm-1"), &["inbox", "--json"]);
    let pm_inbox: Value = serde_json::from_str(&text(&listed.stdout)).unwrap_or(Value::Null);
    let message_ids: Vec<String> = pm_inbox["messages"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row["messageId"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(message_ids.len(), 1, "{pm_inbox}");
    let acked = run(&front, &project, Some("pm-1"), &["ack", &message_ids[0]]);
    assert_eq!(acked.status.code(), Some(0), "{}", text(&acked.stderr));
    let after = run(&front, &project, Some("pm-1"), &["inbox"]);
    assert_eq!(after.status.code(), Some(0));
    assert!(
        !text(&after.stdout).contains(body),
        "{}",
        text(&after.stdout)
    );

    // wait: mail that is already queued ends the wait.
    let wake = "wake dev-1";
    let sent = run(&front, &project, Some("pm-1"), &["send", "dev-1", wake]);
    assert_eq!(sent.status.code(), Some(0), "{}", text(&sent.stderr));
    let waited = run(&front, &project, Some("dev-1"), &["wait"]);
    assert_eq!(waited.status.code(), Some(0), "{}", text(&waited.stderr));
    assert!(
        text(&waited.stdout).contains(wake),
        "{}",
        text(&waited.stdout)
    );
    let inbox = run(&front, &project, Some("dev-1"), &["inbox"]);
    assert!(text(&inbox.stdout).contains(wake));

    assert_eq!(daemon.terminate(), Some(0), "stderr: {}", daemon.stderr());
    assert!(!project.socket.exists(), "the socket file is gone");
}
