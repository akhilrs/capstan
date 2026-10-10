//! `cstan status --watch` (`src/watch.ts`): a compact status block that rings the bell when the operator has something new
//! to see.
use std::collections::BTreeSet;
use std::sync::LazyLock;
use std::time::Duration;

use capstan_wire::js::{self, Value};
use regex::Regex;

use crate::jsops::to_js_string;
use crate::operator::Operator;
use crate::{take_flag, take_interval_seconds, Cli, Fail, Flow, WatchPlan};

const BELL: &str = "\u{7}";

static UNSAFE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]").expect("the pattern is valid"));

/// `clean`: everything from the daemon is shown without control or format characters.
pub fn clean(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(v) => UNSAFE
            .replace_all(&to_js_string(v).to_utf8_lossy(), " ")
            .into_owned(),
    }
}

fn list(value: Option<&Value>) -> &[Value] {
    match value {
        Some(Value::Array(items)) => items,
        _ => &[],
    }
}

fn field<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.get(key)
}

fn is_string(value: Option<&Value>, text: &str) -> bool {
    matches!(value, Some(Value::String(s)) if s.to_utf8_lossy() == text)
}

fn s(value: Option<&Value>) -> String {
    value.map_or_else(|| "undefined".into(), |v| to_js_string(v).to_utf8_lossy())
}

/// `signalsOf`: keys of everything that should ring once: a new or changed notification, a new clear, a new stuck entry.
pub fn signals_of(status: &Value) -> BTreeSet<String> {
    let mut signals = BTreeSet::new();
    for message in list(field(status, "messages")) {
        let notified = field(message, "lastNotifiedAt");
        if !matches!(notified, None | Some(Value::Null)) {
            signals.insert(format!(
                "notified:{}:{}",
                s(field(message, "messageId")),
                s(notified)
            ));
        }
    }
    for clear in list(field(status, "inputClears")) {
        signals.insert(format!("clear:{}", s(field(clear, "clearId"))));
    }
    for stuck in list(field(status, "stuck")) {
        signals.insert(format!(
            "stuck:{}:{}",
            s(field(stuck, "messageId")),
            s(field(stuck, "reason"))
        ));
    }
    for id in list(field(status, "lostAgentIds")) {
        signals.insert(format!("lost:{}", s(Some(id))));
    }
    for finding in list(field(status, "agentFindings")) {
        if is_string(field(finding, "state"), "escalated") {
            signals.insert(format!(
                "finding-escalated:{}",
                s(field(finding, "findingId"))
            ));
        }
    }
    signals
}

/// `operatorLines`: full auto and the session grants in force; nothing when the Operator is off.
pub fn operator_lines(operator: Option<&Value>) -> Vec<String> {
    let Some(operator @ Value::Object(_)) = operator else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    let full_auto = field(operator, "fullAuto");
    if matches!(full_auto.and_then(|f| f.get("on")), Some(Value::Bool(true))) {
        let remaining = match full_auto.and_then(|f| f.get("remainingSeconds")) {
            Some(Value::Number(n)) => *n,
            None => 0.0,
            Some(other) => crate::jsops::to_number(Some(other)),
        };
        lines.push(format!(
            "FULL AUTO ON: {} minutes left",
            js::number_to_string((remaining / 60.0).ceil())
        ));
    }
    for grant in list(field(operator, "grants")) {
        lines.push(format!(
            "grant {} ({}) \"{}\" until {}",
            clean(field(grant, "grantId")),
            clean(field(grant, "kind")),
            clean(field(grant, "text")),
            clean(field(grant, "expiresAt"))
        ));
    }
    lines
}

