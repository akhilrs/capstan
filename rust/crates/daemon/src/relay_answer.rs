//! The capture, check, begin, answer, finish pipeline of a worker's permission prompt, shared by the PM's `prompt`
//! command (handlers/relay.rs) and by the controller's own auto-answer loop (loops/auto_answer.rs).
//!
//! The PM path is the one the command has always had: `begin` runs as the launcher's `before_type` hook, after every
//! check that precedes the first key. The controller path answers inside an approval window: it begins first (so one
//! answer wins and a restart finds the row typing), re-reads the live prompt, and only then lets the adapter type.

use crate::deps::{new_context, Deps};
use crate::handlers::shared::{
    fail, map_error, ok, relay_refusal_text, CommandResponse, ErrorCode, HandlerError, ParsedAnswer,
};
use capstan_herdr::api::RelayOption;
use capstan_herdr::api::{CapturedPrompt, HookError, PromptAnswer, RelayOutcome};
use capstan_herdr::prompt_relay::allow_once_of;
use capstan_kernel::KernelError;
use capstan_launcher::api::{AnswerPromptRequest, LauncherService};
use serde_json::{json, Map, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

// ------------------------------------------------------------------------------------------------ attempt markers

/// How long an attempt marker holds before it counts as stuck and clears itself.
pub const ATTEMPT_STALE_MS: i64 = 30_000;

/// The auto-answer attempts that are running now, in memory only: a restart loses them on purpose (a row left typing is
/// failed at start, and nothing is answered until a new window opens).
#[derive(Default)]
pub struct Attempts {
    running: Mutex<HashMap<String, i64>>,
    disabled: AtomicBool,
}

impl Attempts {
    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, i64>> {
        self.running.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Marks an attempt on `agent_id` as running; false when one already is (a stale marker is replaced).
    pub fn start(&self, agent_id: &str, now_ms: i64) -> bool {
        let mut running = self.map();
        if running
            .get(agent_id)
            .is_some_and(|since| now_ms - *since < ATTEMPT_STALE_MS)
        {
            return false;
        }
        running.insert(agent_id.to_string(), now_ms);
        true
    }

    /// Clears the marker, on finish, on failure or when the attempt gave up.
    pub fn finish(&self, agent_id: &str) {
        self.map().remove(agent_id);
    }

    /// Whether an attempt on `agent_id` is running; a marker older than `ATTEMPT_STALE_MS` is cleared and does not count.
    pub fn running(&self, agent_id: &str, now_ms: i64) -> bool {
        let mut running = self.map();
        match running.get(agent_id) {
            Some(since) if now_ms - *since < ATTEMPT_STALE_MS => true,
            Some(_) => {
                running.remove(agent_id);
                false
            }
            None => false,
        }
    }

    /// The controller may not auto-answer (set at start when its principal lacks `prompt:relay`).
    pub fn disable(&self) {
        self.disabled.store(true, Ordering::SeqCst);
    }

    pub fn disabled(&self) -> bool {
        self.disabled.load(Ordering::SeqCst)
    }
}

/// The attempts of the daemon that keeps `deps.options.state_directory`: the loop, the driver and the PM's `prompt show`
/// of one daemon share one; daemons of other state directories (tests) do not.
pub fn attempts_of(deps: &Deps) -> Arc<Attempts> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Arc<Attempts>>>> = OnceLock::new();
    let mut registry = REGISTRY
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    Arc::clone(
        registry
            .entry(deps.options.state_directory.clone())
            .or_default(),
    )
}

// ------------------------------------------------------------------------------------------------ helpers

pub(crate) fn field(value: &Value, key: &str) -> Value {
    value.get(key).cloned().unwrap_or(Value::Null)
}

/// `refuse` of the Node handler for a failure of the ledger.
pub(crate) fn refuse(error: &KernelError) -> CommandResponse {
    if error.is_controller_error() {
        fail(ErrorCode::Rejected, error.message())
    } else {
        crate::handlers::shared::map_kernel_error(error)
    }
}

/// The shown form of a captured prompt's options (`number`, `text`, `acceptsText`, `widensPermissions`).
fn options_value(prompt: &CapturedPrompt) -> Vec<Value> {
    prompt
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
        .collect()
}

/// Records a capture in the ledger; `auto_window` is the window a controller capture is made under. Returns the relay.
pub fn record_capture(
    deps: &Deps,
    credential: &str,
    captured: &CapturedPrompt,
    auto_window: Option<&str>,
) -> Result<Value, KernelError> {
    let mut input = json!({"prompt": {
        "agentId": captured.agent_id,
        "paneId": captured.pane_id,
        "hostKind": captured.host_kind,
        "text": captured.text,
        "options": options_value(captured),
        "promptSha": captured.prompt_sha,
        "dialog": captured.dialog,
    }});
    if let Some(window) = auto_window {
        input["autoWindowId"] = json!(window);
    }
    let credential = credential.to_string();
    deps.kernel.run(move |core| {
        let context = new_context(core, &credential)?;
        core.record_prompt_capture(&context, &input)
    })
}

/// The answer as the ledger records it.
fn answer_value(answer: &PromptAnswer) -> Value {
    match answer {
        PromptAnswer::Option { number } => json!({"kind": "option", "number": number}),
        PromptAnswer::Esc => json!({"kind": "esc"}),
        PromptAnswer::Text { number, text } => {
            json!({"kind": "text", "number": number, "text": text})
        }
    }
}

/// `core.promptRelay(relayId)?.state`.
fn relay_state(deps: &Deps, relay_id: &str) -> Option<String> {
    let relay_id = relay_id.to_string();
    deps.kernel
        .run(move |core| core.prompt_relay(&relay_id))
        .ok()
        .and_then(|relay| relay["state"].as_str().map(str::to_string))
}

fn finish(deps: &Deps, relay_id: &str, outcome: Value) {
    let (controller, id) = (deps.credential().to_string(), relay_id.to_string());
    let finished = deps.kernel.run(move |core| {
        let context = new_context(core, &controller)?;
        core.finish_prompt_answer(&context, &json!({"relayId": id, "outcome": outcome}))
    });
    if let Err(error) = finished {
        deps.detail_log(
            "prompt_finish_failed",
            json!({"relayId": relay_id, "error": HandlerError::from(error).to_string()}),
        );
    }
}

/// `String(error).replace(/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/gu, " ").slice(0, 200)`.
fn clean_error_text(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| {
            let unsafe_char = c.is_control()
                || matches!(
                    c,
                    '\u{00AD}'
                        | '\u{0600}'..='\u{0605}'
                        | '\u{061C}'
                        | '\u{06DD}'
                        | '\u{070F}'
                        | '\u{180E}'
                        | '\u{200B}'..='\u{200F}'
                        | '\u{2028}'..='\u{202E}'
                        | '\u{2060}'..='\u{2064}'
                        | '\u{2066}'..='\u{206F}'
                        | '\u{FEFF}'
                        | '\u{FFF9}'..='\u{FFFB}'
                );
            if unsafe_char {
                ' '
            } else {
                c
            }
        })
        .collect();
    // `slice(0, 200)` counts UTF-16 units.
    let mut units = 0;
    let mut out = String::new();
    for c in flat.chars() {
        units += c.len_utf16();
        if units > 200 {
            break;
        }
        out.push(c);
    }
    out
}

