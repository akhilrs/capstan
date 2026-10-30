//! The decide-merge route end to end: a reviewed, verified integration merges into the project's branch, is recorded and
//! confirmed; every gate refuses without a trace; the marker, the lock and the hand-off behave as documented.

mod common;

use capstan_daemon::handlers::decide_merge_seams::{install_seams, DecideMergeSeams, Hold};
use capstan_daemon::handlers::shared::AbortSignal;
use capstan_daemon::handlers::{
    command_limit_ms, handle_frame, DECIDE_MERGE_LIMIT_MS, MAX_RESPONSE_BYTES,
};
use capstan_kernel::integrate::{GitSeams, MergeSeams};
use capstan_kernel::types::Identity;
use common::merge::MergeProject;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MARKER: &str = "capstan-remote-merge";

fn git(project: &MergeProject, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(project.dir.path())
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

/// Waits until `count` PM messages about a merge from a remote client exist; the text of the last one. (The cleanup may
/// send its own message first.)
fn wait_merge_message(project: &MergeProject, count: usize) -> String {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let found: Vec<String> = project
            .pm_messages()
            .iter()
            .map(Value::to_string)
            .filter(|text| text.contains("from a remote client"))
            .collect();
        if found.len() >= count {
            return found.last().unwrap().clone();
        }
        assert!(Instant::now() < deadline, "no merge message for the PM");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn hold() -> (Sender<()>, Option<Hold>) {
    let (sender, receiver) = channel();
    (sender, Some(Mutex::new(receiver)))
}

fn seams(project: &MergeProject, seams: DecideMergeSeams) {
    install_seams(&project.deps, seams);
}

/// A request over a fresh connection to the socket, from any thread.
fn socket_call(socket: &PathBuf, credential: &str, command: &str, args: &[String]) -> Value {
    let mut stream = UnixStream::connect(socket).expect("the socket accepts");
    let frame = json!({"v": 1, "credential": credential, "command": command, "args": args});
    stream.write_all(format!("{frame}\n").as_bytes()).unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap_or_else(|e| panic!("{line:?}: {e}"))
}

/// The merge for `nonce` on another thread over the socket.
fn merge_in_thread(project: &MergeProject, nonce: &str) -> std::thread::JoinHandle<Value> {
    let socket = project.socket.clone();
    let credential = project.credentials["operator"].clone();
    let argv = project.decide_merge_argv(nonce);
    std::thread::spawn(move || socket_call(&socket, &credential, "decide-merge", &argv))
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
        assert!(!git_path(project, MARKER).exists(), "the marker is left");
        assert_eq!(
            git(project, &["status", "--porcelain", "--untracked-files=no"]),
            ""
        );
    }
}

fn refuses(project: &MergeProject, nonce: &str, expected: &str) {
    let before = Snapshot::of(project);
    let answer = project.decide_merge(nonce);
    assert_eq!(code(&answer), expected, "{answer}");
    before.assert_same(project, nonce);
    assert!(project.launcher_calls().is_empty());
}

// ------------------------------------------------------------------------------------------------ the merge

#[test]
fn a_reviewed_integration_merges_and_is_confirmed_and_repeats() {
    let project = new_project();
    let head_before = project.rev("HEAD");
    let argv = project.decide_merge_argv("nonce-001");
    let args: Vec<&str> = argv.iter().map(String::as_str).collect();
    let answer = project.decide_merge_args(&args);
    let result = applied(&answer);
    assert_eq!(result["repeat"], false);
    let merge = &result["merge"];
    assert_eq!(merge["integrationId"], project.integration_id.as_str());
    assert_eq!(merge["branch"], project.branch.as_str());
    assert_eq!(merge["alreadyInHead"], false);
    assert_eq!(merge["confirm"], "confirmed");
    assert_eq!(merge["verify"]["state"], "passed");
    assert_eq!(merge["cleanup"]["state"], "scheduled");
    assert_eq!(
        merge["push"],
        json!({"state": "disabled", "reason": "push_off"})
    );
    let head_after = project.rev("HEAD");
    assert_ne!(head_after, head_before);
    assert_eq!(merge["mergeCommit"], head_after.as_str());
    assert_eq!(project.integration()["state"], "confirmed");

    wait_merge_message(&project, 1);

    // The same call again: the stored result, no new commit, no new message.
    let messages = project.pm_messages().len();
    let again = project.decide_merge_args(&args);
    let repeat = applied(&again);
    assert_eq!(repeat["repeat"], true);
    assert_eq!(repeat["merge"]["confirm"], "confirmed");
    assert_eq!(project.rev("HEAD"), head_after);
    assert_eq!(project.pm_messages().len(), messages);

    // Another nonce: the integration is not waiting any more.
    let other = project.decide_merge("nonce-002");
    assert_eq!(code(&other), "not_waiting", "{other}");
    assert!(project.launcher_calls().is_empty());
}

#[test]
fn decide_with_an_integration_merge_is_invalid() {
    let project = new_project();
    let argv = project.decide_merge_argv("nonce-001");
    let args: Vec<&str> = argv.iter().map(String::as_str).collect();
    let answer = project.call("operator", "decide", &args);
    assert_eq!(code(&answer), "invalid_request", "{answer}");
    let other = project.call(
        "operator",
        "decide-merge",
        &[
            "op:x",
            "--expect",
            "aaaaaaaaaaaa",
            "--nonce",
            "n",
            "approve",
        ],
    );
    assert_eq!(code(&other), "invalid_request", "{other}");
}

#[test]
fn an_agent_may_not_merge() {
    let project = new_project();
    let argv = project.decide_merge_argv("nonce-001");
    let args: Vec<&str> = argv.iter().map(String::as_str).collect();
    let answer = project.call("pm-1", "decide-merge", &args);
    assert_eq!(code(&answer), "forbidden", "{answer}");
}

#[test]
fn each_gate_of_phase_a_refuses_without_a_trace() {
    // merge is off
    let mut project = new_project();
    project.set_remote_merge(false);
    refuses(&project, "nonce-off", "merge_disabled");

    // the latest review has findings
    let project = new_project();
    project.set_review_findings();
    refuses(&project, "nonce-findings", "not_reviewed");

    // the verify commands changed after the pass
    let mut project = new_project();
    project.set_verify_commands(&["false"]);
    refuses(&project, "nonce-verify", "not_verified");

    // the integration branch moved
    let project = new_project();
    project.move_branch_tip();
    refuses(&project, "nonce-moved", "branch_moved");

    // a stale --expect
    let project = new_project();
    let before = Snapshot::of(&project);
    let answer = project.decide_merge_args(&[
        &format!("integration:{}", project.integration_id),
        "--expect",
        "000000000000",
        "--nonce",
        "nonce-stale",
        "merge",
    ]);
    assert_eq!(code(&answer), "hash_mismatch", "{answer}");
    before.assert_same(&project, "nonce-stale");
}

#[test]
fn a_conflict_lists_its_files_and_leaves_no_marker() {
    let project = new_project();
    std::fs::write(project.dir.path().join("one.txt"), "other\n").unwrap();
    git(&project, &["add", "one.txt"]);
    git(&project, &["commit", "-q", "-m", "feat: one differently"]);
    let before = Snapshot::of(&project);
    let answer = project.decide_merge("nonce-conflict");
    assert_eq!(code(&answer), "merge_conflict", "{answer}");
    assert!(message(&answer).contains("one.txt"), "{answer}");
    before.assert_same(&project, "nonce-conflict");
}

#[test]
fn a_modified_tracked_file_is_a_dirty_tree() {
    let project = new_project();
    std::fs::write(project.dir.path().join("a.txt"), "changed\n").unwrap();
    let head = project.rev("HEAD");
    let answer = project.decide_merge("nonce-dirty");
    assert_eq!(code(&answer), "dirty_tree", "{answer}");
    assert_eq!(project.rev("HEAD"), head);
    assert!(!git_path(&project, MARKER).exists());
    assert!(!decided(&project, "nonce-dirty"));
}

#[test]
fn a_head_merged_by_hand_is_recorded_as_already_in_head() {
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
    let by_hand = project.rev("HEAD");
    let answer = project.decide_merge("nonce-hand");
    let result = applied(&answer);
    assert_eq!(result["merge"]["alreadyInHead"], true);
    assert_eq!(result["merge"]["mergeCommit"], by_hand.as_str());
    assert_eq!(project.rev("HEAD"), by_hand);
    assert_eq!(project.integration()["state"], "confirmed");
    let text = wait_merge_message(&project, 1);
    assert!(text.contains("did not merge"), "{text}");
}

// ------------------------------------------------------------------------------------------------ marker and lock

fn write_marker(project: &MergeProject, head: &str) -> PathBuf {
    let path = git_path(project, MARKER);
    std::fs::write(
        &path,
        json!({"headSha": head, "nonce": "old", "integrationId": project.integration_id})
            .to_string(),
    )
    .unwrap();
    path
}

fn head_of_integration(project: &MergeProject) -> String {
    project.integration()["headSha"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn a_leftover_marker_blocks_the_merge_until_it_is_deleted() {
    let project = new_project();
    let head = head_of_integration(&project);
    let path = write_marker(&project, &head);
    let before = Snapshot::of(&project);
    let answer = project.decide_merge("nonce-marker");
    assert_eq!(code(&answer), "merge_failed", "{answer}");
    let text = message(&answer);
    assert!(
        text.contains("interrupted") && text.contains("restored false"),
        "{text}"
    );
    assert!(text.contains(&head) && text.contains(MARKER), "{text}");
    assert!(path.exists());
    assert_eq!(project.rev("HEAD"), before.head);
    assert_eq!(project.integration(), before.state);
    assert!(!decided(&project, "nonce-marker"));
    std::fs::remove_file(&path).unwrap();
    applied(&project.decide_merge("nonce-marker"));
}

#[test]
fn a_leftover_marker_of_this_head_is_removed_when_its_merge_is_recorded() {
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
    let path = write_marker(&project, &head_of_integration(&project));
    let result = project.decide_merge("nonce-retire");
    applied(&result);
    assert!(!path.exists());
    project.wait_handoff();
    let text = wait_merge_message(&project, 1);
    assert!(text.contains("interrupted"), "{text}");
}

#[test]
fn a_marker_of_another_head_stays() {
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
    let path = write_marker(&project, &"b".repeat(40));
    applied(&project.decide_merge("nonce-keep"));
    assert!(path.exists());
}

#[test]
fn a_leftover_index_lock_is_reported() {
    let project = new_project();
    let lock = git_path(&project, "index.lock");
    std::fs::write(&lock, "").unwrap();
    let head = project.rev("HEAD");
    let answer = project.decide_merge("nonce-lock");
    assert_eq!(code(&answer), "merge_failed", "{answer}");
    assert!(message(&answer).contains("index_lock_left"), "{answer}");
    assert!(message(&answer).contains("restored false"), "{answer}");
    assert_eq!(project.rev("HEAD"), head);
    assert!(!decided(&project, "nonce-lock"));
    std::fs::remove_file(&lock).unwrap();
}

#[test]
fn the_marker_exists_only_while_phase_b_writes_and_a_second_merge_is_busy() {
    let project = new_project();
    let (release, hold_b) = hold();
    let (release_marker, hold_after_marker) = hold();
    seams(
        &project,
        DecideMergeSeams {
            hold_b,
            hold_after_marker,
            ..DecideMergeSeams::default()
        },
    );
    let marker = git_path(&project, MARKER);
    let first = merge_in_thread(&project, "nonce-first");
    // Held before the marker is written.
    std::thread::sleep(Duration::from_millis(300));
    assert!(!marker.exists());
    let busy = project.decide_merge("nonce-second");
    assert_eq!(code(&busy), "busy", "{busy}");
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !marker.exists() {
        assert!(Instant::now() < deadline, "the marker never appeared");
        std::thread::sleep(Duration::from_millis(20));
    }
    let content: Value = serde_json::from_str(&std::fs::read_to_string(&marker).unwrap()).unwrap();
    assert_eq!(content["headSha"], head_of_integration(&project).as_str());
    assert_eq!(content["nonce"], "nonce-first");
    assert_eq!(content["integrationId"], project.integration_id.as_str());
    assert_eq!(content.as_object().unwrap().len(), 3);
    assert_eq!(
        std::fs::metadata(&marker).unwrap().permissions().mode() & 0o777,
        0o600
    );
    release_marker.send(()).unwrap();
    applied(&first.join().unwrap());
    assert!(!marker.exists());
}

#[test]
fn a_panic_in_phase_b_frees_the_lock_and_the_next_merge_applies() {
    let project = new_project();
    seams(
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
    assert_eq!(project.rev("HEAD"), head);
    assert!(!git_path(&project, MARKER).exists());
    seams(&project, DecideMergeSeams::default());
    applied(&project.decide_merge("nonce-after"));
}

#[test]
fn an_unwritable_git_directory_is_marker_unwritable() {
    let project = new_project();
    let dir = project.dir.path().join(".git");
    let original = std::fs::metadata(&dir).unwrap().permissions();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    // A superuser writes anyway; the case cannot be tested then.
    let probe = dir.join("probe");
    if std::fs::write(&probe, "").is_ok() {
        let _ = std::fs::remove_file(&probe);
        std::fs::set_permissions(&dir, original).unwrap();
        return;
    }
    let answer = project.decide_merge("nonce-ro");
    std::fs::set_permissions(&dir, original).unwrap();
    assert_eq!(code(&answer), "merge_failed", "{answer}");
    assert!(message(&answer).contains("marker_unwritable"), "{answer}");
    assert!(message(&answer).contains("restored true"), "{answer}");
}

#[test]
fn a_marker_created_after_phase_a_is_not_overwritten() {
    let project = new_project();
    let path = git_path(&project, MARKER);
    let planted = path.clone();
    seams(
        &project,
        DecideMergeSeams {
            after_a: Some(Box::new(move || {
                std::fs::write(&planted, "{\"headSha\":\"z\"}").unwrap();
            })),
            ..DecideMergeSeams::default()
        },
    );
    let answer = project.decide_merge("nonce-race");
    assert_eq!(code(&answer), "merge_failed", "{answer}");
    assert!(message(&answer).contains("interrupted"), "{answer}");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "{\"headSha\":\"z\"}"
    );
}

#[test]
fn an_aborted_connection_writes_nothing_and_gets_no_answer() {
    let project = new_project();
    let signal = AbortSignal::new();
    let hook = signal.clone();
    seams(
        &project,
        DecideMergeSeams {
            after_a: Some(Box::new(move || hook.abort("closed"))),
            ..DecideMergeSeams::default()
        },
    );
    let before = Snapshot::of(&project);
    let argv = project.decide_merge_argv("nonce-abort");
    let frame = json!({
        "v": 1,
        "credential": project.credentials["operator"],
        "command": "decide-merge",
        "args": argv,
    });
    let outcome = handle_frame(
        &project.deps,
        frame.to_string().as_bytes(),
        &signal,
        MAX_RESPONSE_BYTES,
    );
    assert!(outcome.response.is_none(), "{:?}", outcome.response);
    before.assert_same(&project, "nonce-abort");
}

// ------------------------------------------------------------------------------------------------ the hand-off

#[test]
fn the_answer_comes_before_the_cleanup_and_the_message() {
    let project = new_project();
    let (release, block) = hold();
    seams(
        &project,
        DecideMergeSeams {
            block_pm_message: block,
            ..DecideMergeSeams::default()
        },
    );
    let messages = project.pm_messages().len();
    applied(&project.decide_merge("nonce-block"));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(project.pm_messages().len(), messages);
    // The branch is still there: the cleanup waits too.
    git(
        &project,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/heads/{}", project.branch),
        ],
    );
    release.send(()).unwrap();
    wait_merge_message(&project, 1);
}

#[test]
fn a_merge_is_applied_without_an_active_pm() {
    let project = new_project();
    let owner = project.credentials["operator"].clone();
    project
        .deps
        .kernel
        .run(move |core| {
            let context = capstan_daemon::deps::new_context(core, &owner)?;
            core.end_agent(&context, "pm-1", None)
        })
        .expect("the PM's agent ends");
    let messages = project.pm_messages().len();
    applied(&project.decide_merge("nonce-nopm"));
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(project.integration()["state"], "confirmed");
    assert_eq!(project.pm_messages().len(), messages);
}

// ------------------------------------------------------------------------------------------------ the seams

#[test]
fn the_hooks_and_switches_each_have_their_effect() {
    // git_seams: an old git
    let project = new_project();
    seams(
        &project,
        DecideMergeSeams {
            git_seams: Some(GitSeams {
                force_old_git: true,
                ..GitSeams::default()
            }),
            ..DecideMergeSeams::default()
        },
    );
    let answer = project.decide_merge("nonce-old");
    assert!(
        ["git_too_old", "git_unavailable"].contains(&code(&answer).as_str()),
        "{answer}"
    );

    // after_a runs once before the lock
    let project = new_project();
    let counter = Arc::new(Mutex::new(0));
    let seen = counter.clone();
    seams(
        &project,
        DecideMergeSeams {
            after_a: Some(Box::new(move || *seen.lock().unwrap() += 1)),
            ..DecideMergeSeams::default()
        },
    );
    applied(&project.decide_merge("nonce-after-a"));
    assert_eq!(*counter.lock().unwrap(), 1);

    // fail_record: nothing settles; the same nonce records through already_in_head
    let project = new_project();
    seams(
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
    assert!(message(&answer).contains(&merged), "{answer}");
    seams(&project, DecideMergeSeams::default());
    let retry = project.decide_merge("nonce-record");
    let result = applied(&retry);
    assert_eq!(result["merge"]["alreadyInHead"], true);
    assert_eq!(result["merge"]["mergeCommit"], merged.as_str());
    assert_eq!(project.rev("HEAD"), merged);

    // lookup_depth 0 finds no merge commit
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
    seams(
        &project,
        DecideMergeSeams {
            lookup_depth: Some(0),
            ..DecideMergeSeams::default()
        },
    );
    let answer = project.decide_merge("nonce-depth");
    let result = applied(&answer);
    assert_eq!(result["merge"]["mergeCommit"], Value::Null);

    // fail_settle: the answer says the confirm failed
    let project = new_project();
    seams(
        &project,
        DecideMergeSeams {
            fail_settle: true,
            ..DecideMergeSeams::default()
        },
    );
    let answer = project.decide_merge("nonce-settle");
    let result = applied(&answer);
    assert_eq!(result["merge"]["confirm"], "failed");

    // crash_after_b: the merge is made and not recorded; the same nonce records it
    let project = new_project();
    seams(
        &project,
        DecideMergeSeams {
            crash_after_b: true,
            ..DecideMergeSeams::default()
        },
    );
    let answer = project.decide_merge("nonce-crash");
    assert_eq!(answer["ok"], false, "{answer}");
    assert!(!decided(&project, "nonce-crash"));
    let merged = project.rev("HEAD");
    seams(&project, DecideMergeSeams::default());
    let answer = project.decide_merge("nonce-crash");
    let result = applied(&answer);
    assert_eq!(result["merge"]["alreadyInHead"], true);
    assert_eq!(result["merge"]["mergeCommit"], merged.as_str());

    // merge_seams and crash_in_b: the marker and index.lock stay
    let project = new_project();
    seams(
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
    let answer = project.decide_merge("nonce-kill");
    assert_eq!(code(&answer), "merge_failed", "{answer}");
    assert!(git_path(&project, MARKER).exists());
    assert!(git_path(&project, "index.lock").exists());
    let next = project.decide_merge("nonce-kill-2");
    assert_eq!(code(&next), "merge_failed", "{next}");
    assert!(message(&next).contains("interrupted"), "{next}");
}

#[test]
fn the_limit_of_the_route_and_none_for_decide() {
    let project = new_project();
    let identity = Identity {
        actor_id: "operator".into(),
        role: "operator".into(),
        capabilities: Vec::new(),
        agent: None,
    };
    assert_eq!(
        command_limit_ms(&project.deps, "decide-merge", &identity),
        Some(260_000)
    );
    assert_eq!(DECIDE_MERGE_LIMIT_MS, 260_000);
    assert_eq!(command_limit_ms(&project.deps, "decide", &identity), None);
}

// ------------------------------------------------------------------------------------------------ the remaining gates

/// A writable connection to the project's ledger, for the rows no command writes.
fn ledger_file(project: &MergeProject) -> rusqlite::Connection {
    let path =
        capstan_ledger::resolve_database_path(&project.deps.options.state_directory).unwrap();
    let conn = rusqlite::Connection::open(path).expect("the ledger opens");
    conn.busy_timeout(Duration::from_secs(10)).unwrap();
    conn
}

fn columns(conn: &rusqlite::Connection, table: &str) -> Vec<String> {
    conn.prepare(&format!("PRAGMA table_info({table})"))
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn a_paused_run_refuses_the_merge() {
    let project = new_project();
    project.ok("operator", "pause", &["--reason", "testing the gate"]);
    refuses(&project, "nonce-paused", "run_paused");
    project.ok("operator", "resume", &["--reason", "done"]);
    applied(&project.decide_merge("nonce-paused"));
}

#[test]
fn an_integration_running_in_the_ledger_is_busy() {
    let project = new_project();
    let conn = ledger_file(&project);
    let names = columns(&conn, "integrations");
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
        [&project.integration_id],
    )
    .expect("a running integration is added");
    drop(conn);
    refuses(&project, "nonce-running", "busy");
}

#[test]
fn a_verify_pass_at_an_older_head_is_not_verified() {
    let project = new_project();
    // The integration head moved on after the pass: the pass is at an older head.
    let conn = ledger_file(&project);
    conn.execute(
        "UPDATE integrations SET head_sha = ? WHERE integration_id = ?",
        [project.rev("feat/dev-1"), project.integration_id.clone()],
    )
    .expect("the head moves");
    drop(conn);
    refuses(&project, "nonce-older", "not_verified");
}

#[test]
fn a_merge_applies_when_messages_per_hour_is_used_up() {
    let project = new_project();
    let file = project.dir.path().join("capstan.toml");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(
        &file,
        text.replace("merge = true", "merge = true\nmessages_per_hour = 1"),
    )
    .unwrap();
    let conn = ledger_file(&project);
    let names = columns(&conn, "remote_decisions");
    assert!(names.contains(&"outcome".to_string()));
    conn.execute(
        "INSERT INTO remote_decisions(project_id, nonce, decision_id, request_id, content_hash, verb, args_json, client,
           outcome, result_json, message_id, decided_at)
         SELECT project_id, 'nonce-spent', 'remote-spent', 'op:spent', 'a', 'approve', '{}', 'test',
           'forwarded', '{}', NULL, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
           FROM integrations WHERE integration_id = ?",
        [&project.integration_id],
    )
    .expect("a forward is counted");
    drop(conn);
    let answer = project.decide_merge("nonce-capped");
    let result = applied(&answer);
    assert_eq!(result["merge"]["confirm"], "confirmed");
    wait_merge_message(&project, 1);
}

// ------------------------------------------------------------------------------------------------ a daemon stop

#[test]
fn a_daemon_stop_during_the_hand_off_loses_the_message_and_cleanup_offers_the_branch() {
    use capstan_daemon::deps::{Deps, KernelHandle, LogEntry, Logger};
    use capstan_kernel::kernel::KernelOptions;
    use capstan_kernel::Core;

    let project = new_project();
    let (release, block) = hold();
    seams(
        &project,
        DecideMergeSeams {
            block_pm_message: block,
            ..DecideMergeSeams::default()
        },
    );
    let answer = project.decide_merge("nonce-stop");
    assert_eq!(applied(&answer)["merge"]["confirm"], "confirmed");

    // The daemon stops: the kernel closes, and the held hand-off finds it closed.
    project.deps.kernel.close();
    release.send(()).unwrap();
    std::thread::sleep(Duration::from_millis(500));

    // It starts again over the same project directory.
    let options = (*project.deps.options).clone();
    let (state, initial, root) = (
        options.state_directory.clone(),
        options.project.clone(),
        options.workspace_root.clone(),
    );
    let kernel = KernelHandle::spawn(move || {
        Core::open(
            &state,
            &initial,
            &KernelOptions {
                workspace_root: Some(root),
                ..KernelOptions::default()
            },
            common::seeded_env("merge-restart"),
        )
    })
    .expect("the kernel opens again");
    let sink: Logger = Arc::new(|_: &LogEntry| {});
    let restarted = Deps::new(kernel, options, sink);

    let integration = restarted
        .kernel
        .run({
            let id = project.integration_id.clone();
            move |core| core.integration(&id)
        })
        .unwrap();
    assert_eq!(integration["state"], "confirmed");
    let messages = restarted
        .kernel
        .run(|core| core.messages_for("pm-1"))
        .unwrap()
        .to_string();
    assert!(!messages.contains("from a remote client"), "{messages}");

    let frame = json!({
        "v": 1,
        "credential": project.credentials["operator"],
        "command": "cleanup",
        "args": Vec::<String>::new(),
    });
    let outcome = handle_frame(
        &restarted,
        frame.to_string().as_bytes(),
        &AbortSignal::new(),
        MAX_RESPONSE_BYTES,
    );
    let offered = outcome.response.expect("an answer");
    assert!(offered.contains(&project.branch), "{offered}");
}
