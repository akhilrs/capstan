//! The delivery driver (src/driver.ts): on every tick it observes agents, applies the message timers, notifies the
//! operator and pushes worker messages through the Herdr adapter. It never types into a PM pane and never resends: a
//! message is recorded as sent before it is typed, and a retry is the operator's decision.
//!
//! The driver runs on its own thread and takes the kernel once per ledger step (`KernelHandle::run`), where Node awaits:
//! a Herdr call or a process sample never runs inside a kernel closure.

use super::activity::{proc_entries, suppress_active_stalls};
use super::notifier::{Clock, NotificationKind, NotificationRequest, Notifier};
use super::pm_mail::{date_parse, pm_mail_summary, PENDING_STATES};
use crate::deps::{new_context, DriverSnapshot, KernelHandle};
use capstan_herdr::api::{
    is_agent_name, AdapterError, ClearInput, DriverAdapter, GuardedSendInput, HerdrState,
    HookError, InputBlocker, ProcessActivityProbe, SendOutcome, WakeOutcome, WakePmInput,
    INPUT_UNREADABLE_DETAIL, MAX_TEXT_BYTES,
};
use capstan_herdr::process_activity::ProcessActivityTracker;
use capstan_kernel::areas::messaging::pm_wake_backoff_seconds;
use capstan_kernel::records::{is_final_state, MAX_INPUT_CLEAR_BYTES};
use capstan_kernel::{KernelError, KernelResult};
use serde_json::{json, Value};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};

pub const MIN_TICK_MS: u64 = 500;
pub const DEFAULT_TICK_MS: u64 = 2_000;
pub const FAILURE_LIMIT: u32 = 3;
/// Observations in a row that fail with a not-found error before an agent is recorded lost.
pub const LOSS_LIMIT: u32 = 3;
/// Ticks in a row in which every observed agent is not found (a Herdr restart?) that the counters are held before they
/// start again from zero.
pub const SUPPRESS_LIMIT: u32 = 10;
pub const STUCK_AFTER_TICKS: u32 = 10;
/// The least time between two process samples of one working agent.
pub const PROCESS_SAMPLE_MS: i64 = 15_000;
const TRUNCATION_MARKER: &str = "[truncated]";
/// The line typed into an idle PM that has an unread message.
pub const PM_WAKE_TEXT: &str = "Run cstan inbox: a teammate has written to you.";
/// The default PM stale time, in minutes (`DEFAULT_PM_STALE_MINUTES`).
pub const DEFAULT_PM_STALE_MINUTES: i64 = 20;

/// The kinds of `HookError` the driver's callbacks raise (the Node class names).
const STALE_ACTION: &str = "StaleActionError";
const MESSAGE_TRANSITION: &str = "MessageTransitionError";
const TYPE_ERROR: &str = "TypeError";

pub type DriverLog = Arc<dyn Fn(&str, Value) + Send + Sync>;

pub struct DriverOptions {
    pub kernel: KernelHandle,
    pub adapter: Arc<dyn DriverAdapter>,
    /// The nine messaging timers in the camelCase shape of `MessagingTimers`.
    pub timers: Value,
    pub notifier: Arc<Notifier>,
    /// The project (operator) credential, which holds controller:reconcile.
    pub credential: String,
    pub now: Clock,
    pub log: DriverLog,
    pub tick_ms: Option<u64>,
    /// Tells whether a tool process under a working agent uses CPU; such an agent is not reported as stalled.
    pub process_probe: Option<Arc<dyn ProcessActivityProbe>>,
    /// A PM message pending this long raises one stale notification per episode.
    pub pm_stale_seconds: i64,
    /// Where the status view of the driver is kept, when the caller made the view before the driver.
    pub snapshot: Option<Arc<Mutex<DriverSnapshot>>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Outcome {
    Ok,
    NotFound,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SkipReason {
    NoPane,
    PaneMismatch,
    PaneNotReady,
    InputUnreadable,
    HerdrError,
    ClearWait,
}

impl SkipReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::NoPane => "no_pane",
            Self::PaneMismatch => "pane_mismatch",
            Self::PaneNotReady => "pane_not_ready",
            Self::InputUnreadable => "input_unreadable",
            Self::HerdrError => "herdr_error",
            Self::ClearWait => "clear_wait",
        }
    }
}

/// What a wake judgement needs of a PM's whole mail: each message's state, and the latest time anything was pulled or
/// acknowledged.
#[derive(Clone)]
struct MailDigest {
    state_by_id: HashMap<String, String>,
    latest_pull_ms: f64,
}

struct Wake {
    wake_at: i64,
    count: i64,
    unanswered_logged: bool,
}

#[derive(Clone)]
struct StaleEpisode {
    since: String,
    oldest_message_id: String,
    pending: usize,
    notified: bool,
}

/// Message lists by agent as of one ledger version: every mutation bumps the version, so a list read at the current
/// version is current. Shared with the callbacks the adapter runs.
#[derive(Default)]
struct OpenMail {
    lists: Mutex<HashMap<String, (i64, Vec<Value>)>>,
}

/// The names the error classes of Node give the failures a callback raises.
fn hook_error_of(error: &KernelError) -> HookError {
    match error {
        KernelError::MessageTransition { code, .. } => {
            HookError::new(MESSAGE_TRANSITION, code.clone())
        }
        other => HookError::new(other.name(), other.message()),
    }
}

/// `String(error)` of an adapter failure.
fn describe(error: &AdapterError) -> String {
    format!("{}: {}", error.name(), error.message())
}

fn truncate_for_record(input: &str) -> String {
    if input.len() <= MAX_INPUT_CLEAR_BYTES {
        return input.to_string();
    }
    let budget = MAX_INPUT_CLEAR_BYTES - TRUNCATION_MARKER.len();
    let mut out = String::new();
    let mut bytes = 0;
    for c in input.chars() {
        let size = c.len_utf8();
        if bytes + size > budget {
            break;
        }
        out.push(c);
        bytes += size;
    }
    out.push_str(TRUNCATION_MARKER);
    out
}

fn str_of<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}

fn state_is_open(state: &str) -> bool {
    state == "queued" || state == "deferred"
}

/// `queueHead` over open messages as JSON: the first (by sequence) that is not final.
fn queue_head(messages: &[Value]) -> Option<&Value> {
    let mut head: Option<&Value> = None;
    for message in messages {
        if is_final_state(str_of(message, "state")) {
            continue;
        }
        let sequence = message["sequence"].as_i64().unwrap_or(i64::MAX);
        if head.is_none_or(|h| sequence < h["sequence"].as_i64().unwrap_or(i64::MAX)) {
            head = Some(message);
        }
    }
    head
}

