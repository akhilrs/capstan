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
use std::sync::{Arc, Barrier, Mutex};

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

/// A launcher that delegates to the recording stub and records the pickers it was asked to close; `sent` is its answer.
struct PickerLauncher {
    stub: StubLauncher,
    closed: Mutex<Vec<(String, String)>>,
    sent: bool,
}

impl PickerLauncher {
    fn new(sent: bool) -> Arc<Self> {
        Arc::new(Self {
            stub: StubLauncher::new(),
            closed: Mutex::new(Vec::new()),
            sent,
        })
    }

    /// The closes so far, waiting a moment for the thread that makes them.
    fn closes(&self, expected: usize) -> Vec<(String, String)> {
        for _ in 0..200 {
            if self.closed.lock().unwrap().len() >= expected {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        self.closed.lock().unwrap().clone()
    }
}

macro_rules! delegate {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $ret:ty;)*) => {
        $(fn $name(&self, $($arg: $ty),*) -> $ret { self.stub.$name($($arg),*) })*
    };
}

impl LauncherService for PickerLauncher {
    delegate! {
        launch_pm() -> capstan_launcher::api::LauncherResult<capstan_launcher::api::LaunchResult>;
        restart_pm() -> capstan_launcher::api::LauncherResult<capstan_launcher::api::LaunchResult>;
        spawn(role_name: &str, options: &capstan_launcher::api::SpawnOptions) -> capstan_launcher::api::LauncherResult<capstan_launcher::api::SpawnResult>;
        rename_branch_for_task(agent_id: &str, task: &str) -> capstan_launcher::api::LauncherResult<capstan_launcher::api::BranchRenameResult>;
        release(agent_id: &str) -> capstan_launcher::api::LauncherResult<capstan_launcher::api::ReleaseResult>;
        replace(agent_id: &str) -> capstan_launcher::api::LauncherResult<capstan_launcher::api::ReplaceResult>;
        observe(agent_id: &str, lines: usize) -> capstan_launcher::api::LauncherResult<capstan_launcher::api::ObserveResult>;
        capture_prompt(agent_id: &str) -> capstan_launcher::api::LauncherResult<capstan_herdr::api::CaptureOutcome>;
        answer_prompt(agent_id: &str, request: capstan_launcher::api::AnswerPromptRequest<'_>) -> capstan_launcher::api::LauncherResult<capstan_herdr::api::RelayOutcome>;
        interrupt(agent_id: &str) -> capstan_launcher::api::LauncherResult<bool>;
        status() -> capstan_launcher::api::LauncherStatus;
        operator_environment() -> std::collections::BTreeMap<String, String>;
        in_flight_operations() -> usize;
        adopt_all() -> capstan_launcher::api::LauncherResult<()>;
        teardown(agent_id: &str, worktree_path: &str) -> capstan_launcher::api::LauncherResult<capstan_launcher::api::SetupOutcome>;
    }

    fn dismiss_question(
        &self,
        agent_id: &str,
        question: &str,
        _options: &[String],
    ) -> capstan_launcher::api::LauncherResult<bool> {
        self.closed
            .lock()
            .unwrap()
            .push((agent_id.to_string(), question.to_string()));
        Ok(self.sent)
    }
}

struct World {
    dir: common::PrivateDir,
    logs: Arc<Mutex<Vec<LogEntry>>>,
    picker: Option<Arc<PickerLauncher>>,
    operator: Option<Arc<capstan_operator::api::StubOperator>>,
    deps: Deps,
    owner: String,
    launcher: Arc<StubLauncher>,
    socket: PathBuf,
    server: Option<DaemonServer>,
    agents: HashMap<String, String>,
}

impl World {
    fn new() -> World {
        World::build(None, false)
    }

    /// A world whose launcher records the pickers it is asked to close, and whose Operator service is a stub.
    fn with_picker(sent: bool, operator: bool) -> World {
        World::build(Some(PickerLauncher::new(sent)), operator)
    }