/// `renderWatch`.
pub fn render_watch(status: &Value) -> String {
    let mut lines = Vec::new();
    let agents: Vec<String> = list(field(status, "agents"))
        .iter()
        .map(|a| {
            format!(
                "{} ({}, {})",
                clean(field(a, "agentId")),
                clean(field(a, "kind")),
                clean(field(a, "state"))
            )
        })
        .collect();
    lines.push(format!(
        "agents: {}",
        if agents.is_empty() {
            "none".to_string()
        } else {
            agents.join(", ")
        }
    ));
    let messages = list(field(status, "messages"));
    if messages.is_empty() {
        lines.push("messages: none unresolved".into());
    }
    if matches!(field(status, "messagesTruncated"), Some(Value::Bool(true))) {
        lines.push(format!(
            "(only the first {} unresolved messages are shown)",
            messages.len()
        ));
    }
    let clears = list(field(status, "inputClears")).len();
    if clears > 0 {
        lines.push(format!("input clears recorded: {clears}"));
    }
    for m in messages {
        let notified = match field(m, "lastNotifiedAt") {
            None | Some(Value::Null) => "no".to_string(),
            other => clean(other),
        };
        lines.push(format!(
            "message {} -> {} [{}] notified: {}",
            clean(field(m, "messageId")),
            clean(field(m, "recipientAgentId")),
            clean(field(m, "state")),
            notified
        ));
    }
    for stuck in list(field(status, "stuck")) {
        lines.push(format!(
            "stuck {}: {}",
            clean(field(stuck, "messageId")),
            clean(field(stuck, "reason"))
        ));
    }
    for f in list(field(status, "agentFindings")) {
        let open = is_string(field(f, "state"), "open");
        let escalated = is_string(field(f, "state"), "escalated");
        if open || escalated {
            let reason = match field(f, "stateReason") {
                None | Some(Value::Null) => String::new(),
                other => format!(", {}", clean(other)),
            };
            lines.push(format!(
                "finding {} on {} ({}) [{}, intervention {} of 2{}]{}",
                clean(field(f, "findingId")),
                clean(field(f, "targetAgentId")),
                clean(field(f, "severity")),
                clean(field(f, "state")),
                clean(field(f, "interventions")),
                reason,
                if escalated { " needs the operator" } else { "" }
            ));
        }
    }
    lines.extend(operator_lines(field(status, "operator")));
    let lost = list(field(status, "lostAgentIds"));
    if !lost.is_empty() {
        let names: Vec<String> = lost.iter().map(|v| clean(Some(v))).collect();
        lines.push(format!("lost: {}", names.join(", ")));
    }
    let stalled = list(field(status, "stalledAgentIds"));
    if !stalled.is_empty() {
        let names: Vec<String> = stalled.iter().map(|v| clean(Some(v))).collect();
        lines.push(format!("stalled: {}", names.join(", ")));
    }
    lines.join("\n")
}

/// `watchStatus`: polls until the daemon stops answering or the iterations run out; the first poll is a baseline and
/// never rings.
pub fn watch_status(
    mut fetch: impl FnMut() -> Option<Value>,
    mut write: impl FnMut(&str),
    mut sleep: impl FnMut(Duration),
    interval: Duration,
    iterations: Option<usize>,
) {
    let mut seen: Option<BTreeSet<String>> = None;
    let mut poll = 0usize;
    while iterations.is_none_or(|n| poll < n) {
        if poll > 0 {
            sleep(interval);
        }
        poll += 1;
        let Some(status) = fetch() else {
            write("the controller stopped answering\n");
            return;
        };
        let current = signals_of(&status);
        let ring = seen
            .as_ref()
            .is_some_and(|seen| current.iter().any(|key| !seen.contains(key)));
        write(&format!(
            "{}{}\n---\n",
            if ring { BELL } else { "" },
            render_watch(&status)
        ));
        seen = Some(current);
    }
}

/// The status of the daemon at `plan`, for the watch loop.
pub fn fetch_status(plan: &WatchPlan) -> Option<Value> {
    match capstan_client::call_daemon(&plan.socket, &plan.credential, "status", &[], 5_000) {
        Ok(capstan_wire::Response::Ok { result }) => Some(result.unwrap_or(Value::Null)),
        _ => None,
    }
}

impl Cli<'_> {
    /// `cstan status --watch [--interval <seconds>]`: the daemon is started if needed, then the process polls it.
    pub fn status_watch(&mut self, rest: &[String]) -> Result<Flow, Fail> {
        let mut flags = rest.to_vec();
        take_flag(&mut flags, "--watch");
        let interval_seconds = take_interval_seconds(&mut flags)?;
        if !flags.is_empty() {
            return Err(Fail::Usage);
        }
        let Operator {
            socket_path,
            credential,
            ..
        } = self.ensure_running()?;
        Ok(Flow::Watch(WatchPlan {
            socket: socket_path,
            credential,
            interval_seconds,
        }))
    }
}
