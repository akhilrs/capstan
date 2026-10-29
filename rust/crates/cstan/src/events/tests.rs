//! Engine tests over a fake ledger, probe, process list and clock; no daemon and no real sleep.
use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use capstan_kernel::areas::feed_cursor::{decode_cursor, encode_cursor, CursorEntry};
use serde_json::{json, Value};

use super::live::LiveFeed;
use super::*;
use crate::projects::EndpointState;

const KEY: &str = "SECRET-KEY-0123456789abcdef";
const BASE: u64 = 1_700_000_000_000;

// ------------------------------------------------------------------------------------------------ the fakes

#[derive(Default)]
struct Led {
    id: String,
    epoch: String,
    head: u64,
    snap_head: Option<u64>,
    events: Vec<Value>,
    waiting: Vec<Vec<Value>>,
    agents: Vec<Value>,
    plans: Vec<Value>,
    page_size: usize,
    null_anchor: bool,
    fail: Option<FeedFailure>,
    probe: Option<EndpointState>,
}

fn anchor_of(seq: u64) -> Option<String> {
    (seq > 0).then(|| format!("{:016x}", seq + 0xabc0000))
}

fn event(seq: u64, k: u64) -> Value {
    json!({"id": format!("p:e:{seq}:{k}"), "seq": seq, "k": k, "type": "agent.registered", "body": {"effects": []}})
}

fn led(id: &str, head: u64) -> Led {
    Led {
        id: id.into(),
        epoch: "0123456789abcdef".into(),
        head,
        events: (1..=head).map(|s| event(s, 0)).collect(),
        waiting: vec![vec![]],
        page_size: 500,
        ..Led::default()
    }
}

struct FakeClock {
    now: AtomicU64,
    stop_at: AtomicU64,
    stop: Arc<AtomicBool>,
}

impl FakeClock {
    fn at(&self) -> u64 {
        self.now.load(Ordering::SeqCst) - BASE
    }
}

type CallHook = Box<dyn Fn(&str) -> Option<FeedFailure> + Send>;
type SleepHook = Box<dyn Fn(u64) + Send>;

struct World {
    led: Mutex<BTreeMap<PathBuf, Led>>,
    calls: Mutex<Vec<String>>,
    asks: Mutex<Vec<u64>>,
    late_calls: AtomicU64,
    clock: Arc<FakeClock>,
    on_call: Mutex<Option<CallHook>>,
    on_sleep: Mutex<Option<SleepHook>>,
}

struct FeedRef(Arc<World>);
struct ProbeRef(Arc<World>);
struct ClockRef(Arc<FakeClock>, Arc<World>);
struct Procs(Vec<Proc>);

impl Processes for Procs {
    fn list(&self) -> Vec<Proc> {
        self.0.clone()
    }
}

impl Clock for ClockRef {
    fn now_ms(&self) -> u64 {
        self.0.now.load(Ordering::SeqCst)
    }
    fn sleep_ms(&self, ms: u64) {
        let now = self.0.now.fetch_add(ms, Ordering::SeqCst) + ms;
        if let Some(hook) = &*self.1.on_sleep.lock().unwrap() {
            hook(now - BASE);
        }
        if now - BASE >= self.0.stop_at.load(Ordering::SeqCst) {
            self.0.stop.store(true, Ordering::SeqCst);
        }
    }
}

impl ProjectProbe for ProbeRef {
    fn key(&self, root: &Path) -> Option<Credential> {
        self.0
            .led
            .lock()
            .unwrap()
            .contains_key(root)
            .then(|| Credential::new(KEY))
    }
    fn ask(&self, root: &Path, _: &Credential) -> EndpointState {
        self.0.asks.lock().unwrap().push(self.0.clock.at());
        match self.0.led.lock().unwrap().get(root) {
            Some(led) => led.probe.unwrap_or(EndpointState::Ready),
            None => EndpointState::Silent,
        }
    }
}

