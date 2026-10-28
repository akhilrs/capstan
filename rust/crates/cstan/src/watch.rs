//! `cstan status --watch` (`src/watch.ts`): a compact status block that rings the bell when the operator has something new
//! to see.
use std::collections::BTreeSet;
use std::sync::LazyLock;
use std::time::Duration;

use capstan_kernel::helpers::UNSAFE_TEXT_PATTERN;
use capstan_wire::js::{self, Value};
use regex::Regex;

use crate::jsops::to_js_string;
use crate::operator::Operator;
use crate::{take_flag, take_interval_seconds, Cli, Fail, Flow, WatchPlan};

const BELL: &str = "\u{7}";

static UNSAFE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(UNSAFE_TEXT_PATTERN).expect("the pattern is valid"));

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

/// The designed window strings (docs/design/plan-44-d1-design-strings.json), looked up by id.
const WINDOW_STRINGS: &str = include_str!("../../../../docs/design/plan-44-d1-design-strings.json");

fn window_template(id: &str) -> String {
    let table: serde_json::Value =
        serde_json::from_str(WINDOW_STRINGS).expect("window strings table is valid JSON");
    table
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["id"] == id))
        .and_then(|row| row["template"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// The auto-approve window line of `cstan status` and `cstan watch`: minutes left and who is covered, or, with
/// `none_line`, the line that says no window is open. `None` when there is nothing to print.
pub fn window_line(operator: Option<&Value>, none_line: bool) -> Option<String> {
    let Some(operator @ Value::Object(_)) = operator else {
        return None;
    };
    let Some(window @ Value::Object(_)) = field(operator, "window") else {
        return none_line.then(|| window_template("status.window.none"));
    };
    let id = if matches!(field(window, "operatorActive"), Some(Value::Bool(true))) {
        "line.window.open"
    } else {
        "line.window.operatorEnded"
    };
    let remaining = crate::jsops::to_number(field(window, "remainingSeconds"));
    Some(
        window_template(id)
            .replace("{left}", &js::number_to_string((remaining / 60.0).ceil()))
            .replace("{windowId}", &clean(field(window, "windowId"))),
    )
}

/// Cuts `line` to `width` cells the way the dash does: keep `width - 1` characters, then `…`.
pub fn cut_to_width(line: &str, width: usize) -> String {
    if line.chars().count() <= width {
        return line.to_string();
    }
    let kept: String = line.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
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
    lines.extend(window_line(Some(operator), false));
    for grant in list(field(operator, "grants")) {
        if clean(field(grant, "kind")) == "timed" {
            let remaining = crate::jsops::to_number(field(grant, "remainingSeconds"));
            lines.push(format!(
                "grant {} (timed) \"{}\" {} min left",
                clean(field(grant, "grantId")),
                clean(field(grant, "text")),
                js::number_to_string((remaining / 60.0).ceil())
            ));
            continue;
        }
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

#[cfg(test)]
mod window_tests {
    use super::*;

    fn operator(json: &str) -> Value {
        crate::to_js(&serde_json::from_str(json).unwrap())
    }

    #[test]
    fn the_window_line_rounds_minutes_up_and_names_who_is_covered() {
        let open = operator(
            r#"{"window":{"windowId":"win-12","operatorActive":true,"remainingSeconds":45}}"#,
        );
        assert_eq!(
            window_line(Some(&open), false).unwrap(),
            "AUTO-APPROVE 1 min left, Operator and workers (win-12)"
        );
        let ended = operator(
            r#"{"window":{"windowId":"win-12","operatorActive":false,"remainingSeconds":2520}}"#,
        );
        assert_eq!(
            window_line(Some(&ended), true).unwrap(),
            "AUTO-APPROVE 42 min left, workers only; Operator ended (win-12)"
        );
    }

    #[test]
    fn no_window_is_a_line_for_status_and_nothing_for_watch() {
        let none = operator(r#"{"grants":[]}"#);
        assert_eq!(
            window_line(Some(&none), true).unwrap(),
            "AUTO-APPROVE off: no window open"
        );
        assert_eq!(window_line(Some(&none), false), None);
        assert_eq!(window_line(None, true), None);
    }

    #[test]
    fn the_window_line_follows_full_auto_and_precedes_grants() {
        let op = operator(
            r#"{"fullAuto":{"on":true,"remainingSeconds":720},"window":{"windowId":"win-12","operatorActive":true,"remainingSeconds":2520},"grants":[{"grantId":"g-7","kind":"timed","text":"cargo test","remainingSeconds":1080}]}"#,
        );
        let lines = operator_lines(Some(&op));
        assert!(lines[0].starts_with("FULL AUTO ON"));
        assert!(lines[1].starts_with("AUTO-APPROVE 42 min left"));
        assert!(lines[2].starts_with("grant g-7"));
    }

    #[test]
    fn minutes_round_up_at_the_minute_edges() {
        for (seconds, minutes) in [(1, 1), (60, 1), (61, 2)] {
            let op = operator(&format!(
                r#"{{"window":{{"windowId":"win-1","operatorActive":true,"remainingSeconds":{seconds}}}}}"#
            ));
            let line = window_line(Some(&op), false).unwrap();
            assert!(
                line.starts_with(&format!("AUTO-APPROVE {minutes} min left")),
                "{line}"
            );
        }
    }

    #[test]
    fn the_line_is_cut_at_forty_columns_keeping_the_state_words() {
        let open = operator(
            r#"{"window":{"windowId":"win-12","operatorActive":true,"remainingSeconds":2520}}"#,
        );
        let ended = operator(
            r#"{"window":{"windowId":"win-12","operatorActive":false,"remainingSeconds":2520}}"#,
        );
        assert_eq!(
            cut_to_width(&window_line(Some(&open), false).unwrap(), 40),
            "AUTO-APPROVE 42 min left, Operator and …"
        );
        assert_eq!(
            cut_to_width(&window_line(Some(&ended), false).unwrap(), 40),
            "AUTO-APPROVE 42 min left, workers only;…"
        );
        assert_eq!(
            cut_to_width(&window_template("status.window.none"), 40),
            "AUTO-APPROVE off: no window open"
        );
    }

    #[test]
    fn every_window_string_says_auto_approve_and_never_full_auto() {
        let table: serde_json::Value = serde_json::from_str(WINDOW_STRINGS).unwrap();
        for row in table.as_array().unwrap() {
            let template = row["template"].as_str().unwrap();
            assert!(template.starts_with("AUTO-APPROVE"), "{template}");
            assert!(!template.to_lowercase().contains("full auto"), "{template}");
        }
    }
}
