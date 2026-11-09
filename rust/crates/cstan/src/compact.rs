//! The compact text output of `cstan status` and `cstan plan show --brief`. `--json` never goes through here: it prints
//! the daemon's result whole.
use capstan_wire::js::{JsStr, Value};

use crate::jsops::to_js_string;

fn text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "-".to_string(),
        Some(v) => to_js_string(v).to_utf8_lossy(),
    }
}

fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(s) => s.len_units() == 0,
        Value::Array(items) => items.is_empty(),
        Value::Object(members) => members.is_empty(),
        _ => false,
    }
}

fn line(text: String) -> Value {
    Value::String(JsStr::from(text.as_str()))
}

/// One line per agent: `developer-201 Developer active gen 1 last 2026-10-12T10:00:00Z`.
fn agent_line(agent: &Value, now_ms: f64) -> Value {
    let mut shown = format!(
        "{} {} {} gen {} last {}",
        text(agent.get("agentId")),
        text(agent.get("kind")),
        text(agent.get("state")),
        text(agent.get("generation")),
        text(agent.get("lastActivityAt")),
    );
    // The model an agent was started with, only when the daemon knows it.
    if let (Some(model @ Value::String(_)), Some(source @ Value::String(_))) =
        (agent.get("model"), agent.get("modelSource"))
    {
        shown.push_str(&format!(
            " model {} ({})",
            text(Some(model)),
            text(Some(source))
        ));
    }
    shown.push_str(&idle_words(agent, now_ms));
    line(shown)
}

/// The days, hours and minutes of `ms` as `1d 2h 5m` (the zero parts left out; `0m` for less than a minute).
fn duration_words(ms: f64) -> String {
    let minutes = (ms.max(0.0) / 60_000.0).floor() as u64;
    let (days, hours, minutes) = (minutes / 1440, minutes / 60 % 24, minutes % 60);
    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 || parts.is_empty() {
        parts.push(format!("{minutes}m"));
    }
    parts.join(" ")
}

/// Milliseconds since the epoch of an ISO time like `2026-10-12T10:00:00.000Z`, or `None`.
fn iso_ms(value: Option<&Value>) -> Option<f64> {
    let Some(Value::String(s)) = value else {
        return None;
    };
    let s = s.to_utf8_lossy();
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let time = time.trim_end_matches('Z');
    let (clock, frac) = time.split_once('.').unwrap_or((time, "0"));
    let mut t = clock.split(':').map(|p| p.parse::<i64>().ok());
    let (hh, mm, ss) = (t.next()??, t.next()??, t.next()??);
    let millis = format!("{frac:0<3}")[..3].parse::<i64>().ok()?;
    // Days since 1970-01-01 (proleptic Gregorian, Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 86_400 + hh * 3600 + mm * 60 + ss) * 1000 + millis) as f64)
}

/// The idle words of a worker line: ` idle 3h 20m` from 15 minutes on, ` release at <time>` once the PM was warned, and
/// ` kept until <time>` while kept. Nothing for an agent without the members.
fn idle_words(agent: &Value, now_ms: f64) -> String {
    let mut words = String::new();
    if let Some(since) = iso_ms(agent.get("idleSince")) {
        let idle = now_ms - since;
        if idle >= 15.0 * 60_000.0 {
            words.push_str(&format!(" idle {}", duration_words(idle)));
        }
    }
    if matches!(agent.get("warnedAt"), Some(Value::String(_))) {
        if let Some(Value::String(at)) = agent.get("autoReleaseAt") {
            words.push_str(&format!(" release at {}", at.to_utf8_lossy()));
        }
    }
    if let Some(Value::String(until)) = agent.get("keptUntil") {
        words.push_str(&format!(" kept until {}", until.to_utf8_lossy()));
    }
    words
}

