//! The clients against a scratch `cstan-daemon`: the Rust `cstan` front end and the Node CLI
//! (`node dist/src/cli.js ping/status/inbox/send/ack/wait`) run unchanged against the same daemon, and answer alike.
//!
//! The Node half needs the built CLI (`npm run build`) and a `node` on the PATH; without them it is skipped with a note on
//! the standard error, unless `CAPSTAN_INTEROP_STRICT=1` makes a missing client a failure. The front end is built
//! with cargo when it is not beside the daemon binary.

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

fn strict() -> bool {
    std::env::var("CAPSTAN_INTEROP_STRICT").is_ok_and(|v| v == "1")
}

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

/// The Node CLI of this repository, when it is built and node is installed.
fn node_cli() -> Option<PathBuf> {
    let entry = repo_root().join("dist").join("src").join("cli.js");
    let node_found = Command::new("node")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if entry.exists() && node_found {
        return Some(entry);
    }
    assert!(
        !strict(),
        "the Node CLI is missing ({}); run npm run build",
        entry.display()
    );
    eprintln!(
        "client_interop: the Node half is skipped: no node or no {} (npm run build)",
        entry.display()
    );
    None
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Client {
    Front,
    Node,
}

struct Clients {
    front: PathBuf,
    node: Option<PathBuf>,
}

impl Clients {
    fn all(&self) -> Vec<Client> {
        let mut clients = vec![Client::Front];
        if self.node.is_some() {
            clients.push(Client::Node);
        }
        clients
    }

    /// Runs `cstan <args>` as an agent (the agent environment) or, without `agent`, as the operator in the project directory.
    fn run(&self, client: Client, project: &Project, agent: Option<&str>, args: &[&str]) -> Output {
        let mut command = match client {
            Client::Front => Command::new(&self.front),
            Client::Node => {
                let mut command = Command::new("node");
                command.arg(self.node.as_ref().expect("the node client"));
                command
            }
        };
        command
            .args(args)
            .current_dir(project.dir.path())
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", project.dir.path());
        if let Some(agent) = agent {
            command
                .env("CAPSTAN_TOKEN", &project.agents[agent])
                .env("CAPSTAN_SOCKET", &project.socket);
        }
        if client == Client::Front {
            // The front end hands what it does not serve to the Node CLI; the interop test must see only its own answers.
            if let Some(node) = &self.node {
                command.env("CSTAN_NODE_CLI", node);
            }
        }
        command.output().expect("the client runs")
    }
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
fn both_clients_run_unchanged_against_the_rust_daemon() {
    let clients = Clients {
        front: front_end(),
        node: node_cli(),
    };
    let project = project();
    let mut daemon = started(&project);

    // ping: the daemon says who it is, to every client alike.
    let pings: Vec<(Client, Output)> = clients
        .all()
        .into_iter()
        .map(|client| {
            (
                client,
                clients.run(client, &project, Some("dev-1"), &["ping"]),
            )
        })
        .collect();
    for (client, output) in &pings {
        assert_eq!(
            output.status.code(),
            Some(0),
            "{client:?} ping: {}",
            text(&output.stderr)
        );
        assert!(
            text(&output.stdout).contains("pong"),
            "{client:?}: {}",
            text(&output.stdout)
        );
    }
    assert!(
        pings
            .windows(2)
            .all(|pair| pair[0].1.stdout == pair[1].1.stdout),
        "the clients print different pings: {:?}",
        pings
            .iter()
            .map(|(_, o)| text(&o.stdout))
            .collect::<Vec<_>>()
    );

    // status: the same JSON for the same ledger.
    let statuses: Vec<Value> = clients
        .all()
        .into_iter()
        .map(|client| {
            let output = clients.run(client, &project, Some("dev-1"), &["status", "--json"]);
            assert_eq!(
                output.status.code(),
                Some(0),
                "{client:?} status: {}",
                text(&output.stderr)
            );
            serde_json::from_str(&text(&output.stdout)).unwrap_or_else(|e| {
                panic!(
                    "{client:?} status is not JSON ({e}): {}",
                    text(&output.stdout)
                )
            })
        })
        .collect();
    assert!(
        statuses.windows(2).all(|pair| pair[0] == pair[1]),
        "{statuses:?}"
    );
    assert_eq!(statuses[0]["agents"].as_array().map(Vec::len), Some(3));

    // send, inbox, ack: dev-2 writes to the PM with each client, the PM reads and acknowledges.
    let mut ids = Vec::new();
    for client in clients.all() {
        let body = format!("hello from dev-2 by {client:?}");
        let sent = clients.run(client, &project, Some("dev-2"), &["send", "@pm", &body]);
        assert_eq!(
            sent.status.code(),
            Some(0),
            "{client:?} send: {}",
            text(&sent.stderr)
        );
        let printed = text(&sent.stdout);
        assert!(
            !printed.trim().is_empty(),
            "{client:?} send printed nothing"
        );
        ids.push((client, body, printed));
    }
    let inboxes: Vec<(Client, Output)> = clients
        .all()
        .into_iter()
        .map(|client| {
            (
                client,
                clients.run(client, &project, Some("pm-1"), &["inbox"]),
            )
        })
        .collect();
    for (client, output) in &inboxes {
        assert_eq!(
            output.status.code(),
            Some(0),
            "{client:?} inbox: {}",
            text(&output.stderr)
        );
        for (_, body, _) in &ids {
            assert!(
                text(&output.stdout).contains(body.as_str()),
                "{client:?}: {}",
                text(&output.stdout)
            );
        }
    }
    assert!(
        inboxes
            .windows(2)
            .all(|pair| pair[0].1.stdout == pair[1].1.stdout),
        "the clients print different inboxes"
    );
    // Message ids from the PM's own inbox; each client acknowledges one.
    let pm_inbox: Value = {
        let output = clients.run(Client::Front, &project, Some("pm-1"), &["inbox", "--json"]);
        serde_json::from_str(&text(&output.stdout)).unwrap_or(Value::Null)
    };
    let message_ids: Vec<String> = pm_inbox["messages"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row["messageId"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(message_ids.len(), clients.all().len(), "{pm_inbox}");
    for (client, id) in clients.all().into_iter().zip(&message_ids) {
        let acked = clients.run(client, &project, Some("pm-1"), &["ack", id]);
        assert_eq!(
            acked.status.code(),
            Some(0),
            "{client:?} ack: {}",
            text(&acked.stderr)
        );
    }
    let after = clients.run(Client::Front, &project, Some("pm-1"), &["inbox"]);
    assert_eq!(after.status.code(), Some(0));
    assert!(
        !text(&after.stdout).contains("hello from dev-2"),
        "{}",
        text(&after.stdout)
    );

    // wait: mail that is already queued ends the wait at once, with either client.
    for client in clients.all() {
        let body = format!("wake dev-1 for {client:?}");
        let sent = clients.run(
            Client::Front,
            &project,
            Some("pm-1"),
            &["send", "dev-1", &body],
        );
        assert_eq!(sent.status.code(), Some(0), "{}", text(&sent.stderr));
        let started = std::time::Instant::now();
        let waited = clients.run(client, &project, Some("dev-1"), &["wait"]);
        assert_eq!(
            waited.status.code(),
            Some(0),
            "{client:?} wait: {}",
            text(&waited.stderr)
        );
        assert!(
            text(&waited.stdout).contains(&body),
            "{client:?}: {}",
            text(&waited.stdout)
        );
        assert!(started.elapsed() < Duration::from_secs(10));
        let inbox = clients.run(client, &project, Some("dev-1"), &["inbox"]);
        assert!(text(&inbox.stdout).contains(&body));
    }

    assert_eq!(daemon.terminate(), Some(0), "stderr: {}", daemon.stderr());
    assert!(!project.socket.exists(), "the socket file is gone");
}
