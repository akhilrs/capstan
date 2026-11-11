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
    // A worker of a workspace project says how many repos it holds worktrees in.
    if let Some(Value::Array(trees)) = agent.get("worktrees") {
        if !trees.is_empty() {
            shown.push_str(&format!(" · {} repos", trees.len()));
        }
    }
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
        // A set is one line (and a few dim ones) whatever its repo count; its members' own rows drop the `set` member.
        if key.to_utf8_lossy() == "integrationSets" {
            if let Value::Array(sets) = value {
                let ascii = term_is_dumb();
                let lines: Vec<Value> = sets
                    .iter()
                    .flat_map(|set| set_lines(set, ascii))
                    .map(line)
                    .collect();
                if !lines.is_empty() {
                    out.push((key.clone(), Value::Array(lines)));
                }
                continue;
            }
        }
        if key.to_utf8_lossy() == "integrations" {
            if let Value::Array(rows) = value {
                if !rows.is_empty() {
                    out.push((
                        key.clone(),
                        Value::Array(rows.iter().map(without_set).collect()),
                    ));
                }
                continue;
            }
        }
        // A report that named commits in several repos is one line; any other report is shown as it always was.
        if key.to_utf8_lossy() == "reports" {
            if let Value::Array(rows) = value {
                if !rows.is_empty() {
                    out.push((
                        key.clone(),
                        Value::Array(
                            rows.iter()
                                .map(|r| report_commit_line(r).map_or_else(|| r.clone(), line))
                                .collect(),
                        ),
                    ));
                }
                continue;
            }
        }
        if !is_empty(value) {
            out.push((key.clone(), value.clone()));
        }
    }
    Value::Object(out)
}

fn term_is_dumb() -> bool {
    std::env::var("TERM").is_ok_and(|term| term == "dumb")
}

fn without_set(row: &Value) -> Value {
    match row {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(k, _)| k.to_utf8_lossy() != "set")
                .cloned()
                .collect(),
        ),
        other => other.clone(),
    }
}

fn flag(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Bool(true)))
}

fn short_sha(value: Option<&Value>) -> String {
    text(value).chars().take(7).collect()
}

/// The report line of a report with commits in several repos: `<id> <agent> accepted 3 repos  api 9f3c2a1 · web 51ab07e  on
/// <branch>`. `None` for a report without `commits`.
fn report_commit_line(report: &Value) -> Option<String> {
    let Some(Value::Array(commits)) = report.get("commits") else {
        return None;
    };
    let first = commits.first()?;
    let named: Vec<String> = commits
        .iter()
        .map(|c| format!("{} {}", text(c.get("repo")), short_sha(c.get("commitSha"))))
        .collect();
    Some(format!(
        "{} {} {} {} repos  {}  on {}",
        text(report.get("reportId")),
        text(report.get("agentId")),
        text(report.get("state")),
        commits.len(),
        named.join(" · "),
        text(first.get("branch")),
    ))
}

/// The word of a member's state: the one vocabulary of every surface (the feed's `merged` reads `built`).
fn member_word(state: &str, dropped: bool) -> &'static str {
    if dropped {
        return "dropped";
    }
    match state {
        "running" => "building",
        "merged" => "built",
        "confirmed" => "in main",
        "conflicted" => "conflicted",
        "failed" => "failed",
        "discarded" => "discarded",
        _ => "building",
    }
}

fn member_glyph(state: &str, dropped: bool, ascii: bool) -> &'static str {
    let (glyph, plain) = match member_word(state, dropped) {
        "building" => ("◌", "."),
        "built" => ("○", "o"),
        "in main" => ("●", "#"),
        "conflicted" => ("▲", "!"),
        "failed" => ("✕", "x"),
        // dropped and discarded
        _ => ("–", "-"),
    };
    if ascii {
        plain
    } else {
        glyph
    }
}

/// A branch longer than 32 characters is cut in the middle (15 and 16 characters around an ellipsis).
fn cut_branch(branch: &str) -> String {
    let chars: Vec<char> = branch.chars().collect();
    if chars.len() <= 32 {
        return branch.to_string();
    }
    let head: String = chars[..15].iter().collect();
    let tail: String = chars[chars.len() - 16..].iter().collect();
    format!("{head}…{tail}")
}

/// The most glyphs a strip shows before `+n`.
const STRIP_GLYPHS: usize = 12;

