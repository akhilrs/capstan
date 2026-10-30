//! `cstan recent`: the command line it accepts, how a project is found, the answers of a real daemon (a page, a truncated
//! page, the resync answer, the rate limit), the refusals of a fake one and a controller that never answers. The
//! fixtures of `tests/fixtures/recent` hold what `--json` prints; CapstanKit reads them too.
mod support;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use cstan_front::controllers::Proc;
use cstan_front::recent::{self, plan, RecentPlan};
use serde_json::{json, Value};
use support::{RealDaemon, FRONT};

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef";
const EPOCH: &str = "0123456789abcdef";

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

fn plan_of(project: &str, epoch: &str, head: u64, json: bool) -> RecentPlan {
    RecentPlan {
        project: project.to_string(),
        epoch: epoch.to_string(),
        head,
        limit: None,
        json,
    }
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

fn fixture(name: &str) -> Value {
    let path = support::tests_dir().join("fixtures/recent").join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

/// `value` with what differs from run to run replaced: the project id, the epoch, the times and the event ids.
fn normal(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, v)| {
                    let v = match key.as_str() {
                        "projectId" => json!("pfixture"),
                        "epoch" => json!(EPOCH),
                        "ts" => json!("2026-01-01T00:00:00.000Z"),
                        "actorId" => json!("actor-fixture"),
                        "id" if v.as_str().is_some_and(|id| id.split(':').count() >= 3) => {
                            let rest: Vec<&str> = v.as_str().unwrap().split(':').skip(2).collect();
                            json!(format!("pfixture:{EPOCH}:{}", rest.join(":")))
                        }
                        _ => normal(v),
                    };
                    (key.clone(), v)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(normal).collect()),
        other => other.clone(),
    }
}

/// Compares `actual` with the fixture `name`; with `CSTAN_WRITE_FIXTURES` set, writes the fixture instead.
fn assert_fixture(name: &str, actual: &Value) {
    let actual = normal(actual);
    if std::env::var_os("CSTAN_WRITE_FIXTURES").is_some() {
        let path = support::tests_dir().join("fixtures/recent").join(name);
        std::fs::write(path, format!("{actual}\n")).unwrap();
    }
    assert_eq!(actual, fixture(name), "{name}");
}

fn one_line(text: &str) -> Value {
    assert_eq!(text.lines().count(), 1, "{text}");
    serde_json::from_str(text.trim_end()).unwrap()
}

// ------------------------------------------------------------------------------------------------ a fake listener

fn fake_project() -> PathBuf {
    let root = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!(
            "cstan-recent-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
    let capstan = root.join(".capstan");
    std::fs::create_dir_all(capstan.join("state")).unwrap();
    std::fs::set_permissions(&capstan, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(
        capstan.join("project.json"),
        json!({"projectId": "pfake"}).to_string(),
    )
    .unwrap();
    let key = capstan.join("operator.key");
    std::fs::write(&key, KEY).unwrap();
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    root
}

/// Serves one connection of the project's socket: answers `reply` when it is some, else keeps the connection open and
/// says nothing.
fn serve(root: &Path, reply: Option<Value>) {
    let listener = UnixListener::bind(root.join(".capstan/state/control.sock")).unwrap();
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).unwrap();
        match reply {
            Some(reply) => {
                let mut stream = &stream;
                stream.write_all(format!("{reply}\n").as_bytes()).unwrap();
            }
            None => std::thread::sleep(Duration::from_secs(3)),
        }
    });
}

// ----------------------------------------------------------------------------------------------------- usage

#[test]
fn a_bad_command_line_has_no_plan_and_the_process_exits_2() {
    let ok = ["--project", "/x", "--epoch", EPOCH, "--head", "5"];
    assert!(plan(&args(&ok)).is_some());
    let bad: &[&[&str]] = &[
        &[],
        &["--epoch", EPOCH, "--head", "5"],
        &["--project", "/x", "--epoch", EPOCH],
        &["--project", "/x", "--head", "5"],
        &["--project", "./x", "--epoch", EPOCH, "--head", "5"],
        &["--project", "x/y", "--epoch", EPOCH, "--head", "5"],
        &[
            "--project",
            "/x",
            "--project",
            "/y",
            "--epoch",
            EPOCH,
            "--head",
            "5",
        ],
        &[
            "--project",
            "/x",
            "--epoch",
            EPOCH,
            "--head",
            "5",
            "--json",
            "--json",
        ],
        &[
            "--project",
            "/x",
            "--epoch",
            EPOCH,
            "--head",
            "5",
            "--head",
            "6",
        ],
        &["--project", "/x", "--epoch", "XYZ", "--head", "5"],
        &["--project", "/x", "--epoch", EPOCH, "--head", "-1"],
        &[
            "--project",
            "/x",
            "--epoch",
            EPOCH,
            "--head",
            "5",
            "--limit",
            "0",
        ],
        &[
            "--project",
            "/x",
            "--epoch",
            EPOCH,
            "--head",
            "5",
            "--limit",
            "21",
        ],
        &["--project", "/x", "--epoch", EPOCH, "--head", "5", "extra"],
        &["--project", "--json", "--epoch", EPOCH, "--head", "5"],
    ];
    for words in bad {
        assert_eq!(plan(&args(words)), None, "{words:?}");
        let mut command = vec!["recent"];
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
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("recent --project"), "{words:?}: {err}");
        assert!(!err.contains("cstan: unknown command: recent"), "{err}");
    }
}

