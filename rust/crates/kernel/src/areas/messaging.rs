//! The message state machine (src/controller/messaging.ts): pure functions over message and agent facts. The foundation
//! keeps only the constants it needs in `records.rs` (`FINAL_MESSAGE_STATES`, `UNRESOLVED_MESSAGE_STATES`, `TIMER_NAMES`).

use crate::records::{is_final_state, is_unresolved_state, MessageRow};
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// The state machine has no operation of its own; nothing is dispatched by name.
pub(crate) fn dispatch(
    _kernel: &crate::kernel::Kernel,
    _op: &str,
    _args: &[serde_json::Value],
) -> Option<crate::errors::KernelResult<serde_json::Value>> {
    None
}

pub const DEFERRAL_REASONS: [&str; 3] = ["agent_busy", "agent_blocked", "input_not_empty"];
pub const RESOLUTION_DECISIONS: [&str; 3] = ["retry", "skip", "cancel"];

/// The longest wait between two wake lines for one unread message.
pub const PM_WAKE_MAX_INTERVAL_SECONDS: f64 = 1800.0;

/// `isLegalTransition`.
pub fn is_legal_transition(from: &str, to: &str) -> bool {
    let allowed: &[&str] = match from {
        "queued" => &["deferred", "sent", "failed", "cancelled"],
        "deferred" => &["sent", "expired", "failed", "cancelled"],
        "sent" => &["acked", "unacked", "cancelled"],
        "unacked" => &["acked_late", "cancelled"],
        "expired" | "failed" => &["cancelled"],
        _ => &[],
    };
    allowed.contains(&to)
}

/// `resolutionTarget`.
pub fn resolution_target(decision: &str, from: &str) -> Option<&'static str> {
    if decision == "retry" {
        return is_unresolved_state(from).then_some("queued");
    }
    (is_unresolved_state(from) || from == "queued").then_some("cancelled")
}

/// `queueHead`: the first message (by sequence) that is not final; the earliest of equal sequences wins.
pub fn queue_head(messages: &[MessageRow]) -> Option<&MessageRow> {
    let mut head: Option<&MessageRow> = None;
    for message in messages {
        if !is_final_state(&message.state) && head.is_none_or(|h| message.sequence < h.sequence) {
            head = Some(message);
        }
    }
    head
}

/// `Date.parse` of the ISO strings the controller stores (`YYYY-MM-DDTHH:MM:SS.mmmZ`).
pub fn parse_iso_ms(text: &str) -> i64 {
    let number = |range: std::ops::Range<usize>| -> i64 {
        text.get(range)
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0)
    };
    let (year, month, day) = (number(0..4), number(5..7), number(8..10));
    let (hour, minute, second, milli) = (
        number(11..13),
        number(14..16),
        number(17..19),
        number(20..23),
    );
    // Days from civil (Howard Hinnant).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    ((days * 24 + hour) * 60 + minute) * 60_000 + second * 1000 + milli
}

#[derive(Clone, Debug)]
pub struct StateObservation {
    pub state: String,
    pub at_ms: i64,
}

#[derive(Clone, Debug)]
pub struct WaitInterval {
    pub start_ms: i64,
    pub end_ms: Option<i64>,
}

fn state_at(observations: &[StateObservation], at_ms: i64) -> &str {
    let mut state = "unknown";
    for observation in observations {
        if observation.at_ms <= at_ms {
            state = &observation.state;
        } else {
            break;
        }
    }
    state
}

fn in_wait(waits: &[WaitInterval], at_ms: i64) -> bool {
    waits
        .iter()
        .any(|w| w.start_ms <= at_ms && w.end_ms.is_none_or(|end| end > at_ms))
}

/// `countedMillis`: milliseconds in [from, to) that count for a timer; `rule` is "ack" or "stall".
pub fn counted_millis(
    rule: &str,
    from_ms: i64,
    to_ms: i64,
    observations: &[StateObservation],
    waits: &[WaitInterval],
) -> i64 {
    if to_ms <= from_ms {
        return 0;
    }
    let mut cuts = BTreeSet::from([from_ms, to_ms]);
    for observation in observations {
        if observation.at_ms > from_ms && observation.at_ms < to_ms {
            cuts.insert(observation.at_ms);
        }
    }
    for wait in waits {
        if wait.start_ms > from_ms && wait.start_ms < to_ms {
            cuts.insert(wait.start_ms);
        }
        if let Some(end) = wait.end_ms {
            if end > from_ms && end < to_ms {
                cuts.insert(end);
            }
        }
    }
    let points: Vec<i64> = cuts.into_iter().collect();
    let mut total = 0;
    for pair in points.windows(2) {
        let start = pair[0];
        let working = state_at(observations, start) == "working";
        let waiting = in_wait(waits, start);
        let counts = if rule == "ack" {
            !working || waiting
        } else {
            working && !waiting
        };
        if counts {
            total += pair[1] - start;
        }
    }
    total
}

