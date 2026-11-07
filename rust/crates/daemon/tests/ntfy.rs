//! The ntfy sender: queue, lanes, retries, tail, curl transport, status, the `notify` route and the stop bounds. Fake
//! transports and an injected clock drive everything but the stop tests, the 6 s socket test and the curl tests, which use
//! wall time. Tests that need curl skip with a note when it is absent, and fail when `CAPSTAN_REQUIRE_CURL=1`.

mod common;

use capstan_config::{effective_prefix, Ntfy, NtfyAuth, NtfyEvent, Secret};
use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger, Loops};
use capstan_daemon::handlers::{command_limit_ms, DRAIN_FLUSH_MS};
use capstan_daemon::loops::driver::DriverLog;
use capstan_daemon::loops::notifier::Clock;
use capstan_daemon::loops::{RustLoops, StopFlag};
use capstan_daemon::ntfy::sender::{SenderHandle, SenderOptions};
use capstan_daemon::ntfy::tail::{PushSource, Tail, BASE_INTERVAL_MS, MAX_INTERVAL_MS};
use capstan_daemon::ntfy::transport::{
    curl_quote, CurlTransport, NtfyTransport, PublishRequest, TransportError,
};
use capstan_daemon::server::{DaemonServer, ServerOptions};
use capstan_kernel::areas::push_events::{PushEvent, PushPage};
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::{Core, SystemEnv};
use common::private_tempdir;
use common::project::{write_project, Identity};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn wait_for(what: &str, ok: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

// ------------------------------------------------------------------------------------------------ fakes

#[derive(Clone)]
struct FakeClock(Arc<AtomicI64>);

impl FakeClock {
    fn new() -> Self {
        Self(Arc::new(AtomicI64::new(1_000_000)))
    }
    fn clock(&self) -> Clock {
        let inner = Arc::clone(&self.0);
        Arc::new(move || inner.load(Ordering::SeqCst))
    }
    fn advance(&self, ms: i64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

#[derive(Clone, Debug)]
struct Call {
    title: String,
    message: String,
    secret: Option<String>,
}

#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<Call>>,
    script: Mutex<VecDeque<Result<u16, TransportError>>>,
    closed: Mutex<bool>,
    gate: Condvar,
}

impl Fake {
    fn push(&self, result: Result<u16, TransportError>) {
        lock(&self.script).push_back(result);
    }
    fn count(&self) -> usize {
        lock(&self.calls).len()
    }
    fn close_gate(&self) {
        *lock(&self.closed) = true;
    }
    fn open_gate(&self) {
        *lock(&self.closed) = false;
        self.gate.notify_all();
    }
}

impl NtfyTransport for Fake {
    fn publish(
        &self,
        req: &PublishRequest,
        secret: Option<&Secret>,
        _timeout: Duration,
        stop: &StopFlag,
    ) -> Result<u16, TransportError> {
        lock(&self.calls).push(Call {
            title: req.title.clone(),
            message: req.message.clone(),
            secret: secret.map(|s| s.expose().to_string()),
        });
        let mut closed = lock(&self.closed);
        while *closed && !stop.stopped() {
            closed = self
                .gate
                .wait_timeout(closed, Duration::from_millis(20))
                .unwrap()
                .0;
        }
        drop(closed);
        lock(&self.script).pop_front().unwrap_or(Ok(200))
    }
}

struct Panicking;

impl NtfyTransport for Panicking {
    fn publish(
        &self,
        _: &PublishRequest,
        _: Option<&Secret>,
        _: Duration,
        _: &StopFlag,
    ) -> Result<u16, TransportError> {
        panic!("no transport may run while ntfy is off");
    }
}

fn failure(reason: &str, status: Option<u16>) -> TransportError {
    TransportError {
        reason: reason.into(),
        http_status: status,
        error: format!("HTTP {status:?} {reason}"),
        retry_after: None,
    }
}

fn base_ntfy() -> Ntfy {
    Ntfy {
        enabled: true,
        server: "https://ntfy.example".into(),
        topic: "alerts".into(),
        ..Ntfy::default()
    }
}

type Logs = Arc<Mutex<Vec<(String, Value)>>>;

struct Rig {
    sender: Arc<SenderHandle>,
    stop: StopFlag,
    clock: FakeClock,
    fake: Arc<Fake>,
    logs: Logs,
    worker: Option<std::thread::JoinHandle<()>>,
}

struct RigOptions {
    ntfy: Ntfy,
    prefix: String,
    status_path: Option<PathBuf>,
    env: HashMap<String, String>,
    start_worker: bool,
}

impl RigOptions {
    fn new(ntfy: Ntfy) -> Self {
        Self {
            ntfy,
            prefix: "Capstan".into(),
            status_path: None,
            env: HashMap::new(),
            start_worker: true,
        }
    }
}

fn rig_with(options: RigOptions, transport: Option<Arc<dyn NtfyTransport>>) -> Rig {
    let clock = FakeClock::new();
    let fake = Arc::new(Fake::default());
    let logs: Logs = Arc::default();
    let log: DriverLog = {
        let logs = Arc::clone(&logs);
        Arc::new(move |event, details| lock(&logs).push((event.to_string(), details)))
    };
    let stop = StopFlag::new();
    let env = options.env;
    let sender = SenderHandle::new(SenderOptions {
        ntfy: options.ntfy,
        prefix: options.prefix,
        transport: transport.unwrap_or_else(|| fake.clone() as Arc<dyn NtfyTransport>),
        clock: clock.clock(),
        stop: stop.clone(),
        log,
        status_path: options.status_path,
        env: Arc::new(move |name| env.get(name).cloned()),
    });
    let worker = options.start_worker.then(|| {
        let sender = Arc::clone(&sender);
        std::thread::spawn(move || sender.run_worker())
    });
    Rig {
        sender,
        stop,
        clock,
        fake,
        logs,
        worker,
    }
}

fn rig(ntfy: Ntfy) -> Rig {
    rig_with(RigOptions::new(ntfy), None)
}

impl Rig {
    fn start(&mut self) {
        let sender = Arc::clone(&self.sender);
        self.worker = Some(std::thread::spawn(move || sender.run_worker()));
    }
    fn stop(&mut self) -> Duration {
        let started = Instant::now();
        self.stop.stop();
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
        started.elapsed()
    }
    fn log_count(&self, event: &str) -> usize {
        lock(&self.logs).iter().filter(|(e, _)| e == event).count()
    }
    fn stat(&self, key: &str) -> u64 {
        self.sender.status()[key].as_u64().unwrap()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.stop.stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn ev(event: NtfyEvent, subject: &str) -> PushEvent {
    PushEvent {
        seq: 1,
        ts: "2026-01-01T00:00:00.000Z".into(),
        event,
        subject: subject.into(),
        agent_id: None,
        plan_id: None,
        detail: None,
    }
}

// ------------------------------------------------------------------------------------------------ lanes, caps, retries

#[test]
fn enqueue_never_waits_for_the_network() {
    let mut r = rig(base_ntfy());
    r.fake.close_gate();
    r.sender.enqueue(&ev(NtfyEvent::UserQuestion, "1"));
    wait_for("the first send to start", || r.fake.count() == 1);
    // The transport is blocked; more messages queue at once.
    let started = Instant::now();
    for n in 2..6 {
        r.sender
            .enqueue(&ev(NtfyEvent::UserQuestion, &n.to_string()));
    }
    assert!(started.elapsed() < Duration::from_millis(200));
    assert_eq!(r.stat("queued"), 4);
    r.fake.open_gate();
    wait_for("all sends", || r.stat("sent") == 5);
    r.stop();
}

#[test]
fn a_blocked_transport_does_not_hold_a_kernel_job() {
    let (_dir, kernel) = open_kernel();
    let mut r = rig(base_ntfy());
    r.fake.close_gate();
    r.sender.enqueue(&ev(NtfyEvent::UserQuestion, "1"));
    wait_for("the send to block", || r.fake.count() == 1);
    // The sender holds no kernel handle, so a kernel job from another thread runs while the send is blocked.
    let worker_thread = Arc::new(Mutex::new(None));
    let seen = Arc::clone(&worker_thread);
    let job = std::thread::spawn({
        let kernel = kernel.clone();
        move || {
            kernel
                .run(move |core| {
                    *lock(&seen) = std::thread::current().name().map(str::to_string);
                    core.state_version()
                })
                .unwrap()
        }
    });
    job.join().unwrap();
    assert_ne!(lock(&worker_thread).as_deref(), Some("ntfy-send"));
    r.fake.open_gate();
    r.stop();
    kernel.close();
}

fn texts(r: &Rig) -> Vec<String> {
    lock(&r.fake.calls)
        .iter()
        .map(|c| c.message.clone())
        .collect()
}

#[test]
fn a_user_question_goes_before_forty_low_events() {
    let mut r = rig_with(
        RigOptions {
            start_worker: false,
            ..RigOptions::new(base_ntfy())
        },
        None,
    );
    for n in 0..40 {
        r.sender
            .enqueue(&ev(NtfyEvent::AgentStalled, &format!("a{n}")));
    }
    r.sender.enqueue(&ev(NtfyEvent::UserQuestion, "7"));
    r.start();
    wait_for("the first send", || r.fake.count() >= 1);
    assert!(texts(&r)[0].contains("(#7)"), "{:?}", texts(&r)[0]);
    // 20 low events fit under the cap; the other 20 were counted as dropped.
    wait_for("all sends", || r.stat("sent") == 21);
    assert_eq!(r.stat("dropped"), 20);
    r.stop();
}

fn fill_high(r: &Rig, event: NtfyEvent, total: usize) {
    // The cap lets 30 through per 10 minutes; moving the clock past it lets the lane itself overflow.
    for n in 1..=total {
        if n == 31 {
            r.clock.advance(601_000);
        }
        r.sender.enqueue(&ev(event, &n.to_string()));
    }
}

#[test]
fn a_full_high_lane_drops_its_oldest_entry() {
    let mut r = rig_with(
        RigOptions {
            start_worker: false,
            ..RigOptions::new(base_ntfy())
        },
        None,
    );
    fill_high(&r, NtfyEvent::UserQuestion, 33);
    assert_eq!(r.stat("queued"), 32);
    assert_eq!(r.stat("dropped"), 1);
    r.start();
    wait_for("the first send", || r.fake.count() >= 1);
    assert!(texts(&r)[0].contains("(#2)"), "{}", texts(&r)[0]);
    r.stop();
}

#[test]
fn a_newer_high_message_pushes_out_an_older_high_one_of_another_event() {
    let mut r = rig_with(
        RigOptions {
            start_worker: false,
            ..RigOptions::new(base_ntfy())
        },
        None,
    );
    for n in 1..=32 {
        if n == 31 {
            r.clock.advance(601_000);
        }
        let event = if n == 1 {
            NtfyEvent::UserQuestion
        } else {
            NtfyEvent::PromptWaiting
        };
        r.sender.enqueue(&ev(event, &n.to_string()));
    }
    r.sender.enqueue(&ev(NtfyEvent::OperatorProposal, "99"));
    assert_eq!(r.stat("dropped"), 1);
    r.start();
    wait_for("the first send", || r.fake.count() >= 1);
    // The oldest entry, the user_question, was the one that went.
    assert!(!texts(&r)[0].contains("(#1)"));
    r.stop();
}

#[test]
fn a_waiting_retry_does_not_hold_other_messages() {
    let mut ntfy = base_ntfy();
    ntfy.retries = 3;
    let mut r = rig(ntfy);
    for _ in 0..3 {
        r.fake.push(Err(failure("http_5xx", Some(503))));
    }
    r.sender.enqueue(&ev(NtfyEvent::AgentStalled, "a1"));
    wait_for("attempt 1", || r.fake.count() == 1);
    wait_for("requeue 1", || r.stat("queued") == 1);
    r.clock.advance(2_000);
    wait_for("attempt 2", || r.fake.count() == 2);
    wait_for("requeue 2", || r.stat("queued") == 1);
    r.clock.advance(8_000);
    wait_for("attempt 3", || r.fake.count() == 3);
    wait_for("requeue 3", || r.stat("queued") == 1);
    // The low message now waits 30 s; a user question is sent at once.
    r.sender.enqueue(&ev(NtfyEvent::UserQuestion, "5"));
    wait_for("the question", || r.fake.count() == 4);
    assert!(texts(&r)[3].contains("(#5)"));
    r.clock.advance(30_000);
    wait_for("attempt 4", || r.fake.count() == 5);
    r.stop();
}

#[test]
fn retries_stop_after_retries_plus_one_attempts_and_only_for_retriable_reasons() {
    for reason in ["http_5xx", "timeout", "connect_error", "transport_error"] {
        let mut ntfy = base_ntfy();
        ntfy.retries = 2;
        let mut r = rig(ntfy);
        for _ in 0..5 {
            r.fake.push(Err(failure(reason, None)));
        }
        r.sender.enqueue(&ev(NtfyEvent::AgentLost, "a"));
        wait_for("attempt 1", || r.fake.count() == 1);
        wait_for("requeue 1", || r.stat("queued") == 1);
        r.clock.advance(2_000);
        wait_for("attempt 2", || r.fake.count() == 2);
        wait_for("requeue 2", || r.stat("queued") == 1);
        r.clock.advance(8_000);
        wait_for("attempt 3", || r.fake.count() == 3);
        wait_for("the failure", || r.stat("failed") == 1);
        r.clock.advance(100_000);
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(r.fake.count(), 3, "{reason}");
        r.stop();
    }
    for (reason, status) in [
        ("unauthorized", 401),
        ("forbidden", 403),
        ("http_4xx", 400),
        ("http_3xx", 302),
        ("transport_unavailable", 0),
        ("secret_unreadable", 0),
    ] {
        let mut r = rig(base_ntfy());
        r.fake
            .push(Err(failure(reason, (status != 0).then_some(status))));
        r.sender.enqueue(&ev(NtfyEvent::AgentLost, "a"));
        wait_for("the failure", || r.stat("failed") == 1);
        r.clock.advance(100_000);
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(r.fake.count(), 1, "{reason}");
        r.stop();
    }
}

#[test]
fn a_429_waits_the_retry_after_seconds() {
    let mut r = rig(base_ntfy());
    r.fake.push(Err(TransportError {
        retry_after: Some(5),
        ..failure("http_429", Some(429))
    }));
    r.sender.enqueue(&ev(NtfyEvent::AgentLost, "a"));
    wait_for("attempt 1", || r.fake.count() == 1);
    wait_for("requeue", || r.stat("queued") == 1);
    r.clock.advance(4_900);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(r.fake.count(), 1);
    r.clock.advance(100);
    wait_for("attempt 2", || r.fake.count() == 2);
    wait_for("the send", || r.stat("sent") == 1);
    assert_eq!(r.stat("failed"), 0);
    r.stop();
}

#[test]
fn coalescing_and_caps_follow_the_clock() {
    let mut r = rig_with(
        RigOptions {
            start_worker: false,
            ..RigOptions::new(base_ntfy())
        },
        None,
    );
    r.sender.enqueue(&ev(NtfyEvent::AgentLost, "a"));
    r.sender.enqueue(&ev(NtfyEvent::AgentLost, "a"));
    r.sender.enqueue(&ev(NtfyEvent::AgentLost, "b"));
    r.sender.enqueue(&ev(NtfyEvent::AgentBlocked, "a"));
    assert_eq!((r.stat("queued"), r.stat("dropped")), (3, 1));
    r.clock.advance(61_000);
    r.sender.enqueue(&ev(NtfyEvent::AgentLost, "a"));
    assert_eq!((r.stat("queued"), r.stat("dropped")), (4, 1));
    // Low cap: 20 per 10 minutes (4 are counted already).
    for n in 0..20 {
        r.sender
            .enqueue(&ev(NtfyEvent::VerifyFailed, &format!("v{n}")));
    }
    assert_eq!(r.stat("dropped"), 1 + 4);
    r.clock.advance(601_000);
    r.sender.enqueue(&ev(NtfyEvent::VerifyFailed, "late"));
    assert_eq!(r.stat("dropped"), 5);
    r.stop();
}

#[test]
fn events_the_config_does_not_select_are_ignored() {
    let mut ntfy = base_ntfy();
    ntfy.events = vec![NtfyEvent::UserQuestion];
    let mut r = rig_with(
        RigOptions {
            start_worker: false,
            ..RigOptions::new(ntfy)
        },
        None,
    );
    r.sender.enqueue(&ev(NtfyEvent::AgentLost, "a"));
    assert_eq!((r.stat("queued"), r.stat("dropped")), (0, 0));
    r.stop();
}

// ------------------------------------------------------------------------------------------------ the tail

type ScriptedPage = Result<(Vec<PushEvent>, u64), String>;

struct ScriptedSource {
    clock: FakeClock,
    reads: Mutex<Vec<u64>>,
    pages: Mutex<VecDeque<ScriptedPage>>,
    head: u64,
    read_ms: i64,
}

impl PushSource for ScriptedSource {
    fn head(&self) -> Result<u64, String> {
        Ok(self.head)
    }
    fn read(&self, after: u64) -> Result<PushPage, String> {
        lock(&self.reads).push(after);
        self.clock.advance(self.read_ms);
        match lock(&self.pages).pop_front() {
            Some(Ok((events, scanned))) => Ok(PushPage {
                events,
                scanned,
                queries: 1,
            }),
            Some(Err(e)) => Err(e),
            None => Ok(PushPage {
                events: vec![],
                scanned: after,
                queries: 1,
            }),
        }
    }
}

fn tail_over(source: Arc<ScriptedSource>, clock: &FakeClock) -> (Tail, Arc<Mutex<Vec<u64>>>, Logs) {
    let seen: Arc<Mutex<Vec<u64>>> = Arc::default();
    let logs: Logs = Arc::default();
    let log: DriverLog = {
        let logs = Arc::clone(&logs);
        Arc::new(move |event, details| lock(&logs).push((event.to_string(), details)))
    };
    let sink = {
        let seen = Arc::clone(&seen);
        Box::new(move |e: &PushEvent| lock(&seen).push(e.seq)) as Box<dyn Fn(&PushEvent) + Send>
    };
    (Tail::new(source, clock.clock(), log, sink), seen, logs)
}

fn source(clock: &FakeClock, head: u64, read_ms: i64) -> Arc<ScriptedSource> {
    Arc::new(ScriptedSource {
        clock: clock.clone(),
        reads: Mutex::default(),
        pages: Mutex::default(),
        head,
        read_ms,
    })
}

fn event_at(seq: u64) -> PushEvent {
    PushEvent {
        seq,
        ..ev(NtfyEvent::AgentLost, &format!("a{seq}"))
    }
}

#[test]
fn the_tail_survives_a_failed_read_and_yields_each_event_once() {
    let clock = FakeClock::new();
    let src = source(&clock, 40, 1);
    lock(&src.pages).push_back(Err("kernel busy".into()));
    lock(&src.pages).push_back(Ok((vec![event_at(41), event_at(42), event_at(43)], 43)));
    let (mut tail, seen, logs) = tail_over(src.clone(), &clock);
    assert_eq!(tail.step(), Duration::from_millis(4_000));
    assert!(lock(&seen).is_empty());
    assert_eq!(tail.step(), Duration::from_millis(BASE_INTERVAL_MS));
    assert_eq!(*lock(&seen), vec![41, 42, 43]);
    tail.step();
    assert_eq!(*lock(&seen), vec![41, 42, 43]);
    assert_eq!(*lock(&src.reads), vec![40, 40, 43]);
    assert_eq!(
        lock(&logs)
            .iter()
            .filter(|(e, _)| e == "ntfy:read_failed")
            .count(),
        1
    );
}

#[test]
fn an_empty_page_moves_the_cursor_to_scanned() {
    let clock = FakeClock::new();
    let src = source(&clock, 10, 1);
    lock(&src.pages).push_back(Ok((vec![], 40)));
    let (mut tail, _, _) = tail_over(src.clone(), &clock);
    tail.step();
    tail.step();
    assert_eq!(*lock(&src.reads), vec![10, 40]);
    assert_eq!(tail.cursor(), Some(40));
}

#[test]
fn a_slow_read_backs_off_to_thirty_seconds_and_a_fast_one_resets() {
    let clock = FakeClock::new();
    let src = source(&clock, 0, 400);
    let (mut tail, _, _) = tail_over(src.clone(), &clock);
    let waits: Vec<u64> = (0..6).map(|_| tail.step().as_millis() as u64).collect();
    assert_eq!(waits, vec![4_000, 8_000, 16_000, 30_000, 30_000, 30_000]);
    assert_eq!(MAX_INTERVAL_MS, 30_000);
    let fast = source(&clock, 0, 10);
    let (mut tail, _, _) = tail_over(fast, &clock);
    assert_eq!(tail.step(), Duration::from_millis(2_000));
}

// ------------------------------------------------------------------------------------------------ messages

fn event_with(
    event: NtfyEvent,
    subject: &str,
    agent: Option<&str>,
    plan: Option<&str>,
    detail: Option<&str>,
) -> PushEvent {
    PushEvent {
        agent_id: agent.map(String::from),
        plan_id: plan.map(String::from),
        detail: detail.map(String::from),
        ..ev(event, subject)
    }
}

fn sent_text(event: PushEvent) -> (String, String) {
    let mut r = rig(base_ntfy());
    r.sender.enqueue(&event);
    wait_for("the send", || r.fake.count() == 1);
    let call = lock(&r.fake.calls)[0].clone();
    r.stop();
    (call.title, call.message)
}

macro_rules! message_test {
    ($name:ident, $event:expr, $subject:expr, $agent:expr, $plan:expr, $detail:expr, $label:expr, $text:expr) => {
        #[test]
        fn $name() {
            let (title, message) = sent_text(event_with($event, $subject, $agent, $plan, $detail));
            assert_eq!(title, format!("Capstan: {}", $label));
            assert_eq!(message, $text);
        }
    };
}

message_test!(
    user_question_text,
    NtfyEvent::UserQuestion,
    "12",
    Some("dev-1"),
    None,
    None,
    "Question",
    "dev-1 asked you a question (#12). Answer in Capstan."
);
message_test!(
    operator_proposal_text,
    NtfyEvent::OperatorProposal,
    "4",
    Some("dev-1"),
    None,
    None,
    "Approval needed",
    "Operator proposal #4 waits for you."
);
message_test!(
    prompt_waiting_text,
    NtfyEvent::PromptWaiting,
    "dev-1",
    Some("dev-1"),
    None,
    None,
    "Prompt waiting",
    "dev-1 is waiting on a prompt."
);
message_test!(
    design_waiting_text,
    NtfyEvent::DesignWaiting,
    "rep-1",
    Some("dev-1"),
    Some("plan-9"),
    None,
    "Design review",
    "Design rep-1 of plan-9 waits for approval."
);
message_test!(
    plan_signed_off_text,
    NtfyEvent::PlanSignedOff,
    "plan-9",
    None,
    Some("plan-9"),
    Some("int-3"),
    "Plan signed off",
    "plan-9 signed off (int-3)."
);
message_test!(
    agent_blocked_text,
    NtfyEvent::AgentBlocked,
    "dev-2",
    Some("dev-2"),
    None,
    None,
    "Agent blocked",
    "dev-2 is blocked."
);
message_test!(
    agent_stalled_text,
    NtfyEvent::AgentStalled,
    "dev-2",
    Some("dev-2"),
    None,
    None,
    "Agent stalled",
    "dev-2 has stalled."
);
message_test!(
    agent_lost_text,
    NtfyEvent::AgentLost,
    "dev-2",
    Some("dev-2"),
    None,
    None,
    "Agent lost",
    "dev-2 was lost."
);
message_test!(
    verify_failed_text,
    NtfyEvent::VerifyFailed,
    "run-5",
    None,
    None,
    Some("integration int-3"),
    "Verify failed",
    "Verify run-5 failed for integration int-3."
);
message_test!(
    integration_conflicted_text,
    NtfyEvent::IntegrationConflicted,
    "int-3",
    None,
    None,
    Some("3"),
    "Conflict",
    "Integration int-3 conflicted (3 files)."
);
message_test!(
    pm_stuck_text,
    NtfyEvent::PmStuck,
    "pm-1",
    Some("pm-1"),
    None,
    Some("unread_mail:15"),
    "PM stuck",
    "pm-1 looks stuck (unread_mail, 15 min)."
);

#[test]
fn the_title_uses_the_cleaned_prefix_and_the_style_of_the_event() {
    let ntfy = base_ntfy();
    let prefix = effective_prefix(&ntfy, Some("My\nProject"));
    assert_eq!(prefix, "MyProject");
    let mut r = rig_with(
        RigOptions {
            prefix,
            ..RigOptions::new(ntfy)
        },
        None,
    );
    r.sender.enqueue(&ev(NtfyEvent::UserQuestion, "1"));
    wait_for("the send", || r.fake.count() == 1);
    assert_eq!(lock(&r.fake.calls)[0].title, "MyProject: Question");
    r.stop();
}

// ------------------------------------------------------------------------------------------------ status

#[test]
fn the_status_object_has_the_frozen_key_set() {
    let mut r = rig(base_ntfy());
    let keys = |v: &Value| -> Vec<String> {
        let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
        k.sort();
        k
    };
    let mut expected: Vec<String> = [
        "enabled",
        "server",
        "events",
        "sent",
        "failed",
        "dropped",
        "queued",
        "lastSentAt",
        "lastError",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    expected.sort();
    let status = r.sender.status();
    assert_eq!(keys(&status), expected);
    assert_eq!(status["server"], "ntfy.example");
    assert!(status["lastError"].is_null() && status["lastSentAt"].is_null());
    r.fake.push(Err(failure("unauthorized", Some(401))));
    r.sender.enqueue(&ev(NtfyEvent::AgentLost, "a"));
    wait_for("the failure", || r.stat("failed") == 1);
    let status = r.sender.status();
    assert_eq!(
        keys(&status["lastError"]),
        vec!["at", "httpStatus", "reason"]
    );
    assert_eq!(status["lastError"]["httpStatus"], 401);
    r.stop();
}

#[test]
fn the_status_file_is_private_complete_and_free_of_secrets() {
    let dir = private_tempdir();
    let path = dir.path().join("ntfy-status.json");
    let mut ntfy = base_ntfy();
    ntfy.auth = NtfyAuth::TokenEnv("NTFY_TOKEN".into());
    let mut options = RigOptions::new(ntfy);
    options.status_path = Some(path.clone());
    options
        .env
        .insert("NTFY_TOKEN".into(), "tok_SENTINEL".into());
    let mut r = rig_with(options, None);
    r.sender.enqueue(&ev(NtfyEvent::AgentLost, "a"));
    wait_for("the send", || r.stat("sent") == 1);
    r.stop();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("tok_SENTINEL") && !text.contains("NTFY_TOKEN"));
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
        0o600
    );
    let value: Value = serde_json::from_str(&text).unwrap();
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "dropped",
            "enabled",
            "events",
            "failed",
            "lastError",
            "lastSentAt",
            "pid",
            "pidStartTime",
            "queued",
            "sent",
            "server",
            "writtenAt"
        ]
    );
    assert_eq!(value["sent"], 1);
    assert_eq!(value["pid"], std::process::id());
    assert_eq!(
        lock(&r.fake.calls)[0].secret.as_deref(),
        Some("tok_SENTINEL")
    );
}

#[test]
fn an_unwritable_state_directory_does_not_stop_the_sender() {
    let dir = private_tempdir();
    let mut options = RigOptions::new(base_ntfy());
    options.status_path = Some(dir.path().join("missing").join("ntfy-status.json"));
    let mut r = rig_with(options, None);
    for n in 0..3 {
        r.sender.enqueue(&ev(NtfyEvent::AgentLost, &n.to_string()));
    }
    wait_for("the sends", || r.stat("sent") == 3);
    r.stop();
    assert_eq!(r.stat("failed"), 0);
    assert_eq!(r.log_count("ntfy:status_write_failed"), 1);
}

#[test]
fn a_missing_secret_fails_without_naming_it_in_the_status() {
    let mut ntfy = base_ntfy();
    ntfy.auth = NtfyAuth::TokenEnv("NTFY_TOKEN".into());
    let mut r = rig(ntfy);
    r.sender.enqueue(&ev(NtfyEvent::AgentLost, "a"));
    wait_for("the failure", || r.stat("failed") == 1);
    let status = r.sender.status();
    assert_eq!(status["lastError"]["reason"], "secret_missing");
    assert!(!status.to_string().contains("NTFY_TOKEN"));
    assert_eq!(r.fake.count(), 0);
    r.stop();
}

// ------------------------------------------------------------------------------------------------ stop bounds

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn process_gone(pid: i32) -> bool {
    // SAFETY: signal 0 only tests whether the process exists.
    unsafe { libc::kill(pid, 0) != 0 }
}

fn read_pid(path: &Path) -> i32 {
    wait_for("the pid file", || {
        std::fs::read_to_string(path).is_ok_and(|t| t.trim().parse::<i32>().is_ok())
    });
    std::fs::read_to_string(path)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

fn curl_for(ntfy: &Ntfy, program: &Path) -> CurlTransport {
    CurlTransport::new(ntfy)
        .with_program(program)
        .with_env(Arc::new(|name| {
            (name == "PATH").then(|| std::env::var("PATH").unwrap_or_default())
        }))
}

fn request() -> PublishRequest {
    PublishRequest {
        topic: "alerts".into(),
        title: "Capstan: Test".into(),
        message: "hello".into(),
        priority: Some(3),
        tags: vec!["x".into()],
        click: None,
    }
}

#[test]
fn a_stop_kills_a_curl_that_runs_for_a_minute() {
    let dir = private_tempdir();
    let pid_file = dir.path().join("pid");
    let program = script(
        dir.path(),
        "slow-curl",
        &format!("echo $$ > {}\nsleep 60", pid_file.display()),
    );
    let ntfy = base_ntfy();
    let mut r = rig_with(
        RigOptions::new(ntfy.clone()),
        Some(Arc::new(curl_for(&ntfy, &program))),
    );
    r.sender.enqueue(&ev(NtfyEvent::AgentLost, "a"));
    let pid = read_pid(&pid_file);
    let took = r.stop();
    assert!(took < Duration::from_millis(1500), "{took:?}");
    wait_for("the child to be gone", || process_gone(pid));
}

#[test]
fn a_stop_during_a_retry_wait_is_prompt() {
    let mut r = rig(base_ntfy());
    r.fake.push(Err(failure("http_5xx", Some(500))));
    r.sender.enqueue(&ev(NtfyEvent::AgentLost, "a"));
    wait_for("attempt 1", || r.fake.count() == 1);
    wait_for("the retry to queue", || r.stat("queued") == 1);
    assert!(r.stop() < Duration::from_millis(1500));
}

#[test]
fn a_stop_during_a_notify_test_answers_an_error_and_the_child_is_gone() {
    let dir = private_tempdir();
    let pid_file = dir.path().join("pid");
    let program = script(
        dir.path(),
        "slow-curl",
        &format!("echo $$ > {}\nsleep 60", pid_file.display()),
    );
    let ntfy = base_ntfy();
    let r = rig_with(
        RigOptions::new(ntfy.clone()),
        Some(Arc::new(curl_for(&ntfy, &program))),
    );
    let sender = Arc::clone(&r.sender);
    let asker = std::thread::spawn(move || sender.send_test().unwrap());
    let pid = read_pid(&pid_file);
    let started = Instant::now();
    r.stop.stop();
    let answer = asker.join().unwrap();
    assert!(started.elapsed() < Duration::from_millis(1500));
    assert!(!answer.ok && answer.error.is_some());
    wait_for("the child to be gone", || process_gone(pid));
}

// ------------------------------------------------------------------------------------------------ curl: fake programs

fn capture_script(dir: &Path, name: &str, tail: &str) -> (PathBuf, PathBuf, PathBuf) {
    let stdin = dir.join(format!("{name}.stdin"));
    let argv = dir.join(format!("{name}.argv"));
    let program = script(
        dir,
        name,
        &format!(
            "printf '%s\\n' \"$@\" > {}\ncat > {}\n{tail}",
            argv.display(),
            stdin.display()
        ),
    );
    (program, stdin, argv)
}

fn env_of(pairs: &[(&str, &str)]) -> capstan_daemon::ntfy::transport::EnvReader {
    let mut map: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    map.insert("PATH".into(), std::env::var("PATH").unwrap_or_default());
    Arc::new(move |name| map.get(name).cloned())
}

const OK_TAIL: &str = "printf '\\ncapstan-retry-after:\\ncapstan-http-status:200'";

fn secret_of(text: &str) -> Secret {
    let mut ntfy = base_ntfy();
    ntfy.auth = NtfyAuth::TokenEnv("T".into());
    let text = text.to_string();
    capstan_config::ntfy::read_secret(&ntfy.auth, &move |_| Some(text.clone()))
        .unwrap()
        .unwrap()
}

#[test]
fn the_argument_list_holds_no_url_secret_or_body_and_stdin_holds_the_config() {
    let dir = private_tempdir();
    let (program, stdin, argv) = capture_script(dir.path(), "curl", OK_TAIL);
    let mut ntfy = base_ntfy();
    ntfy.ca_file = Some(PathBuf::from("/etc/ca.pem"));
    let transport = curl_for(&ntfy, &program);
    let status = transport
        .publish(
            &request(),
            Some(&secret_of("tok_SENTINEL")),
            Duration::from_secs(5),
            &StopFlag::new(),
        )
        .unwrap();
    assert_eq!(status, 200);
    let args = std::fs::read_to_string(&argv).unwrap();
    for hidden in ["tok_SENTINEL", "ntfy.example", "alerts", "hello"] {
        assert!(!args.contains(hidden), "{hidden} in {args}");
    }
    assert!(args.starts_with("-q\n"));
    assert!(args.contains("--config\n-"));
    let config = std::fs::read_to_string(&stdin).unwrap();
    assert!(config.contains("url = \"https://ntfy.example/\""));
    assert!(config.contains("request = \"POST\""));
    assert!(config.contains("header = \"Authorization: Bearer tok_SENTINEL\""));
    assert!(config.contains("cacert = \"/etc/ca.pem\""));
    assert!(config.contains("data-binary = \"{\\\"topic\\\":\\\"alerts\\\""));
    assert!(config.contains("write-out = \"\\ncapstan-retry-after:%header{retry-after}\\ncapstan-http-status:%{http_code}\""));
}

#[test]
fn proxy_lines_follow_the_scheme_and_no_proxy() {
    let dir = private_tempdir();
    let run = |server: &str, pairs: &[(&str, &str)]| -> String {
        let (program, stdin, _) = capture_script(dir.path(), "curl", OK_TAIL);
        let mut ntfy = base_ntfy();
        ntfy.server = server.into();
        let transport = CurlTransport::new(&ntfy)
            .with_program(&program)
            .with_env(env_of(pairs));
        transport
            .publish(&request(), None, Duration::from_secs(5), &StopFlag::new())
            .unwrap();
        std::fs::read_to_string(&stdin).unwrap()
    };
    let both = [
        ("HTTPS_PROXY", "http://sp:3128"),
        ("HTTP_PROXY", "http://hp:3128"),
    ];
    assert!(run("https://ntfy.example", &both).contains("proxy = \"http://sp:3128\""));
    assert!(run("http://localhost:8080", &both).contains("proxy = \"http://hp:3128\""));
    let covered = [
        ("HTTPS_PROXY", "http://sp:3128"),
        ("NO_PROXY", "other.test,.example"),
    ];
    assert!(!run("https://ntfy.example", &covered).contains("proxy"));
    let lower = [
        ("https_proxy", "http://lp:3128"),
        ("no_proxy", "elsewhere.test"),
    ];
    assert!(run("https://ntfy.example", &lower).contains("proxy = \"http://lp:3128\""));
    let lower_covered = [
        ("https_proxy", "http://lp:3128"),
        ("no_proxy", "ntfy.example"),
    ];
    assert!(!run("https://ntfy.example", &lower_covered).contains("proxy"));
}

#[test]
fn a_control_character_in_a_proxy_value_sends_nothing() {
    let dir = private_tempdir();
    let (program, stdin, _) = capture_script(dir.path(), "curl", OK_TAIL);
    let ntfy = base_ntfy();
    let transport = CurlTransport::new(&ntfy)
        .with_program(&program)
        .with_env(env_of(&[(
            "HTTPS_PROXY",
            "http://p:1\nurl = \"http://evil\"",
        )]));
    let error = transport
        .publish(&request(), None, Duration::from_secs(5), &StopFlag::new())
        .unwrap_err();
    assert_eq!(error.reason, "transport_error");
    assert!(!stdin.exists(), "curl must not have run");
    assert!(curl_quote("a\u{1}b").is_err());
}

#[test]
fn the_status_is_the_final_line_only() {
    let dir = private_tempdir();
    let ntfy = base_ntfy();
    let run = |tail: &str| {
        let (program, _, _) = capture_script(dir.path(), "curl", tail);
        curl_for(&ntfy, &program).publish(
            &request(),
            None,
            Duration::from_secs(5),
            &StopFlag::new(),
        )
    };
    let forged = "printf 'x\\ncapstan-http-status:500\\n'\nprintf '\\ncapstan-http-status:200'";
    assert_eq!(run(forged), Ok(200));
    let forged_ok = "printf 'x\\ncapstan-http-status:200\\n'\nprintf '\\ncapstan-http-status:503'";
    assert_eq!(run(forged_ok).unwrap_err().reason, "http_5xx");
    assert_eq!(
        run("printf 'no marker'").unwrap_err().reason,
        "transport_error"
    );
    assert_eq!(run("exit 4").unwrap_err().reason, "transport_unavailable");
    assert_eq!(run("exit 28").unwrap_err().reason, "timeout");
    assert_eq!(run("exit 7").unwrap_err().reason, "connect_error");
    let missing = CurlTransport::new(&ntfy).with_program(dir.path().join("none"));
    assert_eq!(
        missing
            .publish(&request(), None, Duration::from_secs(5), &StopFlag::new())
            .unwrap_err()
            .reason,
        "transport_unavailable"
    );
    let limited = "printf '\\ncapstan-retry-after:900\\ncapstan-http-status:429'";
    let error = run(limited).unwrap_err();
    assert_eq!(
        (error.reason.as_str(), error.retry_after),
        ("http_429", Some(60))
    );
}

#[test]
fn secrets_and_the_server_url_never_reach_the_log_status_or_answers() {
    let dir = private_tempdir();
    // The fake curl echoes everything it was given to stderr and fails.
    let program = script(dir.path(), "echo-curl", "cat >&2\nexit 22");
    let mut ntfy = base_ntfy();
    ntfy.auth = NtfyAuth::TokenEnv("NTFY_TOKEN".into());
    ntfy.retries = 0;
    let transport = CurlTransport::new(&ntfy)
        .with_program(&program)
        .with_env(env_of(&[("HTTPS_PROXY", "http://u:p@proxy:3128")]));
    let mut options = RigOptions::new(ntfy);
    options
        .env
        .insert("NTFY_TOKEN".into(), "tok_SENTINEL".into());
    let mut r = rig_with(options, Some(Arc::new(transport)));
    let answer = r.sender.send_test().unwrap();
    r.sender.enqueue(&ev(NtfyEvent::AgentLost, "a"));
    wait_for("the failure", || r.stat("failed") == 2);
    r.stop();
    let everything = format!(
        "{}{}{}",
        answer.to_value(),
        r.sender.status(),
        serde_json::to_string(&*lock(&r.logs)).unwrap()
    );
    for hidden in [
        "tok_SENTINEL",
        "http://u:p@proxy",
        "u:p",
        "https://ntfy.example",
    ] {
        assert!(!everything.contains(hidden), "{hidden} in {everything}");
    }
    assert!(answer.error.unwrap().chars().count() <= 200);
}

// ------------------------------------------------------------------------------------------------ curl: the real program

fn have_curl() -> bool {
    let found = std::process::Command::new("curl")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !found {
        assert!(
            std::env::var("CAPSTAN_REQUIRE_CURL").as_deref() != Ok("1"),
            "curl is required (CAPSTAN_REQUIRE_CURL=1) and is missing"
        );
        eprintln!("skipped: curl is not installed");
    }
    found
}

static CURL_TESTS: Mutex<()> = Mutex::new(());

/// A local HTTP listener: reads one request, tells `seen` its body, waits for `release`, answers `status`.
fn listener(
    status: u16,
    seen: std::sync::mpsc::Sender<String>,
    release: Option<std::sync::mpsc::Receiver<()>>,
) -> (String, std::thread::JoinHandle<()>) {
    let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}", socket.local_addr().unwrap().port());
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = socket.accept().unwrap();
        let mut data = Vec::new();
        let mut chunk = [0u8; 4096];
        let (head_end, length) = loop {
            let n = stream.read(&mut chunk).unwrap();
            data.extend_from_slice(&chunk[..n]);
            if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&data[..at]).to_lowercase();
                let length = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .map(|v| v.trim().parse::<usize>().unwrap())
                    .unwrap_or(0);
                break (at + 4, length);
            }
        };
        while data.len() < head_end + length {
            let n = stream.read(&mut chunk).unwrap();
            data.extend_from_slice(&chunk[..n]);
        }
        let _ = seen.send(String::from_utf8_lossy(&data[head_end..head_end + length]).into_owned());
        if let Some(release) = release {
            let _ = release.recv_timeout(Duration::from_secs(20));
        }
        let _ = write!(
            stream,
            "HTTP/1.1 {status} X\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
        );
    });
    (url, handle)
}

