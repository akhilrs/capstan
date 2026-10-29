//! The `feed` route through `handle_frame` on a seeded kernel (no launcher, no operator, no Herdr): the eight subcommands
//! answer what the kernel answers, agents are refused, bad input is `invalid_request`, nothing is written, and the largest
//! pages fit the response limit and cross a real socket.

mod common;

use capstan_client::{call_feed, FeedCall, Response};
use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::shared::{AbortSignal, MAX_SEND_BODY_BYTES};
use capstan_daemon::handlers::MAX_RESPONSE_BYTES;
use capstan_daemon::server::{DaemonServer, ServerOptions};
use capstan_kernel::areas::event_feed::{self, Position};
use capstan_kernel::areas::snapshot_feed;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::Core;
use common::project::{write_project, Identity};
use common::{private_tempdir, seeded_env};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

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
        seeded_env("feed-setup"),
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
    agents: HashMap<String, String>,
}

impl World {
    fn new() -> World {
        let dir = private_tempdir();
        let identity = Identity {
            project_id: "pfeed".into(),
            name: "Feed".into(),
            credential: format!("operator-{}", "f".repeat(40)),
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
                    seeded_env("feed-routes"),
                )
            })
            .expect("the kernel opens")
        };
        let sink: Logger = Arc::new(|_: &LogEntry| {});
        let options =
            DaemonOptions::new(state, identity.initial_project(), dir.path().to_path_buf());
        World {
            dir,
            deps: Deps::new(kernel, options, sink),
            owner: identity.credential,
            agents,
        }
    }

    fn call(&self, credential: &str, args: &[&str]) -> Value {
        self.command(credential, "feed", args)
    }

    fn command(&self, credential: &str, command: &str, args: &[&str]) -> Value {
        let frame = json!({"v": 1, "credential": credential, "command": command, "args": args});
        let outcome = handle_frame(
            &self.deps,
            frame.to_string().as_bytes(),
            &AbortSignal::new(),
            MAX_RESPONSE_BYTES,
        );
        serde_json::from_str(&outcome.response.expect("an answer")).unwrap()
    }

    fn operator(&self, args: &[&str]) -> Value {
        self.call(&self.owner, args)
    }

    fn send(&self, body: &str) {
        let answer = self.command(&self.agents["dev-1"], "send", &["--action", "pm-1", body]);
        assert_eq!(answer["ok"], true, "{answer}");
    }

    /// Seeds a pending operator proposal (an item that waits for the user; a message to an agent does not).
    fn propose(&self, command: &str) {
        let credential = self.agents["dev-1"].clone();
        let command = command.to_string();
        self.direct(move |c| {
            Ok(setup_step(
                c,
                &credential,
                "proposeOperatorAction",
                json!([{"kind": "command", "command": command, "reason": "r", "maxPending": 100_000}]),
            ))
        });
    }

    fn direct<T: serde::Serialize + Send + 'static>(
        &self,
        f: impl FnOnce(&Core) -> capstan_kernel::KernelResult<T> + Send + 'static,
    ) -> Value {
        serde_json::to_value(self.deps.kernel.run(f).unwrap()).unwrap()
    }

    fn counts(&self) -> (i64, i64, i64) {
        let version = self.deps.kernel.run(|core| core.state_version()).unwrap();
        let database =
            capstan_ledger::resolve_database_path(&self.dir.path().join(".capstan/state")).unwrap();
        let connection = rusqlite::Connection::open(database).unwrap();
        let count = |table: &str| -> i64 {
            connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap()
        };
        (
            version,
            count("controller_events"),
            count("mutation_requests"),
        )
    }
}

fn result(answer: &Value) -> &Value {
    assert_eq!(answer["ok"], true, "{answer}");
    &answer["result"]
}

fn code(answer: &Value) -> &str {
    assert_eq!(answer["ok"], false, "{answer}");
    answer["code"].as_str().unwrap()
}

