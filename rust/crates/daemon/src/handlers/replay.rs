//! The reply cache: a client that lost the reply to a request retries it with the same request id (`rid`) and gets the
//! answer the first attempt produced, never a second execution; and a `send` that repeats an earlier one of the same
//! caller within `[daemon] repeat_window_seconds` is folded into it. In memory only: a restart forgets everything.
//!
//! One `Mutex` guards both maps. Every lock recovers from poisoning, as the rest of the request path does.
//!
//! * `by_rid`: (caller, rid) -> the fingerprint of command and arguments, and the state of the first attempt:
//!   `InFlight`, `Done` (the answer text), `Kept` (completed, the answer is not kept) or `Aborted` (the handler panicked).
//! * `by_send`: (caller, hash of every send argument) -> the answer of a send that ended ok, for the repeat window.
//!
//! The caller is the SHA-256 of the credential, so two credentials never share an entry.

use super::shared::{fail, AbortSignal, CommandResponse, ErrorCode};
use capstan_wire::js;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// An answer over this many bytes is not stored; the entry becomes `Kept`.
pub const MAX_ANSWER_BYTES: usize = 64 * 1024;
/// The texts of all stored answers together.
pub const MAX_TOTAL_BYTES: usize = 16 * 1024 * 1024;
/// The entries of one map.
pub const MAX_ENTRIES: usize = 4096;
/// The entries one caller may hold in one map; its oldest goes first.
pub const MAX_PER_CALLER: usize = 256;
/// How long a rid entry lives.
pub const RID_TTL_MS: i64 = 600_000;
/// The longest a waiter waits for a first attempt that is still running.
pub const MAX_WAIT_MS: u64 = 60_000;
/// How long a waiter waits for a command that has no time limit.
pub const DEFAULT_WAIT_MS: u64 = 30_000;
/// The longest rid.
pub const MAX_RID_LEN: usize = 64;

pub const RUNNING_MESSAGE: &str =
    "the first attempt is still running; check before running it again";
pub const FAILED_MESSAGE: &str =
    "the first attempt failed inside the daemon; check before running it again";
pub const NOT_KEPT_MESSAGE: &str =
    "the first attempt completed; its answer is not kept: check with cstan status";
pub const REUSED_MESSAGE: &str = "request id reused for a different request";

type Caller = [u8; 32];

fn sha(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// The caller key of a credential.
pub fn caller_of(credential: &str) -> Caller {
    sha(&[credential.as_bytes()])
}

/// The fingerprint of a request: its command and every argument.
pub fn fingerprint(command: &str, args: &[String]) -> [u8; 32] {
    let mut parts: Vec<&[u8]> = vec![command.as_bytes()];
    parts.extend(args.iter().map(|arg| arg.as_bytes()));
    sha(&parts)
}

/// The `rid` of a frame: 1..=64 characters of `[A-Za-z0-9-]`; anything else (also a member of another type) is ignored.
pub fn parse_rid(value: Option<&js::Value>) -> Option<String> {
    let js::Value::String(text) = value? else {
        return None;
    };
    let text = text.to_utf8_lossy();
    let valid = !text.is_empty()
        && text.len() <= MAX_RID_LEN
        && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    valid.then_some(text)
}

/// Whether an answer must not be stored, so that a retry executes: the daemon was busy or shutting down, or the kernel
/// failed under the request (`kernel_failed`, from `deps::take_kernel_failure`, never decided by the answer's text).
/// A handler panic is not stored either: it marks the rid `Aborted`.
pub fn is_unstored(response: &CommandResponse, kernel_failed: bool) -> bool {
    kernel_failed
        || matches!(
            response,
            CommandResponse::Err {
                code: ErrorCode::Busy | ErrorCode::ShuttingDown,
                ..
            }
        )
}

enum Slot {
    InFlight,
    Done { text: String, at: i64 },
    Kept { at: i64 },
    Aborted { at: i64 },
}

struct RidEntry {
    seq: u64,
    fingerprint: [u8; 32],
    slot: Slot,
    started: i64,
}

impl RidEntry {
    fn bytes(&self) -> usize {
        match &self.slot {
            Slot::Done { text, .. } => text.len(),
            _ => 0,
        }
    }
}

struct SendEntry {
    seq: u64,
    at: i64,
    recipient: String,
    result: Value,
}

struct Inner {
    by_rid: HashMap<(Caller, String), RidEntry>,
    by_send: HashMap<(Caller, [u8; 32]), SendEntry>,
    rid_per_caller: HashMap<Caller, usize>,
    send_per_caller: HashMap<Caller, usize>,
    bytes: usize,
    seq: u64,
}

impl Inner {
    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    fn drop_rid(&mut self, key: &(Caller, String)) {
        if let Some(entry) = self.by_rid.remove(key) {
            self.bytes -= entry.bytes();
            if let Some(count) = self.rid_per_caller.get_mut(&key.0) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.rid_per_caller.remove(&key.0);
                }
            }
        }
    }

    fn drop_send(&mut self, key: &(Caller, [u8; 32])) {
        if self.by_send.remove(key).is_some() {
            if let Some(count) = self.send_per_caller.get_mut(&key.0) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.send_per_caller.remove(&key.0);
                }
            }
        }
    }

    /// The oldest entry that may be evicted: never one that is still running (`InFlight`); with `done_only`, only one
    /// that holds a text.
    fn oldest_rid(&self, caller: Option<&Caller>, done_only: bool) -> Option<(Caller, String)> {
        self.by_rid
            .iter()
            .filter(|(key, entry)| {
                caller.is_none_or(|caller| &key.0 == caller)
                    && !matches!(entry.slot, Slot::InFlight)
                    && (!done_only || matches!(entry.slot, Slot::Done { .. }))
            })
            .min_by_key(|(_, entry)| entry.seq)
            .map(|(key, _)| key.clone())
    }

    fn oldest_send(&self, caller: Option<&Caller>) -> Option<(Caller, [u8; 32])> {
        self.by_send
            .iter()
            .filter(|(key, _)| caller.is_none_or(|caller| &key.0 == caller))
            .min_by_key(|(_, entry)| entry.seq)
            .map(|(key, _)| *key)
    }
}

