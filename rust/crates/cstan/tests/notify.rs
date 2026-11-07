//! `cstan notify test`, the `ntfy:` line of `cstan status` and the ntfy guide. The end-to-end test runs the real binary
//! (which embeds the daemon) against a listener on a local port; it needs `curl` and prints `skipped: curl not found`
//! without it, unless `CAPSTAN_REQUIRE_CURL=1`, which turns a missing curl into a failure.
mod support;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use capstan_wire::js::{self, Value};
use cstan_front::render;
use serde_json::json;

const OPERATOR: &str = "operator-0123456789abcdef0123456789abcdef0123456789";

fn repo_file(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../daemon/tests/fixtures/ntfy_notify_answers.json");
    let text = std::fs::read_to_string(path).expect("the sender's notify fixture");
    js::parse(&text)
        .expect("json")
        .get(name)
        .cloned()
        .unwrap_or_else(|| panic!("fixture {name}"))
}

fn value(text: &str) -> Value {
    js::parse(text).expect("json")
}

// ---- parser and help ----

fn run(args: &[&str], cwd: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cstan"))
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("CAPSTAN_TRUST_DIR", support::trust_dir_of(cwd))
        .env("HOME", "/nonexistent")
        .stdin(Stdio::null())
        .output()
        .expect("cstan runs")
}

#[test]
fn bare_notify_and_unknown_subcommands_print_usage() {
    let here = std::env::temp_dir();
    for args in [
        &["notify"][..],
        &["notify", "nope"],
        &["notify", "test", "extra"],
        &["notify", "--json"],
    ] {
        let out = run(args, &here);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("notify test [--json]"), "{args:?}: {err}");
    }
}

#[test]
fn the_help_and_the_man_page_list_notify() {
    let help = cstan_front::usage::help_text();
    assert!(help.contains("notify test [--json]"), "{help}");
    let page = cstan_front::usage::help_for(&["--man".to_string()], "0").unwrap();
    assert!(page.contains("notify test"), "{page}");
    let one = cstan_front::usage::help_for(&["notify".to_string()], "0").unwrap();
    assert!(one.contains("usage: cstan notify test [--json]"), "{one}");
}

// ---- rendering ----

#[test]
fn the_test_answers_of_the_sender_render() {
    let (lines, ok) = render::ntfy_test_lines(Some(&fixture("ok")));
    assert_eq!(
        (lines, ok),
        (vec!["ntfy: test message sent (HTTP 200)".to_string()], true)
    );
    let (lines, ok) = render::ntfy_test_lines(Some(&fixture("failed")));
    assert_eq!(
        (lines, ok),
        (
            vec!["ntfy: unauthorized: HTTP 401 unauthorized".to_string()],
            false
        )
    );
    let (lines, ok) = render::ntfy_test_lines(Some(&fixture("secret_missing")));
    assert_eq!(
        (lines, ok),
        (
            vec!["ntfy: secret_missing (secret: NTFY_TOKEN)".to_string()],
            false
        )
    );
}

#[test]
fn a_long_or_dirty_error_is_cut_and_cleaned() {
    let long = "x".repeat(2000);
    let answer = value(&format!(
        "{{\"ok\":false,\"reason\":\"bad\\u001b[31mreason\",\"error\":\"line\\none\\u0007 {long}\",\"secretSource\":\"/home/u/token\"}}"
    ));
    let (lines, ok) = render::ntfy_test_lines(Some(&answer));
    assert!(!ok);
    assert_eq!(lines.len(), 1);
    assert!(!lines[0].chars().any(char::is_control), "{:?}", lines[0]);
    assert!(lines[0].chars().count() < 500, "{}", lines[0]);
    assert!(
        lines[0].contains("…") && lines[0].ends_with("(secret: /home/u/token)"),
        "{}",
        lines[0]
    );
}