/// The lines of one integration set of the status: `set <id> <branch>  <strip>  <summary>`, then dim lines only for the
/// members that need a word (review subject and dropped repos; a conflicted member with the command that redoes it; a
/// failed one with its attempt). The line count does not grow with the repo count.
pub fn set_lines(set: &Value, ascii: bool) -> Vec<String> {
    let set_id = text(set.get("setId"));
    let members: &[Value] = match set.get("members") {
        Some(Value::Array(members)) => members,
        _ => &[],
    };
    let state_of = |m: &Value| text(m.get("state"));
    let dropped_of = |m: &Value| flag(m.get("dropped"));
    let live: Vec<&Value> = members.iter().filter(|m| !dropped_of(m)).collect();
    let in_main = live.iter().filter(|m| state_of(m) == "confirmed").count();
    let built = live
        .iter()
        .filter(|m| matches!(state_of(m).as_str(), "merged" | "confirmed"))
        .count();
    let first_in = |state: &str| {
        live.iter()
            .find(|m| state_of(m) == state)
            .map(|m| text(m.get("repo")))
    };
    let next = live
        .iter()
        .find(|m| state_of(m) != "confirmed")
        .map(|m| text(m.get("repo")));
    let complete = !live.is_empty() && built == live.len();
    let summary = if text(set.get("state")) == "discarded" {
        if in_main == 0 {
            "discarded · nothing reached main".to_string()
        } else {
            format!("discarded · {in_main} of {} stay in main", live.len())
        }
    } else if let Some(repo) = first_in("conflicted") {
        format!("conflicted in {repo}")
    } else if let Some(repo) = first_in("failed") {
        format!("failed in {repo}")
    } else if complete && in_main == live.len() {
        format!("in main · all {} repos", live.len())
    } else {
        let next_part = next.map(|n| format!(" · next: {n}")).unwrap_or_default();
        if in_main > 0 {
            format!("{in_main} of {} in main{next_part}", live.len())
        } else if complete {
            format!("built · none in main yet{next_part}")
        } else {
            format!("building · {built} of {} built", live.len())
        }
    };
    // The strip: one glyph per member in merge order; past the cap `+n`, and it starts two before the next repo when that
    // one is beyond the first glyphs.
    let next_index = members
        .iter()
        .position(|m| !dropped_of(m) && state_of(m) != "confirmed");
    let start = match next_index {
        Some(index) if index >= STRIP_GLYPHS => index - 2,
        _ => 0,
    };
    let end = (start + STRIP_GLYPHS).min(members.len());
    let mut strip = String::new();
    if start > 0 {
        strip.push_str(&format!("+{start} "));
    }
    for member in &members[start..end] {
        strip.push_str(member_glyph(&state_of(member), dropped_of(member), ascii));
    }
    if end < members.len() {
        strip.push_str(&format!(" +{}", members.len() - end));
    }
    let mut lines = vec![format!(
        "set {set_id} {}  {strip}  {summary}",
        cut_branch(&text(set.get("branch")))
    )];
    let mut notes = Vec::new();
    if complete {
        if let Some(subject) = live
            .iter()
            .find(|m| !matches!(m.get("integrationId"), None | Some(Value::Null)))
        {
            notes.push(format!("review subject: {}", text(subject.get("repo"))));
        }
    }
    let dropped: Vec<String> = members
        .iter()
        .filter(|m| dropped_of(m))
        .map(|m| text(m.get("repo")))
        .collect();
    if !dropped.is_empty() {
        notes.push(format!("dropped: {}", dropped.join(",")));
    }
    if !notes.is_empty() {
        lines.push(format!("    {}", notes.join(" · ")));
    }
    for member in &live {
        let repo = text(member.get("repo"));
        match state_of(member).as_str() {
            "conflicted" => lines.push(format!(
                "    {repo} conflicted: cstan integrate redo {set_id} --repo {repo}"
            )),
            "failed" => {
                let attempt = as_attempt(member.get("attempt"));
                if attempt > 1 {
                    lines.push(format!("    {repo} failed, attempt {attempt}"));
                } else {
                    lines.push(format!("    {repo} failed"));
                }
            }
            _ => {}
        }
    }
    lines.extend(gate_lines(set, members, &set_id));
    lines
}