// ------------------------------------------------------------------------------------------------ the answer

/// Who answers and under what.
pub enum AnswerMode<'a> {
    /// The PM answers; nothing else happens.
    Pm,
    /// The PM answers an option that allows once and opens an approval window from this relay.
    PmOpensWindow {
        /// `{windowKind, maxMinutes}` as the ledger takes it.
        spec: Value,
        /// The configured floor and the tool-prompt switch, for the notice that tells the window opened.
        floor: Vec<String>,
        tool_prompts: bool,
    },
    /// The controller answers inside an open window.
    Auto {
        window_id: &'a str,
        /// Why the action could not be checked against the floor, when it could not.
        unchecked: Option<&'a str>,
        /// Run after the row is typing and before any key: the live prompt must still be the one that was captured.
        recheck: &'a dyn Fn(&Value) -> Result<(), String>,
    },
}

fn window_needs_allow_once() -> CommandResponse {
    fail(
        ErrorCode::Rejected,
        "window_needs_allow_once: only the option that allows once (Yes, no widening, no text) opens a window or is answered inside one",
    )
}

/// Checks, begins, types and finishes one answer to a captured prompt. Every failure before a key is typed leaves the
/// worker's pane as it was.
pub fn run_answer(
    deps: &Deps,
    launcher: &dyn LauncherService,
    credential: &str,
    relay_id: &str,
    hash: &str,
    wanted: &ParsedAnswer,
    mode: &AnswerMode<'_>,
) -> CommandResponse {
    let opens_window = matches!(mode, AnswerMode::PmOpensWindow { .. });
    let checked = {
        let (relay_id, hash, wanted) = (relay_id.to_string(), hash.to_string(), wanted.clone());
        deps.kernel.run(move |core| {
            let stored = core.prompt_relay(&relay_id)?;
            let stored_options: Option<Vec<Value>> = if stored.is_null() {
                None
            } else {
                Some(stored["options"].as_array().cloned().unwrap_or_default())
            };
            if matches!(&stored_options, Some(options) if options.is_empty())
                && wanted != ParsedAnswer::Esc
            {
                return Ok(Err(fail(
                    ErrorCode::Rejected,
                    "no_such_option: only Esc can be sent to an unrecognised dialog",
                )));
            }
            if opens_window {
                let allowed = match (&wanted, &stored_options) {
                    (ParsedAnswer::Option { number }, Some(options)) => {
                        let shown: Vec<RelayOption> = options
                            .iter()
                            .map(|option| RelayOption {
                                number: option["number"].as_i64().unwrap_or(0),
                                text: option["text"].as_str().unwrap_or("").to_string(),
                                accepts_text: option["acceptsText"].as_bool().unwrap_or(false),
                                widens_permissions: option["widensPermissions"]
                                    .as_bool()
                                    .unwrap_or(false),
                            })
                            .collect();
                        allow_once_of(&shown) == Some(*number)
                    }
                    _ => false,
                };
                if !allowed {
                    return Ok(Err(window_needs_allow_once()));
                }
            }
            let answer = match &wanted {
                ParsedAnswer::Option { number } => PromptAnswer::Option { number: *number },
                ParsedAnswer::Esc => PromptAnswer::Esc,
                ParsedAnswer::Text { text } => {
                    // The text goes to the one option that takes text; with none or several there is nothing safe to pick.
                    let targets: Vec<&Value> = stored_options
                        .iter()
                        .flatten()
                        .filter(|option| option["acceptsText"].as_bool().unwrap_or(false))
                        .collect();
                    if stored_options.is_some() && targets.is_empty() {
                        return Ok(Err(fail(
                            ErrorCode::Rejected,
                            "no_text_option: no option of the shown prompt accepts text",
                        )));
                    }
                    if targets.len() > 1 {
                        return Ok(Err(fail(
                            ErrorCode::Rejected,
                            "ambiguous_text_option: more than one option accepts text",
                        )));
                    }
                    PromptAnswer::Text {
                        number: targets
                            .first()
                            .and_then(|option| option["number"].as_i64())
                            .unwrap_or(0),
                        text: text.clone(),
                    }
                }
            };
            let record =
                core.check_prompt_answer(&relay_id, &json!(hash), &answer_value(&answer))?;
            Ok(Ok((record, answer)))
        })
    };
    let (record, answer) = match checked {
        Ok(Ok(pair)) => pair,
        Ok(Err(refusal)) => return refusal,
        Err(error) => return refuse(&error),
    };
    let relay_id = field(&record, "relayId").as_str().unwrap_or("").to_string();
    let agent_id = field(&record, "agentId").as_str().unwrap_or("").to_string();
    let prompt_sha = field(&record, "promptSha")
        .as_str()
        .unwrap_or("")
        .to_string();

    // The input of the ledger's `begin`.
    let mut begin_input =
        json!({"relayId": relay_id, "hash": hash, "answer": answer_value(&answer)});
    match mode {
        AnswerMode::Pm => {}
        AnswerMode::PmOpensWindow {
            spec,
            floor,
            tool_prompts,
        } => {
            begin_input["openWindow"] = spec.clone();
            begin_input["approvalFloor"] = json!(floor);
            begin_input["windowToolPrompts"] = json!(tool_prompts);
        }
        AnswerMode::Auto {
            window_id,
            unchecked,
            ..
        } => {
            begin_input["autoWindowId"] = json!(window_id);
            if let Some(reason) = unchecked {
                begin_input["uncheckedReason"] = json!(reason);
            }
        }
    }
    let begin = |deps: &Deps| {
        let (credential, input) = (credential.to_string(), begin_input.clone());
        deps.kernel.run(move |core| {
            let context = new_context(core, &credential)?;
            core.begin_prompt_answer(&context, &input)
        })
    };

    let begin_failure: RefCell<Option<KernelError>> = RefCell::new(None);
    let begun: RefCell<Option<Value>> = RefCell::new(None);
    let auto = matches!(mode, AnswerMode::Auto { .. });
    if let AnswerMode::Auto { recheck, .. } = mode {
        // The controller moves the row to typing first: one answer wins, and a restart finds the row typing.
        match begin(deps) {
            Ok(value) => *begun.borrow_mut() = Some(value),
            Err(error) => return refuse(&error),
        }
        if let Err(reason) = recheck(&record) {
            finish(
                deps,
                &relay_id,
                json!({"typed": false, "reason": reason, "keys": []}),
            );
            return fail(ErrorCode::Rejected, format!("recheck_failed: {reason}"));
        }
    }
    let outcome = {
        let mut before_type = || -> Result<(), HookError> {
            if auto {
                return Ok(());
            }
            match begin(deps) {
                Ok(value) => {
                    *begun.borrow_mut() = Some(value);
                    Ok(())
                }
                Err(error) => {
                    let hook = HookError::new("controller", error.message());
                    *begin_failure.borrow_mut() = Some(error);
                    Err(hook)
                }
            }
        };
        launcher.answer_prompt(
            &agent_id,
            AnswerPromptRequest {
                prompt_sha: &prompt_sha,
                answer: &answer,
                before_type: &mut before_type,
            },
        )
    };
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            // Keys may already have been sent: a row that reached typing is failed, never left open.
            let thrown = begin_failure.into_inner();
            let text = match &thrown {
                Some(kernel_error) => kernel_error.to_string(),
                None => HandlerError::from(error.clone()).to_string(),
            };
            if relay_state(deps, &relay_id).as_deref() == Some("typing") {
                finish(
                    deps,
                    &relay_id,
                    json!({
                        "typed": false,
                        "reason": format!("error: {}", clean_error_text(&text)),
                        "keys": [],
                        "failed": true,
                    }),
                );
            }
            return match thrown {
                Some(kernel_error) => refuse(&kernel_error),
                None => map_error(&HandlerError::from(error)),
            };
        }
    };
    let state = relay_state(deps, &relay_id);
    match outcome {
        RelayOutcome::Typed {
            keys,
            input_readable,
        } => {
            if state.as_deref() == Some("typing") {
                finish(deps, &relay_id, json!({"typed": true, "keys": keys}));
            }
            if let Some(readable) = input_readable {
                let note = if readable {
                    "Esc was sent once and the input box reads again; the waiting message is delivered on the next tick".to_string()
                } else {
                    format!("Esc was sent once but the input box is not readable yet; this is not a failure, look with cstan observe {agent_id}")
                };
                return ok(json!({
                    "relayId": relay_id,
                    "agentId": agent_id,
                    "state": "answered",
                    "inputReadable": readable,
                    "note": note,
                }));
            }
            let mut result = Map::new();
            result.insert("relayId".into(), json!(relay_id));
            result.insert("agentId".into(), json!(agent_id));
            result.insert("state".into(), json!("answered"));
            if let Some(window) = begun
                .borrow()
                .as_ref()
                .map(|record| field(record, "window"))
                .filter(|window| !window.is_null())
            {
                result.insert("window".into(), window);
            }
            ok(Value::Object(result))
        }
        RelayOutcome::Refused { reason, keys } => {
            let message = format!("{}: {}", reason.as_str(), relay_refusal_text(reason));
            if state.as_deref() == Some("typing") {
                finish(
                    deps,
                    &relay_id,
                    json!({"typed": false, "reason": reason.as_str(), "keys": keys}),
                );
            } else if state.as_deref() == Some("captured") {
                let (credential, id, shown) = (
                    credential.to_string(),
                    relay_id.clone(),
                    answer_value(&answer),
                );
                let refused = deps.kernel.run(move |core| {
                    let context = new_context(core, &credential)?;
                    core.refuse_prompt_answer(
                        &context,
                        &json!({"relayId": id, "answer": shown, "reason": reason.as_str()}),
                    )
                });
                if let Err(error) = refused {
                    deps.detail_log(
                        "prompt_refuse_failed",
                        json!({"relayId": relay_id, "error": HandlerError::from(error).to_string()}),
                    );
                }
            }
            fail(ErrorCode::Rejected, message)
        }
    }
}
