//! What the Node CLI prints for a daemon result: `renderMessages`, `controllerLines`, `pauseLines`, `pmMailLines`,
//! `age` and the unread notice, each with the same output for the same JSON, errors from odd shapes included.
use capstan_wire::js::{date_parse_ms, number_to_string, JsStr, Value};

use crate::jsops::{
    as_number, concat, is_true, nullish, read, read_opt, strict_eq, template, template_or_empty,
    to_js_string, to_number, JsError,
};

fn js(text: &str) -> JsStr {
    JsStr::from(text)
}

/// `renderMessages(result)`.
pub fn render_messages(result: Option<&Value>) -> Result<JsStr, JsError> {
    let messages_value = read(result, "messages")?;
    let value = result.expect("read succeeded");
    let length_zero = match messages_value {
        None | Some(Value::Null) => true,
        Some(Value::Array(items)) => items.is_empty(),
        Some(Value::String(s)) => s.len_units() == 0,
        Some(object @ Value::Object(_)) => {
            matches!(object.get("length"), Some(Value::Number(n)) if *n == 0.0)
        }
        Some(_) => false,
    };
    if length_zero {
        return Ok(js(if is_true(read_opt(Some(value), "timedOut")) {
            "no messages (the wait timed out)"
        } else {
            "no messages"
        }));
    }
    let Some(Value::Array(items)) = messages_value else {
        return Err(JsError::type_error("messages.map is not a function"));
    };
    let mut out = JsStr::default();
    if as_number(read_opt(Some(value), "count")).is_some() {
        out.push_str(&format!(
            "{} message(s) to act on; ack each one with cstan ack <message-id>\n\n",
            items.len()
        ));
    }
    let mut rendered = Vec::with_capacity(items.len());
    for item in items {
        let action = read(Some(item), "actionNeeded")?;
        let mut line = JsStr::default();
        if is_true(action) {
            line.push_str("[ACTION NEEDED]\n");
        }
        let id = template_or_empty(item.get("messageId"));
        let state = template_or_empty(item.get("state"));
        let from = template_or_empty(item.get("from"));
        line = concat(&[
            &line,
            &js("message "),
            &id,
            &js(" ["),
            &state,
            &js("] from "),
            &from,
        ]);
        if !strict_eq(item.get("fromAgentId"), item.get("from")) {
            let agent = template_or_empty(item.get("fromAgentId"));
            line = concat(&[&line, &js(" ("), &agent, &js(")")]);
        }
        let body = template_or_empty(item.get("body"));
        rendered.push(concat(&[&line, &js("\n"), &body]));
    }
    for (i, line) in rendered.iter().enumerate() {
        if i > 0 {
            out.push_str("\n\n");
        }
        out = concat(&[&out, line]);
    }
    Ok(out)
}

/// `project <root>` and `ledger <path>`, or the older-daemon line.
pub fn controller_lines(result: Option<&Value>) -> Vec<JsStr> {
    let controller = read_opt(result, "controller");
    let pick = |key: &str| {
        let inner = read_opt(controller, key);
        if nullish(inner) {
            read_opt(result, key)
        } else {
            inner
        }
    };
    match (pick("projectRoot"), pick("ledgerPath")) {
        (Some(Value::String(root)), Some(Value::String(ledger))) => vec![
            concat(&[&js("project "), root]),
            concat(&[&js("ledger "), ledger]),
        ],
        _ => vec![js("project unknown (older daemon)")],
    }
}

/// `age(iso, nowMs)` of the dashboard.
pub fn age(iso: Option<&Value>, now_ms: f64) -> JsStr {
    let truthy = iso.is_some_and(Value::truthy);
    if !truthy {
        return js("-");
    }
    let text = to_js_string(iso.expect("truthy"));
    let Some(then) = text.to_utf8().and_then(|t| date_parse_ms(&t)) else {
        return js("-");
    };
    let seconds = ((now_ms - then as f64) / 1000.0).floor().max(0.0);
    js(&if seconds < 60.0 {
        format!("{}s", number_to_string(seconds))
    } else if seconds < 3600.0 {
        format!("{}m", number_to_string((seconds / 60.0).floor()))
    } else if seconds < 86_400.0 {
        format!("{}h", number_to_string((seconds / 3600.0).floor()))
    } else {
        format!("{}d", number_to_string((seconds / 86_400.0).floor()))
    })
}

