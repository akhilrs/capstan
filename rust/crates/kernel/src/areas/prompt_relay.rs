//! The prompt relay (src/controller/prompt-relay.ts): what the PM was shown, the answer typed back, and the captures
//! that expire. Private here: `relayTextProblem` and the relay constants of src/herdr/prompt-relay.ts.

use crate::dispatch::{arg, call, Args};
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{execute, is_js_space, js_trim, query_all, query_opt, safe_id, safe_text};
use crate::json::stringify;
use crate::kernel::Kernel;
use crate::records::{AgentRow, MutationEvent, MutationOutput, PROMPT_RELAY_TEXT_MAX_CHARS};
use crate::types::MutationContext;
use capstan_ledger::iso_from_millis;
use regex::Regex;
use rusqlite::Row;
use serde_json::{json, Map, Value};
use std::sync::LazyLock;

use super::messaging::parse_iso_ms;

const RELAY_TEXT_MAX_BYTES: usize = 1000;
const RELAY_PROMPT_MAX_BYTES: usize = 8192;
const DEFAULT_CAPTURE_TTL_SECONDS: f64 = 600.0;

static UNSAFE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[\p{Cc}\p{Cf}\p{Zl}\p{Zp}\p{Noncharacter_Code_Point}]").expect("a valid pattern")
});

// ------------------------------------------------------------------------------------------------- config

/// The in-memory `{ enabled, captureTtlSeconds }` of a core (`PromptRelayArea.#config`), kept on the kernel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PromptRelayConfig {
    pub(crate) enabled: bool,
    pub(crate) capture_ttl_seconds: f64,
}

impl Default for PromptRelayConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            capture_ttl_seconds: DEFAULT_CAPTURE_TTL_SECONDS,
        }
    }
}

fn config_of(kernel: &Kernel) -> PromptRelayConfig {
    *kernel.prompt_relay.borrow()
}

/// `configurePromptRelay`: set once by the daemon from `[prompt_relay]`; off, no notice or status view mentions the relay.
pub fn configure(kernel: &Kernel, config: &Value) -> KernelResult<Value> {
    *kernel.prompt_relay.borrow_mut() = PromptRelayConfig {
        enabled: truthy(&config["enabled"]),
        capture_ttl_seconds: config["captureTtlSeconds"].as_f64().unwrap_or(f64::NAN),
    };
    Ok(Value::Null)
}

/// `promptRelayEnabled`.
pub fn enabled(kernel: &Kernel) -> KernelResult<bool> {
    Ok(config_of(kernel).enabled)
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

// --------------------------------------------------------------------------------------- relay text rules

/// `relayTextProblem`: why a text answer may not be typed, or none when it is acceptable. (A text that is not
/// well-formed UTF-16 cannot be held by a Rust string, so that refusal has no counterpart here.)
fn relay_text_problem(text: &Value) -> Option<String> {
    let Some(text) = text.as_str() else {
        return Some("the text is not a string".into());
    };
    if js_trim(text).is_empty() {
        return Some("the text is empty".into());
    }
    if UNSAFE.is_match(text) {
        return Some("the text has a newline, control or format character".into());
    }
    let starts_command = text.starts_with('\t')
        || text
            .trim_start_matches(is_js_space)
            .chars()
            .next()
            .is_some_and(|c| "/!#?@".contains(c));
    if starts_command {
        return Some("the text starts with a tab or one of / ! # ? @".into());
    }
    if text.len() > RELAY_TEXT_MAX_BYTES {
        return Some(format!(
            "the text is longer than {RELAY_TEXT_MAX_BYTES} bytes"
        ));
    }
    None
}

// ---------------------------------------------------------------------------------------------------- rows

struct RelayRow {
    relay_id: String,
    sequence: i64,
    agent_id: String,
    pane_id: String,
    host_kind: String,
    prompt_text: String,
    options_json: String,
    prompt_sha: String,
    captured_by_actor_id: String,
    captured_at: String,
    expires_at: String,
    state: String,
    answer_kind: Option<String>,
    answer_option: Option<i64>,
    answer_widens_permissions: Option<i64>,
    answer_text: Option<String>,
    answered_by_actor_id: Option<String>,
    answered_at: Option<String>,
    outcome_reason: Option<String>,
    keys_json: Option<String>,
}

impl RelayRow {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            relay_id: row.get("relay_id")?,
            sequence: row.get("sequence")?,
            agent_id: row.get("agent_id")?,
            pane_id: row.get("pane_id")?,
            host_kind: row.get("host_kind")?,
            prompt_text: row.get("prompt_text")?,
            options_json: row.get("options_json")?,
            prompt_sha: row.get("prompt_sha")?,
            captured_by_actor_id: row.get("captured_by_actor_id")?,
            captured_at: row.get("captured_at")?,
            expires_at: row.get("expires_at")?,
            state: row.get("state")?,
            answer_kind: row.get("answer_kind")?,
            answer_option: row.get("answer_option")?,
            answer_widens_permissions: row.get("answer_widens_permissions")?,
            answer_text: row.get("answer_text")?,
            answered_by_actor_id: row.get("answered_by_actor_id")?,
            answered_at: row.get("answered_at")?,
            outcome_reason: row.get("outcome_reason")?,
            keys_json: row.get("keys_json")?,
        })
    }

    fn options(&self) -> KernelResult<Vec<Value>> {
        match serde_json::from_str::<Value>(&self.options_json)? {
            Value::Array(items) => Ok(items),
            _ => Ok(Vec::new()),
        }
    }
}