fn real_curl_ntfy(url: &str) -> Ntfy {
    let mut ntfy = base_ntfy();
    ntfy.server = url.into();
    ntfy
}

#[test]
fn awkward_text_reaches_a_listener_as_the_exact_json() {
    if !have_curl() {
        return;
    }
    let _guard = lock(&CURL_TESTS);
    let (tx, rx) = std::sync::mpsc::channel();
    let (url, server) = listener(200, tx, None);
    let ntfy = real_curl_ntfy(&url);
    let req = PublishRequest {
        title: "Say \"hi\" \\ back\nnew line \u{1F389} é".into(),
        message: "tab\there $HOME `x` @file".into(),
        ..request()
    };
    let transport = CurlTransport::new(&ntfy);
    assert_eq!(
        transport.publish(&req, None, Duration::from_secs(10), &StopFlag::new()),
        Ok(200)
    );
    assert_eq!(rx.recv().unwrap(), req.to_json());
    server.join().unwrap();
}

#[test]
fn the_curl_child_holds_no_secret_in_its_command_line_or_environment() {
    if !have_curl() || !cfg!(target_os = "linux") {
        return;
    }
    let _guard = lock(&CURL_TESTS);
    let (tx, rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (url, server) = listener(200, tx, Some(release_rx));
    let mut ntfy = real_curl_ntfy(&url);
    ntfy.auth = NtfyAuth::TokenEnv("T".into());
    let transport = Arc::new(CurlTransport::new(&ntfy));
    let secret = secret_of("tok_SENTINEL");
    let publisher = {
        let transport = Arc::clone(&transport);
        std::thread::spawn(move || {
            transport.publish(
                &request(),
                Some(&secret),
                Duration::from_secs(20),
                &StopFlag::new(),
            )
        })
    };
    // The listener holds the request: curl is alive and waiting for the answer.
    rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let me = std::process::id();
    let mut checked = false;
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let pid = entry.file_name().to_string_lossy().to_string();
        if !pid.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        let after = stat.rsplit_once(')').map(|(_, r)| r.trim()).unwrap_or("");
        let mut fields = after.split(' ');
        let ppid = fields.nth(1).unwrap_or("");
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
        if ppid != me.to_string() || comm.trim() != "curl" {
            continue;
        }
        let (Ok(cmdline), Ok(environ)) = (
            std::fs::read(format!("/proc/{pid}/cmdline")),
            std::fs::read(format!("/proc/{pid}/environ")),
        ) else {
            eprintln!("curl child {pid} unreadable: {stat}");
            continue;
        };
        let cmd = String::from_utf8_lossy(&cmdline).replace('\0', " ");
        for hidden in ["tok_SENTINEL", "127.0.0.1", "alerts", "hello", "http"] {
            assert!(
                !cmd.replace("=http,https", "").contains(hidden),
                "{hidden} in {cmd}"
            );
        }
        for var in environ.split(|b| *b == 0).filter(|v| !v.is_empty()) {
            let name = String::from_utf8_lossy(var);
            let name = name.split('=').next().unwrap().to_string();
            assert!(
                ["PATH", "SSL_CERT_FILE", "SSL_CERT_DIR", "CURL_CA_BUNDLE"]
                    .contains(&name.as_str()),
                "unexpected variable {name}"
            );
        }
        checked = true;
    }
    assert!(
        checked,
        "no curl child was found while the listener held the request"
    );
    release_tx.send(()).unwrap();
    assert_eq!(publisher.join().unwrap(), Ok(200));
    server.join().unwrap();
}

