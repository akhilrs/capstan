//! `cstan decide`: the command line it accepts, how a project is found (a root, or a project id among the running
//! controllers), the exit codes and lines of each outcome, and a controller that never answers.
mod support;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cstan_front::controllers::Proc;
use cstan_front::decide::{self, plan, DecidePlan, CLI_ERROR_CODES};
use serde_json::{json, Value};
use support::FRONT;

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef";
const ARGV: [&str; 6] = [
    "op:abc",
    "--expect",
    "0123456789ab",
    "--nonce",
    "nonce-0001",
    "deny",
];

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A project root with `.capstan/` (mode 0700), an operator key (mode 0600) and a project id.
fn project(id: &str) -> PathBuf {
    let root = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!(
            "cstan-decide-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
    let capstan = root.join(".capstan");
    std::fs::create_dir_all(capstan.join("state")).unwrap();
    std::fs::set_permissions(&capstan, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(
        capstan.join("project.json"),
        json!({"projectId": id}).to_string(),
    )
    .unwrap();
    let key = capstan.join("operator.key");
    std::fs::write(&key, KEY).unwrap();
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    root
}

fn proc_of(root: &Path) -> Proc {
    Proc {
        pid: 1,
        project: Some(root.to_path_buf()),
        project_gone: false,
        started_at: None,
        rss_bytes: None,
        cpu_percent: None,
    }
}

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

fn plan_of(project: &str, json: bool) -> DecidePlan {
    DecidePlan {
        project: project.to_string(),
        json,
        argv: args(&ARGV),
    }
}

/// Serves one connection of the project's socket: sends the frame it read on `frames` and answers `reply` when it is some;
/// with none it keeps the connection open and says nothing.
fn serve(root: &Path, reply: Option<Value>) -> mpsc::Receiver<Value> {
    let listener = UnixListener::bind(root.join(".capstan/state/control.sock")).unwrap();
    let (frames, received) = mpsc::channel();
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).unwrap();
        let _ = frames.send(serde_json::from_str(&line).unwrap());
        match reply {
            Some(reply) => {
                let mut stream = &stream;
                stream.write_all(format!("{reply}\n").as_bytes()).unwrap();
            }
            None => std::thread::sleep(Duration::from_secs(3)),
        }
    });
    received
}

fn applied() -> Value {
    json!({"ok": true, "result": {"v": 1, "decisionId": "d1", "requestId": "op:abc", "nonce": "nonce-0001",
        "outcome": "applied", "repeat": false, "kind": "op", "verb": "deny"}})
}

#[test]
fn a_bad_command_line_has_no_plan_and_the_process_exits_2() {
    let bad: &[&[&str]] = &[
        &[],
        &[
            "op:abc",
            "--expect",
            "0123456789ab",
            "--nonce",
            "nonce-0001",
            "deny",
        ],
        &["--project", "/x", "op:abc", "--nonce", "nonce-0001", "deny"],
        &[
            "--project",
            "/x",
            "op:abc",
            "--expect",
            "0123456789ab",
            "deny",
        ],
        &[
            "--project",
            "/x",
            "op:abc",
            "--expect",
            "0123456789ab",
            "--nonce",
            "nonce-0001",
        ],
        &[
            "--project",
            "/x",
            "--bogus",
            "op:abc",
            "--expect",
            "0123456789ab",
            "--nonce",
            "nonce-0001",
            "deny",
        ],
        &[
            "--project",
            "/x",
            "--project",
            "/y",
            "op:abc",
            "--expect",
            "0123456789ab",
            "--nonce",
            "nonce-0001",
            "deny",
        ],
        &[
            "--project",
            "/x",
            "--json",
            "--json",
            "op:abc",
            "--expect",
            "0123456789ab",
            "--nonce",
            "nonce-0001",
            "deny",
        ],
        &[
            "--project",
            "/x",
            "op:abc",
            "--project",
            "/y",
            "--expect",
            "0123456789ab",
            "--nonce",
            "nonce-0001",
            "deny",
        ],
        &[
            "--project",
            "/x",
            "op:abc",
            "--expect",
            "0123456789ab",
            "--nonce",
            "nonce-0001",
            "--json",
            "deny",
        ],
        &[
            "--project",
            "/x",
            "op:abc",
            "--expect",
            "0123456789ab",
            "--nonce",
            "nonce-0001",
            "deny",
            "--json",
        ],
        &["--project"],
        &["--project", "--json", "op:abc"],
    ];
    for words in bad {
        assert_eq!(plan(&args(words)), None, "{words:?}");
        let mut command = vec!["decide"];
        command.extend_from_slice(words);
        let out = Command::new(FRONT)
            .args(&command)
            .current_dir(std::env::temp_dir())
            .env_clear()
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{words:?}");
        assert!(out.stdout.is_empty(), "{words:?}");
        assert!(!out.stderr.is_empty(), "{words:?}");
    }
}

#[test]
fn flags_before_the_request_id_may_come_in_either_order() {
    let want = Some(plan_of("p1", true));
    let mut words = vec!["--json", "--project", "p1"];
    words.extend(ARGV);
    assert_eq!(plan(&args(&words)), want);
    let mut words = vec!["--project", "p1", "--json"];
    words.extend(ARGV);
    assert_eq!(plan(&args(&words)), want);
}