fn relay_row(kernel: &Kernel, relay_id: &str) -> KernelResult<Option<RelayRow>> {
    query_opt(
        &kernel.database,
        "SELECT * FROM prompt_relays WHERE project_id = ? AND relay_id = ?",
        [&kernel.project_id, relay_id],
        RelayRow::from_row,
    )
}

fn relay_record(row: &RelayRow) -> KernelResult<Value> {
    let answer = match &row.answer_kind {
        None => Value::Null,
        Some(kind) => json!({
            "kind": kind,
            "option": row.answer_option,
            "widensPermissions": row.answer_widens_permissions == Some(1),
            "text": row.answer_text,
        }),
    };
    let keys = match &row.keys_json {
        None => Value::Null,
        Some(text) => serde_json::from_str(text)?,
    };
    Ok(json!({
        "relayId": row.relay_id,
        "sequence": row.sequence,
        "agentId": row.agent_id,
        "paneId": row.pane_id,
        "hostKind": row.host_kind,
        "promptText": row.prompt_text,
        "options": serde_json::from_str::<Value>(&row.options_json)?,
        "promptSha": row.prompt_sha,
        "hash12": row.prompt_sha.get(..12).unwrap_or(&row.prompt_sha),
        "capturedByActorId": row.captured_by_actor_id,
        "capturedAt": row.captured_at,
        "expiresAt": row.expires_at,
        "state": row.state,
        "answer": answer,
        "answeredByActorId": row.answered_by_actor_id,
        "answeredAt": row.answered_at,
        "outcomeReason": row.outcome_reason,
        "keys": keys,
    }))
}

fn record_of(kernel: &Kernel, relay_id: &str) -> KernelResult<Value> {
    let row = relay_row(kernel, relay_id)?
        .ok_or_else(|| KernelError::controller("the prompt relay vanished"))?;
    relay_record(&row)
}

fn relay_event(relay_id: &str, from: Option<&str>, to: &str, details: Value) -> MutationEvent {
    MutationEvent {
        from_state: from.map(str::to_string),
        to_state: Some(to.to_string()),
        details: Some(details),
        ..MutationEvent::new("prompt_relay", relay_id, 0)
    }
}

fn active_pm_of(kernel: &Kernel, actor_id: &str) -> KernelResult<AgentRow> {
    match kernel.agent_by_actor(actor_id)? {
        Some(agent) if agent.kind == "PM" => Ok(agent),
        _ => Err(KernelError::controller(
            "forbidden: only an active PM agent relays prompts",
        )),
    }
}