pub struct Driver {
    steps: Steps,
    adapter: Arc<dyn DriverAdapter>,
    timers: Value,
    notifier: Arc<Notifier>,
    now: Clock,
    log: DriverLog,
    tick_ms: u64,
    process_probe: Option<Arc<dyn ProcessActivityProbe>>,
    pm_stale_seconds: i64,
    snapshot: Arc<Mutex<DriverSnapshot>>,
    stalled: Vec<String>,
    once: HashSet<String>,
    /// Once-keys whose condition occurred in the current tick; the rest are forgotten so a recurrence is reported again.
    seen: HashSet<String>,
    failures: HashMap<String, u32>,
    skip_counts: Vec<(String, SkipReason, u32)>,
    tick_skips: Vec<(String, SkipReason)>,
    skipped_now: HashSet<String>,
    skipped_agents: HashSet<String>,
    stuck: Vec<(String, String)>,
    /// Consecutive not-found observations per agent.
    missing: HashMap<String, u32>,
    lost: BTreeSet<String>,
    suppressed_ticks: u32,
    /// After the hold limit, an all-not-found tick counts again until some agent answers.
    suppression_spent: bool,
    activity: ProcessActivityTracker,
    last_sample_ms: HashMap<String, i64>,
    probe_failed: HashSet<String>,
    mail_digest: HashMap<String, (i64, MailDigest)>,
    wakes: Vec<(String, Wake)>,
    stale_episodes: Vec<(String, StaleEpisode)>,
    /// Open unreadable-input episodes by agent: the deferred head message that opened each.
    unreadable: HashMap<String, String>,
}

impl Driver {
    pub fn new(options: DriverOptions) -> Self {
        Self {
            steps: Steps::new(options.kernel, options.credential),
            adapter: options.adapter,
            timers: options.timers,
            notifier: options.notifier,
            now: options.now,
            log: options.log,
            tick_ms: options.tick_ms.unwrap_or(DEFAULT_TICK_MS).max(MIN_TICK_MS),
            process_probe: options.process_probe,
            pm_stale_seconds: options.pm_stale_seconds,
            snapshot: options.snapshot.unwrap_or_else(|| {
                Arc::new(Mutex::new(DriverSnapshot {
                    lost_agent_ids: Some(Vec::new()),
                    ..DriverSnapshot::default()
                }))
            }),
            stalled: Vec::new(),
            once: HashSet::new(),
            seen: HashSet::new(),
            failures: HashMap::new(),
            skip_counts: Vec::new(),
            tick_skips: Vec::new(),
            skipped_now: HashSet::new(),
            skipped_agents: HashSet::new(),
            stuck: Vec::new(),
            missing: HashMap::new(),
            lost: BTreeSet::new(),
            suppressed_ticks: 0,
            suppression_spent: false,
            activity: ProcessActivityTracker::new(),
            last_sample_ms: HashMap::new(),
            probe_failed: HashSet::new(),
            mail_digest: HashMap::new(),
            wakes: Vec::new(),
            stale_episodes: Vec::new(),
            unreadable: HashMap::new(),
        }
    }

    pub fn tick_ms(&self) -> u64 {
        self.tick_ms
    }

    /// The view `status` reads, updated at the end of every tick.
    pub fn snapshot_handle(&self) -> Arc<Mutex<DriverSnapshot>> {
        Arc::clone(&self.snapshot)
    }