/// The nine timers, in seconds (`MessagingTimers`).
#[derive(Clone, Debug)]
pub struct MessagingTimers {
    pub max_deferral_seconds: f64,
    pub max_busy_deferral_seconds: f64,
    pub pm_ack_timeout_seconds: f64,
    pub pm_notify_after_seconds: f64,
    pub notify_interval_seconds: f64,
    pub stall_after_seconds: f64,
    pub worker_ack_timeout_seconds: f64,
    pub pm_wake_after_seconds: f64,
    pub pm_wake_interval_seconds: f64,
}

impl MessagingTimers {
    /// Reads timers `assert_timers` has already accepted.
    pub fn from_value(timers: &Value) -> Self {
        let get = |name: &str| timers.get(name).and_then(Value::as_f64).unwrap_or(0.0);
        Self {
            max_deferral_seconds: get("maxDeferralSeconds"),
            max_busy_deferral_seconds: get("maxBusyDeferralSeconds"),
            pm_ack_timeout_seconds: get("pmAckTimeoutSeconds"),
            pm_notify_after_seconds: get("pmNotifyAfterSeconds"),
            notify_interval_seconds: get("notifyIntervalSeconds"),
            stall_after_seconds: get("stallAfterSeconds"),
            worker_ack_timeout_seconds: get("workerAckTimeoutSeconds"),
            pm_wake_after_seconds: get("pmWakeAfterSeconds"),
            pm_wake_interval_seconds: get("pmWakeIntervalSeconds"),
        }
    }
}

/// `pmWakeBackoffSeconds`: the interval doubles per wake, capped.
pub fn pm_wake_backoff_seconds(interval_seconds: f64, wake_count: i64) -> f64 {
    let doublings = (wake_count - 1).clamp(0, 30);
    (interval_seconds * 2f64.powi(doublings as i32)).min(PM_WAKE_MAX_INTERVAL_SECONDS)
}

#[derive(Clone, Debug)]
pub struct AgentFacts {
    pub agent_id: String,
    pub kind: String,
    pub last_activity_ms: i64,
    pub observations: Vec<StateObservation>,
    pub waits: Vec<WaitInterval>,
    pub paused: bool,
}

#[derive(Clone, Debug)]
pub struct MessageFacts {
    pub message_id: String,
    pub recipient_agent_id: String,
    pub state: String,
    pub sequence: i64,
    pub queued_ms: i64,
    pub sent_ms: Option<i64>,
    pub deferred_ms: Option<i64>,
    pub deferred_reason: Option<String>,
    pub input_clear_recorded: bool,
    pub last_notified_ms: Option<i64>,
    pub wake_count: i64,
    pub last_wake_ms: Option<i64>,
}

/// A transition the timers make due: `to` is `unacked` or `expired`.
#[derive(Clone, Debug)]
pub struct DueTransition {
    pub message_id: String,
    pub to: &'static str,
}

#[derive(Clone, Debug)]
pub struct MessagingEvaluation {
    pub transitions: Vec<DueTransition>,
    pub actions: Vec<Value>,
    pub stalled_agent_ids: Vec<String>,
    pub attention: Vec<Value>,
}

impl MessagingEvaluation {
    /// The evaluation as `advance()` reads it.
    pub fn to_value(&self) -> Value {
        json!({
            "actions": self.actions,
            "stalledAgentIds": self.stalled_agent_ids,
            "attention": self.attention,
        })
    }
}

/// When the agent's current Herdr state began: the earliest observation of an unbroken run of that state up to `at_ms`.
fn state_since(observations: &[StateObservation], at_ms: i64) -> Option<i64> {
    let current = state_at(observations, at_ms);
    let mut since: Option<i64> = None;
    for observation in observations {
        if observation.at_ms > at_ms {
            break;
        }
        if observation.state == current {
            since.get_or_insert(observation.at_ms);
        } else {
            since = None;
        }
    }
    since
}