fn same_number(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

fn answer_kind(answer: &Value) -> Option<&str> {
    answer.get("kind").and_then(Value::as_str)
}

/// The stored relay when the answer may be typed to it now, else a ControllerError naming the reason.
fn answerable_relay(
    kernel: &Kernel,
    relay_id: &Value,
    hash: &Value,
    answer: &Value,
) -> KernelResult<RelayRow> {
    let relay_id = safe_id(relay_id, "relay id")?;
    let Some(row) = relay_row(kernel, relay_id)? else {
        return Err(KernelError::controller(format!(
            "unknown_relay: relay {relay_id} does not exist"
        )));
    };
    if row.state == "typing" {
        return Err(KernelError::controller(format!(
            "relay_in_progress: relay {relay_id} is being typed"
        )));
    }
    if row.state != "captured" {
        return Err(KernelError::controller(format!(
            "relay_not_open: relay {relay_id} is {}",
            row.state
        )));
    }
    if parse_iso_ms(&row.expires_at) <= parse_iso_ms(&kernel.now()) {
        return Err(KernelError::controller(format!(
            "capture_expired: the capture of relay {relay_id} expired; run cstan prompt show again"
        )));
    }
    let hash_ok = hash.as_str().is_some_and(|hash| {
        (12..=64).contains(&hash.len())
            && hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            && row.prompt_sha.starts_with(hash)
    });
    if !hash_ok {
        return Err(KernelError::controller(
            "hash_mismatch: --hash must be the hash shown by cstan prompt show for this relay",
        ));
    }
    if answer_kind(answer) != Some("esc") {
        let number = &answer["number"];
        let options = row.options()?;
        let Some(option) = options
            .iter()
            .find(|entry| same_number(&entry["number"], number))
        else {
            return Err(KernelError::controller(format!(
                "no_such_option: option {} is not in the shown prompt",
                display(number)
            )));
        };
        if answer_kind(answer) == Some("text") {
            if !truthy(&option["acceptsText"]) {
                return Err(KernelError::controller(format!(
                    "no_text_option: option {} does not accept text",
                    display(number)
                )));
            }
            if let Some(problem) = relay_text_problem(&answer["text"]) {
                return Err(KernelError::controller(format!("text_refused: {problem}")));
            }
            let text = answer["text"].as_str().unwrap_or("");
            if text != js_trim(text) {
                return Err(KernelError::controller(
                    "text_refused: the text has leading or trailing whitespace",
                ));
            }
            if text.chars().count() > PROMPT_RELAY_TEXT_MAX_CHARS {
                return Err(KernelError::controller(format!(
                    "text_refused: the text is longer than {PROMPT_RELAY_TEXT_MAX_CHARS} characters"
                )));
            }
        }
    }
    Ok(row)
}

/// `${value}` of a JSON scalar.
fn display(value: &Value) -> String {
    match value {
        Value::Null => "undefined".into(),
        Value::String(s) => s.clone(),
        other => stringify(other),
    }
}

fn answer_widens(row: &RelayRow, answer: &Value) -> KernelResult<bool> {
    if answer_kind(answer) == Some("esc") {
        return Ok(false);
    }
    Ok(row
        .options()?
        .iter()
        .find(|entry| same_number(&entry["number"], &answer["number"]))
        .is_some_and(|entry| entry["widensPermissions"] == Value::Bool(true)))
}

/// `checkPromptAnswer`: read-only; the stored relay when `answer` may be typed to it, else a ControllerError.
pub fn check_prompt_answer(
    kernel: &Kernel,
    relay_id: &str,
    hash: &Value,
    answer: &Value,
) -> KernelResult<Value> {
    kernel.assert_open()?;
    relay_record(&answerable_relay(kernel, &json!(relay_id), hash, answer)?)
}

/// `promptRelay`.
pub fn prompt_relay(kernel: &Kernel, relay_id: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    match relay_row(kernel, relay_id)? {
        None => Ok(Value::Null),
        Some(row) => relay_record(&row),
    }
}

/// `listPromptRelays`: newest first.
pub fn list_prompt_relays(kernel: &Kernel, limit: Option<i64>) -> KernelResult<Value> {
    kernel.assert_open()?;
    let rows = query_all(
        &kernel.database,
        "SELECT * FROM prompt_relays WHERE project_id = ? ORDER BY sequence DESC LIMIT ?",
        rusqlite::params![kernel.project_id, limit.unwrap_or(20)],
        RelayRow::from_row,
    )?;
    Ok(Value::Array(
        rows.iter()
            .map(relay_record)
            .collect::<KernelResult<Vec<_>>>()?,
    ))
}

/// `promptRelayStatus`: open captures and the last answers.
pub fn prompt_relay_status(kernel: &Kernel) -> KernelResult<Value> {
    kernel.assert_open()?;
    let rows = query_all(
        &kernel.database,
        "SELECT * FROM prompt_relays WHERE project_id = ? AND state IN ('captured', 'typing') ORDER BY sequence",
        [&kernel.project_id],
        RelayRow::from_row,
    )?;
    let mut open = Vec::new();
    for row in rows {
        if row.state == "typing" || parse_iso_ms(&row.expires_at) > parse_iso_ms(&kernel.now()) {
            open.push(relay_record(&row)?);
        }
    }
    let answered = query_all(
        &kernel.database,
        "SELECT * FROM prompt_relays WHERE project_id = ? AND state IN ('answered', 'refused', 'failed') ORDER BY answered_at DESC, sequence DESC LIMIT 5",
        [&kernel.project_id],
        RelayRow::from_row,
    )?
    .iter()
    .map(relay_record)
    .collect::<KernelResult<Vec<_>>>()?;
    Ok(json!({
        "enabled": config_of(kernel).enabled,
        "openCaptures": open.iter().map(|record| json!({
            "relayId": record["relayId"],
            "agentId": record["agentId"],
            "hash12": record["hash12"],
            "state": record["state"],
            "expiresAt": record["expiresAt"],
        })).collect::<Vec<_>>(),
        "lastAnswers": answered.iter().map(|record| json!({
            "relayId": record["relayId"],
            "agentId": record["agentId"],
            "hash12": record["hash12"],
            "state": record["state"],
            "answer": record["answer"],
            "widensPermissions": record["answer"]["widensPermissions"] == Value::Bool(true),
            "actorId": record["answeredByActorId"],
            "at": record["answeredAt"],
        })).collect::<Vec<_>>(),
    }))
}

/// A payload object whose `undefined` members are left out, as `JSON.stringify` leaves them.
fn payload(members: &[(&str, Option<&Value>)]) -> Value {
    let mut map = Map::new();
    for (name, value) in members {
        if let Some(value) = value {
            map.insert((*name).to_string(), (*value).clone());
        }
    }
    Value::Object(map)
}

/// `recordPromptCapture`: records what the PM was shown. An older captured row of the same agent becomes expired
/// (superseded) in the same transaction; a row that is being typed refuses a new capture.
pub fn record_prompt_capture(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let prompt = &input["prompt"];
    let prompt_sha = prompt["promptSha"].as_str().unwrap_or("");
    if prompt_sha.len() != 64
        || !prompt_sha
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(KernelError::type_error(
            "the prompt hash must be 64 lowercase hex characters",
        ));
    }
    let text = prompt["text"]
        .as_str()
        .ok_or_else(|| KernelError::type_error("The \"string\" argument must be of type string"))?;
    if text.len() > RELAY_PROMPT_MAX_BYTES {
        return Err(KernelError::type_error(format!(
            "the prompt text must be at most {RELAY_PROMPT_MAX_BYTES} bytes"
        )));
    }
    let agent_id = safe_id(&prompt["agentId"], "agent id")?.to_string();
    kernel.mutate(
        context,
        "prompt_relay.capture",
        "prompt:relay",
        &json!({"agentId": agent_id, "promptSha": prompt_sha}),
        |actor| {
            active_pm_of(kernel, &actor.actor_id)?;
            let config = config_of(kernel);
            if !config.enabled {
                return Err(KernelError::controller(
                    "not_configured: the prompt relay is not enabled",
                ));
            }
            match kernel.agent_row(&agent_id)? {
                Some(agent) if agent.state == "active" => {}
                _ => {
                    return Err(KernelError::controller(
                        "agent_not_active: the agent is not active",
                    ))
                }
            }
            let now = kernel.now();
            let mut events = Vec::new();
            let open = query_all(
                &kernel.database,
                "SELECT * FROM prompt_relays WHERE project_id = ? AND agent_id = ? AND state IN ('captured', 'typing')",
                [&kernel.project_id, &agent_id],
                RelayRow::from_row,
            )?;
            if let Some(typing) = open.iter().find(|row| row.state == "typing") {
                return Err(KernelError::controller(format!(
                    "relay_in_progress: relay {} is being typed",
                    typing.relay_id
                )));
            }
            for row in &open {
                execute(
                    &kernel.database,
                    "UPDATE prompt_relays SET state = 'expired', outcome_reason = 'superseded' WHERE project_id = ? AND relay_id = ?",
                    [&kernel.project_id, &row.relay_id],
                )?;
                events.push(relay_event(
                    &row.relay_id,
                    Some("captured"),
                    "expired",
                    json!({"reason": "superseded"}),
                ));
            }
            let sequence = query_opt(
                &kernel.database,
                "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM prompt_relays WHERE project_id = ?",
                [&kernel.project_id],
                |row| row.get::<_, i64>(0),
            )?
            .unwrap_or(1);
            let relay_id = format!("relay-{sequence}");
            let expires_at = iso_from_millis(
                (parse_iso_ms(&now) as f64 + config.capture_ttl_seconds * 1000.0) as i64,
            );
            let options: Vec<Value> = prompt["options"]
                .as_array()
                .map(|options| {
                    options
                        .iter()
                        .map(|option| {
                            payload(&[
                                ("number", option.get("number")),
                                ("text", option.get("text")),
                                ("acceptsText", option.get("acceptsText")),
                                ("widensPermissions", option.get("widensPermissions")),
                            ])
                        })
                        .collect()
                })
                .unwrap_or_default();
            execute(
                &kernel.database,
                "INSERT INTO prompt_relays(project_id, relay_id, sequence, agent_id, pane_id, host_kind, prompt_text, options_json,
                   prompt_sha, captured_by_actor_id, captured_at, expires_at, state)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'captured')",
                rusqlite::params![
                    kernel.project_id,
                    relay_id,
                    sequence,
                    agent_id,
                    prompt["paneId"].as_str(),
                    prompt["hostKind"].as_str(),
                    text,
                    stringify(&Value::Array(options)),
                    prompt_sha,
                    actor.actor_id,
                    now,
                    expires_at
                ],
            )?;
            let record = record_of(kernel, &relay_id)?;
            let mut output = MutationOutput::new(
                record.clone(),
                relay_event(
                    &relay_id,
                    None,
                    "captured",
                    json!({"agentId": agent_id, "hash12": record["hash12"]}),
                ),
            );
            output.extra_events = events;
            Ok(output)
        },
    )
}

