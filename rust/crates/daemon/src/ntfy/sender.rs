//! The queue and the worker: two lanes, coalescing, caps, retries with a not-before time, the status object and its file.
//! Nothing here touches the kernel; the tail thread feeds [`SenderHandle::enqueue`] and returns.

use super::message;
use super::redact;
use super::transport::{retriable, NtfyTransport, PublishRequest, TransportError};
use crate::loops::driver::DriverLog;
use crate::loops::notifier::Clock;
use crate::loops::StopFlag;
use capstan_config::{read_secret, Ntfy, NtfyAuth, NtfyEvent, SecretError};
use capstan_kernel::areas::push_events::PushEvent;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How many messages one lane holds.
pub const LANE_SIZE: usize = 32;
/// Messages with the same event and subject inside this window count once.
pub const COALESCE_MS: i64 = 60_000;
const CAP_WINDOW_MS: i64 = 600_000;
const HIGH_CAP: usize = 30;
const LOW_CAP: usize = 20;
/// The waits before attempts 2 and on, in seconds; the last one repeats.
const BACKOFF_SECONDS: [i64; 5] = [2, 8, 30, 30, 30];
const WORKER_POLL: Duration = Duration::from_millis(100);
const STATUS_INTERVAL: Duration = Duration::from_secs(1);
/// The file the status object is written to, in the state directory.
pub const STATUS_FILE: &str = "ntfy-status.json";

fn is_high(event: NtfyEvent) -> bool {
    matches!(
        event,
        NtfyEvent::UserQuestion
            | NtfyEvent::OperatorProposal
            | NtfyEvent::PromptWaiting
            | NtfyEvent::DesignWaiting
            | NtfyEvent::PmStuck
    )
}

/// Reads one variable of the daemon's environment.
pub type EnvFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

pub struct SenderOptions {
    pub ntfy: Ntfy,
    pub prefix: String,
    pub transport: Arc<dyn NtfyTransport>,
    pub clock: Clock,
    pub stop: StopFlag,
    pub log: DriverLog,
    /// Where the status file goes; None writes none.
    pub status_path: Option<PathBuf>,
    pub env: EnvFn,
}

#[derive(Clone)]
struct Item {
    event: NtfyEvent,
    request: PublishRequest,
    attempts: u32,
    not_before: i64,
}

#[derive(Clone)]
struct LastError {
    at: i64,
    reason: String,
    http_status: Option<u16>,
}

#[derive(Default)]
struct State {
    high: VecDeque<Item>,
    low: VecDeque<Item>,
    high_times: VecDeque<i64>,
    low_times: VecDeque<i64>,
    recent: HashMap<(NtfyEvent, String), i64>,
    sent: u64,
    failed: u64,
    dropped: u64,
    last_sent_at: Option<i64>,
    last_error: Option<LastError>,
}

#[derive(Default)]
struct StatusFile {
    last_write: Option<Instant>,
    failing: bool,
}

/// The answer of the test notification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestAnswer {
    pub ok: bool,
    pub http_status: Option<u16>,
    pub reason: Option<String>,
    pub error: Option<String>,
    pub secret_source: Option<String>,
}

impl TestAnswer {
    pub fn to_value(&self) -> Value {
        json!({
            "ok": self.ok,
            "httpStatus": self.http_status,
            "reason": self.reason,
            "error": self.error,
            "secretSource": self.secret_source,
        })
    }
}

/// One attempt's end.
enum Attempt {
    Sent,
    Stopped,
    Failed(Failure),
}

struct Failure {
    reason: String,
    http_status: Option<u16>,
    error: Option<String>,
    retry_after: Option<u64>,
    secret_source: Option<String>,
}

/// A test notification is already running.
#[derive(Debug)]
pub struct TestBusy;

