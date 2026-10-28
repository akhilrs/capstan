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
use capstan_kernel::areas::feed_cursor::{decode_cursor, encode_cursor, CursorEntry};
use capstan_kernel::canonical::sha256;
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

    /// Waits until the ledger head has held still for 6 s: the daemon retires the seeded developer some time after it starts.
    fn settle(&self) {
        let mut last = self.head();
        let mut still = 0;
        for _ in 0..60 {
            std::thread::sleep(Duration::from_secs(1));
            let now = self.head();
            still = if now == last { still + 1 } else { 0 };
            last = now;
            if still >= 6 {
                return;
            }
        }
        panic!("the ledger head does not settle");
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

// ------------------------------------------------------------------------------------------------- helpers of the scenarios

/// (section, key) of the snapshot lines.
fn snapshot_keys(lines: &[Value]) -> Vec<(String, String)> {
    lines
        .iter()
        .filter(|l| l["kind"] == "snapshot")
        .map(|l| {
            (
                l["section"].as_str().unwrap().to_string(),
                l["key"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// The cursor of a project that stands at its first event, built from the first page of its feed. Everything the ledger
/// holds after that first event is "new" to a stream resumed from it.
fn cursor_at_start(project: &Project) -> String {
    let page = project.feed(&FeedCall::Page {
        after_seq: 0,
        after_k: 0,
        limit: 1,
        anchor: None,
    });
    let scanned = &page["scanned"];
    let entry = CursorEntry {
        e: page["epoch"].as_str().unwrap().to_string(),
        s: scanned["seq"].as_u64().unwrap(),
        k: scanned["k"].as_u64().unwrap() as u32,
        h: page["anchor"]
            .as_str()
            .unwrap_or("0000000000000000")
            .to_string(),
        r: project.root.to_string_lossy().into_owned(),
        d: None,
    };
    encode_cursor(&BTreeMap::from([(PROJECT_ID.to_string(), entry)]))
}

/// The base64url text of `bytes`, unpadded.
fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().fold(0u32, |n, b| (n << 8) | *b as u32) << (8 * (3 - chunk.len()));
        for i in 0..=chunk.len() {
            out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

/// A stream from `cursor` to the cursor line after its snapshot, stopped; every line it wrote.
fn run_with_cursor(project: &Project, cursor: &str) -> Vec<Value> {
    let mut stream = project.stream(&["--cursor", cursor]);
    stream.snapshot();
    let (_, code) = stream.finish();
    assert_eq!(code, Some(0));
    stream.lines.clone()
}

/// The resync line at `at` is followed by a snapshot of the project before any event.
fn snapshot_follows(lines: &[Value], at: usize) {
    let end = lines
        .iter()
        .position(|l| l["kind"] == "snapshotEnd")
        .expect("a snapshotEnd follows the resync");
    assert!(end > at, "{lines:?}");
    assert!(
        lines[at..end].iter().any(|l| l["kind"] == "snapshot"),
        "{lines:?}"
    );
    assert!(
        !lines[at..end].iter().any(|l| l["kind"] == "event"),
        "no event before the snapshot ends: {lines:?}"
    );
}

/// 2000 ledger rows that become public events.
fn backlog(label: &str) -> Project {
    let (project, _) = project(label, |core, creds| {
        for n in 0..2000 {
            message(core, creds, &format!("backlog {n}"));
        }
    });
    project
}

// ------------------------------------------------------------------------------------------------- (a) fresh

#[test]
fn a_fresh_stream_sends_hello_the_project_its_snapshot_and_then_events() {
    let project = basic("fresh");
    project.start();
    let (epoch, head) = project.head();
    let mut stream = project.stream(&[]);
    let first = stream.snapshot();

    assert_eq!(first[0]["kind"], "hello", "{first:?}");
    assert_eq!(first[0]["projects"], 1);
    let line = &first[1];
    assert_eq!(line["kind"], "project");
    assert_eq!(line["projectId"], PROJECT_ID);
    assert_eq!(line["state"], "ready");
    assert_eq!(line["epoch"], epoch.as_str());
    assert_eq!(line["head"], head);
    assert_eq!(line["root"], project.root.to_string_lossy().as_ref());

    let keys = snapshot_keys(&first);
    let unique: BTreeSet<_> = keys.iter().collect();
    assert_eq!(unique.len(), keys.len(), "an item twice: {keys:?}");
    let has = |section: &str, key: &str| keys.iter().any(|(s, k)| s == section && k == key);
    assert!(
        keys.iter()
            .any(|(s, k)| s == "waiting" && k.starts_with("message:")),
        "{keys:?}"
    );
    assert!(has("agents", "pm-1"), "{keys:?}");
    assert!(has("plans", "1"), "{keys:?}");
    assert!(has("run", "run"), "{keys:?}");
    let end = first.iter().find(|l| l["kind"] == "snapshotEnd").unwrap();
    assert_eq!(end["projectId"], PROJECT_ID);
    assert_eq!(end["head"], head);
    assert!(!kinds(&first).contains(&"event"), "{first:?}");
    let at_end = entries(first.last().unwrap());
    assert_eq!(at_end[PROJECT_ID].e, epoch);
    assert_eq!(at_end[PROJECT_ID].s, head);

    project.poke();
    let events = stream.until_kind(15, "event");
    let event = &events.last().unwrap()["event"];
    assert!(event["seq"].as_u64().unwrap() > head, "{event}");
    assert!(
        event["id"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{PROJECT_ID}:{epoch}:")),
        "{event}"
    );
    let cursor = stream.until_kind(10, "cursor");
    let newer = entries(cursor.last().unwrap());
    assert!(newer[PROJECT_ID].s > head, "{newer:?}");
}

// ------------------------------------------------------------------------------------------------- (b) resume

#[test]
fn a_resumed_stream_has_no_snapshot_and_exactly_the_events_after_the_cursor() {
    let project = basic("resume");
    project.start();
    let mut stream = project.stream(&[]);
    stream.snapshot();
    project.poke();
    stream.until_kind(15, "event");
    stream.until_kind(10, "cursor");
    let (_, code) = stream.finish();
    assert_eq!(code, Some(0));
    let cursor = stream.last_cursor();

    // Two more writes while nothing follows the ledger.
    project.poke();
    let at = decode_cursor(&cursor).unwrap()[PROJECT_ID].clone();
    let mut expected: Vec<String> = Vec::new();
    let (mut seq, mut k, mut anchor) = (at.s, at.k, (at.s > 0).then_some(at.h.clone()));
    loop {
        let page = project.feed(&FeedCall::Page {
            after_seq: seq,
            after_k: k,
            limit: 500,
            anchor: anchor.clone(),
        });
        assert_eq!(page["anchorOk"], true, "{page}");
        for event in page["events"].as_array().unwrap() {
            expected.push(event["id"].as_str().unwrap().to_string());
        }
        seq = page["scanned"]["seq"].as_u64().unwrap();
        k = page["scanned"]["k"].as_u64().unwrap() as u32;
        anchor = page["anchor"].as_str().map(str::to_string);
        if page["hasMore"] != true {
            break;
        }
    }
    assert!(expected.len() >= 2, "{expected:?}");

    let mut resumed = project.stream(&["--cursor", &cursor]);
    let last = expected.last().unwrap().clone();
    resumed.until(20, |l| {
        l["kind"] == "event" && l["event"]["id"] == last.as_str()
    });
    resumed.until_kind(10, "cursor");
    let (_, code) = resumed.finish();
    assert_eq!(code, Some(0));
    let got: Vec<String> = resumed
        .lines
        .iter()
        .filter(|l| l["kind"] == "event")
        .map(|l| l["event"]["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(got, expected, "none lost, none repeated");
    assert!(
        !kinds(&resumed.lines).contains(&"snapshot") && !kinds(&resumed.lines).contains(&"resync"),
        "{:?}",
        resumed.lines
    );
}

// ------------------------------------------------------------------------------------------------- (c) resync

#[test]
fn a_cursor_that_does_not_fit_the_ledger_gives_a_resync_and_a_new_snapshot() {
    let project = basic("resync");
    project.start();
    let (_, head) = project.head();
    let mut stream = project.stream(&[]);
    stream.snapshot();
    let (_, code) = stream.finish();
    assert_eq!(code, Some(0));
    let good = decode_cursor(&stream.last_cursor()).unwrap();
    assert!(head > 0);

    let changed = |edit: &dyn Fn(&mut CursorEntry)| {
        let mut entries = good.clone();
        edit(entries.get_mut(PROJECT_ID).unwrap());
        encode_cursor(&entries)
    };
    let cases: [(&str, String); 3] = [
        ("epoch", changed(&|e| e.e = "0123456789abcdef".into())),
        ("ahead", changed(&|e| e.s = head + 1000)),
        ("anchor", changed(&|e| e.h = "ffffffffffffffff".into())),
    ];
    for (reason, cursor) in &cases {
        let lines = run_with_cursor(&project, cursor);
        let at = lines
            .iter()
            .position(|l| l["kind"] == "resync")
            .unwrap_or_else(|| panic!("no resync for {reason}: {lines:?}"));
        assert_eq!(lines[at]["projectId"], PROJECT_ID, "{reason}");
        assert_eq!(lines[at]["reason"], *reason, "{lines:?}");
        assert_eq!(lines[at + 1]["kind"], "cursor", "{reason}: {lines:?}");
        assert!(
            !entries(&lines[at + 1]).contains_key(PROJECT_ID),
            "{reason}: {lines:?}"
        );
        snapshot_follows(&lines, at);
        assert_eq!(
            lines.iter().filter(|l| l["kind"] == "resync").count(),
            1,
            "{reason}"
        );
    }

    // A cursor that does not decode: undecodable text, and an entry with k 0 (the codec omits k when 0).
    let entry = &good[PROJECT_ID];
    let compact = format!(
        "{{\"p\":{{\"{PROJECT_ID}\":{{\"e\":\"{}\",\"h\":\"{}\",\"k\":0,\"r\":{},\"s\":{}}}}}}}",
        entry.e,
        entry.h,
        json!(entry.r),
        entry.s
    );
    let k_zero = format!("c1.{}", base64url(compact.as_bytes()));
    assert!(decode_cursor(&k_zero).is_err(), "k 0 does not decode");
    for cursor in ["c1.!!not-base64!!", k_zero.as_str()] {
        let lines = run_with_cursor(&project, cursor);
        let at = lines
            .iter()
            .position(|l| l["kind"] == "resync")
            .unwrap_or_else(|| panic!("no resync for {cursor}: {lines:?}"));
        assert_eq!(lines[at]["projectId"], Value::Null, "{lines:?}");
        assert_eq!(lines[at]["reason"], "cursor", "{lines:?}");
        // No entry is held for the project, so its snapshot follows at once.
        assert_eq!(lines[at + 1]["kind"], "snapshot", "{lines:?}");
        snapshot_follows(&lines, at);
    }
}

// ------------------------------------------------------------------------------------------------- (d) silent

#[test]
fn a_stopped_controller_is_silent_in_the_stream_and_continues_when_it_returns() {
    let project = basic("silent");
    project.start();
    let mut stream = project.stream(&[]);
    stream.snapshot();
    let before = stream.lines.len();

    project.out(&["stop"]);
    stream.until(20, |l| {
        l["kind"] == "cursor"
            && entries(l)
                .get(PROJECT_ID)
                .is_some_and(|entry| entry.d.is_some())
    });
    let silent = &stream.lines[before..];
    let line = silent
        .iter()
        .find(|l| l["kind"] == "project")
        .unwrap_or_else(|| panic!("no project line: {silent:?}"));
    assert_eq!(line["state"], "silent", "{line}");
    assert_eq!(line["projectId"], PROJECT_ID);
    assert!(line["epoch"].is_null() && line["head"].is_null());

    let restarted = stream.lines.len();
    project.start();
    stream.until(15, |l| l["kind"] == "project" && l["state"] == "ready");
    let cleared = stream.until_kind(10, "cursor");
    assert!(
        entries(cleared.last().unwrap())[PROJECT_ID].d.is_none(),
        "{cleared:?}"
    );
    project.poke();
    stream.until_kind(15, "event");
    let after = &stream.lines[restarted..];
    assert!(
        !after
            .iter()
            .any(|l| l["kind"] == "resync" || l["kind"] == "snapshot"),
        "no resync after the restart: {after:?}"
    );
}

// ------------------------------------------------------------------------------------------------- (e) line cap

#[test]
fn text_at_its_caps_with_escapes_gives_big_event_lines_and_no_error() {
    let (project, _) = project("caps", |core, creds| {
        message(core, creds, &"\"\\".repeat(8000));
        core.configure_prompt_relay(&json!({"enabled": true, "captureTtlSeconds": 600}))
            .unwrap();
        let options: Vec<Value> = (1..=32)
            .map(|n| json!({"number": n, "text": "\"".repeat(256), "acceptsText": false, "widensPermissions": false}))
            .collect();
        // The writer's hash of the prompt, which the feed recomputes to trust the text.
        let text = "\"\\".repeat(4000);
        let mut canonical = serde_json::Map::new();
        canonical.insert("agentId".into(), json!("dev-1"));
        canonical.insert("paneId".into(), json!("pane-1"));
        canonical.insert("hostKind".into(), json!("claude"));
        canonical.insert("text".into(), json!(text));
        canonical.insert("options".into(), Value::Array(options.clone()));
        let prompt_sha = sha256(&Value::Object(canonical).to_string());
        core.record_prompt_capture(
            &context(core, &creds.pm),
            &json!({"prompt": {
                "agentId": "dev-1", "paneId": "pane-1", "hostKind": "claude",
                "text": text, "options": options, "promptSha": prompt_sha,
            }}),
        )
        .unwrap_or_else(|e| panic!("a prompt with 32 options of 256 bytes: {e}"));
    });
    project.start();
    let cursor = cursor_at_start(&project);
    let (_, head) = project.head();
    let mut stream = project.stream(&["--cursor", &cursor]);
    stream.until(30, |l| {
        l["kind"] == "cursor"
            && entries(l)
                .get(PROJECT_ID)
                .is_some_and(|entry| entry.s >= head)
    });
    let (_, code) = stream.finish();
    assert_eq!(code, Some(0));

    assert!(
        !kinds(&stream.lines).contains(&"error"),
        "{:?}",
        stream.stderr()
    );
    let biggest = stream
        .raw
        .iter()
        .filter(|l| l.contains("\"kind\":\"event\""))
        .map(String::len)
        .max()
        .expect("event lines");
    let sizes: Vec<(String, usize)> = stream
        .lines
        .iter()
        .zip(&stream.raw)
        .filter(|(l, _)| l["kind"] == "event")
        .map(|(l, r)| (l["event"]["type"].to_string(), r.len()))
        .collect();
    assert!(
        biggest >= 16 * 1024,
        "the biggest event line is {biggest}: {sizes:?}"
    );
    assert!(
        stream.raw.iter().all(|l| l.len() <= 65536),
        "a line over 65536 bytes"
    );
    let types: Vec<&str> = stream
        .lines
        .iter()
        .filter(|l| l["kind"] == "event")
        .map(|l| l["event"]["type"].as_str().unwrap())
        .collect();
    assert!(types.contains(&"prompt.waiting"), "{types:?}");
}

// ------------------------------------------------------------------------------------------------- (g) quiet and read only

#[test]
fn an_idle_stream_writes_nothing_to_the_ledger_and_starts_no_daemon() {
    let project = basic("quiet");
    project.start();
    project.settle();
    let (epoch, head) = project.head();
    let version = project.state_version();
    let mut stream = project.stream(&[]);
    let first = stream.snapshot();
    let seen = first.len();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let _ = stream.next(1);
    }
    let (_, code) = stream.finish();
    assert_eq!(code, Some(0));
    assert_eq!(project.head(), (epoch, head), "feed head moved");
    assert_eq!(project.state_version(), version, "stateVersion moved");
    let quiet = &stream.lines[seen..];
    assert!(
        quiet.iter().all(|l| l["kind"] == "cursor"),
        "only the final cursor after the snapshot: {quiet:?}"
    );

    // The operator key is nowhere in what the stream wrote.
    assert!(!stream.raw.concat().contains(OPERATOR));
    assert!(!stream.stderr().contains(OPERATOR));
    for line in stream.lines.iter().filter(|l| l["kind"] == "cursor") {
        let decoded = format!("{:?}", entries(line));
        assert!(!decoded.contains(OPERATOR), "{decoded}");
    }

    // A project whose controller is stopped is listed and not started.
    let idle = basic("idle");
    assert!(!idle.socket().exists());
    let mut stream = idle.stream(&[]);
    let lines = stream.until_kind(30, "project");
    assert_eq!(lines.last().unwrap()["state"], "silent", "{lines:?}");
    let (_, code) = stream.finish();
    assert_eq!(code, Some(0));
    assert!(!idle.socket().exists(), "the stream started a daemon");
}

// ------------------------------------------------------------------------------------------------- (h) stopping

#[test]
fn sigterm_in_the_middle_of_a_snapshot_gives_a_final_cursor_and_exit_0() {
    let project = backlog("stop-snapshot");
    project.start();
    let mut stream = project.stream(&[]);
    stream.until_kind(30, "project");
    let (_, code) = stream.finish();
    assert_eq!(code, Some(0));
    assert_eq!(
        stream.lines.last().unwrap()["kind"],
        "cursor",
        "{:?}",
        kinds(&stream.lines)
    );
}

#[test]
fn a_closed_pipe_in_the_middle_of_a_backlog_gives_exit_0() {
    let project = backlog("closed-pipe");
    project.start();
    let cursor = cursor_at_start(&project);
    let mut stream = project.stream(&["--cursor", &cursor]);
    for _ in 0..50 {
        stream.next(30).expect("50 lines of the backlog");
    }
    stream.quit.store(true, Ordering::SeqCst);
    assert_eq!(stream.exit(10), Some(0));
}