/// `beginPromptAnswer`: the target of the launcher's beforeType: captured -> typing, with the answer, in one
/// transaction that re-checks state, expiry and hash.
pub fn begin_prompt_answer(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    kernel.mutate(
        context,
        "prompt_relay.begin",
        "prompt:relay",
        &payload(&[
            ("relayId", input.get("relayId")),
            ("hash", input.get("hash")),
            ("answer", input.get("answer")),
        ]),
        |actor| {
            active_pm_of(kernel, &actor.actor_id)?;
            let answer = &input["answer"];
            let row = answerable_relay(kernel, &input["relayId"], &input["hash"], answer)?;
            record_answer(kernel, &row, answer, &actor.actor_id, "typing", None, None)?;
            Ok(MutationOutput::new(
                record_of(kernel, &row.relay_id)?,
                relay_event(
                    &row.relay_id,
                    Some("captured"),
                    "typing",
                    json!({"answerKind": answer["kind"]}),
                ),
            ))
        },
    )
}

fn record_answer(
    kernel: &Kernel,
    row: &RelayRow,
    answer: &Value,
    actor_id: &str,
    state: &str,
    reason: Option<&str>,
    keys: Option<&Value>,
) -> KernelResult<()> {
    let now = kernel.now();
    let kind = answer_kind(answer);
    execute(
        &kernel.database,
        "UPDATE prompt_relays SET state = ?, answer_kind = ?, answer_option = ?, answer_widens_permissions = ?,
           answer_text = ?, answered_by_actor_id = ?, answered_at = ?, outcome_reason = ?, keys_json = ?
         WHERE project_id = ? AND relay_id = ?",
        rusqlite::params![
            state,
            kind,
            if kind == Some("esc") { None } else { answer["number"].as_i64() },
            i64::from(answer_widens(row, answer)?),
            if kind == Some("text") { answer["text"].as_str() } else { None },
            actor_id,
            if state == "typing" { None } else { Some(now) },
            reason,
            keys.map(stringify),
            kernel.project_id,
            row.relay_id
        ],
    )?;
    Ok(())
}