/// Whether the default text lists an agent: any state but ended or released, or still holding a pane, or lost or stalled.
fn needs_attention(agent: &Value, status: &Value) -> bool {
    let state = agent.get("state");
    let open = state.is_none_or(|s| {
        !matches!(s, Value::String(t) if matches!(t.to_utf8_lossy().as_str(), "ended" | "released"))
    });
    let id = agent.get("agentId");
    let listed = |key: &str| match status.get(key) {
        Some(Value::Array(ids)) => id.is_some_and(|id| ids.contains(id)),
        _ => false,
    };
    let pane = match status.get("panes") {
        Some(Value::Array(rows)) => {
            id.is_some_and(|id| rows.iter().any(|r| r.get("agentId") == Some(id)))
        }
        _ => false,
    };
    open || pane || listed("lostAgentIds") || listed("stalledAgentIds")
}

/// The status as the text output shows it: the agents that are active or need attention (all of them with `all`), one line each, and none of
/// the members that are empty. Anything that is not an object is left as the daemon sent it.
pub fn status(result: &Value, all: bool, now_ms: f64) -> Value {
    let Value::Object(members) = result else {
        return result.clone();
    };
    let mut out = Vec::new();
    for (key, value) in members {
        if key.to_utf8_lossy() == "agents" {
            if let Value::Array(agents) = value {
                let shown: Vec<&Value> = agents
                    .iter()
                    .filter(|a| all || needs_attention(a, result))
                    .collect();
                let hidden = agents.len() - shown.len();
                let mut lines: Vec<Value> =
                    shown.into_iter().map(|a| agent_line(a, now_ms)).collect();
                if hidden > 0 {
                    lines.push(line(format!(
                        "({hidden} ended agents hidden; cstan status --all lists them)"
                    )));
                }
                if !lines.is_empty() {
                    out.push((key.clone(), Value::Array(lines)));
                }
                continue;
            }
        }
        // The text output prints the verify section as lines of its own.
        if key.to_utf8_lossy() == "verify" {
            continue;
        }
        if !is_empty(value) {
            out.push((key.clone(), value.clone()));
        }
    }
    Value::Object(out)
}

fn counts(value: Option<&Value>) -> String {
    match value {
        Some(Value::Object(members)) if !members.is_empty() => members
            .iter()
            .map(|(k, v)| format!("{} {}", text(Some(v)), k.to_utf8_lossy()))
            .collect::<Vec<_>>()
            .join(", "),
        _ => "no packages".to_string(),
    }
}

/// ` reassigned <from> -> <to> <at>` for a plan that has been handed to another architect.
fn reassigned(plan: &Value) -> String {
    match plan.get("lastReassignment") {
        Some(last @ Value::Object(_)) => format!(
            " reassigned {} -> {} {}",
            text(last.get("fromAgentId")),
            text(last.get("toAgentId")),
            text(last.get("createdAt")),
        ),
        _ => String::new(),
    }
}

fn nexora(entry: &Value) -> String {
    match entry.get("externalId") {
        Some(Value::String(id)) => format!(
            " nexora {}/{}",
            id.to_utf8_lossy(),
            text(entry.get("syncedState"))
        ),
        _ => String::new(),
    }
}