#[test]
fn the_subcommands_answer_what_the_kernel_answers() {
    let w = World::new();
    for n in 0..5 {
        w.send(&format!("message {n}"));
        w.propose(&format!("ls {n}"));
    }
    assert_eq!(
        *result(&w.operator(&["head"])),
        w.direct(event_feed::feed_head)
    );
    assert_eq!(
        *result(&w.operator(&["snapshot"])),
        w.direct(snapshot_feed::snapshot_head)
    );
    let page = w.direct(|c| event_feed::feed_page(c, Position { seq: 0, k: 0 }, 50, None));
    assert!(!page["events"].as_array().unwrap().is_empty());
    assert_eq!(
        *result(&w.operator(&["page", "--after", "0:0", "--limit", "50"])),
        page
    );
    let anchor = w.direct(|c| capstan_kernel::areas::feed_cursor::anchor(c, 1));
    let anchor = anchor.as_str().unwrap().to_string();
    let expected = {
        let a = anchor.clone();
        w.direct(move |c| event_feed::feed_page(c, Position { seq: 1, k: 0 }, 10, Some(&a)))
    };
    assert_eq!(
        *result(&w.operator(&["page", "--after", "1:0", "--limit", "10", "--anchor", &anchor])),
        expected
    );
    let waiting = w.direct(|c| snapshot_feed::waiting_page(c, None, 100, None));
    assert!(!waiting["items"].as_array().unwrap().is_empty());
    assert_eq!(
        *result(&w.operator(&["waiting", "--limit", "100"])),
        waiting
    );
    for kind in ["message", "op"] {
        assert_eq!(
            *result(&w.operator(&["waiting", "--limit", "100", "--kind", kind])),
            w.direct(move |c| snapshot_feed::waiting_page(c, None, 100, Some(kind)))
        );
    }
    let agents = w.direct(|c| snapshot_feed::agents_page(c, None, 100));
    assert_eq!(*result(&w.operator(&["agents", "--limit", "100"])), agents);
    let after = agents["items"][0]["key"].as_str().unwrap().to_string();
    assert_eq!(
        *result(&w.operator(&["agents", "--limit", "100", "--after", &after])),
        {
            let a = after.clone();
            w.direct(move |c| snapshot_feed::agents_page(c, Some(&a), 100))
        }
    );
    assert_eq!(
        *result(&w.operator(&["plans", "--limit", "100"])),
        w.direct(|c| snapshot_feed::plans_page(c, None, 100))
    );
    // item and hash, for every key and request id the snapshot lists, and for ids nothing waits on.
    let mut targets: Vec<(String, String, String)> = Vec::new();
    for item in waiting["items"].as_array().unwrap() {
        targets.push((
            "waiting".into(),
            item["key"].as_str().unwrap().into(),
            item["requestId"].as_str().unwrap().into(),
        ));
    }
    for item in agents["items"].as_array().unwrap() {
        targets.push((
            "agents".into(),
            item["key"].as_str().unwrap().into(),
            item["requestId"].as_str().unwrap().into(),
        ));
    }
    targets.push(("run".into(), "run".into(), "run".into()));
    for (section, key, request_id) in targets {
        let item = {
            let (s, k) = (section.clone(), key.clone());
            w.direct(move |c| snapshot_feed::snapshot_item(c, &s, &k))
        };
        assert!(!item.is_null(), "{section} {key}");
        assert_eq!(
            *result(&w.operator(&["item", &section, &key])),
            json!({"section": section, "key": key, "item": item})
        );
        let hash = {
            let r = request_id.clone();
            w.direct(move |c| snapshot_feed::content_hash(c, &r))
        };
        assert!(hash.is_string(), "{request_id}");
        assert_eq!(
            *result(&w.operator(&["hash", &request_id])),
            json!({"requestId": request_id, "contentHash": hash})
        );
    }
    for request_id in [
        "op:nope",
        "prompt:nope",
        "design:p/q",
        "message:nope",
        "integration:nope",
        "agent:nope",
    ] {
        assert_eq!(
            *result(&w.operator(&["hash", request_id])),
            json!({"requestId": request_id, "contentHash": null})
        );
    }
    let gone = result(&w.operator(&["item", "agents", "agent:nobody"])).clone();
    assert!(gone["item"].is_null(), "{gone}");
}