/// The limits of a cache; the defaults are the constants of this module.
#[derive(Clone, Copy)]
struct Limits {
    bytes: usize,
    entries: usize,
    per_caller: usize,
    wait_override: Option<Duration>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            bytes: MAX_TOTAL_BYTES,
            entries: MAX_ENTRIES,
            per_caller: MAX_PER_CALLER,
            wait_override: None,
        }
    }
}

pub(crate) type GateState = Arc<(Mutex<bool>, Condvar)>;

type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// A fault a test arms to reach the paths a healthy handler does not take.
#[derive(Default)]
struct Seams {
    panic_next: bool,
    kernel_panic_next: bool,
    hold: Option<GateState>,
}

pub struct ReplyCache {
    inner: Mutex<Inner>,
    changed: Condvar,
    clock: Clock,
    limits: Limits,
    seams: Mutex<Seams>,
}

/// What `begin` decided for a request.
pub enum Begin<'a> {
    /// Execute it; `guard` (when the request has a rid) records the answer.
    Run(Option<Flight<'a>>),
    /// The caps are full of running attempts: execute without an entry (its retry runs uncached too).
    Uncached,
    /// Answer with this without executing; `Some(age_ms)` marks a stored answer, which the log records.
    Answer(Reply, Option<i64>),
    /// The client is gone or the daemon is draining while waiting.
    Gone(Option<CommandResponse>),
}

/// An answer without execution: a stored response line or a fresh response.
pub enum Reply {
    Line(String),
    Response(CommandResponse),
}

/// The first attempt of a rid while it runs. Dropping it without `finish` marks the rid `Aborted` and wakes every
/// waiter.
pub struct Flight<'a> {
    cache: &'a ReplyCache,
    key: (Caller, String),
    seq: u64,
    finished: bool,
}

/// How a first attempt ended.
pub enum Ended {
    /// Store the answer text.
    Text(String),
    /// Completed, but the answer is not kept.
    Kept,
    /// Not stored: the retry executes.
    Forget,
    /// The handler panicked.
    Aborted,
}

impl Default for ReplyCache {
    fn default() -> Self {
        Self::new()
    }
}

fn system_clock() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

impl ReplyCache {
    /// A cache with the default limits and the system clock.
    pub fn new() -> Self {
        Self::build(Arc::new(system_clock), Limits::default())
    }

    #[doc(hidden)]
    pub fn with_clock(clock: impl Fn() -> i64 + Send + Sync + 'static) -> Self {
        Self::build(Arc::new(clock), Limits::default())
    }

