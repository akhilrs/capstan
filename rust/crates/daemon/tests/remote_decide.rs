//! The `decide` route over a real socket on a seeded kernel: only the operator key decides, each refusal of the kernel comes
//! back with its own code, each outcome (applied, forwarded, repeat) has the kernel's shape, nothing is typed into a pane,
//! and two calls with one nonce that run at the same time give one applied and one repeat.

mod common;

use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::server::{DaemonServer, ServerOptions};
use capstan_kernel::areas::snapshot_feed::content_hash;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::Core;
use capstan_launcher::api::{LauncherService, StubLauncher};
use common::project::{write_project, Identity};
use common::{private_tempdir, seeded_env};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};

fn setup_step(core: &Core, owner: &str, op: &str, args: Value) -> Value {
    let context = json!({
        "credential": owner,
        "requestId": format!("req-{op}-{}", core.state_version().unwrap()),
        "idempotencyKey": format!("idem-{op}-{}", core.state_version().unwrap()),
        "expectedVersion": core.state_version().unwrap(),
        "inputRevision": core.input_revision().unwrap(),
    });
    let mut all = vec![context];
    all.extend(args.as_array().cloned().unwrap());
    core.dispatch(op, &all)
        .unwrap_or_else(|e| panic!("setup {op}: {e}"))
}