// ------------------------------------------------------------------------------------------------ the daemon

fn open_kernel() -> (common::PrivateDir, KernelHandle) {
    let dir = private_tempdir();
    let state = dir.path().to_path_buf();
    let identity = fixture_identity();
    let project = identity.initial_project();
    let opened = state.clone();
    let kernel = KernelHandle::spawn(move || {
        Core::open(
            &opened,
            &project,
            &KernelOptions {
                workspace_root: Some(opened.clone()),
                ..KernelOptions::default()
            },
            Box::new(SystemEnv),
        )
    })
    .expect("the kernel opens");
    (dir, kernel)
}

fn fixture_identity() -> Identity {
    Identity {
        project_id: "pntfy".into(),
        name: "Ntfy Test".into(),
        credential: format!("operator-{}", "n".repeat(40)),
    }
}

struct World {
    dir: common::PrivateDir,
    deps: Deps,
    server: Option<DaemonServer>,
    socket: PathBuf,
    owner: String,
    agent: String,
    logs: Arc<Mutex<Vec<LogEntry>>>,
}

fn register_agent(state: &Path, directory: &Path, identity: &Identity) -> String {
    let core = Core::open(
        state,
        &identity.initial_project(),
        &KernelOptions {
            workspace_root: Some(directory.to_path_buf()),
            ..KernelOptions::default()
        },
        Box::new(SystemEnv),
    )
    .unwrap();
    let owner = identity.credential.as_str();
    let step = |op: &str, args: Value| -> Value {
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
            .unwrap_or_else(|e| panic!("{op}: {e}"))
    };
    step(
        "syncRoleDefinitions",
        json!([[
            {"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)},
            {"name": "pm", "kind": "PM", "host": "claude", "configHash": "b".repeat(64)},
        ]]),
    );
    step(
        "createSeat",
        json!([{"seatId": "seat-dev-1", "name": "seat-dev-1", "role": "Developer"}]),
    );
    let actor = step(
        "createActor",
        json!([{"displayName": "dev-1", "role": "Developer", "seatId": "seat-dev-1"}]),
    );
    step(
        "registerAgent",
        json!([{"agentId": "dev-1", "roleName": "developer", "seatId": "seat-dev-1", "actorId": actor["actorId"]}]),
    );
    let credential = actor["credential"].as_str().unwrap().to_string();
    core.close();
    credential
}

const BASE_TOML: &str = "schema_version = 1\n\n[hosts.claude]\nkind = \"claude\"\n\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n\n[roles.developer]\nkind = \"Developer\"\nhost = \"claude\"\n\n[review]\nwarm_reviewer = false\n";

impl World {
    fn start(
        ntfy_toml: &str,
        transport: Option<Arc<dyn NtfyTransport>>,
    ) -> (World, Arc<RustLoops>) {
        let dir = private_tempdir();
        let identity = fixture_identity();
        let state = write_project(dir.path(), &identity);
        let agent = register_agent(&state, dir.path(), &identity);
        let file = dir.path().join("capstan.toml");
        std::fs::write(&file, BASE_TOML).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        // `[ntfy]` connection keys are honoured only from the untracked local file.
        let local = dir.path().join("capstan.local.toml");
        std::fs::write(&local, ntfy_toml).unwrap();
        std::fs::set_permissions(&local, std::fs::Permissions::from_mode(0o600)).unwrap();
        let trust = common::kept_trust_store();
        common::trusted_project(dir.path(), &trust);
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
                    Box::new(SystemEnv),
                )
            })
            .expect("the daemon's kernel opens")
        };
        let logs: Arc<Mutex<Vec<LogEntry>>> = Arc::default();
        let sink: Logger = {
            let logs = Arc::clone(&logs);
            Arc::new(move |entry: &LogEntry| lock(&logs).push(entry.clone()))
        };
        let mut options = DaemonOptions::new(
            state.clone(),
            identity.initial_project(),
            dir.path().to_path_buf(),
        );
        options.trust_store = trust.clone();
        options.capstan = Some(Arc::new(
            capstan_config::load_role_config_with(dir.path(), &trust).unwrap(),
        ));
        let deps = Deps::new(kernel, options, sink);
        let loops = RustLoops::new();
        if let Some(transport) = transport {
            loops.set_ntfy_transport(transport);
        }
        loops.start_ticks(&deps);
        let socket = state.join("control.sock");
        let server = DaemonServer::start(deps.clone(), ServerOptions::new(socket.clone())).unwrap();
        (
            World {
                dir,
                deps,
                server: Some(server),
                socket,
                owner: identity.credential,
                agent,
                logs,
            },
            loops,
        )
    }

    fn call(&self, credential: &str, command: &str, args: &[&str]) -> Value {
        let mut stream = UnixStream::connect(&self.socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        let frame = json!({"v": 1, "credential": credential, "command": command, "args": args});
        stream.write_all(format!("{frame}\n").as_bytes()).unwrap();
        let mut text = String::new();
        stream.read_to_string(&mut text).unwrap();
        serde_json::from_str(text.trim()).unwrap()
    }

    fn operator(&self, command: &str, args: &[&str]) -> Value {
        self.call(&self.owner.clone(), command, args)
    }
}