    /// For tests: small limits and the wait a waiter gives a running first attempt.
    #[doc(hidden)]
    pub fn with_limits(bytes: usize, entries: usize, per_caller: usize, waits: Duration) -> Self {
        Self::build(
            Arc::new(system_clock),
            Limits {
                bytes,
                entries,
                per_caller,
                wait_override: Some(waits),
            },
        )
    }

    /// `with_clock` and `with_limits` together.
    #[doc(hidden)]
    pub fn with_clock_and_limits(
        clock: impl Fn() -> i64 + Send + Sync + 'static,
        bytes: usize,
        entries: usize,
        per_caller: usize,
        waits: Duration,
    ) -> Self {
        Self::build(
            Arc::new(clock),
            Limits {
                bytes,
                entries,
                per_caller,
                wait_override: Some(waits),
            },
        )
    }

    fn build(clock: Clock, limits: Limits) -> Self {
        Self {
            inner: Mutex::new(Inner {
                by_rid: HashMap::new(),
                by_send: HashMap::new(),
                rid_per_caller: HashMap::new(),
                send_per_caller: HashMap::new(),
                bytes: 0,
                seq: 0,
            }),
            changed: Condvar::new(),
            clock,
            limits,
            seams: Mutex::new(Seams::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn now(&self) -> i64 {
        (self.clock)()
    }

    /// The entries of both maps (tests).
    #[doc(hidden)]
    pub fn len(&self) -> (usize, usize) {
        let inner = self.lock();
        (inner.by_rid.len(), inner.by_send.len())
    }

    #[doc(hidden)]
    pub fn is_empty(&self) -> bool {
        self.len() == (0, 0)
    }

    /// The bytes of all stored texts (tests).
    #[doc(hidden)]
    pub fn stored_bytes(&self) -> usize {
        self.lock().bytes
    }

    /// Whether a rid of `credential` is stored (tests).
    #[doc(hidden)]
    pub fn has_rid(&self, credential: &str, rid: &str) -> bool {
        self.lock()
            .by_rid
            .contains_key(&(caller_of(credential), rid.to_string()))
    }

    // -------------------------------------------------------------------------------------------- the test seams

    /// The next dispatch panics in the handler.
    #[doc(hidden)]
    pub fn panic_next_dispatch(&self) {
        self.seams
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .panic_next = true;
    }

    /// The next dispatch runs a kernel job that panics, then answers as a closed kernel does.
    #[doc(hidden)]
    pub fn kernel_panic_next_dispatch(&self) {
        self.seams
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .kernel_panic_next = true;
    }

    /// The next dispatch blocks before its handler until `release` is called on the returned gate.
    #[doc(hidden)]
    pub fn hold_next_dispatch(&self) -> Gate {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        self.seams.lock().unwrap_or_else(|p| p.into_inner()).hold = Some(Arc::clone(&gate));
        Gate(gate)
    }

    /// True once a dispatch has taken the gate `hold_next_dispatch` armed.
    #[doc(hidden)]
    pub fn hold_taken(&self) -> bool {
        self.seams
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .hold
            .is_none()
    }

    /// What a test armed for the dispatch that starts now.
    pub(crate) fn take_seams(&self) -> (bool, bool, Option<GateState>) {
        let mut seams = self.seams.lock().unwrap_or_else(|p| p.into_inner());
        (
            std::mem::take(&mut seams.panic_next),
            std::mem::take(&mut seams.kernel_panic_next),
            seams.hold.take(),
        )
    }

    // -------------------------------------------------------------------------------------------- lookups

    fn wait_for(&self, limit_ms: Option<u64>) -> Duration {
        self.limits.wait_override.unwrap_or_else(|| {
            Duration::from_millis(limit_ms.map_or(DEFAULT_WAIT_MS, |limit| limit.min(MAX_WAIT_MS)))
        })
    }

    /// The first step of a request with a `rid`: the stored answer, a wait for the running first attempt, or the right
    /// to execute.
    pub fn begin(
        &self,
        caller: Caller,
        rid: &str,
        print: [u8; 32],
        limit_ms: Option<u64>,
        signal: &AbortSignal,
        on_wait: &mut dyn FnMut(Duration),
    ) -> Begin<'_> {
        let key = (caller, rid.to_string());
        let deadline = Instant::now() + self.wait_for(limit_ms);
        let mut announced = false;
        let mut inner = self.lock();
        loop {
            let now = self.now();
            let expired = inner.by_rid.get(&key).is_some_and(|entry| {
                let at = match &entry.slot {
                    Slot::InFlight => return false,
                    Slot::Done { at, .. } | Slot::Kept { at } | Slot::Aborted { at } => *at,
                };
                now < at || now - at >= RID_TTL_MS
            });
            if expired {
                inner.drop_rid(&key);
            }
            let Some(entry) = inner.by_rid.get(&key) else {
                let Some(seq) = self.insert_rid(&mut inner, &key, print, now) else {
                    // Every entry that could make room is still running: run uncached rather than drop one.
                    return Begin::Uncached;
                };
                return Begin::Run(Some(Flight {
                    cache: self,
                    key,
                    seq,
                    finished: false,
                }));
            };
            if entry.fingerprint != print {
                return Begin::Answer(
                    Reply::Response(fail(ErrorCode::InvalidRequest, REUSED_MESSAGE)),
                    None,
                );
            }
            match &entry.slot {
                Slot::Done { text, at } => {
                    return Begin::Answer(Reply::Line(text.clone()), Some((now - at).max(0)));
                }
                Slot::Kept { .. } => {
                    return Begin::Answer(
                        Reply::Response(fail(ErrorCode::AnswerNotKept, NOT_KEPT_MESSAGE)),
                        None,
                    );
                }
                Slot::Aborted { .. } => {
                    return Begin::Answer(
                        Reply::Response(fail(ErrorCode::AttemptFailed, FAILED_MESSAGE)),
                        None,
                    );
                }
                Slot::InFlight => {}
            }
            // The first attempt is still running: wait for it.
            if !announced {
                announced = true;
                let remaining = deadline.saturating_duration_since(Instant::now());
                drop(inner);
                on_wait(remaining);
                inner = self.lock();
                continue;
            }
            if let Some(reason) = signal.reason() {
                return Begin::Gone(
                    (reason == "shutdown")
                        .then(|| fail(ErrorCode::ShuttingDown, "the daemon is shutting down")),
                );
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Begin::Answer(
                    Reply::Response(fail(ErrorCode::AttemptRunning, RUNNING_MESSAGE)),
                    None,
                );
            }
            let slice = remaining.min(Duration::from_millis(100));
            inner = self
                .changed
                .wait_timeout(inner, slice)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
    }

    fn insert_rid(
        &self,
        inner: &mut Inner,
        key: &(Caller, String),
        print: [u8; 32],
        now: i64,
    ) -> Option<u64> {
        self.purge_expired(inner, now);
        while inner.rid_per_caller.get(&key.0).copied().unwrap_or(0) >= self.limits.per_caller {
            let oldest = inner.oldest_rid(Some(&key.0), false)?;
            inner.drop_rid(&oldest);
        }
        while inner.by_rid.len() >= self.limits.entries {
            let oldest = inner.oldest_rid(None, false)?;
            inner.drop_rid(&oldest);
        }
        let seq = inner.next_seq();
        inner.by_rid.insert(
            key.clone(),
            RidEntry {
                seq,
                fingerprint: print,
                slot: Slot::InFlight,
                started: now,
            },
        );
        *inner.rid_per_caller.entry(key.0).or_insert(0) += 1;
        Some(seq)
    }

    fn purge_expired(&self, inner: &mut Inner, now: i64) {
        let stale: Vec<(Caller, String)> = inner
            .by_rid
            .iter()
            .filter(|(_, entry)| match &entry.slot {
                Slot::InFlight => now >= entry.started && now - entry.started >= RID_TTL_MS * 6,
                Slot::Done { at, .. } | Slot::Kept { at } | Slot::Aborted { at } => {
                    now < *at || now - *at >= RID_TTL_MS
                }
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in stale {
            inner.drop_rid(&key);
        }
    }

    fn finish(&self, key: &(Caller, String), seq: u64, ended: Ended) {
        let now = self.now();
        let mut inner = self.lock();
        let current = inner.by_rid.get(key).is_some_and(|entry| entry.seq == seq);
        if current {
            match ended {
                Ended::Forget => inner.drop_rid(key),
                Ended::Text(text) => {
                    let size = text.len();
                    if let Some(entry) = inner.by_rid.get_mut(key) {
                        entry.slot = Slot::Done { text, at: now };
                    }
                    inner.bytes += size;
                    while inner.bytes > self.limits.bytes {
                        match inner.oldest_rid(None, true) {
                            Some(oldest) => inner.drop_rid(&oldest),
                            None => break,
                        }
                    }
                }
                Ended::Kept => {
                    if let Some(entry) = inner.by_rid.get_mut(key) {
                        entry.slot = Slot::Kept { at: now };
                    }
                }
                Ended::Aborted => {
                    if let Some(entry) = inner.by_rid.get_mut(key) {
                        entry.slot = Slot::Aborted { at: now };
                    }
                }
            }
        }
        drop(inner);
        self.changed.notify_all();
    }

    // -------------------------------------------------------------------------------------------- the send window

    /// The key of a send in `by_send`: every argument, flags included.
    pub fn send_key(caller: Caller, args: &[String]) -> (Caller, [u8; 32]) {
        let parts: Vec<&[u8]> = args.iter().map(|arg| arg.as_bytes()).collect();
        (caller, sha(&parts))
    }

    /// The answer of an earlier identical send younger than `window_seconds`, as a folded answer.
    pub fn fold(&self, key: &(Caller, [u8; 32]), window_seconds: i64) -> Option<CommandResponse> {
        if window_seconds <= 0 {
            return None;
        }
        let now = self.now();
        let mut inner = self.lock();
        let entry = inner.by_send.get(key)?;
        let age = now - entry.at;
        if age < 0 || age >= window_seconds.saturating_mul(1000) {
            inner.drop_send(key);
            return None;
        }
        let mut result = entry.result.clone();
        let seconds = age / 1000;
        let id = result["messageId"].as_str().unwrap_or("?").to_string();
        let recipient = entry.recipient.clone();
        let members = result.as_object_mut()?;
        members.insert("repeat".into(), json!(true));
        members.insert("repeatAgeMs".into(), json!(age));
        members.insert(
            "note".into(),
            json!(format!(
                "not sent again: the same message went to {recipient} {seconds} s ago (message {id})"
            )),
        );
        Some(CommandResponse::Ok(result))
    }

    /// Remembers a send that ended ok.
    pub fn remember_send(
        &self,
        key: (Caller, [u8; 32]),
        recipient: String,
        result: &Value,
        window_seconds: i64,
    ) {
        if window_seconds <= 0 || !result.is_object() {
            return;
        }
        let now = self.now();
        let mut inner = self.lock();
        inner.drop_send(&key);
        while inner.send_per_caller.get(&key.0).copied().unwrap_or(0) >= self.limits.per_caller {
            match inner.oldest_send(Some(&key.0)) {
                Some(oldest) => inner.drop_send(&oldest),
                None => break,
            }
        }
        while inner.by_send.len() >= self.limits.entries {
            match inner.oldest_send(None) {
                Some(oldest) => inner.drop_send(&oldest),
                None => break,
            }
        }
        let seq = inner.next_seq();
        *inner.send_per_caller.entry(key.0).or_insert(0) += 1;
        inner.by_send.insert(
            key,
            SendEntry {
                seq,
                at: now,
                recipient,
                result: result.clone(),
            },
        );
    }
}

impl<'a> Flight<'a> {
    /// Records how the first attempt ended and wakes every waiter.
    pub fn finish(mut self, ended: Ended) {
        self.finished = true;
        self.cache.finish(&self.key, self.seq, ended);
    }
}

impl Drop for Flight<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.cache.finish(&self.key, self.seq, Ended::Aborted);
        }
    }
}

/// The gate `hold_next_dispatch` returns.
#[doc(hidden)]
pub struct Gate(Arc<(Mutex<bool>, Condvar)>);

impl Gate {
    pub fn release(&self) {
        let (open, condvar) = &*self.0;
        *open.lock().unwrap_or_else(|p| p.into_inner()) = true;
        condvar.notify_all();
    }
}

/// Blocks until the gate opens (at most 30 s, so a broken test cannot hang).
pub(crate) fn pass_gate(gate: &(Mutex<bool>, Condvar)) {
    let (open, condvar) = gate;
    let mut guard = open.lock().unwrap_or_else(|p| p.into_inner());
    let deadline = Instant::now() + Duration::from_secs(30);
    while !*guard {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        guard = condvar
            .wait_timeout(guard, left)
            .unwrap_or_else(|p| p.into_inner())
            .0;
    }
}