/// `refusePromptAnswer`: a launcher refusal before any key was typed: captured -> refused.
pub fn refuse_prompt_answer(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    kernel.mutate(
        context,
        "prompt_relay.refuse",
        "prompt:relay",
        &payload(&[
            ("relayId", input.get("relayId")),
            ("answer", input.get("answer")),
            ("reason", input.get("reason")),
        ]),
        |actor| {
            active_pm_of(kernel, &actor.actor_id)?;
            let relay_id = safe_id(&input["relayId"], "relay id")?;
            let row = match relay_row(kernel, relay_id)? {
                Some(row) if row.state == "captured" => row,
                _ => {
                    return Err(KernelError::controller(format!(
                        "relay_not_open: relay {relay_id} is not captured"
                    )))
                }
            };
            let reason = safe_text(&input["reason"], "reason", 1024, false)?;
            record_answer(
                kernel,
                &row,
                &input["answer"],
                &actor.actor_id,
                "refused",
                Some(&reason),
                Some(&json!([])),
            )?;
            Ok(MutationOutput::new(
                record_of(kernel, &row.relay_id)?,
                relay_event(
                    &row.relay_id,
                    Some("captured"),
                    "refused",
                    json!({"reason": reason}),
                ),
            ))
        },
    )
}

/// `finishPromptAnswer`: ends a row that is typing, as the controller's own bookkeeping so it still works when the PM
/// was released meanwhile.
pub fn finish_prompt_answer(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    kernel.mutate(
        context,
        "prompt_relay.finish",
        "controller:reconcile",
        &payload(&[
            ("relayId", input.get("relayId")),
            ("outcome", input.get("outcome")),
        ]),
        |_| {
            let relay_id = safe_id(&input["relayId"], "relay id")?;
            let row = match relay_row(kernel, relay_id)? {
                Some(row) if row.state == "typing" => row,
                _ => {
                    return Err(KernelError::controller(format!(
                        "relay_not_typing: relay {relay_id} is not being typed"
                    )))
                }
            };
            let outcome = &input["outcome"];
            let typed = truthy(&outcome["typed"]);
            let keys = &outcome["keys"];
            let no_keys = keys.as_array().is_some_and(|keys| keys.is_empty());
            let state = if typed {
                "answered"
            } else if no_keys && outcome["failed"] != Value::Bool(true) {
                "refused"
            } else {
                "failed"
            };
            let reason = if typed {
                None
            } else {
                Some(safe_text(&outcome["reason"], "reason", 1024, false)?)
            };
            end_typing(kernel, &row, state, reason.as_deref(), keys)?;
            Ok(MutationOutput::new(
                record_of(kernel, &row.relay_id)?,
                relay_event(
                    &row.relay_id,
                    Some("typing"),
                    state,
                    json!({"reason": reason, "keys": keys}),
                ),
            ))
        },
    )
}