#[test]
fn head_snapshot_and_page_answers_carry_the_anchor_the_kernel_computes() {
    let w = World::new();
    for n in 0..3 {
        w.send(&format!("message {n}"));
    }
    let head = w.direct(event_feed::feed_head);
    let want = w.direct(move |c| {
        capstan_kernel::areas::feed_cursor::anchor(c, head["head"].as_i64().unwrap())
    });
    assert!(want.is_string());
    assert_eq!(result(&w.operator(&["head"]))["anchor"], want);
    assert_eq!(result(&w.operator(&["snapshot"]))["anchor"], want);
    let page = result(&w.operator(&["page", "--after", "1:0", "--limit", "2"])).clone();
    let seq = page["scanned"]["seq"].as_i64().unwrap();
    let scanned = w.direct(move |c| capstan_kernel::areas::feed_cursor::anchor(c, seq));
    assert!(scanned.is_string());
    assert_eq!(page["anchor"], scanned);
}

#[test]
fn agents_and_strangers_are_refused() {
    let w = World::new();
    let subcommands: [&[&str]; 8] = [
        &["head"],
        &["page", "--after", "0:0", "--limit", "5"],
        &["snapshot"],
        &["waiting", "--limit", "5"],
        &["agents", "--limit", "5"],
        &["plans", "--limit", "5"],
        &["item", "run", "run"],
        &["hash", "run"],
    ];
    for args in subcommands {
        for agent in ["pm-1", "dev-1"] {
            assert_eq!(
                code(&w.call(&w.agents[agent].clone(), args)),
                "forbidden",
                "{args:?}"
            );
        }
        assert_eq!(code(&w.call("nobody", args)), "unauthorized");
        assert_eq!(w.operator(args)["ok"], true, "{args:?}");
    }
}

#[test]
fn bad_input_is_invalid_request() {
    let w = World::new();
    let cases: Vec<Vec<&str>> = vec![
        vec![],
        vec!["bogus"],
        vec!["head", "extra"],
        vec!["snapshot", "--limit", "1"],
        vec!["page", "--after", "0:0", "--limit", "5", "--unknown", "1"],
        vec!["page", "--after", "0:0", "--after", "0:0", "--limit", "5"],
        vec!["page", "--after", "0:0", "--limit", "0"],
        vec!["page", "--after", "0:0", "--limit", "501"],
        vec!["page", "--after", "0:0", "--limit", "x"],
        vec!["page", "--after", "0:0", "--limit", "-1"],
        vec!["page", "--after", "0:0"],
        vec!["page", "--limit", "5"],
        vec!["page", "--after", "0:0", "--limit"],
        vec!["page", "--after", "1", "--limit", "5"],
        vec!["page", "--after", "18446744073709551616:0", "--limit", "5"],
        vec!["page", "--after", "0:4294967296", "--limit", "5"],
        vec![
            "page",
            "--after",
            "1:0",
            "--limit",
            "5",
            "--anchor",
            "abcdef012345678",
        ],
        vec![
            "page",
            "--after",
            "1:0",
            "--limit",
            "5",
            "--anchor",
            "ABCDEF0123456789",
        ],
        vec![
            "page",
            "--after",
            "0:0",
            "--limit",
            "5",
            "--anchor",
            "abcdef0123456789",
        ],
        vec!["waiting", "--limit", "101"],
        vec!["waiting", "--limit", "0"],
        vec!["waiting", "--limit", "5", "--kind", "bogus"],
        vec!["waiting", "--limit", "5", "--after", "not a key"],
        vec!["agents", "--limit", "101"],
        vec!["plans", "--limit", "5", "--after", "pm"],
        vec!["plans", "--limit", "101"],
        vec!["plans", "--limit", "5", "--kind", "message"],
        vec!["item", "nowhere", "key"],
        vec!["item", "run"],
        vec!["item", "run", "run", "extra"],
        vec!["hash"],
        vec!["hash", "a", "b"],
    ];
    for args in cases {
        assert_eq!(code(&w.operator(&args)), "invalid_request", "{args:?}");
    }
    let answer = w.operator(&["waiting", "--limit", "5", "--kind", "bad\u{7}kind\u{202e}x"]);
    assert_eq!(code(&answer), "invalid_request");
    let message = answer["message"].as_str().unwrap();
    assert!(
        !message.chars().any(|c| c.is_control() || c == '\u{202e}'),
        "{message:?}"
    );
    assert!(message.chars().count() <= 200);
}