/// `evaluateMessaging`: pure evaluation of every timer rule.
pub fn evaluate_messaging(
    agents: &[AgentFacts],
    messages: &[MessageFacts],
    now_ms: i64,
    timers: &MessagingTimers,
) -> MessagingEvaluation {
    let mut transitions = Vec::new();
    let mut actions = Vec::new();
    let mut stalled_agent_ids = Vec::new();
    let mut attention = Vec::new();
    let ms = |seconds: f64| seconds * 1000.0;
    for agent in agents {
        let own: Vec<&MessageFacts> = messages
            .iter()
            .filter(|m| m.recipient_agent_id == agent.agent_id)
            .collect();
        // `queueHead` over the facts.
        let mut head: Option<&MessageFacts> = None;
        for message in &own {
            if !is_final_state(&message.state) && head.is_none_or(|h| message.sequence < h.sequence)
            {
                head = Some(message);
            }
        }
        for message in &own {
            if message.state == "sent" {
                if let Some(sent_ms) = message.sent_ms {
                    let due = if agent.kind == "PM" {
                        counted_millis("ack", sent_ms, now_ms, &agent.observations, &agent.waits)
                            as f64
                            >= ms(timers.pm_ack_timeout_seconds)
                    } else {
                        (now_ms - sent_ms) as f64 >= ms(timers.worker_ack_timeout_seconds)
                    };
                    if due {
                        transitions.push(DueTransition {
                            message_id: message.message_id.clone(),
                            to: "unacked",
                        });
                    }
                }
            }
        }
        if !agent.paused {
            if let Some(h) = head {
                if h.state == "deferred" {
                    if let Some(deferred_ms) = h.deferred_ms {
                        if h.deferred_reason.as_deref() == Some("input_not_empty") {
                            if (now_ms - deferred_ms) as f64 >= ms(timers.max_deferral_seconds) {
                                actions.push(json!({
                                    "kind": "clear_then_send",
                                    "messageId": h.message_id,
                                    "notifyOperator": !h.input_clear_recorded,
                                }));
                            }
                        } else if (now_ms - deferred_ms) as f64
                            >= ms(timers.max_busy_deferral_seconds)
                        {
                            transitions.push(DueTransition {
                                message_id: h.message_id.clone(),
                                to: "expired",
                            });
                        }
                    }
                }
            }
        }
        let mut pending: Option<&MessageFacts> = None;
        for message in &own {
            if matches!(message.state.as_str(), "queued" | "sent" | "unacked")
                && pending.is_none_or(|p| message.sequence < p.sequence)
            {
                pending = Some(message);
            }
        }
        if !agent.paused && agent.kind == "PM" {
            if let Some(pending) = pending {
                let counted = counted_millis(
                    "ack",
                    pending.queued_ms,
                    now_ms,
                    &agent.observations,
                    &agent.waits,
                );
                if counted as f64 >= ms(timers.pm_notify_after_seconds) {
                    match pending.last_notified_ms {
                        None => actions.push(json!({
                            "kind": "notify_operator",
                            "messageId": pending.message_id,
                            "repeat": false,
                        })),
                        Some(last)
                            if (now_ms - last) as f64 >= ms(timers.notify_interval_seconds) =>
                        {
                            actions.push(json!({
                                "kind": "notify_operator",
                                "messageId": pending.message_id,
                                "repeat": true,
                            }))
                        }
                        Some(_) => {}
                    }
                }
            }
        }
        // The wake target is the oldest unread message, whatever state the head is in.
        let mut wake_target: Option<&MessageFacts> = None;
        for message in &own {
            if message.state == "queued"
                && wake_target.is_none_or(|w| message.sequence < w.sequence)
            {
                wake_target = Some(message);
            }
        }
        if !agent.paused && agent.kind == "PM" && timers.pm_wake_after_seconds > 0.0 {
            if let Some(target) = wake_target {
                if (now_ms - target.queued_ms) as f64 >= ms(timers.pm_wake_after_seconds)
                    && target.last_wake_ms.is_none_or(|last| {
                        (now_ms - last) as f64
                            >= ms(pm_wake_backoff_seconds(
                                timers.pm_wake_interval_seconds,
                                target.wake_count,
                            ))
                    })
                {
                    let state = state_at(&agent.observations, now_ms);
                    if state == "idle" || state == "done" {
                        actions.push(json!({"kind": "wake_pm", "messageId": target.message_id}));
                    }
                }
            }
        }
        if agent.paused {
            continue;
        }
        if agent.kind != "PM"
            && agent.kind != "Supervisor"
            && state_at(&agent.observations, now_ms) == "blocked"
        {
            if let Some(since) = state_since(&agent.observations, now_ms) {
                if (now_ms - since) as f64 >= ms(timers.stall_after_seconds) {
                    attention.push(json!({
                        "agentId": agent.agent_id,
                        "kind": "blocked",
                        "episodeMs": agent.last_activity_ms,
                    }));
                }
            }
        }
        if state_at(&agent.observations, now_ms) == "working" {
            let stalled = counted_millis(
                "stall",
                agent.last_activity_ms,
                now_ms,
                &agent.observations,
                &agent.waits,
            ) as f64
                >= ms(timers.stall_after_seconds);
            if stalled {
                stalled_agent_ids.push(agent.agent_id.clone());
                if agent.kind != "PM" && agent.kind != "Supervisor" {
                    attention.push(json!({
                        "agentId": agent.agent_id,
                        "kind": "stalled",
                        "episodeMs": agent.last_activity_ms,
                    }));
                }
            }
        }
    }
    MessagingEvaluation {
        transitions,
        actions,
        stalled_agent_ids,
        attention,
    }
}