fn register_agents(state: &Path, directory: &Path, identity: &Identity) -> HashMap<String, String> {
    let core = Core::open(
        state,
        &identity.initial_project(),
        &KernelOptions {
            workspace_root: Some(directory.to_path_buf()),
            ..KernelOptions::default()
        },
        seeded_env("decide-setup"),
    )
    .expect("the setup controller opens");
    let owner = identity.credential.as_str();
    setup_step(
        &core,
        owner,
        "syncRoleDefinitions",
        json!([[
            {"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)},
            {"name": "pm", "kind": "PM", "host": "claude", "configHash": "b".repeat(64)},
        ]]),
    );
    let mut credentials = HashMap::new();
    for (agent, role_name, kind) in [("pm-1", "pm", "PM"), ("dev-1", "developer", "Developer")] {
        let seat = format!("seat-{agent}");
        setup_step(
            &core,
            owner,
            "createSeat",
            json!([{"seatId": seat, "name": seat, "role": kind}]),
        );
        let actor = setup_step(
            &core,
            owner,
            "createActor",
            json!([{"displayName": agent, "role": kind, "seatId": seat}]),
        );
        setup_step(
            &core,
            owner,
            "registerAgent",
            json!([{
                "agentId": agent,
                "roleName": role_name,
                "seatId": seat,
                "actorId": actor["actorId"],
            }]),
        );
        credentials.insert(
            agent.to_string(),
            actor["credential"].as_str().unwrap().to_string(),
        );
    }
    core.close();
    credentials
}

struct World {
    dir: common::PrivateDir,
    deps: Deps,
    owner: String,
    launcher: Arc<StubLauncher>,
    socket: PathBuf,
    server: Option<DaemonServer>,
    agents: HashMap<String, String>,
}

impl World {
    fn new() -> World {
        let dir = private_tempdir();
        let identity = Identity {
            project_id: "pdecide".into(),
            name: "Decide".into(),
            credential: format!("operator-{}", "d".repeat(40)),
        };
        let state = write_project(dir.path(), &identity);
        let agents = register_agents(&state, dir.path(), &identity);
        let kernel = {
            let (state, project, root) = (
                state.clone(),
                identity.initial_project(),
                dir.path().to_path_buf(),
            );
            KernelHandle::spawn(move || {
                Core::open(
                    &state,
                    &project,
                    &KernelOptions {
                        workspace_root: Some(root),
                        ..KernelOptions::default()
                    },
                    seeded_env("decide-routes"),
                )
            })
            .expect("the kernel opens")
        };
        let sink: Logger = Arc::new(|_: &LogEntry| {});
        let options = DaemonOptions::new(
            state.clone(),
            identity.initial_project(),
            dir.path().to_path_buf(),
        );
        let launcher = Arc::new(StubLauncher::new());
        let mut deps = Deps::new(kernel, options, sink);
        deps.launcher = Some(launcher.clone() as Arc<dyn LauncherService>);
        let socket = state.join("control.sock");
        let server = DaemonServer::start(deps.clone(), ServerOptions::new(socket.clone()))
            .expect("the server starts");
        World {
            dir,
            deps,
            owner: identity.credential,
            launcher,
            socket,
            server: Some(server),
            agents,
        }
    }

    /// One request over a fresh connection to the socket.
    fn command(&self, credential: &str, command: &str, args: &[&str]) -> Value {
        call_socket(&self.socket, credential, command, args)
    }

    fn decide(&self, args: &[&str]) -> Value {
        self.command(&self.owner, "decide", args)
    }

    fn core<T: Send + 'static>(
        &self,
        credential: &str,
        f: impl FnOnce(
                &Core,
                &capstan_kernel::types::MutationContext,
            ) -> capstan_kernel::KernelResult<T>
            + Send
            + 'static,
    ) -> T {
        let credential = credential.to_string();
        self.deps
            .kernel
            .run(move |core| {
                let context = capstan_daemon::deps::new_context(core, &credential)?;
                f(core, &context)
            })
            .expect("the kernel call works")
    }

    fn propose(&self, command: &str) -> String {
        let credential = self.agents["dev-1"].clone();
        let command = command.to_string();
        let proposal = self.core(&credential, move |core, context| {
            core.propose_operator_action(
                context,
                &json!({"kind": "command", "command": command, "reason": "r", "maxPending": 20}),
            )
        });
        format!("op:{}", proposal["proposalId"].as_str().unwrap())
    }

    fn capture(&self, options: Value) -> String {
        let credential = self.agents["pm-1"].clone();
        self.deps
            .kernel
            .run(|core| {
                core.configure_prompt_relay(&json!({"enabled": true, "captureTtlSeconds": 600}))
            })
            .unwrap();
        let relay = self.core(&credential, move |core, context| {
            let text = "Allow Bash(ls)?";
            let sha = capstan_kernel::canonical::sha256(
                &json!({"agentId": "dev-1", "paneId": "pane-1", "hostKind": "claude", "text": text, "options": options})
                    .to_string(),
            );
            core.record_prompt_capture(
                context,
                &json!({"prompt": {
                    "agentId": "dev-1", "paneId": "pane-1", "hostKind": "claude", "text": text,
                    "options": options, "promptSha": sha,
                }}),
            )
        });
        format!("prompt:{}", relay["relayId"].as_str().unwrap())
    }

    fn hash12(&self, request: &str) -> String {
        let request = request.to_string();
        let hash = self
            .deps
            .kernel
            .run(move |core| content_hash(core, &request))
            .unwrap()
            .expect("the request waits");
        hash[..12].to_string()
    }

    fn pm_messages(&self) -> i64 {
        let database =
            capstan_ledger::resolve_database_path(&self.dir.path().join(".capstan/state")).unwrap();
        rusqlite::Connection::open(database)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE recipient_agent_id = 'pm-1'",
                [],
                |r| r.get(0),
            )
            .unwrap()
    }

    fn decision_rows(&self) -> i64 {
        let database =
            capstan_ledger::resolve_database_path(&self.dir.path().join(".capstan/state")).unwrap();
        rusqlite::Connection::open(database)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM remote_decisions", [], |r| r.get(0))
            .unwrap()
    }

    fn finish(mut self) {
        self.server
            .take()
            .unwrap()
            .close()
            .expect("the server closes");
        self.deps.kernel.close();
    }
}

fn call_socket(socket: &Path, credential: &str, command: &str, args: &[&str]) -> Value {
    let mut stream = UnixStream::connect(socket).expect("the socket accepts");
    let frame = json!({"v": 1, "credential": credential, "command": command, "args": args});
    stream.write_all(format!("{frame}\n").as_bytes()).unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap_or_else(|e| panic!("{line:?}: {e}"))
}

fn result(answer: &Value) -> &Value {
    assert_eq!(answer["ok"], true, "{answer}");
    &answer["result"]
}