    fn build(picker: Option<Arc<PickerLauncher>>, operator: bool) -> World {
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
        let logs: Arc<Mutex<Vec<LogEntry>>> = Arc::new(Mutex::new(Vec::new()));
        let sink: Logger = {
            let logs = logs.clone();
            Arc::new(move |entry: &LogEntry| logs.lock().unwrap().push(entry.clone()))
        };
        let options = DaemonOptions::new(
            state.clone(),
            identity.initial_project(),
            dir.path().to_path_buf(),
        );
        let launcher = Arc::new(StubLauncher::new());
        let mut deps = Deps::new(kernel, options, sink);
        deps.launcher = Some(match &picker {
            Some(picker) => picker.clone() as Arc<dyn LauncherService>,
            None => launcher.clone() as Arc<dyn LauncherService>,
        });
        let stub_operator = operator.then(|| Arc::new(capstan_operator::api::StubOperator::new()));
        deps.operator = stub_operator
            .clone()
            .map(|stub| stub as Arc<dyn capstan_operator::api::OperatorService>);
        let socket = state.join("control.sock");
        let server = DaemonServer::start(deps.clone(), ServerOptions::new(socket.clone()))
            .expect("the server starts");
        World {
            dir,
            logs,
            picker,
            operator: stub_operator,
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

// ------------------------------------------------------------------------------------------------ the operator picker

impl World {
    fn picker(&self) -> &Arc<PickerLauncher> {
        self.picker
            .as_ref()
            .expect("a world with a picker launcher")
    }

    /// The question the PM's picker of this proposal opens with.
    fn question_of(&self, request: &str) -> String {
        let id = request.strip_prefix("op:").unwrap().to_string();
        let record = self
            .deps
            .kernel
            .run(move |core| core.operator_proposal(&id))
            .unwrap();
        format!(
            "Operator proposal {} hash {}",
            record["proposalId"].as_str().unwrap(),
            &record["commandSha"].as_str().unwrap()[..12]
        )
    }

    fn picker_logs(&self) -> Vec<Value> {
        self.logs
            .lock()
            .unwrap()
            .iter()
            .filter(|entry| entry.command == "daemon:operator_picker_closed")
            .filter_map(|entry| entry.detail.clone())
            .collect()
    }

    fn wait_logs(&self, count: usize) -> Vec<Value> {
        for _ in 0..200 {
            if self.picker_logs().len() >= count {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        self.picker_logs()
    }
}

fn settle() {
    std::thread::sleep(std::time::Duration::from_millis(300));
}

#[test]
fn a_deny_and_a_forward_close_the_pm_picker_and_a_repeat_or_a_refusal_does_not() {
    let w = World::with_picker(false, false);
    let denied = w.propose("rm -rf build");
    let hash = w.hash12(&denied);
    let question = w.question_of(&denied);
    let args = [&*denied, "--expect", &hash, "--nonce", "nonce-0001", "deny"];
    assert_eq!(result(&w.decide(&args))["outcome"], "applied");
    assert_eq!(
        w.picker().closes(1),
        vec![("pm-1".to_string(), question.clone())]
    );
    // A working PM gives sent false and no error.
    let logs = w.wait_logs(1);
    assert_eq!(logs[0]["sent"], false);
    assert_eq!(logs[0]["error"], Value::Null);
    assert_eq!(logs[0]["agentId"], "pm-1");
    // The same nonce again is a repeat: nothing more is closed.
    assert_eq!(result(&w.decide(&args))["repeat"], true);
    // The phone decided already, so an approve now is refused and closes nothing.
    let late = w.decide(&[
        &denied,
        "--expect",
        &hash,
        "--nonce",
        "nonce-0002",
        "approve",
    ]);
    assert_eq!(code(&late), "not_waiting", "{late}");
    settle();
    assert_eq!(w.picker().closes(1).len(), 1);

    // A forward closes the picker, a second forward with a new nonce closes it again.
    let shipped = w.propose("git push origin main");
    let hash = w.hash12(&shipped);
    let question = w.question_of(&shipped);
    for nonce in ["nonce-0003", "nonce-0004"] {
        let forwarded = w.decide(&[&shipped, "--expect", &hash, "--nonce", nonce, "approve"]);
        assert_eq!(result(&forwarded)["outcome"], "forwarded");
    }
    let closes = w.picker().closes(3);
    assert_eq!(closes.len(), 3);
    assert_eq!(closes[1], ("pm-1".to_string(), question.clone()));
    assert_eq!(closes[2], ("pm-1".to_string(), question));
    w.finish();
}

#[test]
fn without_an_active_pm_nothing_is_closed_and_the_log_says_so() {
    let w = World::with_picker(true, false);
    let denied = w.propose("rm -rf build");
    let hash = w.hash12(&denied);
    let database =
        capstan_ledger::resolve_database_path(&w.dir.path().join(".capstan/state")).unwrap();
    rusqlite::Connection::open(database)
        .unwrap()
        .execute("UPDATE agents SET state = 'ended' WHERE kind = 'PM'", [])
        .unwrap();
    let answer = w.decide(&[&denied, "--expect", &hash, "--nonce", "nonce-0001", "deny"]);
    assert_eq!(result(&answer)["outcome"], "applied");
    let logs = w.wait_logs(1);
    assert_eq!(logs[0]["error"], "no_active_pm");
    assert_eq!(logs[0]["sent"], false);
    settle();
    assert!(w.picker().closes(0).is_empty());
    w.finish();
}

#[test]
fn approve_as_goes_to_the_operator_service_closes_the_picker_and_a_repeat_does_not() {
    let w = World::with_picker(true, true);
    let proposal = w.propose("git log --oneline");
    let hash = w.hash12(&proposal);
    let question = w.question_of(&proposal);
    let applied = json!({"v": 1, "decisionId": "d1", "requestId": proposal, "nonce": "nonce-0001",
        "outcome": "applied", "repeat": false, "kind": "op", "verb": "approve-as"});
    let mut repeat = applied.clone();
    repeat["repeat"] = json!(true);
    let operator = w.operator.clone().unwrap();
    operator.script("decide", Ok(applied));
    operator.script("decide", Ok(repeat));
    let args = [
        &*proposal,
        "--expect",
        &hash,
        "--nonce",
        "nonce-0001",
        "approve-as",
        "exact",
    ];
    assert_eq!(result(&w.decide(&args))["outcome"], "applied");
    assert_eq!(w.picker().closes(1), vec![("pm-1".to_string(), question)]);
    assert_eq!(result(&w.decide(&args))["repeat"], true);
    settle();
    assert_eq!(w.picker().closes(1).len(), 1);
    assert_eq!(operator.calls().len(), 2);
    w.finish();
}

#[test]
fn without_an_operator_service_approve_as_is_needs_terminal_and_an_old_approve_is_forwarded() {
    let w = World::with_picker(true, false);
    let proposal = w.propose("git log --oneline");
    let hash = w.hash12(&proposal);
    let answer = w.decide(&[
        &proposal,
        "--expect",
        &hash,
        "--nonce",
        "nonce-0001",
        "approve-as",
        "once",
    ]);
    assert_eq!(code(&answer), "needs_terminal", "{answer}");
    settle();
    assert!(w.picker().closes(0).is_empty());
    let forwarded = w.decide(&[
        &proposal,
        "--expect",
        &hash,
        "--nonce",
        "nonce-0002",
        "approve",
    ]);
    assert_eq!(result(&forwarded)["outcome"], "forwarded");
    w.finish();
}
