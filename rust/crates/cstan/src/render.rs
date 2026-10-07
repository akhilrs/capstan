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
