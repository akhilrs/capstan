//! The reply cache through `handle_frame_with` on a real ledger: a retried request id gets the first attempt's answer, a
//! repeated send is folded within the window, and what must not be stored is not.

mod common;

use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::replay::ReplyCache;
use capstan_daemon::handlers::shared::{AbortSignal, CommandState};
use capstan_daemon::handlers::{handle_frame_with, FrameOutcome, MAX_RESPONSE_BYTES};
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::{Core, SystemEnv};
use common::project::{write_project, Identity};
use common::{private_tempdir, PrivateDir};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

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
        Box::new(SystemEnv),
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
    for (agent, role_name, kind) in [
        ("pm-1", "pm", "PM"),
        ("dev-1", "developer", "Developer"),
        ("dev-2", "developer", "Developer"),
    ] {
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

fn role_config(dir: &Path) -> capstan_config::RoleConfig {
    let file = dir.join("capstan.toml");
    std::fs::write(
        &file,
        "schema_version = 1\n\n[hosts.claude]\nkind = \"claude\"\n\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n\n[review]\nwarm_reviewer = false\n\n[roles.developer]\nkind = \"Developer\"\nhost = \"claude\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    capstan_config::load_role_config(dir).expect("the configuration loads")
}

struct World {
    _dir: PrivateDir,
    deps: Deps,
    agents: HashMap<String, String>,
    logs: Arc<Mutex<Vec<LogEntry>>>,
    clock: Arc<AtomicI64>,
}

/// `with_config`: the daemon has a capstan.toml (the repeat window is its default of 60 s) or none (a transcript replay).
fn world_with(with_config: bool, cache: impl FnOnce(Arc<AtomicI64>) -> ReplyCache) -> World {
    let dir = private_tempdir();
    let identity = Identity {
        project_id: "preply".into(),
        name: "Reply Test".into(),
        credential: format!("operator-{}", "r".repeat(40)),
    };
    let state = write_project(dir.path(), &identity);
    let agents = register_agents(&state, dir.path(), &identity);
    let kernel = {
        let (state, project, directory) = (
            state.clone(),
            identity.initial_project(),
            dir.path().to_path_buf(),
        );
        KernelHandle::spawn(move || {
            Core::open(
                &state,
                &project,
                &KernelOptions {
                    workspace_root: Some(directory),
                    ..KernelOptions::default()
                },
                Box::new(SystemEnv),
            )
        })
        .expect("the kernel opens")
    };
    let logs: Arc<Mutex<Vec<LogEntry>>> = Arc::default();
    let sink: Logger = {
        let logs = Arc::clone(&logs);
        Arc::new(move |entry: &LogEntry| logs.lock().unwrap().push(entry.clone()))
    };
    let mut options =
        DaemonOptions::new(state, identity.initial_project(), dir.path().to_path_buf());
    if with_config {
        options.capstan = Some(Arc::new(role_config(dir.path())));
    }
    let clock = Arc::new(AtomicI64::new(1_000_000));
    let mut deps = Deps::new(kernel, options, sink);
    deps.commands = Arc::new(CommandState {
        replies: cache(Arc::clone(&clock)),
        ..CommandState::default()
    });
    World {
        _dir: dir,
        deps,
        agents,
        logs,
        clock,
    }
}

fn world() -> World {
    world_with(true, |clock| {
        ReplyCache::with_clock(move || clock.load(Ordering::SeqCst))
    })
}

fn frame(credential: &str, command: &str, args: &[&str], rid: Option<&str>) -> Vec<u8> {
    let mut request = json!({"v": 1, "credential": credential, "command": command, "args": args});
    if let Some(rid) = rid {
        request["rid"] = json!(rid);
    }
    request.to_string().into_bytes()
}

fn outcome(deps: &Deps, frame: &[u8]) -> FrameOutcome {
    handle_frame_with(
        deps,
        frame,
        &AbortSignal::new(),
        MAX_RESPONSE_BYTES,
        &mut |_, _| {},
    )
}

impl World {
    fn credential(&self, agent: &str) -> String {
        self.agents[agent].clone()
    }

    fn call(&self, agent: &str, command: &str, args: &[&str], rid: Option<&str>) -> Value {
        let bytes = frame(&self.credential(agent), command, args, rid);
        let out = outcome(&self.deps, &bytes);
        serde_json::from_str(&out.response.expect("an answer")).unwrap()
    }

    fn unread(&self, agent: &str) -> i64 {
        let credential = self.credential(agent);
        self.deps
            .kernel
            .run(move |core| core.unread_summary(&credential))
            .unwrap()["count"]
            .as_i64()
            .unwrap()
    }

    fn replies(&self) -> &ReplyCache {
        &self.deps.commands.replies
    }

    fn logged(&self, event: &str) -> usize {
        let wanted = format!("daemon:{event}");
        self.logs
            .lock()
            .unwrap()
            .iter()
            .filter(|entry| entry.command == wanted)
            .count()
    }
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out: {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

fn text(value: &Value) -> String {
    value.to_string()
}

#[test]
fn the_same_send_with_the_same_rid_is_one_message_and_one_answer() {
    let w = world();
    let first = w.call("pm-1", "send", &["dev-1", "hello"], Some("r-1"));
    let second = w.call("pm-1", "send", &["dev-1", "hello"], Some("r-1"));
    assert_eq!(first["ok"], true, "{first}");
    assert_eq!(text(&first), text(&second));
    assert_eq!(w.unread("dev-1"), 1);
    assert_eq!(w.logged("reply_cache_hit"), 1);
}

#[test]
fn a_retry_while_the_first_runs_waits_and_gets_the_same_text() {
    let w = Arc::new(world());
    let gate = w.replies().hold_next_dispatch();
    let first = {
        let w = Arc::clone(&w);
        thread::spawn(move || w.call("pm-1", "send", &["dev-1", "slow"], Some("slow-1")))
    };
    wait_until("the first attempt starts", || {
        w.replies().has_rid(&w.credential("pm-1"), "slow-1")
    });
    let second = {
        let w = Arc::clone(&w);
        thread::spawn(move || w.call("pm-1", "send", &["dev-1", "slow"], Some("slow-1")))
    };
    thread::sleep(Duration::from_millis(200));
    gate.release();
    let (first, second) = (first.join().unwrap(), second.join().unwrap());
    assert_eq!(first["ok"], true, "{first}");
    assert_eq!(text(&first), text(&second));
    assert_eq!(w.unread("dev-1"), 1);
}

#[test]
fn a_waiter_gives_up_after_its_wait_and_the_first_still_completes() {
    let w = Arc::new(world_with(true, |_| {
        ReplyCache::with_limits(1 << 20, 4096, 256, Duration::from_millis(50))
    }));
    let gate = w.replies().hold_next_dispatch();
    let first = {
        let w = Arc::clone(&w);
        thread::spawn(move || w.call("pm-1", "send", &["dev-1", "hang"], Some("hang-1")))
    };
    wait_until("the first attempt starts", || {
        w.replies().has_rid(&w.credential("pm-1"), "hang-1")
    });
    let started = Instant::now();
    let waiter = w.call("pm-1", "send", &["dev-1", "hang"], Some("hang-1"));
    assert!(started.elapsed() >= Duration::from_millis(50));
    assert_eq!(waiter["ok"], false);
    assert_eq!(waiter["code"], "attempt_running");
    assert_eq!(
        waiter["message"],
        "the first attempt is still running; check before running it again"
    );
    gate.release();
    assert_eq!(first.join().unwrap()["ok"], true);
    assert_eq!(w.unread("dev-1"), 1);
    // The retry after the first finished gets its answer.
    assert_eq!(
        w.call("pm-1", "send", &["dev-1", "hang"], Some("hang-1"))["ok"],
        true
    );
    assert_eq!(w.unread("dev-1"), 1);
}

#[test]
fn identical_sends_with_different_rids_at_once_are_two_messages() {
    let w = Arc::new(world());
    let gate = w.replies().hold_next_dispatch();
    let first = {
        let w = Arc::clone(&w);
        thread::spawn(move || w.call("pm-1", "send", &["dev-1", "twin"], Some("twin-1")))
    };
    wait_until("the first attempt starts", || {
        w.replies().has_rid(&w.credential("pm-1"), "twin-1")
    });
    let second = w.call("pm-1", "send", &["dev-1", "twin"], Some("twin-2"));
    gate.release();
    let first = first.join().unwrap();
    assert_eq!(first["ok"], true, "{first}");
    assert_eq!(second["ok"], true, "{second}");
    assert!(first["result"].get("repeat").is_none());
    assert!(second["result"].get("repeat").is_none());
    assert_ne!(first["result"]["messageId"], second["result"]["messageId"]);
    assert_eq!(w.unread("dev-1"), 2);
}

#[test]
fn one_caller_cannot_evict_another() {
    let w = world();
    let other = w.call("dev-1", "ping", &[], Some("other-0"));
    assert_eq!(other["ok"], true);
    for n in 0..257 {
        let rid = format!("many-{n}");
        assert_eq!(w.call("pm-1", "ping", &[], Some(&rid))["ok"], true);
    }
    let pm = w.credential("pm-1");
    assert!(
        !w.replies().has_rid(&pm, "many-0"),
        "the oldest of the caller goes"
    );
    assert!(w.replies().has_rid(&pm, "many-1"));
    assert!(w.replies().has_rid(&pm, "many-256"));
    assert!(w.replies().has_rid(&w.credential("dev-1"), "other-0"));
    assert_eq!(w.replies().len().0, 257);
}

#[test]
fn a_panicking_first_attempt_is_failed_for_its_waiter_and_a_retry_and_state_survives() {
    let w = Arc::new(world());
    let gate = w.replies().hold_next_dispatch();
    w.replies().panic_next_dispatch();
    let first = {
        let w = Arc::clone(&w);
        thread::spawn(move || w.call("pm-1", "send", &["dev-1", "boom"], Some("boom-1")))
    };
    wait_until("the first attempt starts", || {
        w.replies().has_rid(&w.credential("pm-1"), "boom-1")
    });
    let waiter = {
        let w = Arc::clone(&w);
        thread::spawn(move || w.call("pm-1", "send", &["dev-1", "boom"], Some("boom-1")))
    };
    thread::sleep(Duration::from_millis(200));
    gate.release();
    let first = first.join().unwrap();
    assert_eq!(first["ok"], false);
    assert_eq!(first["message"], "internal error");
    let waiter = waiter.join().unwrap();
    assert_eq!(waiter["code"], "attempt_failed", "{waiter}");
    let later = w.call("pm-1", "send", &["dev-1", "boom"], Some("boom-1"));
    assert_eq!(later["code"], "attempt_failed");
    assert_eq!(
        later["message"],
        "the first attempt failed inside the daemon; check before running it again"
    );
    assert_eq!(w.unread("dev-1"), 0, "no second execution");
    // The next status and send are served normally.
    assert_eq!(w.call("pm-1", "status", &[], None)["ok"], true);
    assert_eq!(
        w.call("pm-1", "send", &["dev-1", "after"], None)["ok"],
        true
    );
    assert_eq!(w.unread("dev-1"), 1);
}

#[test]
fn an_internal_error_is_not_stored_and_the_retry_runs() {
    let w = world();
    w.replies().kernel_panic_next_dispatch();
    let failed = w.call("pm-1", "send", &["dev-1", "kernel"], Some("kp-1"));
    assert_eq!(failed["ok"], false);
    assert_eq!(w.unread("dev-1"), 0);
    assert!(!w.replies().has_rid(&w.credential("pm-1"), "kp-1"));
    // The kernel thread survived: the retry executes once and commits once.
    let retry = w.call("pm-1", "send", &["dev-1", "kernel"], Some("kp-1"));
    assert_eq!(retry["ok"], true, "{retry}");
    assert_eq!(w.unread("dev-1"), 1);
    let again = w.call("pm-1", "send", &["dev-1", "kernel"], Some("kp-1"));
    assert_eq!(text(&retry), text(&again));
    assert_eq!(w.unread("dev-1"), 1);
}

#[test]
fn a_busy_or_shutting_down_answer_is_not_stored() {
    use capstan_daemon::handlers::replay::is_unstored;
    use capstan_daemon::handlers::shared::{fail, ErrorCode};
    assert!(is_unstored(&fail(ErrorCode::Busy, "x"), false));
    assert!(is_unstored(&fail(ErrorCode::ShuttingDown, "x"), false));
    assert!(is_unstored(&fail(ErrorCode::Error, "anything"), true));
    assert!(!is_unstored(
        &fail(ErrorCode::Error, "internal error"),
        false
    ));
    assert!(!is_unstored(&fail(ErrorCode::SelfSend, "x"), false));
}

#[test]
fn a_panicking_kernel_job_leaves_the_thread_serving_and_nothing_committed() {
    let w = world();
    let version = w.deps.kernel.run(|core| core.state_version()).unwrap();
    // A job that opens a write transaction, changes the schema and then panics.
    let gone = w.deps.kernel.call(|core| {
        core.kernel().database.exec("BEGIN IMMEDIATE").unwrap();
        core.kernel()
            .database
            .exec("CREATE TABLE panic_probe (x INTEGER)")
            .unwrap();
        panic!("injected kernel panic");
    });
    assert!(gone.is_err(), "the caller sees a closed kernel");
    // The next job runs, outside any transaction, and finds nothing of the first.
    let (tables, begins) = w
        .deps
        .kernel
        .call(|core| {
            let connection = core.kernel().database.connection();
            let tables: i64 = connection
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name = 'panic_probe'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let begins = core.kernel().database.exec("BEGIN IMMEDIATE").is_ok();
            let _ = core.kernel().database.exec("ROLLBACK");
            (tables, begins)
        })
        .expect("the kernel thread still serves");
    assert_eq!(tables, 0, "the transaction was rolled back");
    assert!(begins, "no transaction was left open");
    assert_eq!(
        w.deps.kernel.run(|core| core.state_version()).unwrap(),
        version
    );
    assert_eq!(
        w.call("pm-1", "send", &["dev-1", "after"], None)["ok"],
        true
    );
}

#[test]
fn a_spawn_answer_is_kept_never_replayed_as_text() {
    let w = world();
    let first = w.call("pm-1", "spawn", &["developer"], Some("sp-1"));
    assert!(first["code"].is_string(), "{first}");
    assert_ne!(first["code"], "answer_not_kept");
    assert!(w.replies().has_rid(&w.credential("pm-1"), "sp-1"));
    let again = w.call("pm-1", "spawn", &["developer"], Some("sp-1"));
    assert_eq!(again["ok"], false);
    assert_eq!(again["code"], "answer_not_kept");
    assert_eq!(
        again["message"],
        "the first attempt completed; its answer is not kept: check with cstan status"
    );
    assert_eq!(w.replies().stored_bytes(), 0);
}

#[test]
fn a_repeat_with_another_rid_inside_the_window_is_folded() {
    let w = world();
    let first = w.call("pm-1", "send", &["dev-1", "same"], Some("a-1"));
    w.clock.fetch_add(5_000, Ordering::SeqCst);
    let folded = w.call("pm-1", "send", &["dev-1", "same"], Some("a-2"));
    assert_eq!(folded["ok"], true, "{folded}");
    assert_eq!(w.unread("dev-1"), 1);
    let id = first["result"]["messageId"].as_str().unwrap();
    assert_eq!(folded["result"]["messageId"], id);
    assert_eq!(folded["result"]["repeat"], true);
    assert_eq!(folded["result"]["repeatAgeMs"], 5_000);
    assert_eq!(
        folded["result"]["note"],
        format!("not sent again: the same message went to dev-1 5 s ago (message {id})")
    );
    assert_eq!(w.logged("send_repeat_folded"), 1);
    // The retry of the folded request gets the folded answer.
    let retry = w.call("pm-1", "send", &["dev-1", "same"], Some("a-2"));
    assert_eq!(text(&folded), text(&retry));
    // Without a rid it folds too; past the window it is a new message.
    assert_eq!(
        w.call("pm-1", "send", &["dev-1", "same"], None)["result"]["repeat"],
        true
    );
    w.clock.fetch_add(60_000, Ordering::SeqCst);
    let fresh = w.call("pm-1", "send", &["dev-1", "same"], Some("a-3"));
    assert_eq!(fresh["ok"], true);
    assert!(fresh["result"].get("repeat").is_none(), "{fresh}");
    assert_eq!(w.unread("dev-1"), 2);
}

#[test]
fn a_user_question_and_a_plain_send_with_one_body_are_not_folded() {
    let w = world();
    let plain = w.call("pm-1", "send", &["@user", "same body"], None);
    let action = w.call("pm-1", "send", &["--action", "@user", "same body"], None);
    assert_eq!(plain["ok"], true, "{plain}");
    assert_eq!(action["ok"], true, "{action}");
    assert!(plain["result"].get("repeat").is_none());
    assert!(action["result"].get("repeat").is_none());
    assert_ne!(plain["result"]["messageId"], action["result"]["messageId"]);
}

#[test]
fn only_send_is_folded() {
    let w = world();
    let first = w.call("pm-1", "pause", &["--reason", "again"], None);
    assert_eq!(first["ok"], true, "{first}");
    // The second ran: the run was already paused (a folded answer would have repeated the first).
    let second = w.call("pm-1", "pause", &["--reason", "again"], None);
    assert_eq!(second["code"], "conflict", "{second}");
    assert_eq!(w.logged("send_repeat_folded"), 0);
}

#[test]
fn two_credentials_never_share_an_entry() {
    let w = world();
    let one = w.call("dev-1", "send", &["@pm", "shared text"], Some("same-rid"));
    let two = w.call("dev-2", "send", &["@pm", "shared text"], Some("same-rid"));
    assert_eq!(one["ok"], true, "{one}");
    assert_eq!(two["ok"], true, "{two}");
    assert_ne!(one["result"]["messageId"], two["result"]["messageId"]);
    assert!(two["result"].get("repeat").is_none());
    assert_eq!(w.unread("pm-1"), 2);
}

#[test]
fn a_large_answer_is_kept_not_stored() {
    let w = world();
    for n in 0..6 {
        let body = format!("{n}{}", "x".repeat(14_000));
        let sent = w.call("pm-1", "send", &["dev-1", &body], None);
        assert_eq!(sent["ok"], true, "{sent}");
    }
    let first = w.call("dev-1", "inbox", &[], Some("big-1"));
    assert_eq!(first["ok"], true, "{}", first.to_string().len());
    assert!(first.to_string().len() > 64 * 1024);
    assert_eq!(w.replies().stored_bytes(), 0);
    assert_eq!(
        w.call("dev-1", "inbox", &[], Some("big-1"))["code"],
        "answer_not_kept"
    );
}

#[test]
fn past_the_byte_limit_the_oldest_is_evicted() {
    let w = world_with(true, |_| {
        ReplyCache::with_limits(300, 4096, 256, Duration::from_millis(50))
    });
    for n in 0..20 {
        let rid = format!("b-{n}");
        assert_eq!(w.call("pm-1", "ping", &[], Some(&rid))["ok"], true);
    }
    assert!(w.replies().stored_bytes() <= 300);
    let pm = w.credential("pm-1");
    assert!(!w.replies().has_rid(&pm, "b-0"));
    assert!(w.replies().has_rid(&pm, "b-19"));
}

#[test]
fn a_fresh_command_state_is_empty() {
    assert!(CommandState::new().replies.is_empty());
    assert!(CommandState::default().replies.is_empty());
    assert!(world().replies().is_empty());
}

#[test]
fn a_rid_reused_for_another_request_is_refused_and_runs_nothing() {
    let w = world();
    assert_eq!(
        w.call("pm-1", "send", &["dev-1", "one"], Some("re-1"))["ok"],
        true
    );
    let other = w.call("pm-1", "send", &["dev-1", "two"], Some("re-1"));
    assert_eq!(other["code"], "invalid_request");
    assert_eq!(
        other["message"],
        "request id reused for a different request"
    );
    assert_eq!(w.unread("dev-1"), 1);
}

#[test]
fn a_bad_rid_and_an_unknown_member_are_ignored() {
    let w = world();
    let plain = w.call("pm-1", "ping", &[], None);
    for rid in ["", "has space", &"x".repeat(65), "bad/rid"] {
        let answer = w.call("pm-1", "ping", &[], Some(rid));
        assert_eq!(text(&plain), text(&answer));
    }
    assert!(w.replies().is_empty());
    let mut request: Value =
        serde_json::from_slice(&frame(&w.credential("pm-1"), "ping", &[], None)).unwrap();
    request["surprise"] = json!({"a": [1, 2]});
    let out = outcome(&w.deps, request.to_string().as_bytes());
    assert_eq!(
        text(&plain),
        text(&serde_json::from_str(&out.response.unwrap()).unwrap())
    );
    assert!(out.identified);
}

#[test]
fn a_run_without_a_config_never_folds_a_send() {
    let w = world_with(false, |_| ReplyCache::new());
    assert_eq!(
        capstan_config::daemon_limits(w.deps.options.capstan.as_deref()).repeat_window_seconds,
        0
    );
    assert_eq!(
        w.call("pm-1", "send", &["dev-1", "twice"], None)["ok"],
        true
    );
    let second = w.call("pm-1", "send", &["dev-1", "twice"], None);
    assert!(second["result"].get("repeat").is_none());
    assert_eq!(w.unread("dev-1"), 2);
}

#[test]
fn the_outcome_says_whether_the_credential_was_identified() {
    let w = world();
    let good = outcome(&w.deps, &frame(&w.credential("pm-1"), "ping", &[], None));
    assert!(good.identified);
    let bad = outcome(&w.deps, &frame("not-a-credential", "ping", &[], None));
    assert!(!bad.identified);
    let junk = outcome(&w.deps, b"not json");
    assert!(!junk.identified);
}

/// Starts `n` held first attempts of one caller (each its own rid) and returns their threads and gates.
fn hold_many(
    w: &Arc<World>,
    agent: &'static str,
    prefix: &str,
    n: usize,
) -> Vec<(
    thread::JoinHandle<Value>,
    capstan_daemon::handlers::replay::Gate,
)> {
    let mut held = Vec::new();
    for i in 0..n {
        let gate = w.replies().hold_next_dispatch();
        let rid = format!("{prefix}-{i}");
        let handle = {
            let (w, rid) = (Arc::clone(w), rid.clone());
            thread::spawn(move || w.call(agent, "ping", &[], Some(&rid)))
        };
        wait_until("a held attempt starts", || {
            w.replies().has_rid(&w.credential(agent), &rid)
        });
        wait_until("the held attempt takes its gate", || {
            w.replies().hold_taken()
        });
        held.push((handle, gate));
    }
    held
}

#[test]
fn a_full_cap_of_running_attempts_evicts_none_and_the_next_runs_uncached() {
    // The per-caller cap: 256 running attempts of one caller.
    let w = Arc::new(world());
    let held = hold_many(&w, "pm-1", "run", 256);
    let extra = w.call("pm-1", "ping", &[], Some("run-256"));
    assert_eq!(extra["ok"], true);
    let pm = w.credential("pm-1");
    for i in 0..256 {
        assert!(
            w.replies().has_rid(&pm, &format!("run-{i}")),
            "run-{i} was evicted"
        );
    }
    assert!(
        !w.replies().has_rid(&pm, "run-256"),
        "the 257th ran uncached"
    );
    assert_eq!(w.logged("reply_cache_uncached"), 1);
    for (handle, gate) in held {
        gate.release();
        assert_eq!(handle.join().unwrap()["ok"], true);
    }
}

#[test]
fn a_full_global_cap_of_running_attempts_evicts_none_and_the_next_runs_uncached() {
    let w = Arc::new(world_with(true, |_| {
        ReplyCache::with_limits(1 << 20, 3, 256, Duration::from_millis(50))
    }));
    let held = hold_many(&w, "pm-1", "g", 3);
    let other = w.call("dev-1", "ping", &[], Some("g-other"));
    assert_eq!(other["ok"], true);
    for i in 0..3 {
        assert!(w
            .replies()
            .has_rid(&w.credential("pm-1"), &format!("g-{i}")));
    }
    assert!(!w.replies().has_rid(&w.credential("dev-1"), "g-other"));
    for (handle, gate) in held {
        gate.release();
        handle.join().unwrap();
    }
}

/// The reply cache decides "not stored" from a mark the kernel handle sets on the thread that saw the kernel gone. That is
/// sound while no handler lets another thread's kernel failure reach its answer. The handlers that start a thread are
/// these three, and none of them feeds its answer from the thread: `close-operator-picker` and `close-answered-picker`
/// (decide.rs) only log a launcher call, and `decide-merge-hand-off` (decide_merge.rs) runs after the answer is built;
/// `decide` and `decide-merge` are in `KEPT_COMMANDS`, so their answers are never stored as text anyway. A new thread
/// start in a handler fails this test until it is reviewed and listed here.
#[test]
fn no_handler_answers_from_a_kernel_call_on_another_thread() {
    let dir = common::crate_dir().join("src").join("handlers");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        let production = source.split("#[cfg(test)]").next().unwrap();
        let count = production.matches("thread::Builder").count()
            + production.matches("thread::spawn").count()
            + production.matches("thread::scope").count();
        if count > 0 {
            found.push((
                path.file_name().unwrap().to_string_lossy().to_string(),
                count,
            ));
        }
    }
    found.sort();
    assert_eq!(
        found,
        vec![
            ("decide.rs".to_string(), 2),
            ("decide_merge.rs".to_string(), 1)
        ]
    );
}