struct BusyGuard<'a>(&'a AtomicBool);

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// The running sender: what `status` and `notify` use.
pub struct SenderHandle {
    ntfy: Ntfy,
    prefix: String,
    transport: Arc<dyn NtfyTransport>,
    clock: Clock,
    stop: StopFlag,
    log: DriverLog,
    status_path: Option<PathBuf>,
    env: EnvFn,
    state: Mutex<State>,
    wake: Condvar,
    dirty: AtomicBool,
    file: Mutex<StatusFile>,
    test_running: AtomicBool,
    test_counter: Mutex<u64>,
}

impl SenderHandle {
    pub fn new(options: SenderOptions) -> Arc<Self> {
        Arc::new(Self {
            ntfy: options.ntfy,
            prefix: options.prefix,
            transport: options.transport,
            clock: options.clock,
            stop: options.stop,
            log: options.log,
            status_path: options.status_path,
            env: options.env,
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
            dirty: AtomicBool::new(true),
            file: Mutex::new(StatusFile::default()),
            test_running: AtomicBool::new(false),
            test_counter: Mutex::new(0),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn now(&self) -> i64 {
        (self.clock)()
    }

    /// Queues the message of `event` unless the event is not selected, repeats a recent one or exceeds a cap. Never blocks
    /// on the network.
    pub fn enqueue(&self, event: &PushEvent) {
        if !self.ntfy.events.contains(&event.event) {
            return;
        }
        let request = message::request(&self.ntfy, &self.prefix, event);
        let now = self.now();
        {
            let mut st = self.state();
            if st.recent.len() > 256 {
                st.recent.retain(|_, at| now - *at < COALESCE_MS);
            }
            let key = (event.event, event.subject.clone());
            if st
                .recent
                .get(&key)
                .is_some_and(|at| now - *at < COALESCE_MS)
            {
                st.dropped += 1;
                self.dirty.store(true, Ordering::SeqCst);
                return;
            }
            let high = is_high(event.event);
            let (times, cap) = if high {
                (&mut st.high_times, HIGH_CAP)
            } else {
                (&mut st.low_times, LOW_CAP)
            };
            while times.front().is_some_and(|at| now - *at >= CAP_WINDOW_MS) {
                times.pop_front();
            }
            if times.len() >= cap {
                st.dropped += 1;
                self.dirty.store(true, Ordering::SeqCst);
                return;
            }
            times.push_back(now);
            st.recent.insert(key, now);
            Self::push(
                &mut st,
                high,
                Item {
                    event: event.event,
                    request,
                    attempts: 0,
                    not_before: now,
                },
            );
        }
        self.dirty.store(true, Ordering::SeqCst);
        self.wake.notify_all();
    }

    fn push(st: &mut State, high: bool, item: Item) {
        let lane = if high { &mut st.high } else { &mut st.low };
        lane.push_back(item);
        if lane.len() > LANE_SIZE {
            lane.pop_front();
            st.dropped += 1;
        }
    }

    /// The `ntfy` member of `status`.
    pub fn status(&self) -> Value {
        let st = self.state();
        json!({
            "enabled": true,
            "server": super::transport::host_of(&self.ntfy.server),
            "events": self.ntfy.events.iter().map(|e| e.as_str()).collect::<Vec<_>>(),
            "sent": st.sent,
            "failed": st.failed,
            "dropped": st.dropped,
            "queued": st.high.len() + st.low.len(),
            "lastSentAt": st.last_sent_at.map(capstan_ledger::iso_from_millis),
            "lastError": st.last_error.as_ref().map(|e| json!({
                "at": capstan_ledger::iso_from_millis(e.at),
                "reason": e.reason,
                "httpStatus": e.http_status,
            })),
        })
    }

    fn secret_masks(&self, secret: Option<&capstan_config::Secret>) -> Vec<String> {
        let mut masks = vec![self.ntfy.server.clone()];
        if let Some(secret) = secret {
            masks.push(secret.expose().to_string());
        }
        masks
    }

    fn secret_source(&self) -> Option<String> {
        match &self.ntfy.auth {
            NtfyAuth::None => None,
            NtfyAuth::TokenEnv(name)
            | NtfyAuth::BasicEnv {
                password_env: name, ..
            } => Some(name.clone()),
            NtfyAuth::TokenFile(path)
            | NtfyAuth::BasicFile {
                password_file: path,
                ..
            } => Some(path.to_string_lossy().into_owned()),
        }
    }

    /// One attempt: reads the secret anew, then publishes.
    fn attempt(&self, request: &PublishRequest) -> Attempt {
        let secret = match read_secret(&self.ntfy.auth, &*self.env) {
            Ok(secret) => secret,
            Err(error) => {
                let reason = match error {
                    SecretError::MissingEnv(_) | SecretError::MissingFile(_) => "secret_missing",
                    _ => "secret_unreadable",
                };
                return Attempt::Failed(Failure {
                    reason: reason.to_string(),
                    http_status: None,
                    error: None,
                    retry_after: None,
                    secret_source: self.secret_source(),
                });
            }
        };
        let timeout = Duration::from_secs(u64::from(self.ntfy.timeout_seconds));
        match self
            .transport
            .publish(request, secret.as_ref(), timeout, &self.stop)
        {
            Ok(_) => Attempt::Sent,
            Err(TransportError { reason, .. }) if reason == "stopped" => Attempt::Stopped,
            Err(error) => {
                let masks = self.secret_masks(secret.as_ref());
                Attempt::Failed(Failure {
                    error: Some(redact::scrub(&error.error, &masks)),
                    reason: error.reason,
                    http_status: error.http_status,
                    retry_after: error.retry_after,
                    secret_source: None,
                })
            }
        }
    }

    fn record_sent(&self, event: NtfyEvent) {
        let now = self.now();
        {
            let mut st = self.state();
            st.sent += 1;
            st.last_sent_at = Some(now);
        }
        self.dirty.store(true, Ordering::SeqCst);
        (self.log)("ntfy:sent", json!({"event": event.as_str()}));
    }

    fn record_failed(&self, failure: &Failure) {
        let now = self.now();
        {
            let mut st = self.state();
            st.failed += 1;
            st.last_error = Some(LastError {
                at: now,
                reason: failure.reason.clone(),
                http_status: failure.http_status,
            });
        }
        self.dirty.store(true, Ordering::SeqCst);
    }

    fn log_failed(&self, event: NtfyEvent, failure: &Failure, attempt: u32) {
        (self.log)(
            "ntfy:send_failed",
            json!({
                "event": event.as_str(),
                "reason": failure.reason,
                "httpStatus": failure.http_status,
                "attempt": attempt,
            }),
        );
    }

    /// The test notification: one attempt, no queue, coalescing or caps, a fresh subject. Counted like a send.
    pub fn send_test(&self) -> Result<TestAnswer, TestBusy> {
        if self.test_running.swap(true, Ordering::SeqCst) {
            return Err(TestBusy);
        }
        let _guard = BusyGuard(&self.test_running);
        let subject = {
            let mut counter = self.test_counter.lock().unwrap_or_else(|p| p.into_inner());
            *counter += 1;
            format!("test-{}-{}", self.now(), *counter)
        };
        let event = PushEvent {
            seq: 0,
            ts: String::new(),
            event: NtfyEvent::Test,
            subject,
            agent_id: None,
            plan_id: None,
            detail: None,
        };
        let request = message::request(&self.ntfy, &self.prefix, &event);
        Ok(match self.attempt(&request) {
            Attempt::Sent => {
                self.record_sent(NtfyEvent::Test);
                TestAnswer {
                    ok: true,
                    http_status: Some(200),
                    reason: None,
                    error: None,
                    secret_source: None,
                }
            }
            Attempt::Stopped => TestAnswer {
                ok: false,
                http_status: None,
                reason: Some("stopped".into()),
                error: Some("the controller is stopping".into()),
                secret_source: None,
            },
            Attempt::Failed(failure) => {
                self.log_failed(NtfyEvent::Test, &failure, 1);
                self.record_failed(&failure);
                TestAnswer {
                    ok: false,
                    http_status: failure.http_status,
                    reason: Some(failure.reason),
                    error: failure.error,
                    secret_source: failure.secret_source,
                }
            }
        })
    }

    /// The first ready item, high lane first.
    fn take_ready(st: &mut State, now: i64) -> Option<(Item, bool)> {
        for (lane, high) in [(&mut st.high, true), (&mut st.low, false)] {
            if let Some(index) = lane.iter().position(|item| item.not_before <= now) {
                return lane.remove(index).map(|item| (item, high));
            }
        }
        None
    }

    /// The worker's body: runs until the stop flag is set, then writes the status once more.
    pub fn run_worker(&self) {
        while !self.stop.stopped() {
            self.write_status_if_due(false);
            let taken = {
                let mut st = self.state();
                let now = self.now();
                match Self::take_ready(&mut st, now) {
                    Some(taken) => Some(taken),
                    None => {
                        let _ = self.wake.wait_timeout(st, WORKER_POLL);
                        None
                    }
                }
            };
            let Some((mut item, high)) = taken else {
                continue;
            };
            let attempts_max = self.ntfy.retries + 1;
            item.attempts += 1;
            match self.attempt(&item.request) {
                Attempt::Sent => self.record_sent(item.event),
                Attempt::Stopped => break,
                Attempt::Failed(failure) => {
                    self.log_failed(item.event, &failure, item.attempts);
                    if retriable(&failure.reason) && item.attempts < attempts_max {
                        let index = (item.attempts as usize - 1).min(BACKOFF_SECONDS.len() - 1);
                        let seconds = failure
                            .retry_after
                            .map_or(BACKOFF_SECONDS[index], |s| s as i64);
                        item.not_before = self.now() + seconds * 1000;
                        let mut st = self.state();
                        Self::push(&mut st, high, item);
                        self.dirty.store(true, Ordering::SeqCst);
                    } else {
                        self.record_failed(&failure);
                    }
                }
            }
        }
        self.write_status_if_due(true);
    }

    /// Writes the status file when something changed and a second has passed (or `force`). A failure is logged once per streak
    /// and changes no counter.
    fn write_status_if_due(&self, force: bool) {
        let Some(path) = &self.status_path else {
            return;
        };
        let mut file = self.file.lock().unwrap_or_else(|p| p.into_inner());
        if !force {
            if !self.dirty.load(Ordering::SeqCst) {
                return;
            }
            if file
                .last_write
                .is_some_and(|at| at.elapsed() < STATUS_INTERVAL)
            {
                return;
            }
        }
        file.last_write = Some(Instant::now());
        self.dirty.store(false, Ordering::SeqCst);
        let mut value = self.status();
        if let Some(map) = value.as_object_mut() {
            map.insert(
                "writtenAt".into(),
                json!(capstan_ledger::iso_from_millis(self.now())),
            );
            let pid = std::process::id();
            map.insert("pid".into(), json!(pid));
            map.insert(
                "pidStartTime".into(),
                json!(capstan_process::start_time(i64::from(pid))),
            );
        }
        match write_private(path, value.to_string().as_bytes()) {
            Ok(()) => file.failing = false,
            Err(error) => {
                if !file.failing {
                    (self.log)(
                        "ntfy:status_write_failed",
                        json!({"error": redact::clip(&error.kind().to_string())}),
                    );
                }
                file.failing = true;
            }
        }
    }
}

/// Writes `bytes` to `path` through a temporary file in the same directory (mode 0600) and a rename.
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    let written = file.write_all(bytes);
    drop(file);
    if let Err(error) = written.and_then(|()| std::fs::rename(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    Ok(())
}