fn pause_line(entry: &Value, subject: &JsStr, now_ms: f64) -> JsStr {
    concat(&[
        &js("PAUSED ("),
        &age(entry.get("pausedAt"), now_ms),
        &js(")"),
        subject,
        &js(": "),
        &template(entry.get("reason")),
        &js(" by "),
        &template(entry.get("actorId")),
    ])
}

/// `pauseLines(status, nowMs)`.
pub fn pause_lines(status: Option<&Value>, now_ms: f64) -> Result<Vec<JsStr>, JsError> {
    let pause = read_opt(status, "pause");
    let Some(pause) = pause.filter(|p| p.truthy()) else {
        return Ok(Vec::new());
    };
    let mut lines = Vec::new();
    if let Some(run) = pause.get("run").filter(|r| r.truthy()) {
        lines.push(pause_line(run, &JsStr::default(), now_ms));
    }
    match pause.get("agents") {
        None | Some(Value::Null) => {}
        Some(Value::Array(agents)) => {
            for entry in agents {
                let agent_id = read(Some(entry), "agentId")?;
                let subject = concat(&[&js(" "), &template_or_empty(agent_id)]);
                lines.push(pause_line(entry, &subject, now_ms));
            }
        }
        Some(_) => {
            return Err(JsError::type_error(
                "(pause.agents ?? []).map is not a function or its return value is not iterable",
            ))
        }
    }
    Ok(lines)
}

/// `pmMailLines(status)`.
pub fn pm_mail_lines(status: Option<&Value>) -> Vec<JsStr> {
    let mail = read_opt(status, "pmMail");
    let Some(mail) = mail.filter(|m| m.truthy()) else {
        return Vec::new();
    };
    let Some(pending) = mail.get("pending").filter(|p| p.truthy()) else {
        return Vec::new();
    };
    let age_seconds = match mail.get("oldestAgeSeconds") {
        None | Some(Value::Null) => 0.0,
        other => to_number(other),
    };
    let minutes = number_to_string((age_seconds / 60.0).floor());
    let pending = to_js_string(pending);
    if is_true(mail.get("stale")) {
        let id = match mail.get("oldestMessageId") {
            None | Some(Value::Null) => js("-"),
            Some(v) => to_js_string(v),
        };
        return vec![concat(&[
            &js("PM MAIL STALE: "),
            &pending,
            &js(&format!(" message(s) pending, oldest {minutes} min (")),
            &id,
            &js(")"),
        ])];
    }
    vec![concat(&[
        &js("pm mail: "),
        &pending,
        &js(&format!(" pending, oldest {minutes} min")),
    ])]
}

/// `oldestMinutes(queuedAt)`, as the number the notice prints.
pub fn oldest_minutes(queued_at: Option<&Value>, now_ms: f64) -> f64 {
    let queued = match queued_at {
        Some(Value::String(s)) => s
            .to_utf8()
            .and_then(|t| date_parse_ms(&t))
            .map_or(f64::NAN, |ms| ms as f64),
        _ => f64::NAN,
    };
    if queued.is_nan() {
        return 0.0;
    }
    ((now_ms - queued) / 60_000.0).floor().max(0.0)
}

/// The `notice:` line for a result with `unread.count`, without its newline.
pub fn unread_notice(result: Option<&Value>, now_ms: f64) -> Option<String> {
    let unread = read_opt(result, "unread");
    let count = as_number(read_opt(unread, "count"))?;
    let minutes = oldest_minutes(read_opt(unread, "oldestQueuedAt"), now_ms);
    let action = as_number(read_opt(unread, "actionNeeded")).filter(|n| *n > 0.0);
    Some(format!(
        "notice: {} message(s) wait for you (oldest {} min): run cstan inbox{}",
        number_to_string(count),
        number_to_string(minutes),
        action.map_or(String::new(), |n| format!(
            ", {} need action",
            number_to_string(n)
        )),
    ))
}

fn text_of(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(s)) => Some(s.to_utf8_lossy()),
        _ => None,
    }
}

fn items(value: Option<&Value>) -> &[Value] {
    match value {
        Some(Value::Array(items)) => items,
        _ => &[],
    }
}