fn code(answer: &Value) -> &str {
    assert_eq!(answer["ok"], false, "{answer}");
    answer["code"].as_str().unwrap()
}

fn options() -> Value {
    json!([
        {"number": 1, "text": "Yes, once", "acceptsText": false, "widensPermissions": false},
        {"number": 2, "text": "Yes, always", "acceptsText": false, "widensPermissions": true},
        {"number": 3, "text": "No, say why", "acceptsText": true, "widensPermissions": false},
    ])
}

const FIELDS: &[&str] = &[
    "v",
    "decisionId",
    "requestId",
    "nonce",
    "outcome",
    "repeat",
    "kind",
    "verb",
    "contentHash",
    "eventSeq",
    "messageId",
    "replaces",
];

fn assert_shape(result: &Value) {
    for key in result.as_object().unwrap().keys() {
        assert!(FIELDS.contains(&key.as_str()), "unexpected field {key}");
    }
    for key in [
        "decisionId",
        "requestId",
        "nonce",
        "outcome",
        "kind",
        "verb",
    ] {
        assert!(result[key].is_string(), "{key} in {result}");
    }
}

#[test]
fn only_the_operator_key_decides() {
    let w = World::new();
    let request = w.propose("rm -rf build");
    let hash = w.hash12(&request);
    let args = [
        request.as_str(),
        "--expect",
        &hash,
        "--nonce",
        "nonce-0001",
        "deny",
    ];
    for agent in ["dev-1", "pm-1"] {
        let answer = w.command(&w.agents[agent], "decide", &args);
        assert_eq!(code(&answer), "forbidden", "{agent}: {answer}");
    }
    let answer = w.command("not-a-credential", "decide", &args);
    assert_eq!(code(&answer), "unauthorized", "{answer}");
    assert_eq!(w.decision_rows(), 0);
    let answer = w.decide(&args);
    assert_eq!(result(&answer)["outcome"], "applied");
    w.finish();
}

#[test]
fn each_refusal_of_the_kernel_has_its_code_over_the_socket() {
    let w = World::new();
    let request = w.propose("git push origin main");
    let hash = w.hash12(&request);
    let r = request.as_str();

    let answer = w.decide(&[
        "op:no-such-proposal",
        "--expect",
        &hash,
        "--nonce",
        "nonce-0001",
        "deny",
    ]);
    assert_eq!(code(&answer), "not_waiting", "{answer}");

    let wrong = "f".repeat(12);
    assert_ne!(wrong, hash);
    let answer = w.decide(&[r, "--expect", &wrong, "--nonce", "nonce-0002", "deny"]);
    assert_eq!(code(&answer), "hash_mismatch", "{answer}");

    let answer = w.decide(&[r, "--expect", &hash, "--nonce", "nonce-0003", "approve"]);
    assert_eq!(result(&answer)["outcome"], "forwarded");
    let answer = w.decide(&[r, "--expect", &wrong, "--nonce", "nonce-0003", "approve"]);
    assert_eq!(code(&answer), "nonce_reused", "{answer}");

    let prompt = w.capture(options());
    let prompt_hash = w.hash12(&prompt);
    let answer = w.decide(&[
        &prompt,
        "--expect",
        &prompt_hash,
        "--nonce",
        "nonce-0004",
        "option",
        "2",
    ]);
    assert_eq!(code(&answer), "needs_terminal", "{answer}");

    let answer = w.decide(&[r, "--expect", &hash, "--nonce", "nonce-0005", "bogus"]);
    assert_eq!(code(&answer), "invalid_request", "{answer}");
    let answer = w.decide(&[]);
    assert_eq!(code(&answer), "invalid_request", "{answer}");

    let owner = w.owner.clone();
    w.core(&owner, |core, context| {
        core.end_agent(context, "pm-1", None)
    });
    let answer = w.decide(&[r, "--expect", &hash, "--nonce", "nonce-0006", "approve"]);
    assert_eq!(code(&answer), "no_active_pm", "{answer}");
    w.finish();
}