impl FeedSource for FeedRef {
    fn call(&self, root: &Path, _: &Credential, call: &FeedCall) -> Result<Value, FeedFailure> {
        let world = &self.0;
        let args = call.args();
        if world.clock.stop.load(Ordering::SeqCst) {
            world.late_calls.fetch_add(1, Ordering::SeqCst);
        }
        world.calls.lock().unwrap().push(args[0].clone());
        if let Some(hook) = &*world.on_call.lock().unwrap() {
            if let Some(failure) = hook(&args[0]) {
                return Err(failure);
            }
        }
        let leds = world.led.lock().unwrap();
        let led = leds.get(root).ok_or(FeedFailure::NoAnswer)?;
        if let Some(failure) = &led.fail {
            return Err(failure.clone());
        }
        let flag = |name: &str| {
            args.iter()
                .position(|a| a == name)
                .map(|i| args[i + 1].clone())
        };
        Ok(match args[0].as_str() {
            "head" => {
                json!({"projectId": led.id, "epoch": led.epoch, "head": led.head, "anchor": anchor_of(led.head)})
            }
            "snapshot" => {
                let head = led.snap_head.unwrap_or(led.head);
                json!({"epoch": led.epoch, "head": head, "anchor": anchor_of(head),
                    "run": {"requestId": "run", "state": "active", "paused": false, "contentHash": "h"}})
            }
            "waiting" => {
                let at = flag("--after").map_or(0, |a| a.trim_start_matches('p').parse().unwrap());
                let next = (at + 1 < led.waiting.len()).then(|| format!("p{}", at + 1));
                json!({"head": led.head, "items": led.waiting[at], "next": next})
            }
            "agents" => json!({"head": led.head, "items": led.agents, "next": null}),
            "plans" => json!({"head": led.head, "items": led.plans, "next": null}),
            "page" => {
                let after = flag("--after").unwrap();
                let (s, k) = after.split_once(':').unwrap();
                let (s, k): (u64, u64) = (s.parse().unwrap(), k.parse().unwrap());
                let given = flag("--anchor");
                assert_eq!(given.is_some(), s > 0, "anchor goes with every s above 0");
                let ahead = s > led.head;
                let ok = !ahead && given.is_none_or(|g| Some(g) == anchor_of(s));
                let later: Vec<&Value> = led
                    .events
                    .iter()
                    .filter(|e| (e["seq"].as_u64().unwrap(), e["k"].as_u64().unwrap()) > (s, k))
                    .collect();
                let taken: Vec<&Value> = later.iter().take(led.page_size).copied().collect();
                let scanned = taken.last().map_or((s, k), |e| {
                    (e["seq"].as_u64().unwrap(), e["k"].as_u64().unwrap())
                });
                let anchor = if led.null_anchor {
                    None
                } else {
                    anchor_of(scanned.0)
                };
                json!({"projectId": led.id, "epoch": led.epoch, "head": led.head, "anchorOk": ok,
                    "ahead": ahead, "events": if ok { taken.clone() } else { vec![] },
                    "scanned": {"seq": scanned.0, "k": scanned.1}, "anchor": anchor,
                    "hasMore": later.len() > taken.len(), "rowsRead": 1})
            }
            other => panic!("the engine called {other}"),
        })
    }
}

struct Sink {
    lines: Arc<Mutex<Vec<String>>>,
    batches: usize,
    fail_from: usize,
}