/// One group of `cstan cleanup --dry-run`: a heading with the count, then each branch with its commit, worktree and reason.
fn cleanup_group(lines: &mut Vec<String>, title: &str, entries: &[Value]) {
    lines.push(format!("{title} ({})", entries.len()));
    if entries.is_empty() {
        lines.push("  none".to_string());
    }
    for entry in entries {
        let get = |key: &str| text_of(entry.get(key));
        lines.push(format!(
            "  {} {}",
            get("branch").unwrap_or_default(),
            get("sha").unwrap_or_default()
        ));
        lines.push(format!(
            "    worktree: {}",
            get("worktree").unwrap_or_else(|| "-".to_string())
        ));
        lines.push(format!("    reason: {}", get("reason").unwrap_or_default()));
    }
}

/// What `cstan cleanup` prints (without `--json`) for the daemon's answer: the dry-run groups, an offer, what an apply
/// deleted and skipped with the commands that bring the branches back, or the decline.
pub fn cleanup_lines(result: Option<&Value>) -> Vec<String> {
    let Some(result) = result else {
        return Vec::new();
    };
    let id = text_of(result.get("cleanupId"));
    let mut lines = Vec::new();
    if matches!(result.get("dryRun"), Some(Value::Bool(true))) {
        cleanup_group(
            &mut lines,
            "Landed: merged and safe to delete",
            items(result.get("landed")),
        );
        lines.push(String::new());
        cleanup_group(
            &mut lines,
            "Unlanded: no confirmed integration holds them",
            items(result.get("unlanded")),
        );
        lines.push(String::new());
        cleanup_group(
            &mut lines,
            "Active or protected: never deleted",
            items(result.get("protected")),
        );
        return lines;
    }
    match text_of(result.get("state")).as_deref() {
        Some("declined") => {
            lines.push(format!(
                "cleanup {} declined; nothing was deleted",
                id.unwrap_or_default()
            ));
        }
        Some("applied") => {
            lines.push(format!("cleanup {} applied", id.unwrap_or_default()));
            for deleted in items(result.get("deleted")) {
                lines.push(format!(
                    "deleted {} {}",
                    text_of(deleted.get("branch")).unwrap_or_default(),
                    text_of(deleted.get("sha")).unwrap_or_default()
                ));
            }
            for skipped in items(result.get("skipped")) {
                lines.push(format!(
                    "skipped {}: {}",
                    text_of(skipped.get("branch")).unwrap_or_default(),
                    text_of(skipped.get("reason")).unwrap_or_default()
                ));
            }
            let restore = items(result.get("restore"));
            if !restore.is_empty() {
                lines.push("to bring a branch back:".to_string());
                lines.extend(
                    restore
                        .iter()
                        .map(|c| format!("  {}", text_of(Some(c)).unwrap_or_default())),
                );
            }
        }
        _ => match id {
            None => lines.push("nothing to clean up".to_string()),
            Some(id) => {
                let entries = items(result.get("entries"));
                lines.push(format!(
                    "cleanup {id}: {} branch(es) offered; nothing is deleted yet",
                    entries.len()
                ));
                for entry in entries {
                    lines.push(format!(
                        "  {} {} ({})",
                        text_of(entry.get("branch")).unwrap_or_default(),
                        text_of(entry.get("sha")).unwrap_or_default(),
                        text_of(entry.get("class")).unwrap_or_default()
                    ));
                }
                lines.push(format!(
                    "apply the landed ones: cstan cleanup apply {id} [<branch>...]"
                ));
                lines.push(format!("decline: cstan cleanup decline {id}"));
            }
        },
    }
    lines
}

// ------------------------------------------------------------------------------------------------ cache

/// A byte count as `1.2 GB`: binary units, one decimal that is dropped when it is zero.
pub fn human_bytes(bytes: u64) -> String {
    let (number, unit) = human_parts(bytes);
    format!("{number} {unit}")
}

fn human_parts(bytes: u64) -> (String, &'static str) {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    let number = if unit == 0 {
        format!("{bytes}")
    } else {
        let text = format!("{value:.1}");
        text.strip_suffix(".0").map_or(text.clone(), str::to_string)
    };
    (number, UNITS[unit])
}