impl Drop for World {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            let _ = server.close();
        }
        self.deps.kernel.close();
    }
}

const NTFY_ON: &str = "\n[ntfy]\nenabled = true\nserver = \"http://127.0.0.1:9\"\ntopic = \"alerts\"\ntoken_env = \"NTFY_TOKEN\"\n";
const NTFY_OPEN: &str =
    "\n[ntfy]\nenabled = true\nserver = \"http://127.0.0.1:9\"\ntopic = \"alerts\"\n";

#[test]
fn with_ntfy_off_nothing_runs_and_the_status_has_no_member() {
    for toml in ["", "\n[ntfy]\nenabled = false\n"] {
        let (world, loops) = World::start(toml, Some(Arc::new(Panicking)));
        assert!(world.deps.ntfy().is_none());
        let status = world.operator("status", &[]);
        assert_eq!(status["ok"], true, "{status}");
        assert!(status["result"].get("ntfy").is_none());
        let answer = world.operator("notify", &["test"]);
        assert_eq!(
            (answer["ok"].clone(), answer["code"].clone()),
            (json!(false), json!("not_configured"))
        );
        loops.stop_ticks();
    }
}

#[test]
fn notify_answers_ok_failed_and_secret_missing_as_recorded() {
    let fixture: Value = serde_json::from_str(
        &std::fs::read_to_string(
            common::crate_dir().join("tests/fixtures/ntfy_notify_answers.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let fake = Arc::new(Fake::default());
    let (world, loops) = World::start(NTFY_OPEN, Some(fake.clone() as Arc<dyn NtfyTransport>));
    let answer = world.operator("notify", &["test"]);
    assert_eq!(answer["result"], fixture["ok"]);
    fake.push(Err(TransportError {
        error: "HTTP 401 unauthorized".into(),
        ..failure("unauthorized", Some(401))
    }));
    let answer = world.operator("notify", &["test"]);
    assert_eq!(answer["result"], fixture["failed"]);
    let status = world.operator("status", &[]);
    let ntfy = &status["result"]["ntfy"];
    assert_eq!(
        (ntfy["sent"].clone(), ntfy["failed"].clone()),
        (json!(1), json!(1))
    );
    assert!(!ntfy["lastSentAt"].is_null() && ntfy["lastError"]["httpStatus"] == 401);
    loops.stop_ticks();
    drop(world);

    let (world, loops) = World::start(
        NTFY_ON,
        Some(Arc::new(Fake::default()) as Arc<dyn NtfyTransport>),
    );
    // NTFY_TOKEN is not set in the test process.
    assert!(std::env::var("NTFY_TOKEN").is_err());
    let answer = world.operator("notify", &["test"]);
    assert_eq!(answer["result"], fixture["secret_missing"]);
    loops.stop_ticks();
}

#[test]
fn notify_is_for_the_operator_and_takes_only_test() {
    let (world, loops) = World::start(
        NTFY_OPEN,
        Some(Arc::new(Fake::default()) as Arc<dyn NtfyTransport>),
    );
    let answer = world.call(&world.agent.clone(), "notify", &["test"]);
    assert_eq!(
        (answer["ok"].clone(), answer["code"].clone()),
        (json!(false), json!("forbidden"))
    );
    let answer = world.operator("notify", &["bogus"]);
    assert_eq!(answer["code"], "invalid_request");
    loops.stop_ticks();
}

#[test]
fn a_second_test_is_busy_while_status_still_answers_and_a_slow_one_is_ok() {
    let fake = Arc::new(Fake::default());
    fake.close_gate();
    let (world, loops) = World::start(NTFY_OPEN, Some(fake.clone() as Arc<dyn NtfyTransport>));
    let world = Arc::new(world);
    let first = {
        let world = Arc::clone(&world);
        std::thread::spawn(move || world.operator("notify", &["test"]))
    };
    wait_for("the test to start", || fake.count() == 1);
    let busy = world.operator("notify", &["test"]);
    assert_eq!(busy["code"], "busy");
    assert_eq!(world.operator("status", &[])["ok"], true);
    // Six seconds on a real socket: the answer comes back ok, past the 5 s a request frame may take.
    std::thread::sleep(Duration::from_secs(6));
    fake.open_gate();
    let answer = first.join().unwrap();
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["result"]["ok"], true);
    loops.stop_ticks();
}

#[test]
fn the_notify_limit_covers_the_longest_curl_run() {
    let (world, loops) = World::start(NTFY_OPEN, None);
    let identity = capstan_kernel::types::Identity {
        actor_id: "actor".into(),
        role: "operator".into(),
        capabilities: vec![],
        agent: None,
    };
    assert_eq!(
        command_limit_ms(&world.deps, "notify", &identity),
        Some(70_000)
    );
    assert!(std::hint::black_box(70_000u64) > (60 + 2) * 1000);
    loops.stop_ticks();
}

#[test]
fn the_loops_stop_within_the_drain_flush_with_a_curl_in_flight() {
    let dir = private_tempdir();
    let pid_file = dir.path().join("pid");
    let program = script(
        dir.path(),
        "slow-curl",
        &format!("echo $$ > {}\nsleep 60", pid_file.display()),
    );
    let ntfy = {
        let mut n = base_ntfy();
        n.server = "http://127.0.0.1:9".into();
        n
    };
    let (world, loops) = World::start(NTFY_OPEN, Some(Arc::new(curl_for(&ntfy, &program))));
    let world = Arc::new(world);
    let asker = {
        let world = Arc::clone(&world);
        std::thread::spawn(move || world.operator("notify", &["test"]))
    };
    let pid = read_pid(&pid_file);
    let started = Instant::now();
    loops.stop_ticks();
    let took = started.elapsed();
    assert!(
        took < Duration::from_millis(DRAIN_FLUSH_MS),
        "the loops took {took:?} to stop"
    );
    wait_for("the child to be gone", || process_gone(pid));
    let answer = asker.join().unwrap();
    assert_eq!(answer["result"]["ok"], false);
    let _ = (&world.dir, &world.logs);
}