/// The lines of `plan show --brief`: the plans of the project, or one plan with a line per package. `None` when the
/// result is not the brief shape (an older daemon), so the caller prints it as it is.
pub fn plan_brief(result: &Value) -> Option<Vec<String>> {
    if let Some(Value::Array(plans)) = result.get("plans") {
        if plans.is_empty() {
            return Some(vec!["no plans".to_string()]);
        }
        return Some(
            plans
                .iter()
                .map(|p| {
                    format!(
                        "{} {} {} {} [{}]{}{}",
                        text(p.get("planId")),
                        text(p.get("tier")),
                        text(p.get("state")),
                        text(p.get("title")),
                        counts(p.get("packages")),
                        reassigned(p),
                        nexora(p),
                    )
                })
                .collect(),
        );
    }
    let (plan, Some(Value::Array(packages))) = (result.get("plan")?, result.get("packages")) else {
        return None;
    };
    let mut lines = vec![format!(
        "{} {} {} {}{}{}",
        text(plan.get("planId")),
        text(plan.get("tier")),
        text(plan.get("state")),
        text(plan.get("title")),
        reassigned(plan),
        nexora(plan),
    )];
    for p in packages {
        let depends = match p.get("dependsOn") {
            Some(Value::Array(ids)) if !ids.is_empty() => ids
                .iter()
                .map(|d| text(Some(d)))
                .collect::<Vec<_>>()
                .join(","),
            _ => "-".to_string(),
        };
        let estimate = match p.get("estimateHours") {
            Some(Value::Number(_)) => format!("{}h", text(p.get("estimateHours"))),
            _ => "-".to_string(),
        };
        let gate = match p.get("verify") {
            Some(verify) => format!(" {}", crate::render::verify_summary(Some(verify))),
            None => String::new(),
        };
        lines.push(format!(
            "  {} {} {} deps {} {} {}{}{}",
            text(p.get("packageId")),
            text(p.get("type")),
            estimate,
            depends,
            text(p.get("assignee")),
            text(p.get("progress")),
            gate,
            nexora(p),
        ));
    }
    Some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn string(text: &str) -> Value {
        Value::String(JsStr::from(text))
    }

    fn agent(extra: Vec<(&str, Value)>) -> Value {
        let mut members = vec![
            (JsStr::from("agentId"), string("developer-1")),
            (JsStr::from("kind"), string("Developer")),
            (JsStr::from("state"), string("active")),
            (JsStr::from("generation"), Value::Number(1.0)),
            (
                JsStr::from("lastActivityAt"),
                string("2026-10-12T10:00:00Z"),
            ),
        ];
        members.extend(extra.into_iter().map(|(k, v)| (JsStr::from(k), v)));
        Value::Object(members)
    }

    fn rendered(value: &Value) -> String {
        text(Some(value))
    }

    #[test]
    fn an_agent_with_a_model_and_source_gets_the_suffix() {
        let line = agent_line(
            &agent(vec![
                ("model", string("claude-opus-4-8")),
                ("modelSource", string("plan")),
            ]),
            0.0,
        );
        assert_eq!(
            rendered(&line),
            "developer-1 Developer active gen 1 last 2026-10-12T10:00:00Z model claude-opus-4-8 (plan)"
        );
    }

    #[test]
    fn an_agent_without_a_model_keeps_the_line_it_had() {
        let line = agent_line(&agent(vec![]), 0.0);
        assert_eq!(
            rendered(&line),
            "developer-1 Developer active gen 1 last 2026-10-12T10:00:00Z"
        );
    }

    #[test]
    fn idle_words_follow_the_members() {
        let now = iso_ms(Some(&string("2026-10-12T14:20:00.000Z"))).unwrap();
        let idle = agent_line(
            &agent(vec![("idleSince", string("2026-10-12T10:00:00.000Z"))]),
            now,
        );
        assert!(rendered(&idle).ends_with("Z idle 4h 20m"));
        let short = agent_line(
            &agent(vec![("idleSince", string("2026-10-12T14:10:00.000Z"))]),
            now,
        );
        assert!(!rendered(&short).contains("idle"));
        let warned = agent_line(
            &agent(vec![
                ("idleSince", string("2026-10-12T10:00:00.000Z")),
                ("warnedAt", string("2026-10-12T14:10:00.000Z")),
                ("autoReleaseAt", string("2026-10-12T14:30:00.000Z")),
            ]),
            now,
        );
        assert!(rendered(&warned).ends_with("idle 4h 20m release at 2026-10-12T14:30:00.000Z"));
        let kept = agent_line(
            &agent(vec![("keptUntil", string("2026-10-13T09:00:00.000Z"))]),
            now,
        );
        assert!(rendered(&kept).ends_with("kept until 2026-10-13T09:00:00.000Z"));
    }
}