    pub fn snapshot(&self) -> DriverSnapshot {
        self.snapshot
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// The kernel calls made since the last call, by the `ControllerCore` member's Node name: the cost of the ledger
    /// part of a tick, which the parity test holds equal to Node's.
    pub fn take_kernel_calls(&self) -> BTreeMap<String, u64> {
        self.steps.take_calls()
    }

    fn publish(&self) {
        let stale = self.stale_episodes.first().map(|(_, e)| {
            json!({
                "since": e.since,
                "oldestMessageId": e.oldest_message_id,
                "pending": e.pending,
                "notified": e.notified,
            })
        });
        *self.snapshot.lock().unwrap_or_else(|p| p.into_inner()) = DriverSnapshot {
            stalled_agent_ids: self.stalled.clone(),
            lost_agent_ids: Some(self.lost.iter().cloned().collect()),
            stuck: self.stuck.clone(),
            pm_stale: stale,
        };
    }

    // -------------------------------------------------------------------------------------------- the kernel

    fn read<F>(&self, method: &'static str, f: F) -> KernelResult<Value>
    where
        F: FnOnce(&capstan_kernel::Core) -> KernelResult<Value> + Send + 'static,
    {
        self.steps.read(method, f)
    }

    fn mutate<F>(&self, method: &'static str, f: F) -> KernelResult<Value>
    where
        F: FnOnce(
                &capstan_kernel::Core,
                &capstan_kernel::types::MutationContext,
            ) -> KernelResult<Value>
            + Send
            + 'static,
    {
        self.steps.mutate(method, f)
    }

    fn message(&self, message_id: &str) -> Option<Value> {
        self.steps.message(message_id)
    }

    fn is_paused(&self, agent_id: &str) -> bool {
        self.steps.is_paused(agent_id)
    }

    fn open_messages(&self, agent_id: &str) -> KernelResult<Vec<Value>> {
        self.steps.open_messages(agent_id)
    }

    fn pm_messages(&self, pm: &Value) -> Vec<Value> {
        match self.open_messages(str_of(pm, "agentId")) {
            Ok(messages) => messages,
            Err(error) => {
                self.log("pm_mail_failed", json!({"error": kernel_text(&error)}));
                Vec::new()
            }
        }
    }

    fn log(&self, event: &str, details: Value) {
        (self.log)(event, details);
    }

    fn log_once(&mut self, key: String, event: &str, details: Value) {
        self.seen.insert(key.clone());
        if !self.once.insert(key) {
            return;
        }
        self.log(event, details);
    }

    fn skip(&mut self, message_id: &str, agent_id: &str, reason: SkipReason) {
        match self.tick_skips.iter_mut().find(|(id, _)| id == message_id) {
            Some(entry) => entry.1 = reason,
            None => self.tick_skips.push((message_id.to_string(), reason)),
        }
        self.skipped_agents.insert(agent_id.to_string());
    }

    // -------------------------------------------------------------------------------------------- the tick

    /// Runs one tick.
    pub fn tick(&mut self) {
        self.tick_skips = Vec::new();
        self.skipped_agents = HashSet::new();
        self.seen = HashSet::new();
        let agents = match self.read("activeAgents", |core| core.active_agents()) {
            Ok(Value::Array(agents)) => agents,
            Ok(_) => Vec::new(),
            Err(error) => {
                self.log("tick_failed", json!({"error": kernel_text(&error)}));
                return;
            }
        };
        let mut outcomes: Vec<(String, Outcome)> = Vec::new();
        for agent in &agents {
            self.observe(agent, &mut outcomes);
        }
        self.judge_loss(&agents, &outcomes);
        self.forget_processes(&agents);
        // Wakes are judged before the next one can be typed, so a wake that went unanswered is logged before it is
        // repeated.
        self.judge_wakes(&agents);
        self.advance();
        self.judge_stale(&agents);
        for agent in &agents {
            if str_of(agent, "kind") != "PM"
                && !self.skipped_agents.contains(str_of(agent, "agentId"))
                // A paused agent's head stays queued or deferred: no send, no new deferral.
                && !self.is_paused(str_of(agent, "agentId"))
            {
                if let Err(error) = self.deliver(agent) {
                    self.log("tick_failed", json!({"error": error}));
                    self.publish();
                    return;
                }
            }
        }
        self.update_stuck();
        self.forget(&agents);
        self.publish();
    }

    fn observe(&mut self, agent: &Value, outcomes: &mut Vec<(String, Outcome)>) {
        let agent_id = str_of(agent, "agentId").to_string();
        if self.adapter.pane_for_agent(&agent_id).is_none() {
            // No pane to look at: a count left from before does not carry over to a pane that comes back.
            self.missing.remove(&agent_id);
            return;
        }
        match self.adapter.agent_observation(&agent_id) {
            Ok(state) => {
                let (id, value) = (agent_id.clone(), json!(state.as_str()));
                match self.mutate("recordAgentObservation", move |core, context| {
                    core.record_agent_observation(context, &id, &value)
                }) {
                    Ok(_) => {
                        outcomes.push((agent_id.clone(), Outcome::Ok));
                        if state == HerdrState::Working {
                            self.sample_processes(&agent_id);
                        }
                    }
                    Err(error) => {
                        outcomes.push((agent_id.clone(), Outcome::Other));
                        self.log_once(
                            format!("{agent_id}|observe_failed"),
                            "observe_failed",
                            json!({"agentId": agent_id, "error": error.name()}),
                        );
                    }
                }
            }
            Err(error) => {
                let outcome = match error.herdr_code() {
                    Some("agent_not_found") | Some("pane_not_found") => Outcome::NotFound,
                    _ => Outcome::Other,
                };
                outcomes.push((agent_id.clone(), outcome));
                let name = if matches!(error, AdapterError::AgentPaneMismatch(_)) {
                    "pane_mismatch"
                } else {
                    "observe_failed"
                };
                self.log_once(
                    format!("{agent_id}|{name}"),
                    name,
                    json!({"agentId": agent_id, "error": error.name()}),
                );
            }
        }
    }

    /// Samples the tool processes under a working, unpaused agent, at most once per PROCESS_SAMPLE_MS. A failing probe
    /// suppresses nothing.
    fn sample_processes(&mut self, agent_id: &str) {
        let Some(probe) = self.process_probe.clone() else {
            return;
        };
        if self.is_paused(agent_id) {
            return;
        }
        let Some(pane_id) = self.adapter.pane_for_agent(agent_id) else {
            return;
        };
        let now = (self.now)();
        if let Some(last) = self.last_sample_ms.get(agent_id) {
            if now - last < PROCESS_SAMPLE_MS {
                return;
            }
        }
        self.last_sample_ms.insert(agent_id.to_string(), now);
        match probe.sample(&pane_id) {
            Ok(sample) => {
                let at = (self.now)();
                self.activity
                    .record(agent_id, &proc_entries(&sample), at as u64);
            }
            Err(error) => {
                if !self.probe_failed.insert(agent_id.to_string()) {
                    return;
                }
                self.log(
                    "process_probe_failed",
                    json!({"agentId": agent_id, "error": format!("HerdrError: {}", error.message)}),
                );
            }
        }
    }

    /// Drops the process state of agents that are no longer active or are recorded lost.
    fn forget_processes(&mut self, agents: &[Value]) {
        let live: HashSet<&str> = agents
            .iter()
            .map(|a| str_of(a, "agentId"))
            .filter(|id| !self.lost.contains(*id))
            .collect();
        let gone: Vec<String> = self
            .last_sample_ms
            .keys()
            .filter(|id| !live.contains(id.as_str()))
            .cloned()
            .collect();
        for id in gone {
            self.last_sample_ms.remove(&id);
            self.probe_failed.remove(&id);
            self.activity.forget(&id);
        }
    }

    /// Counts consecutive not-found observations. A tick in which every observed agent (at least two) is not found
    /// counts for nothing: a Herdr server that restarted would otherwise make every live agent look dead. The counters
    /// are held for a few such ticks and then start again from zero.
    fn judge_loss(&mut self, agents: &[Value], outcomes: &[(String, Outcome)]) {
        let active: HashSet<&str> = agents.iter().map(|a| str_of(a, "agentId")).collect();
        self.missing.retain(|id, _| active.contains(id.as_str()));
        self.lost.retain(|id| active.contains(id.as_str()));
        let observed = outcomes.len();
        let not_found = outcomes
            .iter()
            .filter(|(_, o)| *o == Outcome::NotFound)
            .count();
        let all_gone = observed >= 2 && not_found == observed;
        if all_gone && !self.suppression_spent {
            self.suppressed_ticks += 1;
            self.log_once(
                "loss_suppressed".into(),
                "loss_suppressed",
                json!({"agents": observed}),
            );
            if self.suppressed_ticks >= SUPPRESS_LIMIT {
                self.missing.clear();
                self.suppressed_ticks = 0;
                self.suppression_spent = true;
            }
            return;
        }
        if !all_gone {
            self.suppressed_ticks = 0;
            self.suppression_spent = false;
        }
        for (id, outcome) in outcomes {
            match outcome {
                Outcome::Ok => {
                    self.missing.remove(id);
                    self.lost.remove(id);
                }
                Outcome::Other => {
                    // Not knowing is not seeing it gone: the row of not-found observations is broken.
                    self.missing.remove(id);
                }
                Outcome::NotFound => {
                    let count = self.missing.get(id).copied().unwrap_or(0) + 1;
                    self.missing.insert(id.clone(), count);
                    if count >= LOSS_LIMIT && !self.lost.contains(id) {
                        self.mark_lost(id);
                    }
                }
            }
        }
    }

    /// Listed lost only once the ledger took it, so a failed write is tried again on the next tick.
    fn mark_lost(&mut self, agent_id: &str) {
        let id = agent_id.to_string();
        match self.mutate("recordAgentLost", move |core, context| {
            core.record_agent_lost(context, &json!({"agentId": id}))
        }) {
            Ok(result) => {
                self.lost.insert(agent_id.to_string());
                self.log(
                    "agent_lost",
                    json!({"agentId": agent_id, "recorded": result["recorded"]}),
                );
            }
            Err(error) => self.log_once(
                format!("{agent_id}|agent_lost_not_recorded"),
                "agent_lost_not_recorded",
                json!({"agentId": agent_id, "error": kernel_text(&error)}),
            ),
        }
    }

    fn open_unreadable(&mut self, agent_id: &str, message_id: &str, blocker: &str) {
        if self.unreadable.contains_key(agent_id) {
            return;
        }
        self.unreadable
            .insert(agent_id.to_string(), message_id.to_string());
        let input = json!({
            "agentId": agent_id,
            "messageId": message_id,
            "blocker": if blocker == "dialog" || blocker == "permission_prompt" { blocker } else { "unknown" },
        });
        if let Err(error) = self.mutate("queueInputBlockedNotice", move |core, context| {
            core.queue_input_blocked_notice(context, &input)
        }) {
            self.log(
                "attention_notice_failed",
                json!({"error": kernel_text(&error)}),
            );
        }
    }

    fn advance(&mut self) {
        let timers = self.timers.clone();
        let advance = match self.mutate("advanceMessaging", move |core, context| {
            core.advance_messaging(context, &timers)
        }) {
            Ok(advance) => advance,
            Err(error) => {
                self.log("advance_failed", json!({"error": kernel_text(&error)}));
                return;
            }
        };
        let reported: Vec<String> = advance["stalledAgentIds"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let now = (self.now)();
        let stall_after = self.timers["stallAfterSeconds"].as_f64().unwrap_or(0.0);
        let advance = suppress_active_stalls(&advance, self.activity.activity(), now, stall_after);
        let stalled: Vec<String> = advance["stalledAgentIds"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        for agent_id in &reported {
            if !stalled.contains(agent_id) {
                self.log_once(
                    format!("{agent_id}|stall_suppressed"),
                    "stall_suppressed",
                    json!({"agentId": agent_id}),
                );
            }
        }
        self.stalled = stalled.clone();
        for agent_id in &stalled {
            self.log_once(
                format!("{agent_id}|agent_stalled"),
                "agent_stalled",
                json!({"agentId": agent_id}),
            );
        }
        if let Err(error) = self.mutate("queueMissingDeliveryNotices", |core, context| {
            core.queue_missing_delivery_notices(context)
        }) {
            self.log(
                "delivery_notice_failed",
                json!({"error": kernel_text(&error)}),
            );
        }
        let attention = advance["attention"].clone();
        if attention.as_array().is_some_and(|a| !a.is_empty()) {
            if let Err(error) = self.mutate("queueAttentionNotices", move |core, context| {
                core.queue_attention_notices(context, &attention)
            }) {
                self.log(
                    "attention_notice_failed",
                    json!({"error": kernel_text(&error)}),
                );
            }
        }
        for action in advance["actions"].as_array().cloned().unwrap_or_default() {
            let kind = str_of(&action, "kind").to_string();
            let outcome = match kind.as_str() {
                "notify_operator" => self.notify_head(&action),
                "wake_pm" => self.wake(&action),
                _ => self.clear(&action),
            };
            if let Err(error) = outcome {
                self.log("action_failed", json!({"kind": kind, "error": error}));
            }
        }
    }

    fn is_head(&self, message: &Value) -> KernelResult<bool> {
        let open = self.open_messages(str_of(message, "recipientAgentId"))?;
        Ok(queue_head(&open).is_some_and(|h| h["messageId"] == message["messageId"]))
    }

    /// Whether `error` is the ledger moving on under a physical action (logged as `stale_action`).
    fn stale(&self, kind: &str, code: &str, message_id: &str) -> bool {
        if kind == STALE_ACTION {
            self.log("stale_action", json!({"messageId": message_id}));
            return true;
        }
        if kind == MESSAGE_TRANSITION {
            self.log(
                "stale_action",
                json!({"messageId": message_id, "code": code}),
            );
            return true;
        }
        false
    }

    fn is_stale(&self, error: &AdapterError, message_id: &str) -> bool {
        match error {
            AdapterError::Hook(hook) => self.stale(&hook.kind, &hook.message, message_id),
            _ => false,
        }
    }

    fn is_stale_kernel(&self, error: &KernelError, message_id: &str) -> bool {
        match error {
            KernelError::MessageTransition { code, .. } => {
                self.stale(MESSAGE_TRANSITION, code, message_id)
            }
            _ => false,
        }
    }

    /// Types one wake line into an idle PM that has an unread message; the wake is recorded first and never repeated
    /// sooner than the backoff.
    fn wake(&mut self, action: &Value) -> Result<(), String> {
        let message_id = str_of(action, "messageId").to_string();
        let Some(message) = self.message(&message_id) else {
            return Ok(());
        };
        if str_of(&message, "state") != "queued" {
            return Ok(());
        }
        let recipient = str_of(&message, "recipientAgentId").to_string();
        let agent = self.agent_record(&recipient);
        let Some(agent) = agent else { return Ok(()) };
        if str_of(&agent, "state") != "active" || str_of(&agent, "kind") != "PM" {
            return Ok(());
        }
        let Ok(pane_id) = self.owned(&agent) else {
            return Ok(());
        };
        let steps = self.steps.clone();
        let recorded = Cell::new(false);
        let id = message_id.clone();
        let mut before_send = || -> Result<(), HookError> {
            match steps.message(&id) {
                Some(current) if str_of(&current, "state") == "queued" => {}
                _ => {
                    return Err(HookError::new(
                        STALE_ACTION,
                        "the message is no longer unread",
                    ))
                }
            }
            let record = id.clone();
            steps
                .mutate("recordPmWake", move |core, context| {
                    core.record_pm_wake(context, &record)
                })
                .map_err(|e| hook_error_of(&e))?;
            recorded.set(true);
            Ok(())
        };
        let outcome = self.adapter.wake_pm(WakePmInput {
            pane_id: &pane_id,
            text: PM_WAKE_TEXT,
            before_send: &mut before_send,
        });
        match outcome {
            Ok(WakeOutcome::Sent) => {
                let at = (self.now)();
                let count = self
                    .wakes
                    .iter()
                    .find(|(id, _)| *id == message_id)
                    .map_or(0, |(_, w)| w.count)
                    + 1;
                self.set_wake(
                    &message_id,
                    Wake {
                        wake_at: at,
                        count,
                        unanswered_logged: false,
                    },
                );
                self.log("pm_wake_typed", json!({"messageId": message_id}));
            }
            Ok(skipped) => {
                let reason = if skipped == WakeOutcome::PmNotIdle {
                    "pm_not_idle"
                } else {
                    "input_not_empty"
                };
                self.log_once(
                    format!("{message_id}|wake_skipped|{reason}"),
                    "wake_skipped",
                    json!({"messageId": message_id, "reason": reason}),
                );
            }
            Err(error) => {
                if recorded.get() {
                    // The wake is in the ledger, so the next one waits for the backoff; it was not typed, so it is not
                    // a wake.
                    self.log(
                        "pm_wake_failed",
                        json!({"messageId": message_id, "error": describe(&error)}),
                    );
                } else if !self.is_stale(&error, &message_id) {
                    self.log(
                        "wake_failed",
                        json!({"messageId": message_id, "error": describe(&error)}),
                    );
                }
            }
        }
        Ok(())
    }

    fn set_wake(&mut self, message_id: &str, wake: Wake) {
        match self.wakes.iter_mut().find(|(id, _)| id == message_id) {
            Some(entry) => entry.1 = wake,
            None => self.wakes.push((message_id.to_string(), wake)),
        }
    }

    fn agent_record(&self, agent_id: &str) -> Option<Value> {
        let id = agent_id.to_string();
        match self.read("agentRecord", move |core| core.agent_record(&id)) {
            Ok(Value::Null) | Err(_) => None,
            Ok(value) => Some(value),
        }
    }

    /// A wake is answered when the message left the queued state or the PM pulled or acknowledged any message after it;
    /// one still queued after the next backoff step is logged once.
    fn judge_wakes(&mut self, agents: &[Value]) {
        // With no wake outstanding there is nothing to judge, so the PM's whole mail history is not read.
        if self.wakes.is_empty() {
            return;
        }
        let digests: Vec<MailDigest> = agents
            .iter()
            .filter(|a| str_of(a, "kind") == "PM")
            .map(|pm| self.pm_digest(pm))
            .collect();
        let now = (self.now)();
        let interval = self.timers["pmWakeIntervalSeconds"].as_f64().unwrap_or(0.0);
        let mut kept = Vec::new();
        for (message_id, mut wake) in std::mem::take(&mut self.wakes) {
            let state = digests
                .iter()
                .find_map(|d| d.state_by_id.get(&message_id).cloned());
            let pulled_since = digests
                .iter()
                .any(|d| d.latest_pull_ms > wake.wake_at as f64);
            if state.as_deref() != Some("queued") || pulled_since {
                continue;
            }
            let interval_ms = pm_wake_backoff_seconds(interval, wake.count) * 1000.0;
            if !wake.unanswered_logged && (now - wake.wake_at) as f64 >= interval_ms {
                wake.unanswered_logged = true;
                self.log(
                    "pm_wake_unanswered",
                    json!({"messageId": message_id, "wakes": wake.count}),
                );
            }
            kept.push((message_id, wake));
        }
        self.wakes = kept;
    }

    /// A wake also looks at what was pulled or acknowledged since, so it reads the PM's whole mail; only a digest of it
    /// is kept.
    fn pm_digest(&mut self, pm: &Value) -> MailDigest {
        let agent_id = str_of(pm, "agentId").to_string();
        let version = match self.read("stateVersion", |core| core.state_version().map(Value::from))
        {
            Ok(v) => v.as_i64().unwrap_or(-1),
            Err(error) => {
                self.log("pm_mail_failed", json!({"error": kernel_text(&error)}));
                return empty_digest();
            }
        };
        if let Some((cached, digest)) = self.mail_digest.get(&agent_id) {
            if *cached == version {
                return digest.clone();
            }
        }
        let id = agent_id.clone();
        let messages = match self.read("messagesFor", move |core| core.messages_for(&id)) {
            Ok(Value::Array(messages)) => messages,
            Ok(_) => Vec::new(),
            Err(error) => {
                self.log("pm_mail_failed", json!({"error": kernel_text(&error)}));
                return empty_digest();
            }
        };
        let mut state_by_id = HashMap::new();
        let mut latest = f64::NEG_INFINITY;
        for message in &messages {
            state_by_id.insert(
                str_of(message, "messageId").to_string(),
                str_of(message, "state").to_string(),
            );
            for field in ["sentAt", "ackedAt"] {
                if let Some(ms) = message[field].as_str().and_then(date_parse) {
                    if ms as f64 > latest {
                        latest = ms as f64;
                    }
                }
            }
        }
        let digest = MailDigest {
            state_by_id,
            latest_pull_ms: latest,
        };
        self.mail_digest.insert(agent_id, (version, digest.clone()));
        digest
    }

    /// One stale notification per episode; an episode is keyed by the oldest pending message and ends when that message
    /// is no longer pending.
    fn judge_stale(&mut self, agents: &[Value]) {
        let pms: Vec<&Value> = agents
            .iter()
            .filter(|a| str_of(a, "kind") == "PM")
            .collect();
        for pm in &pms {
            self.judge_stale_for(pm);
        }
        let live: HashSet<&str> = pms.iter().map(|a| str_of(a, "agentId")).collect();
        self.stale_episodes
            .retain(|(id, _)| live.contains(id.as_str()));
    }

    fn judge_stale_for(&mut self, pm: &Value) {
        let pm_id = str_of(pm, "agentId").to_string();
        let messages = self.pm_messages(pm);
        let now = (self.now)();
        let summary = pm_mail_summary(&messages, now, self.pm_stale_seconds);
        if let Some((_, episode)) = self.stale_episodes.iter().find(|(id, _)| *id == pm_id) {
            let still_pending = messages.iter().any(|m| {
                str_of(m, "messageId") == episode.oldest_message_id
                    && PENDING_STATES.contains(&str_of(m, "state"))
            });
            if !still_pending {
                self.stale_episodes.retain(|(id, _)| *id != pm_id);
            }
        }
        let Some(oldest) = summary.oldest_message_id.clone() else {
            return;
        };
        if !summary.stale {
            return;
        }
        if !self.stale_episodes.iter().any(|(id, _)| *id == pm_id) {
            self.stale_episodes.push((
                pm_id.clone(),
                StaleEpisode {
                    since: capstan_ledger::iso_from_millis(now),
                    oldest_message_id: oldest,
                    pending: summary.pending,
                    notified: false,
                },
            ));
        }
        let Some((_, current)) = self.stale_episodes.iter_mut().find(|(id, _)| *id == pm_id) else {
            return;
        };
        current.pending = summary.pending;
        let message_id = current.oldest_message_id.clone();
        if current.notified || self.is_paused(&pm_id) {
            return;
        }
        let Some((_, current)) = self.stale_episodes.iter_mut().find(|(id, _)| *id == pm_id) else {
            return;
        };
        current.notified = true;
        let request = NotificationRequest {
            kind: NotificationKind::PmStale,
            message_id,
            recipient_agent_id: pm_id,
            repeat: false,
            detail: Some(format!(
                "{} message{}",
                summary.pending,
                if summary.pending == 1 { "" } else { "s" }
            )),
        };
        let results = self.notifier.send(&request);
        self.notifier.write(&request, &results, false);
    }

    fn notify_head(&mut self, action: &Value) -> Result<(), String> {
        let message_id = str_of(action, "messageId");
        let Some(message) = self.message(message_id) else {
            return Ok(());
        };
        if !self.is_head(&message).map_err(|e| kernel_text(&e))? {
            return Ok(());
        }
        let request = NotificationRequest {
            kind: NotificationKind::PmMessage,
            message_id: message_id.to_string(),
            recipient_agent_id: str_of(&message, "recipientAgentId").to_string(),
            repeat: action["repeat"].as_bool().unwrap_or(false),
            detail: None,
        };
        let results = self.notifier.send(&request);
        if !results.iter().any(|r| r.ok) {
            self.notifier.write(&request, &results, false);
            return Ok(());
        }
        let mut recorded = false;
        let current = self.message(message_id);
        let head = match &current {
            Some(current) => self.is_head(current).map_err(|e| kernel_text(&e))?,
            None => false,
        };
        if head {
            let id = message_id.to_string();
            match self.mutate("recordNotification", move |core, context| {
                core.record_notification(context, &id)
            }) {
                Ok(_) => recorded = true,
                Err(error) => {
                    if !self.is_stale_kernel(&error, message_id) {
                        self.log(
                            "record_notification_failed",
                            json!({"messageId": message_id}),
                        );
                    }
                }
            }
        }
        self.notifier.write(&request, &results, recorded);
        Ok(())
    }

    /// The pane the agent's name and the adapter's registry agree on, or why not.
    fn owned(&mut self, agent: &Value) -> Result<String, SkipReason> {
        let agent_id = str_of(agent, "agentId");
        if !is_agent_name(agent_id) {
            self.log_once(
                format!("{agent_id}|bad_agent_name"),
                "bad_agent_name",
                json!({"agentId": agent_id}),
            );
            return Err(SkipReason::PaneMismatch);
        }
        let Some(pane_id) = self.adapter.pane_for_agent(agent_id) else {
            return Err(SkipReason::NoPane);
        };
        if self
            .adapter
            .pane_entry(&pane_id)
            .and_then(|e| e.agent)
            .as_deref()
            != Some(agent_id)
        {
            return Err(SkipReason::PaneMismatch);
        }
        Ok(pane_id)
    }

    fn clear(&mut self, action: &Value) -> Result<(), String> {
        let message_id = str_of(action, "messageId").to_string();
        let Some(message) = self.message(&message_id) else {
            return Ok(());
        };
        if str_of(&message, "state") != "deferred"
            || str_of(&message, "deferredReason") != "input_not_empty"
            || message["deferredAt"].is_null()
        {
            return Ok(());
        }
        let recipient = str_of(&message, "recipientAgentId").to_string();
        let Some(agent) = self.agent_record(&recipient) else {
            return Ok(());
        };
        if str_of(&agent, "state") != "active" {
            return Ok(());
        }
        let pane_id = match self.owned(&agent) {
            Ok(pane_id) => pane_id,
            Err(reason) => {
                self.skip(&message_id, &recipient, reason);
                self.note_skip(&recipient, &message_id, reason);
                return Ok(());
            }
        };
        let steps = self.steps.clone();
        let log = Arc::clone(&self.log);
        let id = message_id.clone();
        let deferred_for = (self.now)() - date_parse(str_of(&message, "deferredAt")).unwrap_or(0);
        let max_deferral = self.timers["maxDeferralSeconds"].as_f64().unwrap_or(0.0) * 1000.0;
        let mut discard = |text: &str| -> Result<(), HookError> {
            match steps.message(&id) {
                Some(current)
                    if str_of(&current, "state") == "deferred"
                        && str_of(&current, "deferredReason") == "input_not_empty" => {}
                _ => {
                    return Err(HookError::new(
                        STALE_ACTION,
                        "the message moved before the clear",
                    ))
                }
            }
            let (record, text) = (id.clone(), truncate_for_record(text));
            steps
                .mutate("recordInputClear", move |core, context| {
                    core.record_input_clear(context, &record, &text)
                })
                .map(|_| ())
                .map_err(|e| hook_error_of(&e))
        };
        let key_log = move |entry: &capstan_herdr::api::KeyLogEntry| {
            log(
                "key",
                json!({"pane": entry.pane, "key": entry.key, "reason": entry.reason}),
            );
        };
        let cleared = self.adapter.clear_after_deferral(ClearInput {
            pane_id: &pane_id,
            deferred_for_ms: deferred_for as f64,
            max_deferral_ms: max_deferral,
            discard: &mut discard,
            log: &key_log,
        });
        let cleared = match cleared {
            Ok(cleared) => cleared,
            Err(error) => {
                self.clear_failure(&error, &recipient, &message_id);
                return Ok(());
            }
        };
        self.failures.remove(&message_id);
        self.unreadable.remove(&recipient);
        if cleared.cleared && action["notifyOperator"].as_bool().unwrap_or(false) {
            let request = NotificationRequest {
                kind: NotificationKind::InputCleared,
                message_id: message_id.clone(),
                recipient_agent_id: recipient,
                repeat: false,
                detail: Some(cleared.text.chars().count().to_string()),
            };
            let results = self.notifier.send(&request);
            self.notifier.write(&request, &results, true);
        }
        Ok(())
    }

    fn clear_failure(&mut self, error: &AdapterError, agent_id: &str, message_id: &str) {
        if self.is_stale(error, message_id) {
            return;
        }
        match error {
            AdapterError::DeferralNotElapsed(_) | AdapterError::NotIdle(_) => {
                self.skip(message_id, agent_id, SkipReason::ClearWait);
            }
            AdapterError::InputUnreadable { blocker, .. } => {
                self.open_unreadable(agent_id, message_id, blocker.as_str());
                self.log_once(
                    format!("{message_id}|input_unreadable"),
                    "input_unreadable",
                    json!({"messageId": message_id}),
                );
                self.skip(message_id, agent_id, SkipReason::InputUnreadable);
            }
            AdapterError::ClearFailed(_) => {
                self.count_failure(message_id, "input line could not be cleared");
                self.skip(message_id, agent_id, SkipReason::HerdrError);
            }
            AdapterError::Hook(hook) if hook.kind == TYPE_ERROR => {
                // The core refused the text it was asked to record; retrying the same text can only fail the same way,
                // so this counts toward the limit.
                self.count_failure(message_id, "the typed text could not be recorded");
                self.skip(message_id, agent_id, SkipReason::HerdrError);
            }
            other => self.adapter_failure(other, agent_id, message_id),
        }
    }

    fn note_skip(&mut self, agent_id: &str, message_id: &str, reason: SkipReason) {
        self.log_once(
            format!("{message_id}|{}", reason.as_str()),
            reason.as_str(),
            json!({"agentId": agent_id, "messageId": message_id}),
        );
    }

    fn adapter_failure(&mut self, error: &AdapterError, agent_id: &str, message_id: &str) {
        match error {
            AdapterError::AgentPaneMismatch(_) => {
                self.skip(message_id, agent_id, SkipReason::PaneMismatch);
                self.note_skip(agent_id, message_id, SkipReason::PaneMismatch);
            }
            AdapterError::Phase(_) => {
                self.skip(message_id, agent_id, SkipReason::PaneNotReady);
                self.log_once(
                    format!("{message_id}|pane_not_ready"),
                    "pane_not_ready",
                    json!({"agentId": agent_id}),
                );
            }
            AdapterError::UnknownPane(_) => self.skip(message_id, agent_id, SkipReason::NoPane),
            AdapterError::Herdr(herdr) => {
                self.count_failure(message_id, &format!("Herdr failed: {}", herdr.code));
                self.skip(message_id, agent_id, SkipReason::HerdrError);
            }
            other => {
                self.log(
                    "delivery_error",
                    json!({"messageId": message_id, "error": describe(other)}),
                );
                self.skip(message_id, agent_id, SkipReason::HerdrError);
            }
        }
    }

    fn count_failure(&mut self, message_id: &str, reason: &str) {
        let count = self.failures.get(message_id).copied().unwrap_or(0) + 1;
        self.failures.insert(message_id.to_string(), count);
        self.log(
            "delivery_failure",
            json!({"messageId": message_id, "count": count}),
        );
        if count < FAILURE_LIMIT {
            return;
        }
        self.record_failure(message_id, reason);
    }

    fn record_failure(&mut self, message_id: &str, reason: &str) {
        let Some(current) = self.message(message_id) else {
            return;
        };
        if !state_is_open(str_of(&current, "state")) {
            return;
        }
        let (id, reason) = (message_id.to_string(), reason.to_string());
        match self.mutate("recordFailure", move |core, context| {
            core.record_failure(context, &id, &reason)
        }) {
            Ok(_) => {
                self.failures.remove(message_id);
            }
            Err(error) => {
                if !self.is_stale_kernel(&error, message_id) {
                    self.log("record_failure_failed", json!({"messageId": message_id}));
                }
            }
        }
    }

    fn deliver(&mut self, agent: &Value) -> Result<(), String> {
        let agent_id = str_of(agent, "agentId").to_string();
        let open = self.open_messages(&agent_id).map_err(|e| kernel_text(&e))?;
        let Some(head) = queue_head(&open).cloned() else {
            return Ok(());
        };
        let head_id = str_of(&head, "messageId").to_string();
        if !state_is_open(str_of(&head, "state")) {
            return Ok(());
        }
        let sender_actor = str_of(&head, "senderActorId").to_string();
        let sender = self
            .read("senderOf", move |core| core.sender_of(&sender_actor))
            .map_err(|e| kernel_text(&e))?;
        let role = str_of(&sender, "role");
        let from = if role == "operator" {
            "operator".to_string()
        } else if sender["agentId"].is_null() {
            role.to_string()
        } else {
            format!("{role} ({})", str_of(&sender, "agentId"))
        };
        let text = format!(
            "[capstan message {head_id} from {from}]\n{}\nAcknowledge with: cstan ack {head_id}",
            str_of(&head, "body")
        );
        if text.len() > MAX_TEXT_BYTES {
            self.record_failure(&head_id, "message too large to type");
            return Ok(());
        }
        let pane_id = match self.owned(agent) {
            Ok(pane_id) => pane_id,
            Err(reason) => {
                self.skip(&head_id, &agent_id, reason);
                self.note_skip(&agent_id, &head_id, reason);
                return Ok(());
            }
        };
        let steps = self.steps.clone();
        let hid = head_id.clone();
        let mut before_send = || -> Result<(), HookError> {
            let moved = || HookError::new(STALE_ACTION, "the message moved before it was sent");
            let current = steps.message(&hid).ok_or_else(moved)?;
            if !state_is_open(str_of(&current, "state")) {
                return Err(moved());
            }
            let open = steps
                .open_messages(str_of(&current, "recipientAgentId"))
                .map_err(|e| hook_error_of(&e))?;
            if !queue_head(&open).is_some_and(|h| h["messageId"] == current["messageId"]) {
                return Err(moved());
            }
            let id = hid.clone();
            steps
                .mutate("recordSent", move |core, context| {
                    core.record_sent(context, &id)
                })
                .map(|_| ())
                .map_err(|e| hook_error_of(&e))
        };
        let outcome = self.adapter.guarded_send(GuardedSendInput {
            pane_id: &pane_id,
            text: &text,
            before_send: &mut before_send,
        });
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                if self.is_stale(&error, &head_id) {
                    return Ok(());
                }
                match &error {
                    AdapterError::SendAfterRecord(_) => {
                        self.failures.remove(&head_id);
                        self.log("send_after_record", json!({"messageId": head_id}));
                    }
                    AdapterError::InvalidArgument(_) => {
                        self.record_failure(&head_id, "text refused by the input rules");
                    }
                    other => self.adapter_failure(other, &agent_id, &head_id),
                }
                return Ok(());
            }
        };
        self.failures.remove(&head_id);
        match outcome {
            SendOutcome::Sent => {
                self.unreadable.remove(&agent_id);
            }
            SendOutcome::Deferred {
                reason,
                detail,
                blocker,
            } => {
                self.record_deferral(&head_id, reason.as_str());
                let unreadable = detail.as_deref() == Some(INPUT_UNREADABLE_DETAIL);
                if reason.as_str() == "input_not_empty" && !unreadable {
                    self.unreadable.remove(&agent_id);
                }
                if unreadable {
                    self.open_unreadable(
                        &agent_id,
                        &head_id,
                        blocker.unwrap_or(InputBlocker::Unknown).as_str(),
                    );
                    self.skip(&head_id, &agent_id, SkipReason::InputUnreadable);
                    self.log_once(
                        format!("{head_id}|input_unreadable"),
                        "input_unreadable",
                        json!({"messageId": head_id}),
                    );
                }
            }
        }
        Ok(())
    }

    fn record_deferral(&mut self, message_id: &str, reason: &str) {
        let Some(current) = self.message(message_id) else {
            return;
        };
        if !state_is_open(str_of(&current, "state")) {
            return;
        }
        let (id, reason) = (message_id.to_string(), json!(reason));
        if let Err(error) = self.mutate("recordDeferral", move |core, context| {
            core.record_deferral(context, &id, &reason)
        }) {
            if !self.is_stale_kernel(&error, message_id) {
                self.log("record_deferral_failed", json!({"messageId": message_id}));
            }
        }
    }

    /// Drops once-keys whose condition did not occur this tick and failure counts of messages that left the delivery
    /// queue.
    fn forget(&mut self, agents: &[Value]) {
        let active: HashSet<&str> = agents.iter().map(|a| str_of(a, "agentId")).collect();
        self.steps
            .mail
            .lists
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|id, _| active.contains(id.as_str()));
        self.mail_digest
            .retain(|id, _| active.contains(id.as_str()));
        let seen = &self.seen;
        self.once.retain(|key| seen.contains(key));
        let ids: Vec<String> = self.failures.keys().cloned().collect();
        for id in ids {
            let open = self
                .message(&id)
                .is_some_and(|m| state_is_open(str_of(&m, "state")));
            if !open {
                self.failures.remove(&id);
            }
        }
    }

    fn update_stuck(&mut self) {
        for (message_id, reason) in std::mem::take(&mut self.tick_skips) {
            match self
                .skip_counts
                .iter_mut()
                .find(|(id, _, _)| *id == message_id)
            {
                Some(entry) => {
                    entry.2 = if entry.1 == reason { entry.2 + 1 } else { 1 };
                    entry.1 = reason;
                }
                None => self.skip_counts.push((message_id.clone(), reason, 1)),
            }
            self.skipped_now.insert(message_id);
        }
        let skipped = std::mem::take(&mut self.skipped_now);
        self.skip_counts.retain(|(id, _, _)| skipped.contains(id));
        let mut stuck = Vec::new();
        let entries = self.skip_counts.clone();
        for (message_id, reason, ticks) in entries {
            if ticks < STUCK_AFTER_TICKS
                || reason == SkipReason::NoPane
                || reason == SkipReason::ClearWait
            {
                continue;
            }
            stuck.push((message_id.clone(), reason.as_str().to_string()));
            let key = format!("{message_id}|delivery_stuck|{}", reason.as_str());
            self.seen.insert(key.clone());
            if !self.once.insert(key) {
                continue;
            }
            let Some(message) = self.message(&message_id) else {
                continue;
            };
            let request = NotificationRequest {
                kind: NotificationKind::DeliveryStuck,
                message_id,
                recipient_agent_id: str_of(&message, "recipientAgentId").to_string(),
                repeat: false,
                detail: Some(reason.as_str().to_string()),
            };
            let results = self.notifier.send(&request);
            self.notifier.write(&request, &results, false);
        }
        self.stuck = stuck;
    }
}

fn empty_digest() -> MailDigest {
    MailDigest {
        state_by_id: HashMap::new(),
        latest_pull_ms: f64::NEG_INFINITY,
    }
}

fn kernel_text(error: &KernelError) -> String {
    error.to_string()
}

/// What the driver and the callbacks it hands the adapter share of the kernel: every call is one trip to the kernel
/// thread, counted.
#[derive(Clone)]
pub struct Steps {
    kernel: KernelHandle,
    credential: String,
    calls: Arc<Mutex<BTreeMap<String, u64>>>,
    mail: Arc<OpenMail>,
}

impl Steps {
    pub fn new(kernel: KernelHandle, credential: String) -> Self {
        Self {
            kernel,
            credential,
            calls: Arc::new(Mutex::new(BTreeMap::new())),
            mail: Arc::new(OpenMail::default()),
        }
    }

    /// Counts a use of a `ControllerCore` member (by its Node name), as the parity export's counting proxy does.
    pub fn count(&self, method: &str) {
        *self
            .calls
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(method.to_string())
            .or_insert(0) += 1;
    }

    /// The counts since the last call, by `ControllerCore` member name.
    pub fn take_calls(&self) -> BTreeMap<String, u64> {
        std::mem::take(&mut *self.calls.lock().unwrap_or_else(|p| p.into_inner()))
    }

    /// One trip to the kernel thread, counted as a call of `method`.
    pub fn read<F>(&self, method: &'static str, f: F) -> KernelResult<Value>
    where
        F: FnOnce(&capstan_kernel::Core) -> KernelResult<Value> + Send + 'static,
    {
        self.count(method);
        self.kernel.run(f)
    }

    /// A mutation with a context made at the moment of the call (`newContext(core, credential)`).
    pub fn mutate<F>(&self, method: &'static str, f: F) -> KernelResult<Value>
    where
        F: FnOnce(
                &capstan_kernel::Core,
                &capstan_kernel::types::MutationContext,
            ) -> KernelResult<Value>
            + Send
            + 'static,
    {
        let credential = self.credential.clone();
        // `newContext` reads the two counters, then the method runs.
        self.count("stateVersion");
        self.count("inputRevision");
        self.count(method);
        self.kernel.run(move |core| {
            let context = new_context(core, &credential)?;
            f(core, &context)
        })
    }

    fn message(&self, message_id: &str) -> Option<Value> {
        let id = message_id.to_string();
        match self.read("message", move |core| core.message(&id)) {
            Ok(Value::Null) | Err(_) => None,
            Ok(value) => Some(value),
        }
    }

    fn is_paused(&self, agent_id: &str) -> bool {
        let id = agent_id.to_string();
        matches!(
            self.read("isDeliveryPaused", move |core| core.is_delivery_paused(&id)),
            Ok(Value::Bool(true))
        )
    }

    /// The agent's open messages, read again only when the ledger changed since the last read.
    fn open_messages(&self, agent_id: &str) -> KernelResult<Vec<Value>> {
        let version = self
            .read("stateVersion", |core| core.state_version().map(Value::from))?
            .as_i64()
            .unwrap_or(-1);
        if let Some((cached, messages)) = self
            .mail
            .lists
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(agent_id)
        {
            if *cached == version {
                return Ok(messages.clone());
            }
        }
        let id = agent_id.to_string();
        let messages =
            match self.read("openMessagesFor", move |core| core.open_messages_for(&id))? {
                Value::Array(messages) => messages,
                _ => Vec::new(),
            };
        self.mail
            .lists
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(agent_id.to_string(), (version, messages.clone()));
        Ok(messages)
    }
}