/// The gate of a set (`verify.gate.*`) and its per-repo line (`verify.repo*`), when the host sent the members' verify:
/// `set <id> verify  gate waiting: worker`, then `api passed · web passed · worker running · infra outside the gate`.
fn gate_lines(set: &Value, members: &[Value], set_id: &str) -> Vec<String> {
    let Some(gate) = set.get("gate").filter(|g| g.get("state").is_some()) else {
        return Vec::new();
    };
    let repos = match gate.get("repos") {
        Some(Value::Array(names)) => names
            .iter()
            .map(|n| text(Some(n)))
            .collect::<Vec<_>>()
            .join(","),
        _ => String::new(),
    };
    let head = match text(gate.get("state")).as_str() {
        "passed" => "gate passed: every repo's latest run passed".to_string(),
        "failed" => format!("gate failed: {repos}"),
        _ => format!("gate waiting: {repos}"),
    };
    let words: Vec<String> = members
        .iter()
        .take(STRIP_GLYPHS)
        .map(|m| {
            let repo = text(m.get("repo"));
            if flag(m.get("dropped")) {
                return format!("{repo} outside the gate");
            }
            match m.get("verify") {
                Some(run @ Value::Object(_)) => {
                    let state = text(run.get("state"));
                    // The last pass is shown only when it is not the commit of the latest run.
                    let last = text(run.get("lastPassed"));
                    if state != "passed" && !last.is_empty() && last != text(run.get("commitSha")) {
                        format!(
                            "{repo} {state} · last passed {}",
                            short_sha(run.get("lastPassed"))
                        )
                    } else {
                        format!("{repo} {state}")
                    }
                }
                _ => format!("{repo} not run"),
            }
        })
        .collect();
    // Past the cap only a count, but a dropped repo is always named: it is outside the gate.
    let hidden = members.get(STRIP_GLYPHS..).unwrap_or_default();
    let mut tail = String::new();
    if !hidden.is_empty() {
        tail = format!(" · +{}", hidden.len());
        let outside: Vec<String> = hidden
            .iter()
            .filter(|m| flag(m.get("dropped")))
            .map(|m| text(m.get("repo")))
            .collect();
        if !outside.is_empty() {
            tail.push_str(&format!(" · {} outside the gate", outside.join(",")));
        }
    }
    vec![
        format!("set {set_id} verify  {head}"),
        format!("    {}{tail}", words.join(" · ")),
    ]
}

/// The attempt a member is on: the first integration is attempt 1 and each redo adds one, so the stored number is the one
/// shown, and only after a redo.
fn as_attempt(value: Option<&Value>) -> u64 {
    match value {
        Some(Value::Number(n)) if *n >= 1.0 => *n as u64,
        _ => 1,
    }
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

/// ` stale: <reasons>` for a plan the controller reported as stale.
fn stale_suffix(plan: &Value) -> String {
    match plan.get("stale") {
        Some(Value::Array(reasons)) if !reasons.is_empty() => format!(
            " stale: {}",
            reasons
                .iter()
                .map(|r| text(Some(r)))
                .collect::<Vec<_>>()
                .join(", ")
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
                        "{} {} {} {} [{}]{}{}{}",
                        text(p.get("planId")),
                        text(p.get("tier")),
                        // A filtered list carries the status, which says more than the state.
                        text(p.get("status").or(p.get("state"))),
                        text(p.get("title")),
                        counts(p.get("packages")),
                        reassigned(p),
                        nexora(p),
                        stale_suffix(p),
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
        // A workspace package names the repos it works in.
        let repos = match p.get("repos") {
            Some(Value::Array(names)) if !names.is_empty() => format!(
                " repos {}",
                names
                    .iter()
                    .map(|n| text(Some(n)))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            _ => String::new(),
        };
        lines.push(format!(
            "  {} {} {} deps {} {} {}{repos}{}{}",
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

    #[test]
    fn a_filtered_brief_shows_the_status_and_ends_with_the_stale_reasons() {
        let result = capstan_wire::js::parse(
            r#"{"plans":[{"planId":"plan-1","tier":"normal","state":"draft","status":"draft","title":"One","packages":{},"stale":["idle","review_exhausted"]},{"planId":"plan-2","tier":"high_risk","state":"approved","status":"signed_off","title":"Two","packages":{"merged":1}},{"planId":"plan-3","tier":"normal","state":"draft","title":"Three","packages":{}}]}"#,
        )
        .unwrap();
        assert_eq!(
            plan_brief(&result).unwrap(),
            [
                "plan-1 normal draft One [no packages] stale: idle, review_exhausted",
                "plan-2 high_risk signed_off Two [1 merged]",
                "plan-3 normal draft Three [no packages]",
            ]
        );
    }
}