/// `4m ago` for an ISO time in the past.
fn ago(iso: Option<&Value>, now_ms: f64) -> Option<String> {
    let Some(Value::String(text)) = iso else {
        return None;
    };
    let then = date_parse_ms(&text.to_utf8_lossy())? as f64;
    let seconds = ((now_ms - then) / 1000.0).max(0.0) as u64;
    Some(match seconds {
        0..=59 => format!("{seconds}s ago"),
        60..=3599 => format!("{}m ago", seconds / 60),
        3600..=86_399 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86_400),
    })
}

fn bytes_of(value: Option<&Value>) -> Option<u64> {
    as_number(value).map(|n| n.max(0.0) as u64)
}

/// The one cache line of `cstan status`: `cache: rust off (sccache not found), python 1.2 GB; total 1.2 of 20 GB,
/// measured 4m ago`. `None` when the status has no `cache` member.
pub fn cache_status_line(status: Option<&Value>, now_ms: f64) -> Option<String> {
    let cache = status?.get("cache")?;
    if !matches!(cache, Value::Object(_)) {
        return None;
    }
    let entries: Vec<String> = items(cache.get("entries"))
        .iter()
        .map(|entry| {
            let name = text_of(entry.get("name")).unwrap_or_default();
            if matches!(entry.get("enabled"), Some(Value::Bool(false))) {
                return match text_of(entry.get("reason")) {
                    Some(reason) => format!("{name} off ({reason})"),
                    None => format!("{name} off"),
                };
            }
            match bytes_of(entry.get("bytes")) {
                Some(bytes) => format!("{name} {}", human_bytes(bytes)),
                None => format!("{name} not measured"),
            }
        })
        .collect();
    let max = bytes_of(cache.get("maxBytes")).unwrap_or(0);
    let total = match bytes_of(cache.get("totalBytes")) {
        Some(total) => {
            let (number, unit) = human_parts(total);
            let (max_number, max_unit) = human_parts(max);
            let first = if unit == max_unit {
                number
            } else {
                format!("{number} {unit}")
            };
            format!("total {first} of {max_number} {max_unit}")
        }
        None => "not measured yet".to_string(),
    };
    let measured = ago(cache.get("measuredAt"), now_ms)
        .map(|age| format!(", measured {age}"))
        .unwrap_or_default();
    Some(format!("cache: {}; {total}{measured}", entries.join(", ")))
}

fn command_lines(lines: &mut Vec<String>, label: &str, runs: &[Value]) {
    for run in runs {
        let entry = text_of(run.get("entry")).unwrap_or_default();
        let command = text_of(run.get("command")).unwrap_or_default();
        if matches!(run.get("ok"), Some(Value::Bool(true))) {
            lines.push(format!("{label} {entry}: {command}: ok"));
        } else {
            let message = text_of(run.get("message")).unwrap_or_default();
            lines.push(format!("{label} {entry}: {command}: failed: {message}"));
        }
    }
}

fn texts(value: Option<&Value>) -> Vec<String> {
    items(value)
        .iter()
        .filter_map(|v| text_of(Some(v)))
        .collect()
}

