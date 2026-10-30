//! The decide-merge route under concurrency, crashes and recovery: held merges, second calls, a closed client, stops
//! and the retries that finish a merge. Every case installs its seams on its own `MergeProject` (own tempdir, own socket,
//! own `Deps`). Cases (4), (17), (18) and (22) form a serial group (`SERIAL`); the others run on parallel threads. Every
//! wait polls state that can be observed every 50 ms against a deadline; the deadlines bound a failure, they are not
//! how long a case takes.

mod common;

use capstan_daemon::deps::{LogEntry, Logger};
use capstan_daemon::handlers::decide_merge_seams::{install_seams, DecideMergeSeams, Hold};
use capstan_daemon::handlers::REQUEST_TIMEOUT_MS;
use capstan_kernel::areas::snapshot_feed::content_hash;
use capstan_kernel::integrate::MergeSeams;
use common::merge::MergeProject;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const MARKER: &str = "capstan-remote-merge";
/// The failure bound of a wait in a case, and in the serial group.
const DEADLINE: Duration = Duration::from_secs(30);
const SERIAL_DEADLINE: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(50);

/// The cases that use a real socket for timing or a closed client run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|p| p.into_inner())
}

// ------------------------------------------------------------------------------------------------ helpers

fn git_in(directory: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(directory)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("LC_ALL", "C")
        .env("HOME", "/nonexistent")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Merge Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Merge Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn git(project: &MergeProject, args: &[&str]) -> String {
    git_in(project.dir.path(), args)
}

fn git_path(project: &MergeProject, name: &str) -> PathBuf {
    project
        .dir
        .path()
        .join(git(project, &["rev-parse", "--git-path", name]))
}

/// A merge project whose remote names `main` as its default branch, so the merge has a target.
fn new_project() -> MergeProject {
    let project = MergeProject::new();
    git(
        &project,
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
    git(
        &project,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    project
}

fn hold() -> (Sender<()>, Option<Hold>) {
    let (sender, receiver) = channel();
    (sender, Some(Mutex::new(receiver)))
}

fn install(project: &MergeProject, seams: DecideMergeSeams) {
    install_seams(&project.deps, seams);
}

/// Polls `done` every 50 ms until it is true; a failure after `limit`.
fn wait_for(what: &str, limit: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(POLL);
    }
}

/// The result of a thread that has finished, within `limit`.
fn join_within<T>(handle: JoinHandle<T>, what: &str, limit: Duration) -> T {
    wait_for(what, limit, || handle.is_finished());
    handle.join().expect("the thread did not panic")
}

fn lock_held(project: &MergeProject) -> bool {
    matches!(
        project.deps.commands.remote_merge.try_lock(),
        Err(TryLockError::WouldBlock)
    )
}

/// Waits until a merge is inside the merge lock (so at `hold_b` when that is installed).
fn wait_lock(project: &MergeProject, limit: Duration) {
    wait_for("the merge lock to be taken", limit, || lock_held(project));
}

fn wait_lock_free(project: &MergeProject, limit: Duration) {
    wait_for("the merge lock to be free", limit, || !lock_held(project));
}

fn socket_frame(credential: &str, command: &str, args: &[String]) -> String {
    format!(
        "{}\n",
        json!({"v": 1, "credential": credential, "command": command, "args": args})
    )
}

/// A request over a fresh connection to the socket, from any thread.
fn socket_call(socket: &PathBuf, credential: &str, command: &str, args: &[String]) -> Value {
    let mut stream = UnixStream::connect(socket).expect("the socket accepts");
    stream
        .write_all(socket_frame(credential, command, args).as_bytes())
        .unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap_or_else(|e| panic!("{line:?}: {e}"))
}

/// The merge of `argv` on another thread over the socket.
fn merge_argv_in_thread(project: &MergeProject, argv: Vec<String>) -> JoinHandle<Value> {
    let socket = project.socket.clone();
    let credential = project.credentials["operator"].clone();
    std::thread::spawn(move || socket_call(&socket, &credential, "decide-merge", &argv))
}

fn merge_in_thread(project: &MergeProject, nonce: &str) -> JoinHandle<Value> {
    merge_argv_in_thread(project, project.decide_merge_argv(nonce))
}

fn applied(answer: &Value) -> &Value {
    assert_eq!(answer["ok"], true, "{answer}");
    let result = &answer["result"];
    assert_eq!(result["outcome"], "applied", "{answer}");
    result
}

fn code(answer: &Value) -> String {
    assert_eq!(answer["ok"], false, "{answer}");
    answer["code"].as_str().unwrap().to_string()
}

fn message(answer: &Value) -> String {
    answer["message"].as_str().unwrap_or_default().to_string()
}

fn decided(project: &MergeProject, nonce: &str) -> bool {
    let nonce = nonce.to_string();
    project
        .deps
        .kernel
        .run(move |core| capstan_kernel::areas::remote_decisions::decision_for_nonce(core, &nonce))
        .unwrap()
        .is_some()
}

/// How many decision rows exist for `integration`.
fn rows_of(project: &MergeProject, integration: &str) -> i64 {
    let path =
        capstan_ledger::resolve_database_path(&project.deps.options.state_directory).unwrap();
    let conn = rusqlite::Connection::open(path).expect("the ledger opens");
    conn.busy_timeout(Duration::from_secs(10)).unwrap();
    conn.query_row(
        "SELECT count(*) FROM remote_decisions WHERE request_id = ?",
        [format!("integration:{integration}")],
        |row| row.get(0),
    )
    .unwrap()
}

fn rows(project: &MergeProject) -> i64 {
    rows_of(project, &project.integration_id)
}

fn merge_commits(project: &MergeProject) -> i64 {
    git(project, &["rev-list", "--merges", "--count", "HEAD"])
        .parse()
        .unwrap()
}

/// Waits until `count` PM messages about a merge from a remote client exist; the text of the last one.
fn wait_merge_message(project: &MergeProject, count: usize) -> String {
    let mut text = String::new();
    wait_for("the PM's merge message", DEADLINE, || {
        let found: Vec<String> = project
            .pm_messages()
            .iter()
            .map(Value::to_string)
            .filter(|text| text.contains("from a remote client"))
            .collect();
        if found.len() >= count {
            text = found.last().unwrap().clone();
            true
        } else {
            false
        }
    });
    text
}

/// Nothing changed: HEAD, the integration, the rows and the PM's messages.
struct Snapshot {
    head: String,
    state: Value,
    messages: usize,
}

impl Snapshot {
    fn of(project: &MergeProject) -> Snapshot {
        Snapshot {
            head: project.rev("HEAD"),
            state: project.integration(),
            messages: project.pm_messages().len(),
        }
    }

    fn assert_same(&self, project: &MergeProject, nonce: &str) {
        assert_eq!(project.rev("HEAD"), self.head, "HEAD moved");
        assert_eq!(project.integration(), self.state, "the integration changed");
        assert_eq!(
            project.pm_messages().len(),
            self.messages,
            "a PM message was sent"
        );
        assert!(!decided(project, nonce), "a row was written");
        assert_eq!(rows(project), 0, "a row was written");
        assert!(!git_path(project, MARKER).exists(), "the marker is left");
    }
}

/// An `integrations` row in state `running`, as another integration in progress would leave it.
fn insert_running_integration(project: &MergeProject) -> impl Fn() + Send + Sync + 'static {
    let path =
        capstan_ledger::resolve_database_path(&project.deps.options.state_directory).unwrap();
    let source = project.integration_id.clone();
    move || {
        let conn = rusqlite::Connection::open(&path).expect("the ledger opens");
        conn.busy_timeout(Duration::from_secs(10)).unwrap();
        let names: Vec<String> = conn
            .prepare("PRAGMA table_info(integrations)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let select: Vec<String> = names
            .iter()
            .map(|name| match name.as_str() {
                "integration_id" => "'running-other'".to_string(),
                "sequence" => "(SELECT max(sequence) + 1 FROM integrations)".to_string(),
                "branch" => "'integration/running-other'".to_string(),
                "state" => "'running'".to_string(),
                "head_sha" | "completed_at" => "NULL".to_string(),
                other => other.to_string(),
            })
            .collect();
        conn.execute(
            &format!(
                "INSERT INTO integrations ({}) SELECT {} FROM integrations WHERE integration_id = ?",
                names.join(", "),
                select.join(", ")
            ),
            [&source],
        )
        .expect("a running integration is added");
    }
}

/// A review round on `subject` ended with `verdict`, through `deps` (usable from a seam on any thread).
fn review_with(
    deps: &capstan_daemon::deps::Deps,
    pm: &str,
    reviewer: &str,
    subject: &str,
    verdict: &str,
) {
    let run = |credential: &str, op: &str, arg: Value| {
        let (credential, op) = (credential.to_string(), op.to_string());
        deps.kernel
            .run(move |core| {
                let context = capstan_daemon::deps::new_context(core, &credential)?;
                core.dispatch(&op, &[serde_json::to_value(context).unwrap(), arg])
            })
            .unwrap_or_else(|e| panic!("review: {e}"))
    };
    run(
        pm,
        "beginReview",
        json!({"subjectId": subject, "reviewerRole": "reviewer", "reviewerAgentId": "rev-1"}),
    );
    run(
        reviewer,
        "completeReview",
        json!({"verdict": verdict, "text": "Reviewed."}),
    );
}

fn review(project: &MergeProject, subject: &str, verdict: &str) {
    review_with(
        &project.deps,
        &project.credentials["pm-1"],
        &project.credentials["rev-1"],
        subject,
        verdict,
    );
}

/// A second developer's branch, reported and passed in review; the id of its report.
fn second_report(project: &MergeProject) -> (String, String) {
    let root = project.dir.path();
    let main = project.rev("main");
    git_in(root, &["checkout", "--quiet", "-b", "feat/dev-2", "main"]);
    std::fs::write(root.join("two.txt"), "two\n").unwrap();
    git_in(root, &["add", "two.txt"]);
    git_in(root, &["commit", "--quiet", "-m", "feat: two"]);
    let sha = git_in(root, &["rev-parse", "HEAD"]);
    git_in(root, &["checkout", "--quiet", "main"]);

    let owner = project.credentials["operator"].clone();
    let seat = "seat-dev-2";
    project.ledger(
        &owner,
        "createSeat",
        vec![json!({"seatId": seat, "name": seat, "role": "Developer"})],
    );
    let actor = project.ledger(
        &owner,
        "createActor",
        vec![json!({"displayName": "dev-2", "role": "Developer", "seatId": seat})],
    );
    project.ledger(
        &owner,
        "registerAgent",
        vec![json!({"agentId": "dev-2", "roleName": "developer", "seatId": seat, "actorId": actor["actorId"]})],
    );
    let credential = actor["credential"].as_str().unwrap().to_string();
    let internal = project.deps.credential().to_string();
    project.ledger(
        &internal,
        "recordAgentPane",
        vec![json!({
            "agentId": "dev-2",
            "workspaceId": "ws-1",
            "paneId": "pane-dev-2",
            "worktreePath": "/tmp/wt/dev-2",
            "branch": "feat/dev-2",
            "baseSha": main,
        })],
    );
    let evidence = json!({
        "generation": 1,
        "branch": "feat/dev-2",
        "baseSha": main,
        "commitExists": true,
        "branchTip": sha,
        "isAncestorOfTip": true,
        "isAncestorOfBase": false,
        "checkedAt": "2026-01-01T00:00:00.000Z",
    });
    let reported = project.ledger(
        &credential,
        "recordAgentReport",
        vec![json!({"commitSha": sha, "summary": "Two", "evidence": evidence})],
    );
    let report = reported["record"]["reportId"].as_str().unwrap().to_string();
    review(project, &report, "pass");
    (report, credential)
}

/// A passed verify run for `subject` with the commands in `capstan.toml` now.
fn verify_passed(project: &MergeProject, subject: &str) {
    let owner = project.credentials["operator"].clone();
    let internal = project.deps.credential().to_string();
    let config = capstan_config::load_role_config(project.dir.path()).unwrap();
    let verify = config.verify.expect("the project has a [verify] section");
    let queued = project.ledger(
        &owner,
        "queueVerifyRun",
        vec![json!({
            "subjectId": subject,
            "commands": verify.commands,
            "commandsSha": verify.commands_sha(),
            "trigger": "manual",
        })],
    );
    let run = queued["runId"].as_str().unwrap().to_string();
    let scratch = project.dir.path().join(".capstan").join("second-verify");
    let claimed = project.ledger(
        &internal,
        "claimVerifyRun",
        vec![json!({
            "runId": run,
            "maxParallel": 1,
            "scratchPath": scratch.join(&run).to_string_lossy(),
            "logPath": scratch.join(format!("{run}.log")).to_string_lossy(),
        })],
    );
    assert_eq!(claimed["claimed"], true, "{claimed}");
    project.ledger(
        &internal,
        "finishVerifyRun",
        vec![json!({"runId": run, "state": "passed", "exitCode": 0, "durationMs": 1})],
    );
}

/// The argv of the merge of `integration` for `nonce`, with the hash of the waiting item as it is now.
fn merge_argv(project: &MergeProject, integration: &str, nonce: &str) -> Vec<String> {
    let request = format!("integration:{integration}");
    let hash = project
        .deps
        .kernel
        .run({
            let request = request.clone();
            move |core| content_hash(core, &request)
        })
        .expect("the feed is read")
        .map_or_else(|| "0".repeat(12), |hash| hash[..12].to_string());
    vec![
        request,
        "--expect".into(),
        hash,
        "--nonce".into(),
        nonce.into(),
        "merge".into(),
    ]
}

/// A second integration, merged by the PM, reviewed and verified: its id.
fn second_integration(project: &MergeProject) -> String {
    let (report, _) = second_report(project);
    let merged = project.ok("pm-1", "integrate", &[&report]);
    assert_eq!(merged["state"], "merged", "{merged}");
    let integration = merged["integrationId"].as_str().unwrap().to_string();
    review(project, &integration, "pass");
    verify_passed(project, &integration);
    integration
}

/// The thread of a seam that waits for a message and says it has been reached.
struct Gate {
    reached: Receiver<()>,
    release: Sender<()>,
}

/// An `after_a` that lets the calls before call number `at` (counted from 0) pass and holds call `at` until released.
fn after_a_gate(at: usize) -> (Gate, Box<dyn Fn() + Send + Sync>) {
    let (reached_tx, reached) = channel();
    let (release, release_rx) = channel::<()>();
    let reached_tx = Mutex::new(reached_tx);
    let release_rx = Mutex::new(release_rx);
    let count = AtomicUsize::new(0);
    let hook = Box::new(move || {
        if count.fetch_add(1, Ordering::SeqCst) == at {
            let _ = reached_tx.lock().unwrap().send(());
            let _ = release_rx.lock().unwrap().recv();
        }
    });
    (Gate { reached, release }, hook)
}

fn gate_reached(gate: &Gate, what: &str, limit: Duration) {
    wait_for(what, limit, || gate.reached.try_recv().is_ok());
}

// ------------------------------------------------------------------------------------------------ phase A and B

#[test]
fn an_integration_started_after_phase_a_makes_the_merge_busy_and_writes_nothing() {
    let project = new_project();
    install(
        &project,
        DecideMergeSeams {
            after_a: Some(Box::new(insert_running_integration(&project))),
            ..DecideMergeSeams::default()
        },
    );
    let before = Snapshot::of(&project);
    let answer = project.decide_merge("nonce-001");
    assert_eq!(code(&answer), "busy", "{answer}");
    before.assert_same(&project, "nonce-001");
}

#[test]
fn a_held_merge_answers_other_calls_busy_and_repeats_after_the_release() {
    let project = new_project();
    let (release, hold_b) = hold();
    install(
        &project,
        DecideMergeSeams {
            hold_b,
            ..DecideMergeSeams::default()
        },
    );
    let argv = project.decide_merge_argv("nonce-first");
    let first = merge_argv_in_thread(&project, argv.clone());
    wait_lock(&project, DEADLINE);
    // A call that is not a merge is not held.
    let head = project.ok("operator", "feed", &["head"]);
    assert!(head.is_object(), "{head}");
    let other = project.decide_merge("nonce-other");
    assert_eq!(code(&other), "busy", "{other}");
    let same = project.decide_merge("nonce-first");
    assert_eq!(code(&same), "busy", "{same}");
    assert!(!decided(&project, "nonce-first"));
    assert_eq!(rows(&project), 0);
    release.send(()).unwrap();
    let answer = join_within(first, "the held merge", DEADLINE);
    assert_eq!(applied(&answer)["repeat"], false, "{answer}");
    let repeat = project.decide_merge_args(&argv.iter().map(String::as_str).collect::<Vec<_>>());
    assert_eq!(applied(&repeat)["repeat"], true, "{repeat}");
    let after = project.decide_merge("nonce-other");
    assert_eq!(code(&after), "not_waiting", "{after}");
    assert_eq!(rows(&project), 1);
    assert_eq!(merge_commits(&project), 1);
}

#[test]
fn an_op_approve_decide_during_a_held_merge_answers_at_once() {
    let project = new_project();
    let (_, proposer) = second_report(&project);
    let proposal = project.ledger(
        &proposer,
        "proposeOperatorAction",
        vec![json!({"kind": "command", "command": "git push origin main", "reason": "r", "maxPending": 20})],
    );
    let request = format!("op:{}", proposal["proposalId"].as_str().unwrap());
    let hash = project
        .deps
        .kernel
        .run({
            let request = request.clone();
            move |core| content_hash(core, &request)
        })
        .unwrap()
        .expect("the proposal waits")[..12]
        .to_string();
    let (release, hold_b) = hold();
    install(
        &project,
        DecideMergeSeams {
            hold_b,
            ..DecideMergeSeams::default()
        },
    );
    let merge = merge_in_thread(&project, "nonce-merge");
    wait_lock(&project, DEADLINE);
    let started = Instant::now();
    let answer = project.call(
        "operator",
        "decide",
        &[
            &request, "--expect", &hash, "--nonce", "nonce-op", "approve",
        ],
    );
    assert_eq!(answer["ok"], true, "{answer}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the decide waited for the merge"
    );
    assert!(lock_held(&project), "the merge was not released by it");
    release.send(()).unwrap();
    applied(&join_within(merge, "the held merge", DEADLINE));
}

#[test]
fn a_held_merge_over_the_socket_outlives_the_request_timeout_and_a_closed_client_does_not_stop_it()
{
    let _serial = serial();
    // The answer arrives after the request timeout of ordinary commands.
    let project = new_project();
    let (release, hold_b) = hold();
    install(
        &project,
        DecideMergeSeams {
            hold_b,
            ..DecideMergeSeams::default()
        },
    );
    let started = Instant::now();
    let first = merge_in_thread(&project, "nonce-slow");
    wait_lock(&project, SERIAL_DEADLINE);
    let beyond = Duration::from_millis(REQUEST_TIMEOUT_MS) + Duration::from_secs(1);
    wait_for("the request timeout to pass", SERIAL_DEADLINE, || {
        started.elapsed() >= beyond
    });
    assert!(!first.is_finished(), "the request ended at its timeout");
    release.send(()).unwrap();
    let answer = join_within(first, "the held merge", SERIAL_DEADLINE);
    assert!(started.elapsed() >= Duration::from_millis(REQUEST_TIMEOUT_MS));
    applied(&answer);

    // A client that closes during the hold: the merge goes on, and its retry repeats.
    let project = new_project();
    let (release, hold_b) = hold();
    install(
        &project,
        DecideMergeSeams {
            hold_b,
            ..DecideMergeSeams::default()
        },
    );
    let argv = project.decide_merge_argv("nonce-closed");
    let stream = {
        let mut stream = UnixStream::connect(&project.socket).expect("the socket accepts");
        stream
            .write_all(
                socket_frame(&project.credentials["operator"], "decide-merge", &argv).as_bytes(),
            )
            .unwrap();
        stream
    };
    wait_lock(&project, SERIAL_DEADLINE);
    drop(stream);
    release.send(()).unwrap();
    wait_for("the row of the merge", SERIAL_DEADLINE, || {
        decided(&project, "nonce-closed")
    });
    wait_lock_free(&project, SERIAL_DEADLINE);
    assert_eq!(merge_commits(&project), 1);
    let credential = project.credentials["operator"].clone();
    let retry = socket_call(&project.socket, &credential, "decide-merge", &argv);
    assert_eq!(applied(&retry)["repeat"], true, "{retry}");
    assert_eq!(rows(&project), 1);
    assert_eq!(merge_commits(&project), 1);
}

#[test]
fn a_second_call_held_after_phase_a_repeats_when_the_first_completes() {
    let project = new_project();
    let (release_b, hold_b) = hold();
    let (gate, after_a) = after_a_gate(1);
    install(
        &project,
        DecideMergeSeams {
            hold_b,
            after_a: Some(after_a),
            ..DecideMergeSeams::default()
        },
    );
    let first = merge_in_thread(&project, "nonce-same");
    wait_lock(&project, DEADLINE);
    let second = merge_in_thread(&project, "nonce-same");
    gate_reached(&gate, "the second call to pass phase A", DEADLINE);
    release_b.send(()).unwrap();
    applied(&join_within(first, "the first merge", DEADLINE));
    gate.release.send(()).unwrap();
    let answer = join_within(second, "the second call", DEADLINE);
    assert_eq!(applied(&answer)["repeat"], true, "{answer}");
    assert_eq!(rows(&project), 1);
    assert_eq!(merge_commits(&project), 1);
}

// ------------------------------------------------------------------------------------------------ crashes and recovery

#[test]
fn a_crash_after_phase_b_is_finished_by_the_same_nonce() {
    let project = new_project();
    install(
        &project,
        DecideMergeSeams {
            crash_after_b: true,
            ..DecideMergeSeams::default()
        },
    );
    let answer = project.decide_merge("nonce-crash");
    assert_eq!(answer["ok"], false, "{answer}");
    assert!(!decided(&project, "nonce-crash"));
    install(&project, DecideMergeSeams::default());
    let retry = project.decide_merge("nonce-crash");
    let result = applied(&retry);
    assert_eq!(result["merge"]["alreadyInHead"], true, "{retry}");
    assert_eq!(merge_commits(&project), 1);
    assert!(!git_path(&project, MARKER).exists());
}

#[test]
fn a_confirm_by_the_pm_after_a_crash_is_the_record_of_the_merge() {
    let project = new_project();
    install(
        &project,
        DecideMergeSeams {
            crash_after_b: true,
            ..DecideMergeSeams::default()
        },
    );
    let argv = project.decide_merge_argv("nonce-crash");
    let args: Vec<&str> = argv.iter().map(String::as_str).collect();
    let answer = project.decide_merge_args(&args);
    assert_eq!(answer["ok"], false, "{answer}");
    install(&project, DecideMergeSeams::default());
    let merged = project.rev("HEAD");
    // The PM's integrate confirm is the record of this merge: the integration is no longer waiting for a merge, so the
    // same nonce is refused and nothing more is written.
    project.ok("pm-1", "integrate", &["confirm", &project.integration_id]);
    assert_eq!(project.integration()["state"], "confirmed");
    let messages = project.pm_messages().len();
    let retry = project.decide_merge_args(&args);
    assert_eq!(code(&retry), "not_waiting", "{retry}");
    assert_eq!(rows(&project), 0);
    assert_eq!(project.rev("HEAD"), merged);
    assert_eq!(merge_commits(&project), 1);
    assert!(!git_path(&project, MARKER).exists());
    assert_eq!(project.pm_messages().len(), messages);
}

#[test]
fn a_failed_settle_tells_the_pm_and_a_new_nonce_adds_nothing() {
    let project = new_project();
    install(
        &project,
        DecideMergeSeams {
            fail_settle: true,
            ..DecideMergeSeams::default()
        },
    );
    let answer = project.decide_merge("nonce-settle");
    assert_eq!(applied(&answer)["merge"]["confirm"], "failed", "{answer}");
    let text = wait_merge_message(&project, 1);
    assert!(text.contains("integrate confirm"), "{text}");
    let messages = project.pm_messages().len();
    let other = project.decide_merge("nonce-settle-2");
    assert_eq!(code(&other), "not_waiting", "{other}");
    assert_eq!(rows(&project), 1);
    assert_eq!(project.pm_messages().len(), messages);
}

#[test]
fn a_failed_record_leaves_the_merge_and_the_same_nonce_records_it() {
    let project = new_project();
    install(
        &project,
        DecideMergeSeams {
            fail_record: true,
            ..DecideMergeSeams::default()
        },
    );
    let answer = project.decide_merge("nonce-record");
    assert_eq!(code(&answer), "merge_unrecorded", "{answer}");
    assert_eq!(project.integration()["state"], "merged");
    let merged = project.rev("HEAD");
    install(&project, DecideMergeSeams::default());
    let retry = project.decide_merge("nonce-record");
    let result = applied(&retry);
    assert_eq!(result["merge"]["alreadyInHead"], true, "{retry}");
    assert_eq!(result["merge"]["mergeCommit"], merged.as_str(), "{retry}");
    assert_eq!(merge_commits(&project), 1);

    // Findings on the integration before the retry: it is no longer reviewed.
    let project = new_project();
    install(
        &project,
        DecideMergeSeams {
            fail_record: true,
            ..DecideMergeSeams::default()
        },
    );
    let answer = project.decide_merge("nonce-findings");
    assert_eq!(code(&answer), "merge_unrecorded", "{answer}");
    install(&project, DecideMergeSeams::default());
    project.set_review_findings();
    let retry = project.decide_merge("nonce-findings");
    assert_eq!(code(&retry), "not_reviewed", "{retry}");
    assert!(!decided(&project, "nonce-findings"));
}

#[test]
fn a_failed_record_answers_even_when_the_pm_message_is_blocked() {
    let project = new_project();
    let (release, block) = hold();
    install(
        &project,
        DecideMergeSeams {
            fail_record: true,
            block_pm_message: block,
            ..DecideMergeSeams::default()
        },
    );
    let started = Instant::now();
    let answer = project.decide_merge("nonce-blocked");
    assert_eq!(code(&answer), "merge_unrecorded", "{answer}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the answer waited"
    );
    // No hand-off thread exists to be released, so no message follows.
    let messages = project.pm_messages().len();
    release.send(()).unwrap();
    assert_eq!(project.pm_messages().len(), messages);
}

#[test]
fn a_panic_in_phase_b_leaves_no_marker_and_the_next_merge_locks() {
    let project = new_project();
    install(
        &project,
        DecideMergeSeams {
            panic_in_b: true,
            ..DecideMergeSeams::default()
        },
    );
    let head = project.rev("HEAD");
    let answer = project.decide_merge("nonce-panic");
    assert_eq!(code(&answer), "merge_failed", "{answer}");
    assert!(message(&answer).contains("internal"), "{answer}");
    assert!(!git_path(&project, MARKER).exists());
    assert_eq!(project.rev("HEAD"), head);
    // The lock is free again: the next merge takes it and applies.
    assert!(!lock_held(&project));
    install(&project, DecideMergeSeams::default());
    applied(&project.decide_merge("nonce-after"));
}

#[test]
fn a_crash_inside_phase_b_makes_the_next_merge_interrupted() {
    let project = new_project();
    install(
        &project,
        DecideMergeSeams {
            merge_seams: Some(MergeSeams {
                lock_after_read_tree_killed: true,
                ..MergeSeams::default()
            }),
            crash_in_b: true,
            ..DecideMergeSeams::default()
        },
    );
    let head = project.rev("HEAD");
    let answer = project.decide_merge("nonce-kill");
    assert_eq!(code(&answer), "merge_failed", "{answer}");
    let next = project.decide_merge("nonce-kill-2");
    assert_eq!(code(&next), "merge_failed", "{next}");
    assert!(message(&next).contains("interrupted"), "{next}");
    assert!(message(&next).contains("restored false"), "{next}");
    assert_eq!(project.rev("HEAD"), head);
    assert_eq!(rows(&project), 0);
}

#[test]
fn a_merge_commit_beyond_the_lookup_depth_is_recorded_without_a_commit_id() {
    let project = new_project();
    git(
        &project,
        &[
            "merge",
            "--no-ff",
            "-q",
            "-m",
            "merge by hand",
            &project.branch,
        ],
    );
    for n in 0..5 {
        git(
            &project,
            &[
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                &format!("chore: {n}"),
            ],
        );
    }
    install(
        &project,
        DecideMergeSeams {
            lookup_depth: Some(3),
            ..DecideMergeSeams::default()
        },
    );
    let answer = project.decide_merge("nonce-depth");
    let result = applied(&answer);
    assert_eq!(result["merge"]["alreadyInHead"], true, "{answer}");
    assert_eq!(result["merge"]["mergeCommit"], Value::Null, "{answer}");
}

#[test]
fn findings_after_phase_a_stop_the_merge_in_phase_b() {
    let project = new_project();
    let deps = Mutex::new(project.deps.clone());
    let (pm, reviewer) = (
        project.credentials["pm-1"].clone(),
        project.credentials["rev-1"].clone(),
    );
    let integration = project.integration_id.clone();
    install(
        &project,
        DecideMergeSeams {
            after_a: Some(Box::new(move || {
                let deps = deps.lock().unwrap();
                review_with(&deps, &pm, &reviewer, &integration, "findings");
            })),
            ..DecideMergeSeams::default()
        },
    );
    let head = project.rev("HEAD");
    let answer = project.decide_merge("nonce-findings");
    assert_eq!(code(&answer), "not_reviewed", "{answer}");
    assert_eq!(project.rev("HEAD"), head);
    assert!(!decided(&project, "nonce-findings"));
    assert!(!git_path(&project, MARKER).exists());
}

#[test]
fn switching_the_merge_off_between_calls_disables_the_retry() {
    let mut project = new_project();
    install(
        &project,
        DecideMergeSeams {
            fail_record: true,
            ..DecideMergeSeams::default()
        },
    );
    let answer = project.decide_merge("nonce-off");
    assert_eq!(code(&answer), "merge_unrecorded", "{answer}");
    install(&project, DecideMergeSeams::default());
    project.set_remote_merge(false);
    let retry = project.decide_merge("nonce-off");
    assert_eq!(code(&retry), "merge_disabled", "{retry}");
    assert!(!decided(&project, "nonce-off"));
}

#[test]
fn a_blocked_hand_off_with_a_closed_kernel_logs_the_failure_and_does_not_panic() {
    let mut project = new_project();
    let entries: Arc<Mutex<Vec<LogEntry>>> = Arc::default();
    let sink = entries.clone();
    let logger: Logger = Arc::new(move |entry: &LogEntry| sink.lock().unwrap().push(entry.clone()));
    project.deps.log = logger;
    let (release, block) = hold();
    install(
        &project,
        DecideMergeSeams {
            block_pm_message: block,
            ..DecideMergeSeams::default()
        },
    );
    applied(&project.decide_merge("nonce-closed-kernel"));
    project.deps.kernel.close();
    release.send(()).unwrap();
    wait_for("the failure of the PM message in the log", DEADLINE, || {
        entries
            .lock()
            .unwrap()
            .iter()
            .any(|entry| entry.command == "daemon:pm_message_failed")
    });
}

#[test]
fn a_stop_during_a_held_merge_waits_for_the_merge() {
    let _serial = serial();
    let project = new_project();
    let (release, hold_b) = hold();
    install(
        &project,
        DecideMergeSeams {
            hold_b,
            ..DecideMergeSeams::default()
        },
    );
    let merge = merge_in_thread(&project, "nonce-stop");
    wait_lock(&project, SERIAL_DEADLINE);
    let stopping = project.ok("operator", "shutdown", &[]);
    assert_eq!(stopping["stopping"], true, "{stopping}");
    wait_for("the stop request", SERIAL_DEADLINE, || {
        project.deps.shutdown.requested()
    });
    assert!(!merge.is_finished(), "the merge ended at the stop");
    release.send(()).unwrap();
    applied(&join_within(merge, "the held merge", SERIAL_DEADLINE));
    assert_eq!(merge_commits(&project), 1);
}

#[test]
fn two_daemons_on_two_projects_merge_at_once() {
    let _serial = serial();
    let (one, other) = (new_project(), new_project());
    let (release_one, hold_one) = hold();
    let (release_other, hold_other) = hold();
    install(
        &one,
        DecideMergeSeams {
            hold_b: hold_one,
            ..DecideMergeSeams::default()
        },
    );
    install(
        &other,
        DecideMergeSeams {
            hold_b: hold_other,
            ..DecideMergeSeams::default()
        },
    );
    let first = merge_in_thread(&one, "nonce-one");
    let second = merge_in_thread(&other, "nonce-other");
    // Both are inside their own lock at the same time.
    wait_lock(&one, SERIAL_DEADLINE);
    wait_lock(&other, SERIAL_DEADLINE);
    release_one.send(()).unwrap();
    release_other.send(()).unwrap();
    applied(&join_within(first, "the first merge", SERIAL_DEADLINE));
    applied(&join_within(second, "the second merge", SERIAL_DEADLINE));
    assert_eq!(merge_commits(&one), 1);
    assert_eq!(merge_commits(&other), 1);
}

#[test]
fn a_pm_integrate_of_another_report_succeeds_during_a_held_merge() {
    let project = new_project();
    let (report, _) = second_report(&project);
    let (release, hold_b) = hold();
    install(
        &project,
        DecideMergeSeams {
            hold_b,
            ..DecideMergeSeams::default()
        },
    );
    let merge = merge_in_thread(&project, "nonce-held");
    wait_lock(&project, DEADLINE);
    let integrated = project.ok("pm-1", "integrate", &[&report]);
    assert_eq!(integrated["state"], "merged", "{integrated}");
    assert!(lock_held(&project));
    release.send(()).unwrap();
    applied(&join_within(merge, "the held merge", DEADLINE));
}

#[test]
fn a_merge_that_changes_the_verify_commands_is_not_verified_on_the_retry() {
    let mut project = new_project();
    install(
        &project,
        DecideMergeSeams {
            fail_record: true,
            ..DecideMergeSeams::default()
        },
    );
    let answer = project.decide_merge("nonce-verify");
    assert_eq!(code(&answer), "merge_unrecorded", "{answer}");
    // The merged tree now carries other [verify] commands than the passed run used.
    install(&project, DecideMergeSeams::default());
    project.set_verify_commands(&["false"]);
    let retry = project.decide_merge("nonce-verify");
    assert_eq!(code(&retry), "not_verified", "{retry}");
    assert!(!decided(&project, "nonce-verify"));
}

#[test]
fn an_applied_nonce_of_another_integration_repeats_at_once_during_a_held_merge() {
    let project = new_project();
    let second = second_integration(&project);
    // The second integration is merged first, with its own nonce.
    let argv = merge_argv(&project, &second, "nonce-bb01");
    let answer = project.call(
        "operator",
        "decide-merge",
        &argv.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    applied(&answer);
    let (release, hold_b) = hold();
    install(
        &project,
        DecideMergeSeams {
            hold_b,
            ..DecideMergeSeams::default()
        },
    );
    let held = merge_in_thread(&project, "nonce-aa01");
    wait_lock(&project, DEADLINE);
    let started = Instant::now();
    let repeat = project.call(
        "operator",
        "decide-merge",
        &argv.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    assert_eq!(applied(&repeat)["repeat"], true, "{repeat}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the repeat waited for the held merge"
    );
    assert!(lock_held(&project));
    release.send(()).unwrap();
    applied(&join_within(held, "the held merge", DEADLINE));
    assert_eq!(rows_of(&project, &second), 1);
    assert_eq!(rows(&project), 1);
}

#[test]
fn a_client_closed_after_phase_a_leaves_no_commit_and_no_row() {
    let _serial = serial();
    let project = new_project();
    let (gate, after_a) = after_a_gate(0);
    install(
        &project,
        DecideMergeSeams {
            after_a: Some(after_a),
            ..DecideMergeSeams::default()
        },
    );
    let head = project.rev("HEAD");
    let argv = project.decide_merge_argv("nonce-gone");
    let mut stream = UnixStream::connect(&project.socket).expect("the socket accepts");
    stream
        .write_all(socket_frame(&project.credentials["operator"], "decide-merge", &argv).as_bytes())
        .unwrap();
    gate_reached(&gate, "the call to pass phase A", SERIAL_DEADLINE);
    drop(stream);
    // A round trip on a new connection: the server has seen the closed one by the time it answers.
    let ping = project.socket_call("operator", "ping", &[]);
    assert_eq!(ping["ok"], true, "{ping}");
    gate.release.send(()).unwrap();
    wait_lock_free(&project, SERIAL_DEADLINE);
    // A later call merges; had the first one gone on, this one would find the head already merged.
    let later = project.decide_merge("nonce-later");
    let result = applied(&later);
    assert_eq!(result["merge"]["alreadyInHead"], false, "{later}");
    assert!(!decided(&project, "nonce-gone"));
    assert_eq!(rows(&project), 1);
    assert_eq!(merge_commits(&project), 1);
    assert_ne!(project.rev("HEAD"), head);
}