#[test]
fn a_root_resolves_and_the_argv_and_key_reach_the_route() {
    let root = project("p-root");
    let frames = serve(&root, Some(applied()));
    let reply = decide::run(&plan_of(root.to_str().unwrap(), true), &[], 5_000);
    assert_eq!(reply.exit, 0, "{reply:?}");
    let result: Value = serde_json::from_str(reply.stdout.trim_end()).unwrap();
    assert_eq!(result["outcome"], "applied");
    assert_eq!(reply.stdout.lines().count(), 1);
    let frame = frames.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(frame["command"], "decide");
    assert_eq!(frame["credential"], KEY);
    assert_eq!(frame["args"], json!(ARGV));
    assert!(!reply.stdout.contains(KEY) && !reply.stderr.contains(KEY));
}

#[test]
fn a_project_id_resolves_among_the_running_controllers() {
    let root = project("p-by-id");
    let other = project("p-other");
    let frames = serve(&root, Some(applied()));
    let procs = [proc_of(&other), proc_of(&root)];
    let reply = decide::run(&plan_of("p-by-id", false), &procs, 5_000);
    assert_eq!(reply.exit, 0, "{reply:?}");
    assert_eq!(reply.stdout, "op:abc deny: applied\n");
    assert!(reply.stderr.is_empty());
    frames.recv_timeout(Duration::from_secs(5)).unwrap();
}

#[test]
fn a_repeat_and_a_forward_are_worded() {
    let root = project("p-words");
    let mut repeat = applied();
    repeat["result"]["repeat"] = json!(true);
    serve(&root, Some(repeat));
    let reply = decide::run(&plan_of(root.to_str().unwrap(), false), &[], 5_000);
    assert_eq!(reply.stdout, "op:abc deny: applied (repeat)\n");
    assert_eq!(reply.exit, 0);
}

#[test]
fn an_unknown_id_and_a_relative_path_are_project_not_found() {
    let root = project("p-known");
    let procs = [proc_of(&root)];
    for value in [
        "p-unknown",
        root.file_name().unwrap().to_str().unwrap(),
        "./x",
        "../x",
    ] {
        let reply = decide::run(&plan_of(value, true), &procs, 200);
        assert_eq!(reply.exit, 1, "{value}");
        let line: Value = serde_json::from_str(reply.stdout.trim_end()).unwrap();
        assert_eq!(line["v"], 1);
        assert_eq!(line["error"]["code"], "project_not_found", "{value}");
    }
    let reply = decide::run(&plan_of("/no/such/root", false), &[], 200);
    assert_eq!(reply.exit, 1);
    assert!(reply.stdout.is_empty());
    assert!(
        reply.stderr.contains("project_not_found"),
        "{}",
        reply.stderr
    );
}

#[test]
fn a_bad_key_mode_is_no_operator_key() {
    let root = project("p-badkey");
    std::fs::set_permissions(
        root.join(".capstan/operator.key"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let reply = decide::run(&plan_of(root.to_str().unwrap(), true), &[], 200);
    assert_eq!(reply.exit, 1);
    let line: Value = serde_json::from_str(reply.stdout.trim_end()).unwrap();
    assert_eq!(line["error"]["code"], "no_operator_key");
    let by_id = decide::run(&plan_of("p-badkey", true), &[proc_of(&root)], 200);
    assert!(by_id.stdout.contains("no_operator_key"), "{by_id:?}");
    assert!(!by_id.stdout.contains(KEY));
}

#[test]
fn a_refusal_of_the_route_keeps_its_code() {
    let root = project("p-refused");
    serve(
        &root,
        Some(json!({"ok": false, "code": "hash_mismatch", "message": "the item changed"})),
    );
    let reply = decide::run(&plan_of(root.to_str().unwrap(), true), &[], 5_000);
    assert_eq!(reply.exit, 1);
    let line: Value = serde_json::from_str(reply.stdout.trim_end()).unwrap();
    assert_eq!(line["error"]["code"], "hash_mismatch");
    assert_eq!(line["error"]["message"], "the item changed");
}

#[test]
fn a_socket_that_never_answers_is_not_answering_within_a_second() {
    let root = project("p-silent");
    serve(&root, None);
    let started = Instant::now();
    let reply = decide::run(&plan_of(root.to_str().unwrap(), true), &[], 200);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(reply.exit, 1);
    let line: Value = serde_json::from_str(reply.stdout.trim_end()).unwrap();
    assert_eq!(line["error"]["code"], "not_answering");
}

#[test]
fn a_missing_socket_is_not_answering() {
    let root = project("p-nosocket");
    let reply = decide::run(&plan_of(root.to_str().unwrap(), true), &[], 200);
    assert_eq!(reply.exit, 1);
    assert!(reply.stdout.contains("not_answering"), "{reply:?}");
}

#[test]
fn the_codes_of_the_command_are_these() {
    assert_eq!(
        CLI_ERROR_CODES,
        ["project_not_found", "no_operator_key", "not_answering"]
    );
}