#[test]
fn applied_forwarded_and_repeat_have_the_shape_of_the_kernel_and_type_nothing() {
    let w = World::new();
    let denied = w.propose("rm -rf build");
    let hash = w.hash12(&denied);
    let first = w.decide(&[&denied, "--expect", &hash, "--nonce", "nonce-0001", "deny"]);
    let applied = result(&first).clone();
    assert_shape(&applied);
    assert_eq!(applied["outcome"], "applied");
    assert_eq!(applied["repeat"], false);
    assert_eq!(applied["verb"], "deny");
    assert_eq!(applied["kind"], "op");
    let again = w.decide(&[&denied, "--expect", &hash, "--nonce", "nonce-0001", "deny"]);
    let repeat = result(&again);
    assert_shape(repeat);
    assert_eq!(repeat["repeat"], true);
    assert_eq!(repeat["decisionId"], applied["decisionId"]);
    assert_eq!(repeat["outcome"], "applied");

    let shipped = w.propose("git push origin main");
    let hash = w.hash12(&shipped);
    let before = w.pm_messages();
    let forwarded = w.decide(&[
        &shipped,
        "--expect",
        &hash,
        "--nonce",
        "nonce-0002",
        "approve",
    ]);
    let forwarded = result(&forwarded);
    assert_shape(forwarded);
    assert_eq!(forwarded["outcome"], "forwarded");
    assert!(forwarded["messageId"].is_string());
    assert_eq!(w.pm_messages(), before + 1);

    let prompt = w.capture(options());
    let hash = w.hash12(&prompt);
    let option = w.decide(&[
        &prompt,
        "--expect",
        &hash,
        "--nonce",
        "nonce-0003",
        "option",
        "1",
    ]);
    assert_eq!(result(&option)["outcome"], "forwarded");
    assert_eq!(w.pm_messages(), before + 2);

    // The route never reaches the launcher: no pane saw an answer or anything else.
    assert_eq!(w.launcher.calls(), Vec::<String>::new());
    w.finish();
}

#[test]
fn two_calls_with_one_nonce_in_parallel_give_one_applied_and_one_repeat() {
    let w = World::new();
    let request = w.propose("rm -rf build");
    let hash = w.hash12(&request);
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let (socket, owner, barrier) = (w.socket.clone(), w.owner.clone(), barrier.clone());
            let (request, hash) = (request.clone(), hash.clone());
            std::thread::spawn(move || {
                let mut stream = UnixStream::connect(&socket).expect("the socket accepts");
                let frame = json!({"v": 1, "credential": owner, "command": "decide",
                    "args": [request, "--expect", hash, "--nonce", "nonce-race1", "deny"]});
                barrier.wait();
                stream.write_all(format!("{frame}\n").as_bytes()).unwrap();
                let mut line = String::new();
                BufReader::new(stream).read_line(&mut line).unwrap();
                serde_json::from_str::<Value>(&line).unwrap()
            })
        })
        .collect();
    let answers: Vec<Value> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let mut outcomes: Vec<(String, bool)> = answers
        .iter()
        .map(|a| {
            let r = result(a);
            (
                r["outcome"].as_str().unwrap().to_string(),
                r["repeat"].as_bool().unwrap(),
            )
        })
        .collect();
    outcomes.sort();
    // Both are the "applied" decision; exactly one made it and one answered from its row.
    assert_eq!(
        outcomes,
        [
            ("applied".to_string(), false),
            ("applied".to_string(), true)
        ]
    );
    assert_eq!(
        answers[0]["result"]["decisionId"],
        answers[1]["result"]["decisionId"]
    );
    assert_eq!(w.decision_rows(), 1);
    w.finish();
}

#[test]
fn a_request_id_is_never_expanded_from_a_prefix() {
    let w = World::new();
    let request = w.propose("rm -rf build");
    let hash = w.hash12(&request);
    let short = &request[..request.len() - 1];
    assert!(short.len() > "op:".len(), "{request}");
    let answer = w.decide(&[short, "--expect", &hash, "--nonce", "nonce-0001", "deny"]);
    assert_eq!(code(&answer), "not_waiting", "{answer}");
    assert_eq!(w.decision_rows(), 0);
    let source = include_str!("../src/handlers/id_prefix.rs");
    assert!(!source.contains("(\"decide\","));
    w.finish();
}