#[test]
fn the_status_line_renders_counters_and_the_last_error() {
    let status = value(
        r#"{"ntfy":{"enabled":true,"server":"notify.example.org","sent":12,"failed":1,"dropped":0,"queued":0,
        "lastSentAt":null,"future":{"x":1},"lastError":{"at":"2026-11-04T10:42:07.000Z","reason":"unauthorized","httpStatus":401}}}"#,
    );
    assert_eq!(
        render::ntfy_status_line(Some(&status)).as_deref(),
        Some("ntfy: on -> notify.example.org, sent 12, failed 1, dropped 0 (last error 10:42 unauthorized)")
    );
    let quiet = value(
        r#"{"ntfy":{"enabled":true,"server":"notify.example.org","sent":0,"failed":0,"dropped":0,"lastError":null}}"#,
    );
    assert_eq!(
        render::ntfy_status_line(Some(&quiet)).as_deref(),
        Some("ntfy: on -> notify.example.org, sent 0, failed 0, dropped 0")
    );
    let missing = value(
        r#"{"ntfy":{"enabled":true,"server":"notify.example.org","sent":1,"failed":0,"dropped":0}}"#,
    );
    assert_eq!(
        render::ntfy_status_line(Some(&missing)).as_deref(),
        Some("ntfy: on -> notify.example.org, sent 1, failed 0, dropped 0")
    );
    let stale = value(
        r#"{"ntfy":{"enabled":true,"server":"notify.example.org","sent":1,"failed":0,"dropped":0,"stale":true}}"#,
    );
    assert!(render::ntfy_status_line(Some(&stale))
        .unwrap()
        .ends_with(" (stale: controller not running)"));
    assert_eq!(
        render::ntfy_status_line(Some(&value(r#"{"cache":{}}"#))),
        None
    );
    assert_eq!(
        render::ntfy_status_line(Some(&value(r#"{"ntfy":null}"#))),
        None
    );
    assert_eq!(render::ntfy_status_line(None), None);
}

#[test]
fn a_hostile_status_member_is_cleaned() {
    let status = value(&format!(
        r#"{{"ntfy":{{"enabled":true,"server":"evil\u001b[2J{}","sent":1,"failed":0,"dropped":0,
        "lastError":{{"at":"\u001b[31mnot a time","reason":"r\neason{}"}}}}}}"#,
        "h".repeat(500),
        "z".repeat(500)
    ));
    let line = render::ntfy_status_line(Some(&status)).unwrap();
    assert!(!line.chars().any(char::is_control), "{line:?}");
    assert!(line.chars().count() < 200, "{line}");
}

// ---- the guide ----

#[test]
fn the_guide_names_every_event_and_the_divergence_note_exists() {
    let guide = repo_file("docs/reference/ntfy.md");
    for event in capstan_config::NTFY_EVENTS {
        assert!(guide.contains(&format!("`{event}`")), "{event}");
    }
    assert!(guide.contains("example.org"));
    for banned in ["Bearer ", "tk_", "password ="] {
        assert!(!guide.contains(banned), "{banned}");
    }
    for needed in [
        "7.68",
        "7.84",
        "CAPSTAN_REQUIRE_CURL",
        "cstan notify test",
        "restart",
    ] {
        assert!(guide.contains(needed), "{needed}");
    }
    let divergences = repo_file("rust/crates/cstan/tests/divergences/README.md");
    assert!(divergences.lines().any(|l| l == "## `cstan notify`"));
    let commands = repo_file("docs/reference/commands.md");
    assert!(commands.contains("`cstan notify test [--json]`"));
    assert!(repo_file("docs/reference/configuration.md").contains("(ntfy.md)"));
}

// ---- projects ----

struct Project {
    base: PathBuf,
    root: PathBuf,
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = run(&["stop"], &self.root);
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn private(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(output.status.success(), "git {args:?}: {output:?}");
}

/// A scratch project; `extra` is its untracked capstan.local.toml.
fn project(label: &str, extra: &str) -> Project {
    let base = capstan_testkit::short_temp_root()
        .join(format!("capstan-notify-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("proj");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(root.join("a.txt"), "a\n").unwrap();
    std::fs::write(root.join(".git/info/exclude"), ".capstan/\n").unwrap();
    git(&root, &["add", "--all"]);
    git(&root, &["commit", "--quiet", "-m", "chore: initial commit"]);
    let capstan = root.join(".capstan");
    let state = capstan.join("state");
    std::fs::create_dir_all(&state).unwrap();
    private(&capstan, 0o700);
    private(&state, 0o700);
    let config = json!({
        "schemaVersion": 1, "projectId": "pnotifye2e", "name": "Notify",
        "stateDirectory": state.to_string_lossy(),
        "maxSlices": 4, "maxRunMs": 3_600_000, "maxDispatches": 16,
    });
    std::fs::write(root.join(cstan_front::projects::UNDISCOVERABLE_MARKER), "").unwrap();
    for (name, text) in [
        ("project.json", format!("{config:#}\n")),
        ("operator.key", format!("{OPERATOR}\n")),
    ] {
        std::fs::write(capstan.join(name), text).unwrap();
        private(&capstan.join(name), 0o600);
    }
    let toml =
        "schema_version = 1\n[nexora]\ntrack = \"never\"\n[hosts.claude]\nkind = \"claude\"\n\
         [architect]\nenabled = true\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n[roles.architect]\nkind = \"Developer\"\nhost = \"claude\"\n[roles.developer]\nkind = \"Developer\"\nhost = \"claude\"\n";
    std::fs::write(root.join("capstan.toml"), toml).unwrap();
    private(&root.join("capstan.toml"), 0o600);
    support::trust_beside(&root);
    // `[ntfy]` connection keys are honoured only from the untracked local file.
    std::fs::write(root.join("capstan.local.toml"), format!("{extra}\n")).unwrap();
    private(&root.join("capstan.local.toml"), 0o600);
    Project { base, root }
}

impl Project {
    fn state(&self) -> PathBuf {
        self.root.join(".capstan/state")
    }
    fn cstan(&self, args: &[&str]) -> Output {
        run(args, &self.root)
    }
    fn status(&self) -> String {
        let out = self.cstan(&["status"]);
        assert!(
            out.status.success(),
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
    fn write_status_file(&self, body: &serde_json::Value) {
        std::fs::write(self.state().join("ntfy-status.json"), body.to_string()).unwrap();
    }
}

const ENABLED: &str =
    "[ntfy]\nenabled = true\nserver = \"https://notify.example.org\"\ntopic = \"proj\"\n";

fn member(pid: i64, start: serde_json::Value) -> serde_json::Value {
    json!({
        "enabled": true, "server": "notify.example.org", "events": ["user_question"], "sent": 12, "failed": 1,
        "dropped": 0, "queued": 0, "lastSentAt": null,
        "lastError": {"at": "2026-11-04T10:42:07.000Z", "reason": "unauthorized", "httpStatus": 401},
        "writtenAt": "2026-11-04T10:43:00.000Z", "pid": pid, "pidStartTime": start, "futureKey": [1, 2],
    })
}

const LINE: &str =
    "ntfy: on -> notify.example.org, sent 12, failed 1, dropped 0 (last error 10:42 unauthorized)";

fn ntfy_lines(status: &str) -> Vec<&str> {
    status.lines().filter(|l| l.starts_with("ntfy:")).collect()
}

#[test]
fn offline_status_marks_a_dead_pid_or_a_reused_pid_stale_and_a_live_one_not() {
    let p = project("offline", ENABLED);
    // A pid no process has.
    p.write_status_file(&member(4_194_000, json!("123")));
    let stale = format!("{LINE} (stale: controller not running)");
    assert_eq!(ntfy_lines(&p.status()), [stale.as_str()]);
    // A live pid whose start time differs.
    let me = i64::from(std::process::id());
    p.write_status_file(&member(me, json!("not-my-start-time")));
    assert_eq!(ntfy_lines(&p.status()), [stale.as_str()]);
    // A file without a start time cannot prove anything.
    p.write_status_file(&member(me, serde_json::Value::Null));
    assert_eq!(ntfy_lines(&p.status()), [stale.as_str()]);
    // A live pid with its own start time.
    p.write_status_file(&member(me, json!(capstan_process::start_time(me))));
    assert_eq!(ntfy_lines(&p.status()), [LINE]);
    // The JSON status carries the member too.
    let out = p.cstan(&["status", "--json"]);
    let status: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(status["ntfy"]["sent"], 12);
}

#[test]
fn offline_status_prints_nothing_for_a_disabled_section_or_an_unusable_file() {
    let p = project("quiet", ENABLED);
    // No file yet.
    assert!(ntfy_lines(&p.status()).is_empty());
    // A corrupt file.
    std::fs::write(p.state().join("ntfy-status.json"), "{not json").unwrap();
    assert!(ntfy_lines(&p.status()).is_empty());
    // A file that is not an object.
    std::fs::write(p.state().join("ntfy-status.json"), "[1]").unwrap();
    assert!(ntfy_lines(&p.status()).is_empty());
    // An unreadable file (a directory in its place cannot be read as a file).
    std::fs::remove_file(p.state().join("ntfy-status.json")).unwrap();
    std::fs::create_dir(p.state().join("ntfy-status.json")).unwrap();
    assert!(ntfy_lines(&p.status()).is_empty());
    std::fs::remove_dir(p.state().join("ntfy-status.json")).unwrap();
    // An invalid capstan.toml: status behaves as it did and shows no ntfy line.
    p.write_status_file(&member(4_194_000, json!("1")));
    let good = ntfy_lines(&p.status()).len();
    assert_eq!(good, 1);
    std::fs::write(p.root.join("capstan.toml"), "this is = not [valid").unwrap();
    assert!(ntfy_lines(&p.status()).is_empty());
    // A disabled section.
    let q = project("disabled", "[ntfy]\nenabled = false\n");
    q.write_status_file(&member(4_194_000, json!("1")));
    assert!(ntfy_lines(&q.status()).is_empty());
}

// ---- end to end ----

fn curl_available() -> bool {
    let found = Command::new("curl")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !found {
        assert!(
            std::env::var("CAPSTAN_REQUIRE_CURL").map_or(true, |v| v != "1"),
            "curl not found and CAPSTAN_REQUIRE_CURL=1"
        );
        println!("skipped: curl not found");
    }
    found
}

/// Serves one request with `status` and sends the request head back.
fn listener(status: u16) -> (u16, mpsc::Receiver<String>) {
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = socket.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in socket.incoming() {
            let Ok(mut stream) = stream else { return };
            stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
            let mut seen = Vec::new();
            let mut buf = [0u8; 4096];
            while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => seen.extend_from_slice(&buf[..n]),
                }
            }
            let head = String::from_utf8_lossy(&seen).into_owned();
            let reply = format!(
                "HTTP/1.1 {status} X\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
            );
            let _ = stream.write_all(reply.as_bytes());
            let _ = tx.send(head);
        }
    });
    (port, rx)
}

#[test]
fn notify_test_publishes_with_the_token_of_the_file() {
    if !curl_available() {
        return;
    }
    let (port, heads) = listener(200);
    let p = project("e2e", "");
    let token_file = p.base.join("token");
    std::fs::write(&token_file, "tok-0123456789\n").unwrap();
    private(&token_file, 0o600);
    // The section is written before the controller starts.
    std::fs::write(
        p.root.join("capstan.local.toml"),
        format!(
            "[ntfy]\nenabled = true\nserver = \"http://127.0.0.1:{port}\"\ntopic = \"proj\"\ntoken_file = \"{}\"\n",
            token_file.display()
        ),
    )
    .unwrap();
    let out = p.cstan(&["notify", "test"]);
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "{text}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(text, "ntfy: test message sent (HTTP 200)\n");
    let head = heads
        .recv_timeout(Duration::from_secs(10))
        .expect("the listener saw the request");
    assert!(
        head.starts_with("POST / ") && head.contains("\"topic\":\"proj\""),
        "{head}"
    );
    let auth = head
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
        .expect("an Authorization header");
    assert_eq!(
        auth.split_once(':').unwrap().1.trim(),
        "Bearer tok-0123456789"
    );
    // --json prints the controller's answer.
    let out = p.cstan(&["notify", "test", "--json"]);
    let answer: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        (answer["ok"].clone(), answer["httpStatus"].clone()),
        (json!(true), json!(200))
    );
    // The running controller reports the sends in `cstan status`.
    assert!(!p.status().is_empty());
}

#[test]
fn notify_test_names_the_missing_secret_and_a_project_without_ntfy_refuses() {
    if !curl_available() {
        return;
    }
    let p = project("secret", "[ntfy]\nenabled = true\nserver = \"http://127.0.0.1:9\"\ntopic = \"proj\"\ntoken_env = \"NTFY_E2E_UNSET_TOKEN\"\n");
    let out = p.cstan(&["notify", "test"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "ntfy: secret_missing (secret: NTFY_E2E_UNSET_TOKEN)\n"
    );
    let none = project("none", "");
    let out = none.cstan(&["notify", "test"]);
    assert_ne!(out.status.code(), Some(0));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("ntfy is not enabled in capstan.toml") && !err.contains("not_configured"),
        "{err}"
    );
}
