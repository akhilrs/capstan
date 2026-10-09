//! Types shared by the Herdr layer and the launcher for relaying a worker's blocking permission prompt: what was
//! captured, how it is hashed, and which answers may be typed (src/herdr/prompt-relay.ts). Nothing here talks to Herdr.
//! The value types are in `api`; the checks below are the Node ones. (The kernel's own copy of the text check works on
//! JSON values and is private to it.)

use crate::adapter::validate::{command_start, has_unsafe, js_trim};
use crate::api::{CapturedPrompt, PromptAnswer, RelayOption, RelayRefusal};
use crate::api::{RELAY_PROMPT_MAX_BYTES, RELAY_TEXT_MAX_BYTES};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

/// The input of a prompt hash: a prompt before its hash is known.
pub struct HashInput<'a> {
    pub agent_id: &'a str,
    pub pane_id: &'a str,
    pub host_kind: &'a str,
    pub text: &'a str,
    pub options: &'a [RelayOption],
    pub dialog: bool,
}

/// The SHA-256 of a canonical JSON of the prompt, with a fixed key order.
pub fn prompt_hash(input: &HashInput<'_>) -> String {
    let options: Vec<Value> = input
        .options
        .iter()
        .map(|option| {
            json!({
                "number": option.number,
                "text": option.text,
                "acceptsText": option.accepts_text,
                "widensPermissions": option.widens_permissions,
            })
        })
        .collect();
    let mut canonical = Map::new();
    canonical.insert("agentId".into(), json!(input.agent_id));
    canonical.insert("paneId".into(), json!(input.pane_id));
    canonical.insert("hostKind".into(), json!(input.host_kind));
    canonical.insert("text".into(), json!(input.text));
    canonical.insert("options".into(), Value::Array(options));
    if input.dialog {
        canonical.insert("dialog".into(), json!(true));
    }
    let text = Value::Object(canonical).to_string();
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Why a text answer may not be typed, or None when it is acceptable.
pub fn relay_text_problem(text: &str) -> Option<String> {
    if js_trim(text).is_empty() {
        return Some("the text is empty".into());
    }
    if has_unsafe(text) {
        return Some("the text has a newline, control or format character".into());
    }
    if command_start(text) {
        return Some("the text starts with a tab or one of / ! # ? @".into());
    }
    if text.len() > RELAY_TEXT_MAX_BYTES {
        return Some(format!(
            "the text is longer than {RELAY_TEXT_MAX_BYTES} bytes"
        ));
    }
    None
}

/// Checks an answer against the captured prompt; None means it may be typed.
pub fn check_answer(prompt: &CapturedPrompt, answer: &PromptAnswer) -> Option<RelayRefusal> {
    let number = match answer {
        PromptAnswer::Esc => return None,
        PromptAnswer::Option { number } | PromptAnswer::Text { number, .. } => *number,
    };
    let Some(option) = prompt.options.iter().find(|o| o.number == number) else {
        return Some(RelayRefusal::NoSuchOption);
    };
    match answer {
        PromptAnswer::Text { text, .. } => {
            if !option.accepts_text {
                Some(RelayRefusal::NoTextOption)
            } else if relay_text_problem(text).is_some() {
                Some(RelayRefusal::TextRefused)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// A prompt text is within the size the relay accepts.
pub fn prompt_text_fits(text: &str) -> bool {
    text.len() <= RELAY_PROMPT_MAX_BYTES
}