fn cache_status_lines(result: &Value, now_ms: f64) -> Vec<String> {
    let mut lines = vec![format!(
        "cache root: {} (scope {})",
        text_of(result.get("root")).unwrap_or_default(),
        text_of(result.get("scope")).unwrap_or_default()
    )];
    let rows: Vec<[String; 5]> = items(result.get("entries"))
        .iter()
        .map(|entry| {
            let enabled = !matches!(entry.get("enabled"), Some(Value::Bool(false)));
            let name = text_of(entry.get("name")).unwrap_or_default();
            let note = if enabled {
                text_of(entry.get("mode")).map_or(String::new(), |m| format!("mode {m}"))
            } else {
                text_of(entry.get("reason")).unwrap_or_default()
            };
            let state = if enabled { "on" } else { "off" };
            let size = bytes_of(entry.get("bytes")).map_or("-".to_string(), human_bytes);
            let files = as_number(entry.get("files")).map_or("-".to_string(), |n| format!("{n}"));
            let used = ago(entry.get("lastUsedAt"), now_ms).unwrap_or_else(|| "-".to_string());
            [
                name,
                format!("{state} {note}").trim_end().to_string(),
                size,
                files,
                used,
            ]
        })
        .collect();
    let header = ["NAME", "STATE", "SIZE", "FILES", "LAST USED"].map(String::from);
    let mut widths = [0usize; 5];
    for row in std::iter::once(&header).chain(rows.iter()) {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    for row in std::iter::once(&header).chain(rows.iter()) {
        let cells: Vec<String> = row
            .iter()
            .zip(widths)
            .map(|(cell, width)| format!("{cell:<width$}"))
            .collect();
        lines.push(cells.join("  ").trim_end().to_string());
    }
    for snapshot in items(result.get("setupSnapshots")) {
        lines.push(format!(
            "  setup/{}  {}  {} files",
            text_of(snapshot.get("key")).unwrap_or_default(),
            bytes_of(snapshot.get("bytes")).map_or("-".to_string(), human_bytes),
            as_number(snapshot.get("files")).unwrap_or(0.0)
        ));
    }
    lines.push(format!(
        "total {} of {} (entries older than {} days are pruned){}",
        human_bytes(bytes_of(result.get("totalBytes")).unwrap_or(0)),
        human_bytes(bytes_of(result.get("maxBytes")).unwrap_or(0)),
        as_number(result.get("maxAgeDays")).unwrap_or(0.0),
        if matches!(result.get("partial"), Some(Value::Bool(true))) {
            "; the measurement was cut short"
        } else {
            ""
        }
    ));
    lines
}

fn cache_prune_lines(result: &Value) -> Vec<String> {
    let dry = matches!(result.get("dryRun"), Some(Value::Bool(true)));
    let idle = !matches!(result.get("idle"), Some(Value::Bool(false)));
    let mut lines = Vec::new();
    if dry {
        lines.push("dry run: nothing was run or removed".to_string());
        for run in items(result.get("wouldRun")) {
            lines.push(format!(
                "would run {}: {}",
                text_of(run.get("entry")).unwrap_or_default(),
                text_of(run.get("command")).unwrap_or_default()
            ));
        }
    }
    command_lines(&mut lines, "ran", items(result.get("commands")));
    for name in texts(result.get("cleared")) {
        lines.push(format!("cleared {name}"));
    }
    let would = texts(result.get("wouldClear"));
    for name in &would {
        lines.push(format!("would clear {name}"));
    }
    if !dry && !idle && !would.is_empty() {
        lines.push(
            "agents have worktrees open, so nothing was cleared; use cache clear --force to override"
                .to_string(),
        );
    }
    if !dry && texts(result.get("cleared")).is_empty() && would.is_empty() {
        lines.push("nothing to clear".to_string());
    }
    if dry && would.is_empty() && items(result.get("wouldRun")).is_empty() {
        lines.push("nothing to prune".to_string());
    }
    for error in texts(result.get("errors")) {
        lines.push(format!("error: {error}"));
    }
    lines.push(format!(
        "size {} -> {}",
        human_bytes(bytes_of(result.get("totalBefore")).unwrap_or(0)),
        human_bytes(bytes_of(result.get("totalAfter")).unwrap_or(0))
    ));
    lines
}

fn cache_clear_lines(result: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    command_lines(&mut lines, "stopped", items(result.get("stopCommands")));
    let cleared = texts(result.get("cleared"));
    if cleared.is_empty() {
        lines.push("nothing was cleared".to_string());
    }
    for name in cleared {
        lines.push(format!("cleared {name}"));
    }
    for error in texts(result.get("errors")) {
        lines.push(format!("error: {error}"));
    }
    lines.push(format!(
        "cache size now {}",
        human_bytes(bytes_of(result.get("totalBytes")).unwrap_or(0))
    ));
    lines
}

/// What `cstan cache status|prune|clear` prints (without `--json`) for its result.
pub fn cache_lines(result: Option<&Value>, now_ms: f64) -> Vec<String> {
    let Some(result) = result else {
        return Vec::new();
    };
    if result.get("schemaVersion").is_some() {
        cache_status_lines(result, now_ms)
    } else if result.get("dryRun").is_some() {
        cache_prune_lines(result)
    } else {
        cache_clear_lines(result)
    }
}