#[test]
fn flags_come_in_any_order_and_a_limit_is_kept() {
    let want = RecentPlan {
        limit: Some(3),
        ..plan_of("pabc", EPOCH, 7, true)
    };
    let words = [
        "--json",
        "--limit",
        "3",
        "--head",
        "7",
        "--epoch",
        EPOCH,
        "--project",
        "pabc",
    ];
    assert_eq!(plan(&args(&words)), Some(want));
}

#[test]
fn the_usage_error_is_not_the_unknown_command_text() {
    let run = |words: &[&str]| {
        let out = Command::new(FRONT)
            .args(words)
            .current_dir(std::env::temp_dir())
            .env_clear()
            .stdin(Stdio::null())
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stderr).into_owned()
    };
    let wrong = run(&["recent", "--bogus"]);
    let unknown = run(&["recentt"]);
    assert_eq!(
        unknown,
        "cstan: unknown command: recentt\nrun `cstan help` for the list of commands\n"
    );
    assert_ne!(
        wrong,
        "cstan: unknown command: recent\nrun `cstan help` for the list of commands\n"
    );
    assert!(!wrong.contains("unknown command"), "{wrong}");
}

// ------------------------------------------------------------------------------------------------ a real daemon

#[test]
fn a_root_and_a_project_id_give_the_page_and_json_is_the_answer_of_the_route() {
    let daemon = RealDaemon::start("page", "precentpage", 3, 2);
    let (epoch, head) = daemon.head();
    let root = daemon.root.to_str().unwrap();
    let by_root = recent::run(&plan_of(root, &epoch, head, true), &[], 5_000);
    assert_eq!(by_root.exit, 0, "{by_root:?}");
    let answer = one_line(&by_root.stdout);
    assert_eq!(answer["head"], head);
    assert_eq!(answer["epoch"], epoch.as_str());
    assert_eq!(answer["events"].as_array().unwrap().len(), 3);
    assert_eq!(answer["truncated"], false);
    assert_fixture("answer.json", &answer);
    // The test daemon hides from discovery; a project id needs it found.
    std::fs::remove_file(
        daemon
            .root
            .join(cstan_front::projects::UNDISCOVERABLE_MARKER),
    )
    .unwrap();
    let procs = [proc_of(&daemon.root)];
    let by_id = recent::run(&plan_of("precentpage", &epoch, head, true), &procs, 5_000);
    assert_eq!(by_id.exit, 0, "{by_id:?}");
    assert_eq!(by_id.stdout, by_root.stdout);
    // Without --json: one line per event, newest first, nothing on stderr.
    let text = recent::run(&plan_of(root, &epoch, head, false), &[], 5_000);
    assert_eq!(text.exit, 0, "{text:?}");
    assert!(text.stderr.is_empty());
    assert_eq!(text.stdout.lines().count(), 3, "{}", text.stdout);
    assert!(text
        .stdout
        .lines()
        .all(|l| l.contains("integration.finished")));
    assert!(!text.stdout.contains(REAL_KEY_PART));
    // A limit.
    let mut limited = plan_of(root, &epoch, head, false);
    limited.limit = Some(1);
    assert_eq!(recent::run(&limited, &[], 5_000).stdout.lines().count(), 1);
}

const REAL_KEY_PART: &str = "operator-0123456789abcdef";

