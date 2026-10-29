#![cfg(debug_assertions)]
#![allow(dead_code)]
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use capstan_client::{call_feed, FeedCall, Response};
use capstan_daemon::deps::SOCKET_NAME;
use capstan_kernel::areas::feed_cursor::{decode_cursor, CursorEntry};
use capstan_kernel::helpers::execute;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::types::MutationContext;
use capstan_kernel::{Core, SystemEnv};
use capstan_wire::js;
use serde_json::{json, Value};

// ------------------------------------------------------------------------------------------------- harness
const OPERATOR: &str = "operator-0123456789abcdef0123456789abcdef0123456789";
const PROJECT_ID: &str = "pevents";
const AT: &str = "2026-01-01T00:00:00.000Z";
const HOST_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// Credentials of the actors a seed may act as.
struct Creds {
    pm: String,
    dev: String,
    dev_actor: String,
}

struct Project {
    base: PathBuf,
    root: PathBuf,
}

impl Drop for Project {
    fn drop(&mut self) {
        // Stop the daemon this test started before its directory goes.
        let _ = Command::new(env!("CARGO_BIN_EXE_cstan"))
            .arg("stop")
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", HOST_PATH)
            .env("HOME", "/nonexistent")
            .stdin(Stdio::null())
            .output();
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

fn step(core: &Core, op: &str, args: Value) -> Value {
    let context = json!({
        "credential": OPERATOR,
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

fn context(core: &Core, credential: &str) -> MutationContext {
    MutationContext {
        credential: credential.to_string(),
        request_id: format!("req-{}", core.state_version().unwrap()),
        idempotency_key: format!("idem-{}", core.state_version().unwrap()),
        expected_version: core.state_version().unwrap(),
        input_revision: core.input_revision().unwrap(),
    }
}

/// A scratch git project with a controller directory, a ledger holding a PM and a developer, and whatever `seed` writes
/// before any daemon opens it. No daemon runs yet.
fn project(label: &str, seed: impl FnOnce(&Core, &Creds)) -> (Project, Creds) {
    let base = std::env::temp_dir().join(format!("capstan-events-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let root = base.join("proj");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(root.join("a.txt"), "a\n").unwrap();
    std::fs::write(root.join(".git/info/exclude"), ".capstan/\n").unwrap();
    git(&root, &["add", "--all"]);
    git(&root, &["commit", "--quiet", "-m", "chore: initial commit"]);
    let root = std::fs::canonicalize(&root).unwrap();
    let capstan = root.join(".capstan");
    let state = capstan.join("state");
    std::fs::create_dir_all(&state).unwrap();
    private(&capstan, 0o700);
    private(&state, 0o700);
    let config = json!({
        "schemaVersion": 1, "projectId": PROJECT_ID, "name": "Events",
        "stateDirectory": state.to_string_lossy(),
        "maxSlices": 4, "maxRunMs": 3_600_000, "maxDispatches": 16,
    });
    // No undiscoverable marker: this suite polls its own daemon with `cstan events --root`, which the marker would drop.
    for (name, text) in [
        ("project.json", format!("{config:#}\n")),
        ("operator.key", format!("{OPERATOR}\n")),
    ] {
        std::fs::write(capstan.join(name), text).unwrap();
        private(&capstan.join(name), 0o600);
    }
    let toml = "schema_version = 1\n[nexora]\ntrack = \"never\"\n[hosts.claude]\nkind = \"claude\"\n\
         [architect]\nenabled = true\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n[roles.architect]\nkind = \"Developer\"\nhost = \"claude\"\n[roles.developer]\nkind = \"Developer\"\nhost = \"claude\"\n";
    std::fs::write(root.join("capstan.toml"), toml).unwrap();
    private(&root.join("capstan.toml"), 0o600);
    let project = Project { base, root };

    let initial = capstan_kernel::types::InitialProject {
        project_id: PROJECT_ID.into(),
        name: "Events".into(),
        owner_credential: OPERATOR.into(),
        initial_inputs: capstan_daemon::run::placeholder_inputs(),
    };
    let core = Core::open(
        &state,
        &initial,
        &KernelOptions {
            workspace_root: Some(project.root.clone()),
            ..KernelOptions::default()
        },
        Box::new(SystemEnv),
    )
    .expect("the ledger opens");
    step(
        &core,
        "syncRoleDefinitions",
        json!([[
            {"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)},
            {"name": "pm", "kind": "PM", "host": "claude", "configHash": "b".repeat(64)},
        ]]),
    );
    let mut credentials = Vec::new();
    let mut actors = Vec::new();
    for (agent, role_name, kind) in [("pm-1", "pm", "PM"), ("dev-1", "developer", "Developer")] {
        let seat = format!("seat-{agent}");
        step(
            &core,
            "createSeat",
            json!([{"seatId": seat, "name": seat, "role": kind}]),
        );
        let actor = step(
            &core,
            "createActor",
            json!([{"displayName": agent, "role": kind, "seatId": seat}]),
        );
        step(
            &core,
            "registerAgent",
            json!([{"agentId": agent, "roleName": role_name, "seatId": seat, "actorId": actor["actorId"]}]),
        );
        credentials.push(actor["credential"].as_str().unwrap().to_string());
        actors.push(actor["actorId"].as_str().unwrap().to_string());
    }
    let creds = Creds {
        pm: credentials[0].clone(),
        dev: credentials[1].clone(),
        dev_actor: actors[1].clone(),
    };
    seed(&core, &creds);
    core.close();
    (project, creds)
}

/// An approved plan, written as a row the way `verify_e2e` writes its plan.
fn seed_plan(project: &Project, creds: &Creds) {
    let state = project.root.join(".capstan/state");
    let database = capstan_ledger::resolve_database_path(&state).unwrap();
    let db =
        capstan_ledger::open_database(&database, &capstan_ledger::OpenOptions::default()).unwrap();
    execute(
        &db,
        "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, current_revision, approved_revision, created_at, updated_at)
         VALUES (?, 'plan-1', 1, 'Events plan', 'normal', 'draft', 'operator', 0, NULL, ?, ?)",
        [PROJECT_ID, AT, AT],
    )
    .unwrap();
    let body = json!({"title": "Events plan", "packages": [{"id": "p1", "type": "feature", "estimate_hours": 2, "depends_on": []}]}).to_string();
    execute(
        &db,
        "INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
         VALUES (?, 'plan-1', 1, ?, ?, ?, 'dev-1', ?, ?)",
        [PROJECT_ID, "1".repeat(40).as_str(), body.as_str(), "c".repeat(64).as_str(), creds.dev_actor.as_str(), AT],
    )
    .unwrap();
    execute(
        &db,
        "UPDATE plans SET state = 'approved', current_revision = 1, approved_revision = 1 WHERE project_id = ? AND plan_id = 'plan-1'",
        [PROJECT_ID],
    )
    .unwrap();
}

/// What most tests start from: one waiting message, one developer, one approved plan, and the run item.
fn basic(label: &str) -> Project {
    let (project, creds) = project(label, |core, creds| {
        message(core, creds, "please look at plan 1");
    });
    seed_plan(&project, &creds);
    project
}

fn message(core: &Core, creds: &Creds, body: &str) {
    core.enqueue_message(
        &context(core, &creds.dev),
        &json!({"recipientAgentId": "pm-1", "body": body, "actionNeeded": true}),
    )
    .unwrap_or_else(|e| panic!("message: {e}"));
}

impl Project {
    fn cstan(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_cstan"))
            .args(args)
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", HOST_PATH)
            .env("HOME", "/nonexistent")
            .stdin(Stdio::null())
            .output()
            .expect("cstan runs")
    }

    fn out(&self, args: &[&str]) -> String {
        let output = self.cstan(args);
        assert!(
            output.status.success(),
            "cstan {args:?} failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Starts the daemon (a routed call does) and returns once it answers.
    fn start(&self) {
        self.out(&["ping"]);
        // The daemon may write rows of its own as it starts; wait until the head holds still.
        let mut last = self.head();
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(1500));
            let now = self.head();
            if now == last {
                return;
            }
            last = now;
        }
        panic!("the ledger head does not settle");
    }

    /// One write while a stream runs: the whole run paused and resumed (two `run.changed` rows), else a plan opened.
    fn poke(&self) {
        if self.cstan(&["pause", "--reason", "t"]).status.success() {
            self.out(&["resume", "--reason", "t"]);
        } else {
            self.out(&["plan", "open", "normal", "t"]);
        }
    }

    fn socket(&self) -> PathBuf {
        self.root.join(".capstan/state").join(SOCKET_NAME)
    }

    /// One `feed` call over the socket as the operator.
    fn feed(&self, call: &FeedCall) -> Value {
        match call_feed(&self.socket(), OPERATOR, call, 10_000) {
            Ok(Response::Ok {
                result: Some(result),
            }) => serde_json::from_str(&js::stringify(&result, 0).to_utf8_lossy()).unwrap(),
            Ok(other) => panic!("feed {call:?}: {other:?}"),
            Err(_) => panic!("feed {call:?}: no answer"),
        }
    }

    fn head(&self) -> (String, u64) {
        let head = self.feed(&FeedCall::Head);
        (
            head["epoch"].as_str().unwrap().to_string(),
            head["head"].as_u64().unwrap(),
        )
    }

    fn state_version(&self) -> u64 {
        let status: Value = serde_json::from_str(&self.out(&["status", "--json"])).unwrap();
        status["stateVersion"].as_u64().expect("a stateVersion")
    }

    fn stream(&self, extra: &[&str]) -> Stream {
        Stream::spawn(&self.root, &self.base, extra)
    }
}

/// A running `cstan events`: its lines arrive from a reader thread.
struct Stream {
    child: Child,
    rx: Receiver<Vec<u8>>,
    quit: Arc<AtomicBool>,
    err: Arc<Mutex<Vec<u8>>>,
    /// Every line read so far, parsed and as written (newline included).
    lines: Vec<Value>,
    raw: Vec<String>,
}

impl Stream {
    fn spawn(root: &Path, cwd: &Path, extra: &[&str]) -> Stream {
        let mut child = Command::new(env!("CARGO_BIN_EXE_cstan"))
            .args(["events", "--poll-ms", "1000", "--root"])
            .arg(root)
            .args(extra)
            .current_dir(cwd)
            .env_clear()
            .env("PATH", HOST_PATH)
            .env("HOME", "/nonexistent")
            .env("CSTAN_EVENTS_NO_PROCESS_SCAN", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("cstan runs");
        let (tx, rx) = channel();
        let quit = Arc::new(AtomicBool::new(false));
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let flag = quit.clone();
        std::thread::spawn(move || {
            let mut line = Vec::new();
            // Leaving the loop drops the reader and so closes the pipe.
            while !flag.load(Ordering::SeqCst) {
                line.clear();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if tx.send(line.clone()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let err = Arc::new(Mutex::new(Vec::new()));
        let mut stderr = child.stderr.take().unwrap();
        let sink = err.clone();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            while let Ok(n) = stderr.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                sink.lock().unwrap().extend_from_slice(&chunk[..n]);
            }
        });
        Stream {
            child,
            rx,
            quit,
            err,
            lines: Vec::new(),
            raw: Vec::new(),
        }
    }

    fn take(&mut self, bytes: Vec<u8>) -> Value {
        let text = String::from_utf8(bytes).expect("a stdout line is UTF-8");
        assert!(text.ends_with('\n'), "a line ends with a newline: {text:?}");
        let value: Value = serde_json::from_str(text.trim_end_matches('\n'))
            .unwrap_or_else(|e| panic!("a stdout line is JSON ({e}): {text:?}"));
        self.raw.push(text);
        self.lines.push(value.clone());
        value
    }

    /// The next line within `secs`; `None` at the end of the stream or when the time is out.
    fn next(&mut self, secs: u64) -> Option<Value> {
        let bytes = self.rx.recv_timeout(Duration::from_secs(secs)).ok()?;
        Some(self.take(bytes))
    }

    /// Lines up to and including the first that `done` accepts, within `secs` in all.
    fn until(&mut self, secs: u64, done: impl Fn(&Value) -> bool) -> Vec<Value> {
        let deadline = Instant::now() + Duration::from_secs(secs);
        let mut got = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now()).as_secs();
            match self.next(left.max(1)) {
                Some(line) => {
                    let stop = done(&line);
                    got.push(line);
                    if stop {
                        return got;
                    }
                }
                None => panic!(
                    "the stream did not give the line in {secs} s; lines so far:\n{}",
                    self.raw.concat()
                ),
            }
            assert!(
                Instant::now() < deadline,
                "the stream did not give the line in {secs} s; lines so far:\n{}",
                self.raw.concat()
            );
        }
    }

    /// Lines until the first `kind` line.
    fn until_kind(&mut self, secs: u64, kind: &str) -> Vec<Value> {
        self.until(secs, |l| l["kind"] == kind)
    }

    /// The stream's lines up to the first `cursor` line after its `snapshotEnd` (30 s).
    fn snapshot(&mut self) -> Vec<Value> {
        let mut got = self.until_kind(30, "snapshotEnd");
        got.extend(self.until_kind(10, "cursor"));
        got
    }

    /// The last cursor line seen, as text.
    fn last_cursor(&self) -> String {
        self.lines
            .iter()
            .rev()
            .find(|l| l["kind"] == "cursor")
            .and_then(|l| l["cursor"].as_str())
            .expect("a cursor line was seen")
            .to_string()
    }

    fn sigterm(&self) {
        // SAFETY: signals the child this test started.
        unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) };
    }

    /// Waits for the exit; the code, `None` when a signal ended it.
    fn exit(&mut self, secs: u64) -> Option<i32> {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code();
            }
            assert!(Instant::now() < deadline, "no exit within {secs} s");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Reads to the end of the stream (the process has exited or will).
    fn drain(&mut self) -> Vec<Value> {
        let mut got = Vec::new();
        while let Some(line) = self.next(10) {
            got.push(line);
        }
        got
    }

    /// SIGTERM, then the lines to the end and the exit code (10 s).
    fn finish(&mut self) -> (Vec<Value>, Option<i32>) {
        self.sigterm();
        let rest = self.drain();
        let code = self.exit(10);
        (rest, code)
    }

    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.err.lock().unwrap()).into_owned()
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.sigterm();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn kinds(lines: &[Value]) -> Vec<&str> {
    lines.iter().map(|l| l["kind"].as_str().unwrap()).collect()
}

/// The cursor line's entries.
fn entries(line: &Value) -> BTreeMap<String, CursorEntry> {
    decode_cursor(line["cursor"].as_str().unwrap()).expect("a cursor line decodes")
}

// ------------------------------------------------------------------------------------------------- the document

const DOC: &str = include_str!("../../../../docs/remote-protocol.md");

/// The text of section `number` (from its heading to the next).
fn section(number: u32) -> &'static str {
    let start = DOC
        .find(&format!("\n## {number}. "))
        .unwrap_or_else(|| panic!("section {number}"))
        + 1;
    let rest = &DOC[start..];
    let end = rest[3..].find("\n## ").map_or(rest.len(), |at| at + 3);
    &rest[..end]
}

/// The words between backticks.
fn ticked(text: &str) -> Vec<String> {
    text.split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

/// The rows of the table whose header line starts with `header`, each as its cells.
fn table(text: &str, header: &str) -> Vec<Vec<String>> {
    let mut lines = text.lines().skip_while(|l| !l.starts_with(header));
    assert!(lines.next().is_some(), "no table {header}");
    lines
        .skip(1)
        .take_while(|l| l.starts_with('|'))
        .map(|l| {
            l.trim_matches('|')
                .split('|')
                .map(|c| c.trim().to_string())
                .collect()
        })
        .collect()
}

struct Spec {
    kinds: BTreeMap<String, BTreeSet<String>>,
    states: BTreeSet<String>,
    reasons: BTreeSet<String>,
    codes: BTreeSet<String>,
}

fn spec() -> Spec {
    let kinds = table(section(2), "| kind |")
        .into_iter()
        .map(|row| {
            let kind = ticked(&row[0]).remove(0);
            (kind, ticked(&row[1]).into_iter().collect())
        })
        .collect();
    let states = table(section(2), "| state |")
        .into_iter()
        .map(|row| ticked(&row[0]).remove(0))
        .collect();
    let six = section(6);
    let from = six
        .find("`resync` reasons are")
        .expect("the resync reasons");
    let sentence = &six[from..];
    let sentence = &sentence[..sentence.find(". ").unwrap()];
    let reasons = ticked(sentence).into_iter().skip(1).collect();
    let from = six.find("Error codes.").expect("the error codes");
    let paragraph = &six[from..];
    let paragraph = &paragraph[..paragraph.find("\n\n").unwrap()];
    let codes = ticked(paragraph).into_iter().collect();
    Spec {
        kinds,
        states,
        reasons,
        codes,
    }
}

// ------------------------------------------------------------------------------------------------- the checks

/// One stdout line against the doc: size, `v`, exactly its kind's fields, and the values the doc lists.
fn check_line(spec: &Spec, raw: &str, line: &Value, seen: &mut BTreeSet<String>) {
    assert!(raw.len() <= 65536, "a line of {} bytes", raw.len());
    let object = line.as_object().unwrap_or_else(|| panic!("{raw}"));
    assert_eq!(object["v"], 1, "{raw}");
    let kind = object["kind"].as_str().unwrap_or_else(|| panic!("{raw}"));
    let fields = spec
        .kinds
        .get(kind)
        .unwrap_or_else(|| panic!("a kind the doc does not list: {raw}"));
    let mut expected: BTreeSet<String> = fields.clone();
    expected.extend(["v".to_string(), "kind".to_string()]);
    let actual: BTreeSet<String> = object.keys().cloned().collect();
    assert_eq!(actual, expected, "the fields of {kind}: {raw}");
    seen.insert(kind.to_string());
    match kind {
        "hello" => {
            assert!(line["version"].is_string() && line["host"].is_string());
            assert!(line["projects"].is_u64());
        }
        "project" => {
            let state = line["state"].as_str().unwrap();
            assert!(spec.states.contains(state), "state {state}");
            assert!(line["projectId"].is_string() && line["root"].is_string());
            if state == "ready" {
                assert!(line["epoch"].is_string() && line["head"].is_u64(), "{raw}");
            } else {
                assert!(line["epoch"].is_null() && line["head"].is_null(), "{raw}");
            }
        }
        "snapshot" => {
            assert!(
                ["waiting", "agents", "plans", "run"].contains(&line["section"].as_str().unwrap()),
                "{raw}"
            );
            assert!(line["projectId"].is_string() && line["key"].is_string());
            assert!(line["item"].is_object(), "{raw}");
        }
        "snapshotEnd" => assert!(line["projectId"].is_string() && line["head"].is_u64()),
        "event" => {
            let event = line["event"].as_object().expect("an event object");
            for name in [
                "id", "seq", "k", "ts", "type", "actorId", "entity", "from", "to", "body",
            ] {
                assert!(event.contains_key(name), "event field {name}: {raw}");
            }
            assert!(event["body"]["effects"].is_array(), "{raw}");
            assert!(
                line["event"].to_string().len() <= 49152,
                "an event over 49152 bytes"
            );
        }
        "cursor" => {
            decode_cursor(line["cursor"].as_str().unwrap()).expect("a cursor that decodes");
        }
        "heartbeat" => {
            let ts = line["ts"].as_str().unwrap();
            assert!(ts.ends_with('Z') && ts.contains('T'), "{ts}");
        }
        "resync" => {
            let reason = line["reason"].as_str().unwrap();
            assert!(spec.reasons.contains(reason), "reason {reason}");
            assert!(line["projectId"].is_null() || line["projectId"].is_string());
        }
        "error" => {
            let code = line["code"].as_str().unwrap();
            assert!(spec.codes.contains(code), "code {code}");
            assert!(line["message"].is_string());
        }
        other => panic!("kind {other}"),
    }
}

fn check_stream(spec: &Spec, stream: &Stream, seen: &mut BTreeSet<String>) {
    assert_eq!(stream.raw.len(), stream.lines.len());
    for (raw, line) in stream.raw.iter().zip(&stream.lines) {
        assert!(!raw.trim().is_empty(), "a blank line");
        check_line(spec, raw, line, seen);
    }
    // The hello says how many project lines follow it.
    if stream.lines.first().is_some_and(|l| l["kind"] == "hello") {
        let following = stream.lines[1..]
            .iter()
            .take_while(|l| l["kind"] == "project")
            .count();
        assert_eq!(stream.lines[0]["projects"], following);
    }
}

// ------------------------------------------------------------------------------------------------- tests

#[test]
fn the_limits_and_the_synopsis_of_the_doc_equal_the_code() {
    use capstan_kernel::areas::{event_feed, snapshot_feed};
    use capstan_kernel::helpers::MAX_EVENT_BYTES;
    let limits = cstan_front::events::Limits::default();
    let value = |cell: &str| -> u64 {
        cell.trim_end_matches(" ms")
            .parse()
            .unwrap_or_else(|_| panic!("a number: {cell}"))
    };
    let rows = table(section(10), "| limit |");
    assert!(rows.len() >= 8, "{rows:?}");
    for row in &rows {
        let (name, number) = (row[0].as_str(), value(&row[1]));
        let code: Vec<u64> = match name {
            n if n.starts_with("`event_feed::MAX_LIMIT`") => vec![event_feed::MAX_LIMIT as u64],
            n if n.starts_with("`snapshot_feed::MAX_LIMIT`") => {
                vec![snapshot_feed::MAX_LIMIT as u64]
            }
            n if n.starts_with("`event_feed::MAX_SCAN_ROWS`") => {
                vec![event_feed::MAX_SCAN_ROWS as u64]
            }
            n if n.starts_with("`snapshot_feed::MAX_READS`") => {
                vec![snapshot_feed::MAX_READS as u64]
            }
            n if n.starts_with("serialized bytes per page") => vec![
                event_feed::MAX_PAGE_BYTES as u64,
                snapshot_feed::MAX_PAGE_BYTES as u64,
            ],
            "bytes of one event" => vec![MAX_EVENT_BYTES as u64],
            "bytes of one stream line" => vec![limits.line_bytes as u64],
            "minimum poll interval" => vec![limits.min_poll_ms],
            other => panic!("a limit this test does not know: {other}"),
        };
        for c in code {
            assert_eq!(c, number, "{name}");
        }
    }
    let caps = table(section(8), "| cap |");
    for row in &caps {
        let number = value(&row[1]);
        let code = match ticked(&row[0])[0].as_str() {
            "event_feed::TEXT_CAP" => event_feed::TEXT_CAP as u64,
            "event_feed::REASON_CAP" => event_feed::REASON_CAP as u64,
            "event_feed::OPTIONS_CAP" => event_feed::OPTIONS_CAP as u64,
            "event_feed::OPTION_TEXT_CAP" => event_feed::OPTION_TEXT_CAP as u64,
            "snapshot_feed::TEXT_CAP" => snapshot_feed::TEXT_CAP as u64,
            other => panic!("a cap this test does not know: {other}"),
        };
        assert_eq!(code, number, "{row:?}");
    }
    assert_eq!(caps.len(), 5);

    // "The stream follows at most 64 projects".
    let ten = section(10);
    let from = ten.find("follows at most ").expect("the project limit") + "follows at most ".len();
    let digits: String = ten[from..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    assert_eq!(digits.parse::<usize>().unwrap(), limits.projects);

    // The synopsis of section 1 is the usage line.
    let line = section(1)
        .lines()
        .find(|l| l.starts_with("ssh <host> cstan events"))
        .expect("the synopsis");
    let synopsis = line.strip_prefix("ssh <host> cstan ").unwrap();
    let entry = cstan_front::usage::ENTRIES
        .iter()
        .find(|e| e.command == "events")
        .expect("a usage entry for events");
    assert_eq!(entry.syntax, synopsis);
}

#[test]
fn the_tables_of_the_doc_parse() {
    let spec = spec();
    assert_eq!(spec.kinds.len(), 9, "{:?}", spec.kinds.keys());
    assert_eq!(spec.states.len(), 6, "{:?}", spec.states);
    assert_eq!(spec.reasons.len(), 5, "{:?}", spec.reasons);
    for code in ["bad_cursor", "internal", "invalid_request"] {
        assert!(spec.codes.contains(code), "{code}");
    }
}

#[test]
fn every_line_of_every_scenario_is_what_the_doc_says_and_all_nine_kinds_appear() {
    let spec = spec();
    let mut seen = BTreeSet::new();
    let project = basic("proto");
    project.start();

    // A snapshot, events, an idle stretch that gives a heartbeat, then the pipe closed.
    let mut stream = project.stream(&[]);
    stream.snapshot();
    project.poke();
    stream.until_kind(15, "event");
    stream.until_kind(10, "cursor");
    stream.until_kind(30, "heartbeat");
    stream.quit.store(true, Ordering::SeqCst);
    project.poke();
    std::thread::sleep(Duration::from_secs(2));
    project.poke();
    assert_eq!(stream.exit(10), Some(0), "exit 0 after the pipe closed");
    check_stream(&spec, &stream, &mut seen);

    // A cursor that does not decode.
    let mut stream = project.stream(&["--cursor", "c1.%%%"]);
    stream.snapshot();
    let (_, code) = stream.finish();
    assert_eq!(code, Some(0));
    check_stream(&spec, &stream, &mut seen);

    // Too many roots: an error line, exit 1, and nothing else.
    let extra: Vec<String> = (0..64)
        .flat_map(|n| {
            [
                "--root".to_string(),
                project
                    .base
                    .join(format!("root-{n}"))
                    .to_string_lossy()
                    .into_owned(),
            ]
        })
        .collect();
    let extra: Vec<&str> = extra.iter().map(String::as_str).collect();
    let mut stream = project.stream(&extra);
    stream.drain();
    assert_eq!(stream.exit(10), Some(1));
    assert_eq!(kinds(&stream.lines), ["error"], "{:?}", stream.lines);
    assert_eq!(stream.lines[0]["code"], "invalid_request");
    check_stream(&spec, &stream, &mut seen);

    let all: BTreeSet<String> = spec.kinds.keys().cloned().collect();
    assert_eq!(seen, all, "every kind of the doc was produced and checked");
}