fn end_typing(
    kernel: &Kernel,
    row: &RelayRow,
    state: &str,
    reason: Option<&str>,
    keys: &Value,
) -> KernelResult<()> {
    execute(
        &kernel.database,
        "UPDATE prompt_relays SET state = ?, answered_at = ?, outcome_reason = ?, keys_json = ? WHERE project_id = ? AND relay_id = ?",
        rusqlite::params![state, kernel.now(), reason, stringify(keys), kernel.project_id, row.relay_id],
    )?;
    Ok(())
}

/// `hasExpiredPromptCaptures`: read-only; whether a capture has passed its time to live.
pub fn has_expired_prompt_captures(kernel: &Kernel) -> KernelResult<Value> {
    kernel.assert_open()?;
    let now = kernel.now();
    Ok(Value::Bool(
        query_opt(
            &kernel.database,
            "SELECT 1 AS due FROM prompt_relays WHERE project_id = ? AND state = 'captured' AND expires_at <= ? LIMIT 1",
            [&kernel.project_id, &now],
            |_| Ok(()),
        )?
        .is_some(),
    ))
}

/// `expirePromptCaptures`: captures that passed their time to live become expired.
pub fn expire_prompt_captures(kernel: &Kernel, context: &MutationContext) -> KernelResult<Value> {
    kernel.mutate(
        context,
        "prompt_relay.expire",
        "controller:reconcile",
        &json!({}),
        |_| {
            let now = kernel.now();
            let mut expired: Vec<String> = Vec::new();
            let rows = query_all(
                &kernel.database,
                "SELECT * FROM prompt_relays WHERE project_id = ? AND state = 'captured' ORDER BY sequence",
                [&kernel.project_id],
                RelayRow::from_row,
            )?;
            for row in rows {
                if parse_iso_ms(&row.expires_at) <= parse_iso_ms(&now) {
                    execute(
                        &kernel.database,
                        "UPDATE prompt_relays SET state = 'expired', outcome_reason = 'expired' WHERE project_id = ? AND relay_id = ?",
                        [&kernel.project_id, &row.relay_id],
                    )?;
                    expired.push(row.relay_id.clone());
                }
            }
            Ok(MutationOutput::new(
                json!(expired),
                MutationEvent::new(
                    "prompt_relay",
                    expired.first().map_or("none", String::as_str),
                    0,
                )
                .with_details(json!({"expired": expired})),
            ))
        },
    )
}