impl LineSink for Sink {
    fn write_batch(&mut self, lines: &[String]) -> std::io::Result<()> {
        self.batches += 1;
        if self.batches > self.fail_from {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        self.lines.lock().unwrap().extend(lines.iter().cloned());
        Ok(())
    }
}

struct ErrBuf(Arc<Mutex<Vec<u8>>>);

impl Write for ErrBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// ------------------------------------------------------------------------------------------- the harness

struct Run {
    lines: Vec<String>,
    code: i32,
    err: String,
    stop: Arc<AtomicBool>,
}

struct Setup {
    plan: EventsPlan,
    procs: Vec<Proc>,
    stop_at: u64,
    limits: Limits,
    cwd: PathBuf,
    fail_from: usize,
}

fn setup() -> Setup {
    Setup {
        plan: EventsPlan {
            cursor: None,
            roots: vec![],
            poll_ms: 10,
        },
        procs: vec![],
        stop_at: 500,
        limits: Limits::default(),
        cwd: std::env::temp_dir(),
        fail_from: usize::MAX,
    }
}

fn tmp(name: &str) -> PathBuf {
    let dir = capstan_testkit::short_temp_root()
        .join(format!("cstan-events-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::canonicalize(dir).unwrap()
}

fn project(dir: &Path, name: &str) -> PathBuf {
    let root = dir.join(name);
    std::fs::create_dir_all(root.join(".capstan/state")).unwrap();
    std::fs::canonicalize(root).unwrap()
}

fn world(leds: Vec<(&Path, Led)>) -> Arc<World> {
    let stop = Arc::new(AtomicBool::new(false));
    Arc::new(World {
        led: Mutex::new(
            leds.into_iter()
                .map(|(p, l)| (p.to_path_buf(), l))
                .collect(),
        ),
        calls: Mutex::new(vec![]),
        asks: Mutex::new(vec![]),
        late_calls: AtomicU64::new(0),
        clock: Arc::new(FakeClock {
            now: AtomicU64::new(BASE),
            stop_at: AtomicU64::new(u64::MAX),
            stop,
        }),
        on_call: Mutex::new(None),
        on_sleep: Mutex::new(None),
    })
}

fn proc(root: &Path) -> Proc {
    Proc {
        pid: 1,
        project: Some(root.to_path_buf()),
        project_gone: false,
        started_at: None,
        rss_bytes: None,
        cpu_percent: None,
    }
}

fn run(world: &Arc<World>, setup: Setup) -> Run {
    let stop = world.clock.stop.clone();
    world.clock.stop_at.store(setup.stop_at, Ordering::SeqCst);
    let lines = Arc::new(Mutex::new(vec![]));
    let err = Arc::new(Mutex::new(vec![]));
    let mut deps = Deps {
        feed: Box::new(FeedRef(world.clone())),
        probe: Box::new(ProbeRef(world.clone())),
        procs: Box::new(Procs(setup.procs)),
        clock: Box::new(ClockRef(world.clock.clone(), world.clone())),
        err: Box::new(ErrBuf(err.clone())),
        limits: setup.limits,
        cwd: setup.cwd,
    };
    let mut sink = Sink {
        lines: lines.clone(),
        batches: 0,
        fail_from: setup.fail_from,
    };
    let code = run_stream(&setup.plan, &mut deps, &mut sink, &stop);
    let lines = lines.lock().unwrap().clone();
    let err = String::from_utf8_lossy(&err.lock().unwrap()).into_owned();
    Run {
        lines,
        code,
        err,
        stop,
    }
}

fn json_lines(run: &Run) -> Vec<Value> {
    run.lines
        .iter()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// `kind`, with the section for a snapshot line and the reason for a resync.
fn kinds(run: &Run) -> Vec<String> {
    json_lines(run)
        .iter()
        .map(|l| {
            let kind = l["kind"].as_str().unwrap();
            match kind {
                "snapshot" => format!("snapshot:{}", l["section"].as_str().unwrap()),
                "resync" => format!("resync:{}", l["reason"].as_str().unwrap_or("?")),
                _ => kind.to_string(),
            }
        })
        .collect()
}

fn cursors(run: &Run) -> Vec<BTreeMap<String, CursorEntry>> {
    json_lines(run)
        .iter()
        .filter(|l| l["kind"] == "cursor")
        .map(|l| decode_cursor(l["cursor"].as_str().unwrap()).unwrap())
        .collect()
}

fn entry(led: &Led, root: &Path, s: u64, k: u32) -> CursorEntry {
    CursorEntry {
        e: led.epoch.clone(),
        s,
        k,
        h: anchor_of(s).unwrap_or_else(|| "0000000000000000".into()),
        r: root.to_string_lossy().into_owned(),
        d: None,
    }
}

fn cursor_text(entries: Vec<(&str, CursorEntry)>) -> String {
    encode_cursor(
        &entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    )
}

fn b64url(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

fn items(keys: &[&str]) -> Vec<Value> {
    keys.iter()
        .map(|k| json!({"key": k, "requestId": k}))
        .collect()
}

// ------------------------------------------------------------------------------------------------- tests

#[test]
fn fresh_start_gives_hello_projects_snapshots_cursor_events() {
    let dir = tmp("fresh");
    let (b, a) = (project(&dir, "b"), project(&dir, "a"));
    let mut la = led("pa", 3);
    la.snap_head = Some(2);
    la.waiting = vec![items(&["message:1"])];
    la.agents = items(&["dev-1"]);
    la.plans = items(&["1"]);
    let world = world(vec![(&a, la), (&b, led("pb", 0))]);
    let mut s = setup();
    s.plan.roots = vec![b.clone(), a.clone()];
    let out = run(&world, s);
    assert_eq!(out.code, 0);
    assert_eq!(
        kinds(&out),
        [
            "hello",
            "project",
            "project",
            "snapshot:waiting",
            "snapshot:agents",
            "snapshot:plans",
            "snapshot:run",
            "snapshotEnd",
            "cursor",
            "event",
            "cursor",
            "snapshot:run",
            "snapshotEnd",
            "cursor",
            "cursor"
        ]
    );
    let lines = json_lines(&out);
    assert_eq!(lines[0]["projects"], 2);
    assert_eq!(lines[0]["v"], 1);
    assert_eq!(lines[1]["projectId"], "pa");
    assert_eq!(lines[1]["state"], "ready");
    assert_eq!(lines[1]["head"], 3);
    assert_eq!(lines[2]["projectId"], "pb");
    assert_eq!(lines[3]["key"], "message:1");
    assert_eq!(lines[7]["head"], 2);
    assert_eq!(lines[9]["event"]["seq"], 3);
    let last = cursors(&out).pop().unwrap();
    assert_eq!(last["pa"].s, 3);
    assert_eq!(last["pb"].s, 0);
    assert_eq!(last["pb"].h, "0000000000000000");
    // `v` and `kind` come first on every line.
    assert!(out
        .lines
        .iter()
        .all(|l| l.starts_with(r#"{"v":1,"kind":""#)));
}

#[test]
fn resume_from_a_cursor_gives_no_snapshot_and_only_later_events() {
    let dir = tmp("resume");
    let a = project(&dir, "a");
    let la = led("pa", 5);
    let cursor = cursor_text(vec![("pa", entry(&la, &a, 3, 0))]);
    let world = world(vec![(&a, la)]);
    let mut s = setup();
    s.plan.cursor = Some(cursor);
    let out = run(&world, s);
    assert_eq!(
        kinds(&out),
        ["hello", "project", "event", "event", "cursor", "cursor"]
    );
    let seqs: Vec<u64> = json_lines(&out)
        .iter()
        .filter(|l| l["kind"] == "event")
        .map(|l| l["event"]["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(seqs, [4, 5]);
    assert_eq!(cursors(&out)[0]["pa"].s, 5);
}

#[test]
fn epoch_ahead_and_anchor_mismatches_resync_then_snapshot() {
    for (case, reason) in [("epoch", "epoch"), ("ahead", "ahead"), ("anchor", "anchor")] {
        let dir = tmp(case);
        let a = project(&dir, "a");
        let la = led("pa", 4);
        let mut held = entry(&la, &a, 2, 0);
        match case {
            "epoch" => held.e = "fedcba9876543210".into(),
            "ahead" => {
                held.s = 9;
                held.h = anchor_of(9).unwrap();
            }
            _ => held.h = "1111111111111111".into(),
        }
        let cursor = cursor_text(vec![("pa", held)]);
        let world = world(vec![(&a, la)]);
        let mut s = setup();
        s.plan.cursor = Some(cursor);
        let out = run(&world, s);
        let got = kinds(&out);
        let want = [
            "hello".to_string(),
            "project".into(),
            format!("resync:{reason}"),
            "cursor".into(),
            "snapshot:run".into(),
            "snapshotEnd".into(),
            "cursor".into(),
        ];
        assert_eq!(&got[..7], &want[..], "{case}: {got:?}");
        let cs = cursors(&out);
        assert!(
            !cs[0].contains_key("pa"),
            "{case}: the entry is dropped at the resync"
        );
        assert_eq!(cs[1]["pa"].s, 4, "{case}: and returns after the snapshot");
        assert!(json_lines(&out)
            .iter()
            .any(|l| l["kind"] == "resync" && l["projectId"] == "pa"));
    }
}

#[test]
fn an_undecodable_or_k_zero_cursor_resyncs_the_whole_stream() {
    let dir = tmp("badcursor");
    let (a, b) = (project(&dir, "a"), project(&dir, "b"));
    let k0 = format!(
        "c1.{}",
        b64url(
            br#"{"p":{"pa":{"e":"0123456789abcdef","h":"0000000000000000","k":0,"r":"/x","s":1}}}"#
        )
    );
    for cursor in ["c1.not-base64!!".to_string(), k0, "nonsense".to_string()] {
        let world = world(vec![(&a, led("pa", 1)), (&b, led("pb", 1))]);
        let mut s = setup();
        s.plan.roots = vec![a.clone(), b.clone()];
        s.plan.cursor = Some(cursor);
        let out = run(&world, s);
        let lines = json_lines(&out);
        let resync = lines.iter().find(|l| l["kind"] == "resync").unwrap();
        assert_eq!(resync["projectId"], Value::Null);
        assert_eq!(resync["reason"], "cursor");
        assert_eq!(
            lines.iter().filter(|l| l["kind"] == "snapshotEnd").count(),
            2
        );
    }
}

#[test]
fn snapshot_dedupes_by_key_and_pages_continue() {
    let dir = tmp("dedupe");
    let a = project(&dir, "a");
    let mut la = led("pa", 3);
    la.snap_head = Some(2);
    la.null_anchor = true;
    la.events.push(event(3, 1));
    la.events.push(event(3, 2));
    la.waiting = vec![
        vec![
            json!({"key": "message:1", "summary": "old"}),
            json!({"key": "message:2"}),
        ],
        vec![json!({"key": "message:1", "summary": "new"})],
    ];
    let world = world(vec![(&a, la)]);
    let mut s = setup();
    s.plan.roots = vec![a.clone()];
    let out = run(&world, s);
    let lines = json_lines(&out);
    let waiting: Vec<&Value> = lines.iter().filter(|l| l["section"] == "waiting").collect();
    assert_eq!(waiting.len(), 2);
    assert_eq!(waiting[0]["key"], "message:1");
    assert_eq!(waiting[0]["item"]["summary"], "new");
    // The snapshot is complete before the events; (3,0), (3,1) and (3,2) follow it as read.
    let end = lines
        .iter()
        .position(|l| l["kind"] == "snapshotEnd")
        .unwrap();
    let events: Vec<(u64, u64)> = lines[end..]
        .iter()
        .filter(|l| l["kind"] == "event")
        .map(|l| {
            (
                l["event"]["seq"].as_u64().unwrap(),
                l["event"]["k"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(events, [(3, 0), (3, 1), (3, 2)]);
    // A null page anchor gives zeros.
    assert_eq!(
        cursors(&out).iter().rev().nth(1).unwrap()["pa"].h,
        "0000000000000000"
    );
}

#[test]
fn several_pages_give_events_then_a_cursor_each() {
    let dir = tmp("pages");
    let a = project(&dir, "a");
    let la = led("pa", 5);
    let cursor = cursor_text(vec![("pa", entry(&la, &a, 1, 0))]);
    let mut la = la;
    la.page_size = 2;
    let world = world(vec![(&a, la)]);
    let mut s = setup();
    s.plan.cursor = Some(cursor);
    let out = run(&world, s);
    assert_eq!(
        kinds(&out),
        ["hello", "project", "event", "event", "cursor", "event", "event", "cursor", "cursor"]
    );
}

#[test]
fn polling_follows_the_minimum_and_new_events_arrive() {
    let dir = tmp("poll");
    let a = project(&dir, "a");
    let world = world(vec![(&a, led("pa", 1))]);
    let w = world.clone();
    let appended = AtomicBool::new(false);
    *world.on_sleep.lock().unwrap() = Some(Box::new(move |at| {
        if at >= 1000 && !appended.swap(true, Ordering::SeqCst) {
            let mut leds = w.led.lock().unwrap();
            let led = leds.values_mut().next().unwrap();
            led.head = 2;
            led.events.push(event(2, 0));
        }
    }));
    let mut s = setup();
    s.plan.roots = vec![a.clone()];
    s.stop_at = 2500;
    let out = run(&world, s);
    assert_eq!(*world.asks.lock().unwrap(), [0, 1000, 2000]);
    let seqs: Vec<u64> = json_lines(&out)
        .iter()
        .filter(|l| l["kind"] == "event")
        .map(|l| l["event"]["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(seqs, [2]);
}

#[test]
fn heartbeat_follows_fifteen_seconds_of_silence() {
    let world = world(vec![]);
    let mut s = setup();
    s.stop_at = 16_000;
    let out = run(&world, s);
    let lines = json_lines(&out);
    let beats: Vec<&Value> = lines.iter().filter(|l| l["kind"] == "heartbeat").collect();
    assert_eq!(beats.len(), 1);
    assert_eq!(
        beats[0]["ts"],
        capstan_ledger::iso_from_millis((BASE + 15_000) as i64)
    );
    assert_eq!(lines[0]["kind"], "hello");
    assert_eq!(lines[0]["projects"], 0);
}

#[test]
fn an_idle_failing_sink_ends_at_the_next_heartbeat() {
    let world = world(vec![]);
    let mut s = setup();
    s.stop_at = 60_000;
    s.fail_from = 1;
    let out = run(&world, s);
    assert_eq!(out.code, 0);
    assert_eq!(world.clock.at(), 15_000);
    assert_eq!(kinds(&out), ["hello"]);
}

#[test]
fn stop_in_a_snapshot_gives_a_final_cursor_without_the_project() {
    let dir = tmp("stopsnap");
    let a = project(&dir, "a");
    let world = world(vec![(&a, led("pa", 2))]);
    let stop = world.clock.stop.clone();
    *world.on_call.lock().unwrap() = Some(Box::new(move |sub| {
        if sub != "snapshot" {
            return None;
        }
        let stop = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            stop.store(true, Ordering::SeqCst);
        });
        // A call that waits the whole 2000 ms for an answer that never comes.
        let (_keep, rx) = std::sync::mpsc::channel::<()>();
        let _ = rx.recv_timeout(Duration::from_millis(2000));
        Some(FeedFailure::NoAnswer)
    }));
    let mut s = setup();
    s.plan.roots = vec![a.clone()];
    s.stop_at = u64::MAX;
    let started = Instant::now();
    let out = run(&world, s);
    assert!(started.elapsed() < Duration::from_millis(3000));
    assert_eq!(out.code, 0);
    assert!(out.stop.load(Ordering::SeqCst));
    assert_eq!(kinds(&out), ["hello", "project", "cursor"]);
    assert!(cursors(&out)[0].is_empty());
    assert_eq!(world.late_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn a_project_going_silent_gets_d_and_returns_without_resync() {
    let dir = tmp("silent");
    let a = project(&dir, "a");
    let la = led("pa", 2);
    let cursor = cursor_text(vec![("pa", entry(&la, &a, 2, 0))]);
    let world = world(vec![(&a, la)]);
    let w = world.clone();
    *world.on_sleep.lock().unwrap() = Some(Box::new(move |at| {
        let state = if (1000..2000).contains(&at) {
            EndpointState::Silent
        } else {
            EndpointState::Ready
        };
        w.led.lock().unwrap().values_mut().next().unwrap().probe = Some(state);
    }));
    let mut s = setup();
    s.plan.cursor = Some(cursor);
    s.stop_at = 2500;
    let out = run(&world, s);
    assert_eq!(
        kinds(&out),
        ["hello", "project", "project", "cursor", "project", "cursor", "cursor"]
    );
    let lines = json_lines(&out);
    assert_eq!(lines[2]["state"], "silent");
    assert_eq!(lines[2]["epoch"], Value::Null);
    assert_eq!(lines[4]["state"], "ready");
    let cs = cursors(&out);
    assert_eq!(cs[0]["pa"].d, Some((BASE / 1000 + 1) as i64));
    assert_eq!(cs[1]["pa"].d, None);
    assert_eq!(cs[1]["pa"].s, 2);
}

#[test]
fn more_than_sixty_four_roots_is_one_error() {
    let dir = tmp("many");
    let world = world(vec![]);
    let mut s = setup();
    s.plan.roots = (0..65).map(|i| dir.join(format!("r{i}"))).collect();
    let out = run(&world, s);
    assert_eq!(out.code, 1);
    let lines = json_lines(&out);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["kind"], "error");
    assert_eq!(lines[0]["code"], "invalid_request");
}

#[test]
fn an_oversized_item_ends_the_stream_with_an_internal_error() {
    let dir = tmp("bigitem");
    let a = project(&dir, "a");
    let mut la = led("pa", 1);
    la.waiting = vec![vec![
        json!({"key": "message:1", "summary": "x".repeat(70_000)}),
    ]];
    let world = world(vec![(&a, la)]);
    let mut s = setup();
    s.plan.roots = vec![a.clone()];
    let out = run(&world, s);
    assert_eq!(out.code, 1);
    assert_eq!(kinds(&out), ["hello", "project", "error"]);
    assert_eq!(json_lines(&out)[2]["code"], "internal");
    assert!(out.lines.iter().all(|l| l.len() < 65_536));
}

#[test]
fn a_snapshot_over_the_buffer_ends_the_stream_with_an_internal_error() {
    let dir = tmp("buffer");
    let a = project(&dir, "a");
    let mut la = led("pa", 1);
    la.waiting = vec![vec![
        json!({"key": "message:1", "summary": "x".repeat(500)}),
    ]];
    let world = world(vec![(&a, la)]);
    let mut s = setup();
    s.plan.roots = vec![a.clone()];
    s.limits.snapshot_buffer_bytes = 100;
    let out = run(&world, s);
    assert_eq!(out.code, 1);
    assert_eq!(kinds(&out), ["hello", "project", "error"]);
}

#[test]
fn a_cursor_of_long_roots_is_never_written() {
    let world = world(vec![]);
    let entries: Vec<(String, CursorEntry)> = (0..64)
        .map(|i| {
            let r = format!("/{}{i:02}", "x".repeat(1100));
            let e = CursorEntry {
                e: "0123456789abcdef".into(),
                s: 1,
                k: 0,
                h: anchor_of(1).unwrap(),
                r,
                d: None,
            };
            (format!("p{i:02}"), e)
        })
        .collect();
    let mut s = setup();
    s.plan.cursor = Some(encode_cursor(&entries.into_iter().collect()));
    let out = run(&world, s);
    assert_eq!(out.code, 1);
    let got = kinds(&out);
    assert_eq!(got.last().unwrap(), "error");
    assert!(!got.contains(&"cursor".to_string()));
    assert!(out.lines.iter().all(|l| l.len() < 65_536));
}

#[test]
fn spellings_of_one_root_are_one_project() {
    let dir = tmp("spell");
    let p = project(&dir, "p");
    std::os::unix::fs::symlink(&p, dir.join("link")).unwrap();
    let la = led("pa", 1);
    let cursor = cursor_text(vec![("pa", entry(&la, &p, 1, 0))]);
    let world = world(vec![(&p, la)]);
    let mut s = setup();
    s.cwd = dir.clone();
    s.plan.roots = vec!["./p".into(), "p/".into(), "link".into(), dir.join("x/../p")];
    s.plan.cursor = Some(cursor);
    let out = run(&world, s);
    let projects = json_lines(&out)
        .iter()
        .filter(|l| l["kind"] == "project")
        .count();
    assert_eq!(projects, 1);
}

#[test]
fn one_project_id_on_two_roots_lists_the_first() {
    let dir = tmp("dup");
    let (a, b) = (project(&dir, "a"), project(&dir, "b"));
    let world = world(vec![(&a, led("same", 1)), (&b, led("same", 1))]);
    let mut s = setup();
    s.plan.roots = vec![b.clone(), a.clone()];
    let out = run(&world, s);
    let lines = json_lines(&out);
    let listed: Vec<&Value> = lines.iter().filter(|l| l["kind"] == "project").collect();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["root"], a.to_string_lossy().as_ref());
    assert!(out.err.contains("not followed"));
}

static ENV: Mutex<()> = Mutex::new(());

#[test]
fn a_process_list_root_is_listed() {
    let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tmp("procs");
    let a = project(&dir, "a");
    let world = world(vec![(&a, led("pa", 1))]);
    let mut s = setup();
    s.procs = vec![proc(&a)];
    let out = run(&world, s);
    assert_eq!(json_lines(&out)[1]["projectId"], "pa");
}

#[cfg(debug_assertions)]
#[test]
fn the_seam_leaves_the_process_list_out() {
    let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tmp("seam");
    let a = project(&dir, "a");
    std::env::set_var("CSTAN_EVENTS_NO_PROCESS_SCAN", "1");
    let roots = discover_roots(&[proc(&a)], &dir, &[], &[]);
    std::env::remove_var("CSTAN_EVENTS_NO_PROCESS_SCAN");
    assert!(roots.is_empty());
}

#[test]
fn failed_feed_calls_reclassify_the_project() {
    let dir = tmp("states");
    let mut roots = vec![];
    let mut leds = vec![];
    let cases: [(&str, Option<FeedFailure>, &str); 4] = [
        (
            "refused",
            Some(FeedFailure::Refused {
                code: "forbidden".into(),
                message: "m".into(),
            }),
            "refused",
        ),
        (
            "old",
            Some(FeedFailure::Refused {
                code: "unknown_command".into(),
                message: "m".into(),
            }),
            "unsupported",
        ),
        (
            "odd",
            Some(FeedFailure::Refused {
                code: "error".into(),
                message: "m".into(),
            }),
            "silent",
        ),
        ("mute", Some(FeedFailure::NoAnswer), "silent"),
    ];
    for (name, fail, _) in &cases {
        let root = project(&dir, name);
        std::fs::write(
            root.join(".capstan/project.json"),
            format!(r#"{{"projectId":"id-{name}"}}"#),
        )
        .unwrap();
        let mut l = led(&format!("id-{name}"), 1);
        l.fail = fail.clone();
        roots.push(root.clone());
        leds.push((root, l));
    }
    let gone = dir.join("gone");
    let world = world(
        leds.iter()
            .map(|(p, l)| {
                (
                    p.as_path(),
                    Led {
                        fail: l.fail.clone(),
                        ..led(&l.id, 1)
                    },
                )
            })
            .collect(),
    );
    let mut s = setup();
    s.plan.roots = roots.iter().cloned().chain([gone.clone()]).collect();
    let out = run(&world, s);
    let lines = json_lines(&out);
    for (name, _, want) in &cases {
        let l = lines
            .iter()
            .find(|l| l["projectId"] == format!("id-{name}").as_str())
            .unwrap();
        assert_eq!(l["state"], *want, "{name}");
    }
    // A root with no controller directory and no known id is not listed.
    assert!(lines
        .iter()
        .all(|l| l["root"] != gone.to_string_lossy().as_ref()));
    assert_eq!(out.code, 0);
}

#[test]
fn the_key_never_leaves_and_only_the_feed_reads_are_made() {
    let dir = tmp("safety");
    let weird = project(&dir, "w\u{1b}[31m\u{202e}x");
    let a = project(&dir, "a");
    let mut lw = led("pw", 1);
    lw.fail = Some(FeedFailure::NoAnswer);
    let world = world(vec![(&weird, lw), (&a, led("pa", 2))]);
    std::fs::write(weird.join(".capstan/project.json"), r#"{"projectId":"pw"}"#).unwrap();
    let mut s = setup();
    s.plan.roots = vec![weird.clone(), a.clone()];
    let out = run(&world, s);
    assert!(!out.lines.join("\n").contains(KEY));
    assert!(!out.err.contains(KEY));
    assert!(out.err.contains("cannot be followed"));
    assert!(!out.err.contains('\u{1b}') && !out.err.contains('\u{202e}'));
    let allowed = ["head", "snapshot", "page", "waiting", "agents", "plans"];
    assert!(world
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|c| allowed.contains(&c.as_str())));
}

// ----------------------------------------------------------------------------------------- the live source

#[test]
fn the_live_source_gives_up_on_a_peer_that_trickles_bytes() {
    let dir = tmp("trickle");
    let root = project(&dir, "p");
    let listener = UnixListener::bind(root.join(".capstan/state/control.sock")).unwrap();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            for _ in 0..12 {
                if stream.write_all(b" ").is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    });
    let started = Instant::now();
    let got = LiveFeed.call(&root, &Credential::new(KEY), &FeedCall::Head);
    assert_eq!(got, Err(FeedFailure::NoAnswer));
    assert!(started.elapsed() < Duration::from_millis(2500));
}

#[test]
fn a_slow_cycle_still_writes_its_heartbeat() {
    let dir = tmp("slowcycle");
    let a = project(&dir, "a");
    let world = world(vec![(&a, led("pa", 1))]);
    let w = world.clone();
    *world.on_call.lock().unwrap() = Some(Box::new(move |sub| {
        if sub == "snapshot" {
            w.clock.now.fetch_add(16_000, Ordering::SeqCst);
        }
        None
    }));
    let mut s = setup();
    s.plan.roots = vec![a.clone()];
    let out = run(&world, s);
    let got = kinds(&out);
    assert_eq!(&got[..4], ["hello", "project", "heartbeat", "snapshot:run"]);
}

#[test]
fn sub_events_at_the_snapshot_head_replay_with_unchanged_state() {
    let dir = tmp("subevents");
    let a = project(&dir, "a");
    let mut la = led("pa", 3);
    la.events.push(event(3, 1));
    la.events.push(event(3, 2));
    la.agents = items(&["dev-1"]);
    let world = world(vec![(&a, la)]);
    let mut s = setup();
    s.plan.roots = vec![a.clone()];
    let out = run(&world, s);
    let lines = json_lines(&out);
    let end = lines
        .iter()
        .position(|l| l["kind"] == "snapshotEnd")
        .unwrap();
    assert_eq!(lines[end]["head"], 3);
    let replay: Vec<(u64, u64)> = lines[end..]
        .iter()
        .filter(|l| l["kind"] == "event")
        .map(|l| {
            (
                l["event"]["seq"].as_u64().unwrap(),
                l["event"]["k"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(replay, [(3, 1), (3, 2)]);
    // The snapshot lines are the same items as before the replay, and nothing was sent twice.
    let agents = lines.iter().filter(|l| l["section"] == "agents").count();
    assert_eq!(agents, 1);
    let last = cursors(&out).pop().unwrap();
    assert_eq!((last["pa"].s, last["pa"].k), (3, 2));
}

#[test]
fn an_item_changing_while_the_head_moves_is_sent_as_read_last_then_its_event() {
    let dir = tmp("moving");
    let a = project(&dir, "a");
    let mut la = led("pa", 4);
    la.snap_head = Some(2);
    // The page was read at head 4: it already shows the change that event 3 and 4 describe.
    la.waiting = vec![vec![json!({"key": "message:1", "summary": "changed at 4"})]];
    let world = world(vec![(&a, la)]);
    let mut s = setup();
    s.plan.roots = vec![a.clone()];
    let out = run(&world, s);
    let lines = json_lines(&out);
    let item = lines
        .iter()
        .position(|l| l["section"] == "waiting")
        .unwrap();
    let first_event = lines.iter().position(|l| l["kind"] == "event").unwrap();
    assert_eq!(lines[item]["item"]["summary"], "changed at 4");
    assert!(item < first_event);
    assert_eq!(lines.iter().filter(|l| l["kind"] == "event").count(), 2);
}

#[test]
fn a_null_page_anchor_gives_zeros_in_the_entry() {
    let dir = tmp("nullanchor");
    let a = project(&dir, "a");
    let la = led("pa", 5);
    let cursor = cursor_text(vec![("pa", entry(&la, &a, 3, 0))]);
    let mut la = la;
    la.null_anchor = true;
    let world = world(vec![(&a, la)]);
    let mut s = setup();
    s.plan.cursor = Some(cursor);
    let out = run(&world, s);
    let last = cursors(&out).pop().unwrap();
    assert_eq!(
        (last["pa"].s, last["pa"].h.as_str()),
        (5, "0000000000000000")
    );
}