#[test]
fn the_binary_prints_the_page_and_ignores_the_agent_environment() {
    let daemon = RealDaemon::start("binary", "precentbin", 2, 0);
    let (epoch, head) = daemon.head();
    let out = Command::new(FRONT)
        .args(["recent", "--project"])
        .arg(&daemon.root)
        .args(["--epoch", &epoch, "--head", &head.to_string(), "--json"])
        .current_dir(std::env::temp_dir())
        .env_clear()
        .env("CAPSTAN_SOCKET", "/nonexistent/sock")
        .env("CAPSTAN_TOKEN", "wrong")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let answer = one_line(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(answer["events"].as_array().unwrap().len(), 2);
    assert!(!String::from_utf8_lossy(&out.stdout).contains(REAL_KEY_PART));
}

#[test]
fn a_ledger_without_outcomes_gives_a_truncated_empty_answer_with_exit_0() {
    let daemon = RealDaemon::start("truncated", "precenttrunc", 0, 600);
    let (epoch, head) = daemon.head();
    let root = daemon.root.to_str().unwrap();
    let json = recent::run(&plan_of(root, &epoch, head, true), &[], 5_000);
    assert_eq!(json.exit, 0, "{json:?}");
    let answer = one_line(&json.stdout);
    assert_eq!(answer["events"], json!([]));
    assert_eq!(answer["truncated"], true);
    assert_fixture("truncated_empty.json", &answer);
    let text = recent::run(&plan_of(root, &epoch, head, false), &[], 5_000);
    assert_eq!(
        (text.stdout.as_str(), text.stderr.as_str(), text.exit),
        ("", "", 0)
    );
}

#[test]
fn a_position_the_ledger_does_not_fit_is_the_resync_answer_and_exits_1() {
    let daemon = RealDaemon::start("resync", "precentresync", 1, 0);
    let (epoch, head) = daemon.head();
    let root = daemon.root.to_str().unwrap();
    for plan in [
        plan_of(root, &"0".repeat(16), head, true),
        plan_of(root, &epoch, head + 1, true),
    ] {
        let reply = recent::run(&plan, &[], 5_000);
        assert_eq!(reply.exit, 1, "{reply:?}");
        let answer = one_line(&reply.stdout);
        assert_eq!(answer["epoch"], epoch.as_str());
        assert_eq!(answer["head"], head);
        assert!(answer.get("events").is_none());
    }
    let reply = recent::run(&plan_of(root, &"0".repeat(16), head, true), &[], 5_000);
    assert_fixture("resync.json", &one_line(&reply.stdout));
    let text = recent::run(&plan_of(root, &"0".repeat(16), head, false), &[], 5_000);
    assert_eq!(text.exit, 1);
    assert!(text.stdout.is_empty());
    assert!(text.stderr.contains("resync"), "{}", text.stderr);
}

#[test]
fn the_thirty_first_call_in_a_minute_is_rate_limited() {
    let daemon = RealDaemon::start("rate", "precentrate", 1, 0);
    let (epoch, head) = daemon.head();
    let root = daemon.root.to_str().unwrap();
    let plan = plan_of(root, &epoch, head, true);
    for call in 1..=30 {
        let reply = recent::run(&plan, &[], 5_000);
        assert_eq!(reply.exit, 0, "call {call}: {reply:?}");
    }
    let reply = recent::run(&plan, &[], 5_000);
    assert_eq!(reply.exit, 1, "{reply:?}");
    let line = one_line(&reply.stdout);
    assert_eq!(line["v"], 1);
    assert_eq!(line["error"]["code"], "rejected");
    assert!(line["error"]["message"]
        .as_str()
        .unwrap()
        .starts_with("rate_limited: "));
    assert_fixture("rate_limited.json", &line);
}

// -------------------------------------------------------------------------------------------- refusals and silence

#[test]
fn an_unknown_root_makes_no_socket_and_no_daemon() {
    let missing = std::env::temp_dir().join(format!("cstan-recent-missing-{}", std::process::id()));
    for plan in [
        plan_of(missing.to_str().unwrap(), EPOCH, 1, true),
        plan_of("punknownid", EPOCH, 1, true),
    ] {
        let reply = recent::run(&plan, &[], 200);
        assert_eq!(reply.exit, 1, "{reply:?}");
        assert_eq!(
            one_line(&reply.stdout)["error"]["code"],
            "project_not_found"
        );
    }
    assert!(!missing.exists());
    // A project with no controller: not answering, and no socket made by the call.
    let root = fake_project();
    let reply = recent::run(&plan_of(root.to_str().unwrap(), EPOCH, 1, true), &[], 200);
    assert_eq!(reply.exit, 1);
    assert_eq!(one_line(&reply.stdout)["error"]["code"], "not_answering");
    assert!(!root.join(".capstan/state/control.sock").exists());
}

#[test]
fn a_daemon_that_does_not_know_recent_is_refused_as_it_says() {
    for (name, code, message) in [
        (
            "unknown_command.json",
            "unknown_command",
            "unknown command: feed",
        ),
        (
            "old_usage.json",
            "invalid_request",
            fixture("old_usage.json")["error"]["message"]
                .as_str()
                .unwrap(),
        ),
    ] {
        let root = fake_project();
        serve(
            &root,
            Some(json!({"ok": false, "code": code, "message": message})),
        );
        let reply = recent::run(&plan_of(root.to_str().unwrap(), EPOCH, 1, true), &[], 5_000);
        assert_eq!(reply.exit, 1, "{name}: {reply:?}");
        assert_eq!(one_line(&reply.stdout), fixture(name), "{name}");
    }
}

#[test]
fn a_socket_that_never_answers_is_not_answering() {
    let root = fake_project();
    serve(&root, None);
    let reply = recent::run(&plan_of(root.to_str().unwrap(), EPOCH, 1, true), &[], 200);
    assert_eq!(reply.exit, 1, "{reply:?}");
    assert_eq!(one_line(&reply.stdout)["error"]["code"], "not_answering");
}

#[test]
fn a_bad_key_mode_is_no_operator_key() {
    let root = fake_project();
    std::fs::set_permissions(
        root.join(".capstan/operator.key"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let reply = recent::run(&plan_of(root.to_str().unwrap(), EPOCH, 1, false), &[], 200);
    assert_eq!(reply.exit, 1);
    assert!(reply.stderr.contains("no_operator_key"), "{}", reply.stderr);
    assert!(!reply.stderr.contains(KEY));
}