/// `failInterruptedPromptRelays`: startup reconciliation: a row left typing by a daemon that stopped is failed.
pub fn fail_interrupted_prompt_relays(
    kernel: &Kernel,
    context: &MutationContext,
) -> KernelResult<Value> {
    kernel.assert_open()?;
    // Nothing typing: no mutation, so a project that never used the relay sees no new ledger events.
    let typing = query_opt(
        &kernel.database,
        "SELECT 1 AS typing FROM prompt_relays WHERE project_id = ? AND state = 'typing' LIMIT 1",
        [&kernel.project_id],
        |_| Ok(()),
    )?;
    if typing.is_none() {
        return Ok(json!([]));
    }
    kernel.mutate(
        context,
        "prompt_relay.interrupted",
        "controller:reconcile",
        &json!({}),
        |_| {
            let mut failed: Vec<String> = Vec::new();
            let rows = query_all(
                &kernel.database,
                "SELECT * FROM prompt_relays WHERE project_id = ? AND state = 'typing' ORDER BY sequence",
                [&kernel.project_id],
                RelayRow::from_row,
            )?;
            for row in rows {
                end_typing(kernel, &row, "failed", Some("interrupted"), &json!([]))?;
                failed.push(row.relay_id.clone());
            }
            Ok(MutationOutput::new(
                json!(failed),
                MutationEvent::new(
                    "prompt_relay",
                    failed.first().map_or("none", String::as_str),
                    0,
                )
                .with_details(json!({"failed": failed})),
            ))
        },
    )
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "configurePromptRelay" => {
            let config = a.value(0);
            Some(call(configure(kernel, config)))
        }
        "promptRelayEnabled" => Some(call(enabled(kernel).map(Value::Bool))),
        "checkPromptAnswer" => {
            let relay_id = arg!(a.str(0, "relayId"));
            let hash = a.value(1);
            let answer = a.value(2);
            Some(call(check_prompt_answer(kernel, relay_id, hash, answer)))
        }
        "promptRelay" => {
            let relay_id = arg!(a.str(0, "relayId"));
            Some(call(prompt_relay(kernel, relay_id)))
        }
        "listPromptRelays" => {
            let limit = arg!(a.opt_i64(0, "limit"));
            Some(call(list_prompt_relays(kernel, limit)))
        }
        "promptRelayStatus" => Some(call(prompt_relay_status(kernel))),
        "recordPromptCapture" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(record_prompt_capture(kernel, &context, input)))
        }
        "beginPromptAnswer" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(begin_prompt_answer(kernel, &context, input)))
        }
        "refusePromptAnswer" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(refuse_prompt_answer(kernel, &context, input)))
        }
        "finishPromptAnswer" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(finish_prompt_answer(kernel, &context, input)))
        }
        "hasExpiredPromptCaptures" => Some(call(has_expired_prompt_captures(kernel))),
        "expirePromptCaptures" => {
            let context = arg!(a.ctx(0));
            Some(call(expire_prompt_captures(kernel, &context)))
        }
        "failInterruptedPromptRelays" => {
            let context = arg!(a.ctx(0));
            Some(call(fail_interrupted_prompt_relays(kernel, &context)))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    /// Every refusal `relayTextProblem` can give is exported from the prompt-relay sequence, with Node's message. (The
    /// refusal for a text that is not well-formed UTF-16 cannot be exported: a lone surrogate is not valid in a Rust
    /// string or in the fixture JSON that Rust reads.)
    #[test]
    fn every_relay_text_refusal_occurs_in_the_export() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/parity/ops.json");
        let export: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let sequence = export["sequences"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == "prompt-relay")
            .expect("the prompt-relay sequence is exported");
        let messages: Vec<&str> = sequence["steps"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|step| step["message"].as_str())
            .collect();
        for refusal in [
            "text_refused: the text is not a string",
            "text_refused: the text is empty",
            "text_refused: the text has a newline, control or format character",
            "text_refused: the text starts with a tab or one of / ! # ? @",
            "text_refused: the text is longer than 1000 bytes",
            "text_refused: the text has leading or trailing whitespace",
            "text_refused: the text is longer than 200 characters",
        ] {
            assert!(
                messages.contains(&refusal),
                "no exported step was refused with {refusal:?}"
            );
        }
    }
}