#[test]
fn the_route_reads_only() {
    let w = World::new();
    for n in 0..3 {
        w.send(&format!("m{n}"));
    }
    let before = w.counts();
    let agents = w.agents["pm-1"].clone();
    let _ = agents;
    for _ in 0..20 {
        for args in [
            vec!["head"],
            vec!["page", "--after", "0:0", "--limit", "500"],
            vec!["snapshot"],
            vec!["waiting", "--limit", "100"],
            vec!["agents", "--limit", "100"],
            vec!["plans", "--limit", "100"],
            vec!["item", "run", "run"],
            vec!["hash", "run"],
        ] {
            assert_eq!(w.operator(&args)["ok"], true, "{args:?}");
        }
    }
    assert_eq!(w.counts(), before);
}

#[test]
fn the_largest_pages_fit_the_response_limit_and_cross_a_socket() {
    let w = World::new();
    let body = "\"".repeat(MAX_SEND_BODY_BYTES);
    for _ in 0..110 {
        w.send(&body);
        w.propose(&"\"".repeat(4000));
    }
    let page = w.operator(&["page", "--after", "0:0", "--limit", "500"]);
    assert_eq!(page["ok"], true, "{page}");
    let waiting = w.operator(&["waiting", "--limit", "100"]);
    assert_eq!(waiting["ok"], true, "{waiting}");
    assert_eq!(waiting["result"]["items"].as_array().unwrap().len(), 100);
    for answer in [&page, &waiting] {
        assert!(answer.to_string().len() < MAX_RESPONSE_BYTES);
    }
    let state = w.dir.path().join(".capstan/state");
    let socket = state.join("control.sock");
    let server = DaemonServer::start(w.deps.clone(), ServerOptions::new(socket.clone()))
        .expect("the server starts");
    let sent = |call: FeedCall| match call_feed(&socket, &w.owner, &call, 20_000).unwrap() {
        Response::Ok { result } => result.expect("a result"),
        other => panic!("{other:?}"),
    };
    let received = sent(FeedCall::Page {
        after_seq: 0,
        after_k: 0,
        limit: 500,
        anchor: None,
    });
    assert!(format!("{received:?}").len() > 100_000);
    let received = sent(FeedCall::Waiting {
        after: None,
        limit: 100,
        kind: None,
    });
    assert!(format!("{received:?}").len() > 100_000);
    // The refusal and the success of the client's own acceptance test.
    assert!(matches!(
        call_feed(&socket, &w.owner, &FeedCall::Head, 20_000).unwrap(),
        Response::Ok { .. }
    ));
    match call_feed(
        &socket,
        &w.owner,
        &FeedCall::Agents {
            after: None,
            limit: 0,
        },
        20_000,
    )
    .unwrap()
    {
        Response::Refused { code, .. } => assert_eq!(code.to_utf8_lossy(), "invalid_request"),
        other => panic!("{other:?}"),
    }
    server.close().expect("the server closes");
    w.deps.kernel.close();
}

#[test]
fn the_handler_runs_one_kernel_closure_per_subcommand() {
    let source = include_str!("../src/handlers/feed.rs");
    assert_eq!(source.matches(".run(").count(), 8);
    assert_eq!(source.matches(".kernel").count(), 1);
    assert_eq!(source.matches("env.deps").count(), 1);
}
