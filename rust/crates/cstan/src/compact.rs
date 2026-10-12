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
fn agent_line(agent: &Value) -> Value {
    line(format!(
        "{} {} {} gen {} last {}",
        text(agent.get("agentId")),
        text(agent.get("kind")),
        text(agent.get("state")),
        text(agent.get("generation")),
        text(agent.get("lastActivityAt")),
    ))
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
pub fn status(result: &Value, all: bool) -> Value {
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
                let mut lines: Vec<Value> = shown.into_iter().map(agent_line).collect();
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
                        "{} {} {} {} [{}]{}",
                        text(p.get("planId")),
                        text(p.get("tier")),
                        text(p.get("state")),
                        text(p.get("title")),
                        counts(p.get("packages")),
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
        "{} {} {} {}{}",
        text(plan.get("planId")),
        text(plan.get("tier")),
        text(plan.get("state")),
        text(plan.get("title")),
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
