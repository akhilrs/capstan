//! Input to a started agent: guarded send, PM wake, clear, trust dialog, prompt relay and interrupt
//! (src/herdr/adapter-input.ts).

use super::*;
use crate::prompt_relay::{check_answer, prompt_hash, relay_text_problem, HashInput};
use crate::screen::{
    parse_blocking_dialog, parse_host_prompt, parse_trust_dialog_of, prompt_footer,
    text_field_wording, trust_texts, TrustDialog,
};
use std::collections::BTreeMap;

const MAX_CLEAR_ROUNDS: usize = 5;
const SELECTION_REDRAW_MS: u64 = 3_000;

fn deferral_for(status: &str) -> Option<DeferralReason> {
    if status == "idle" || status == "done" {
        return None;
    }
    Some(if status == "blocked" {
        DeferralReason::AgentBlocked
    } else {
        DeferralReason::AgentBusy
    })
}

/// One read of a permission prompt.
struct PromptRead {
    prompt: CapturedPrompt,
    selected_index: usize,
    options: Vec<RelayOption>,
    prompt_text: String,
    footer: Option<String>,
}

enum Check {
    Ok { selected: usize, target: usize },
    Failed(String),
}

fn refuse(reason: RelayRefusal, keys: &[String]) -> RelayOutcome {
    RelayOutcome::Refused {
        reason,
        keys: keys.to_vec(),
    }
}

impl Adapter {
    fn send_key(
        &self,
        pane_id: &str,
        key: &str,
        reason: &str,
        log: KeyLogger<'_>,
    ) -> AdapterResult<()> {
        log(&KeyLogEntry {
            pane: pane_id.to_string(),
            key: key.to_string(),
            reason: reason.to_string(),
        });
        self.run_checked(&strings(&["pane", "send-keys", pane_id, key]))
    }

    pub fn guarded_send_of(&self, input: GuardedSendInput<'_>) -> AdapterResult<SendOutcome> {
        let entry = self.assert_typable(input.pane_id, Action::Send)?;
        if !is_safe_text(input.text)
            || command_start(input.text)
            || input.text.len() > MAX_TEXT_BYTES
        {
            return Err(invalid("message text is not acceptable"));
        }
        let Some(agent) = entry.agent.clone() else {
            return Err(AdapterError::Phase("the pane has no agent".into()));
        };
        let first = self.state_for(&agent, input.pane_id)?;
        if let Some(reason) = deferral_for(&first) {
            return Ok(SendOutcome::Deferred {
                reason,
                detail: None,
                blocker: None,
            });
        }
        let (typed, blocker) = self.input_and_blocker(input.pane_id)?;
        let Some(typed) = typed else {
            return Ok(SendOutcome::Deferred {
                reason: DeferralReason::InputNotEmpty,
                detail: Some(INPUT_UNREADABLE_DETAIL.to_string()),
                blocker: Some(blocker),
            });
        };
        if !typed.is_empty() {
            return Ok(SendOutcome::Deferred {
                reason: DeferralReason::InputNotEmpty,
                detail: None,
                blocker: None,
            });
        }
        if let Some(reason) = deferral_for(&self.state_for(&agent, input.pane_id)?) {
            return Ok(SendOutcome::Deferred {
                reason,
                detail: None,
                blocker: None,
            });
        }
        (input.before_send)()?;
        let args = strings(&["agent", "prompt", &self.herdr_name(&agent)?, input.text]);
        if self.json(&args).is_err() {
            return Err(AdapterError::SendAfterRecord(
                "the message was recorded but Herdr did not accept the prompt".into(),
            ));
        }
        Ok(SendOutcome::Sent)
    }

    /// Types a short wake line into the PM's pane, which `guarded_send` never does. Only a registered, started PM pane
    /// is typed into, and only when Herdr reports it idle or done, its input line reads as empty, and both hold again
    /// right before the keys. A line that cannot be read or has text on it is never typed over.
    pub fn wake_pm_of(&self, input: WakePmInput<'_>) -> AdapterResult<WakeOutcome> {
        require_match(input.pane_id, &PANE_PATTERN, "pane id")?;
        let Some(entry) = self.entry(input.pane_id) else {
            return Err(AdapterError::UnknownPane("pane is not registered".into()));
        };
        if entry.role != PaneRole::Pm || entry.phase != PanePhase::Started {
            return Err(AdapterError::Phase(
                "only a started PM pane can be woken".into(),
            ));
        }
        let Some(agent) = entry.agent.clone() else {
            return Err(AdapterError::Phase("the pane has no agent".into()));
        };
        if !is_safe_text(input.text)
            || input.text.contains(['\r', '\n'])
            || command_start(input.text)
            || input.text.len() > MAX_TEXT_BYTES
        {
            return Err(invalid("wake text is not acceptable"));
        }
        let idle = || -> AdapterResult<bool> {
            Ok(deferral_for(&self.state_for(&agent, input.pane_id)?).is_none())
        };
        if !idle()? {
            return Ok(WakeOutcome::PmNotIdle);
        }
        if self.input_of(input.pane_id)?.as_deref() != Some("") {
            return Ok(WakeOutcome::InputNotEmpty);
        }
        if !idle()? {
            return Ok(WakeOutcome::PmNotIdle);
        }
        (input.before_send)()?;
        self.json(&strings(&[
            "agent",
            "prompt",
            &self.herdr_name(&agent)?,
            input.text,
        ]))?;
        Ok(WakeOutcome::Sent)
    }

    pub fn clear_after_deferral_of(&self, input: ClearInput<'_>) -> AdapterResult<ClearOutcome> {
        if !input.deferred_for_ms.is_finite()
            || !input.max_deferral_ms.is_finite()
            || input.deferred_for_ms < 0.0
            || input.max_deferral_ms <= 0.0
        {
            return Err(invalid(
                "the deferral times must be finite, and the maximum must be positive",
            ));
        }
        if input.deferred_for_ms < input.max_deferral_ms {
            return Err(AdapterError::DeferralNotElapsed(
                "the maximum deferral has not elapsed".into(),
            ));
        }
        let entry = self.assert_typable(input.pane_id, Action::Clear)?;
        let Some(agent) = entry.agent.clone() else {
            return Err(AdapterError::Phase("the pane has no agent".into()));
        };
        let status = self.state_for(&agent, input.pane_id)?;
        if deferral_for(&status).is_some() {
            return Err(AdapterError::NotIdle(
                "only an idle agent's input line is cleared".into(),
            ));
        }
        let (text, blocker) = self.input_and_blocker(input.pane_id)?;
        let Some(text) = text else {
            return Err(AdapterError::InputUnreadable {
                message: "the input line cannot be read, so its text cannot be logged".into(),
                blocker,
            });
        };
        if text.is_empty() {
            return Ok(ClearOutcome {
                cleared: false,
                text: String::new(),
            });
        }
        (input.discard)(&text)?;
        let mut known = text.clone();
        for round in 0..MAX_CLEAR_ROUNDS {
            if round > 0 {
                let again = self.state_for(&agent, input.pane_id)?;
                if deferral_for(&again).is_some() {
                    return Err(AdapterError::NotIdle(
                        "the agent stopped being idle during the clear".into(),
                    ));
                }
            }
            self.send_key(
                input.pane_id,
                "ctrl+u",
                "clear the input line after the maximum deferral",
                input.log,
            )?;
            let (remaining, after) = self.input_and_blocker(input.pane_id)?;
            match remaining {
                Some(remaining) if remaining.is_empty() => {
                    return Ok(ClearOutcome {
                        cleared: true,
                        text,
                    });
                }
                None => {
                    return Err(AdapterError::InputUnreadable {
                        message: "the input line cannot be read after a clear key".into(),
                        blocker: after,
                    });
                }
                Some(remaining) => {
                    if !known.contains(&remaining) {
                        known.push('\n');
                        known.push_str(&remaining);
                        (input.discard)(&remaining)?;
                    }
                }
            }
        }
        Err(AdapterError::ClearFailed(format!(
            "the input line is not empty after {MAX_CLEAR_ROUNDS} rounds"
        )))
    }

    fn same_path(shown: &str, expected: &str) -> bool {
        let normalize = |value: &str| {
            let trimmed = if value.len() > 1 {
                value.trim_end_matches('/')
            } else {
                value
            };
            std::fs::canonicalize(trimmed)
        };
        match (normalize(shown), normalize(expected)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    }

    pub fn answer_trust_dialog_of(
        &self,
        input: TrustDialogInput<'_>,
    ) -> AdapterResult<DialogOutcome> {
        let entry = self.assert_typable(input.pane_id, Action::Dialog)?;
        let (Some(agent), Some(worktree)) = (entry.agent.clone(), entry.worktree_path.clone())
        else {
            return Err(AdapterError::Phase(
                "only a worktree pane the adapter created has a dialog it may answer".into(),
            ));
        };
        let Some((yes, no)) = trust_texts(&entry.kind) else {
            return Ok(DialogOutcome::Unhandled {
                reason: "host_has_no_trust_dialog".into(),
            });
        };
        let state = self.agent_state_of(&agent)?;
        if state.pane_id != input.pane_id || state.status != "blocked" {
            return Err(AdapterError::NotBlocked(
                "the agent is not blocked at its own pane".into(),
            ));
        }
        let check = || -> AdapterResult<Check> {
            let screen = self.screen(input.pane_id, false, None)?;
            let Some(dialog) = parse_trust_dialog_of(&entry.kind, &screen) else {
                return Ok(Check::Failed("no_dialog".into()));
            };
            let TrustDialog::Dialog {
                path,
                options,
                selected_index,
                confirm_is_last_line,
            } = dialog
            else {
                return Ok(Check::Failed("wrapped_path".into()));
            };
            if !confirm_is_last_line {
                return Ok(Check::Failed("dialog_not_last".into()));
            }
            if !Self::same_path(&path, &worktree) {
                return Ok(Check::Failed("path_mismatch".into()));
            }
            let shown: Vec<&str> = options.iter().map(|o| o.text.as_str()).collect();
            match selected_index {
                Some(selected)
                    if shown.len() == 2 && shown.contains(&yes) && shown.contains(&no) =>
                {
                    Ok(Check::Ok {
                        selected,
                        target: shown.iter().position(|t| *t == yes).expect("checked above"),
                    })
                }
                _ => Ok(Check::Failed("unknown_options".into())),
            }
        };
        let (selected, target) = match check()? {
            Check::Failed(reason) => return Ok(DialogOutcome::Unhandled { reason }),
            Check::Ok { selected, target } => (selected, target),
        };
        let mut keys: Vec<String> = Vec::new();
        let steps = target as i64 - selected as i64;
        for _ in 0..steps.unsigned_abs() {
            let key = if steps > 0 { "down" } else { "up" };
            self.send_key(
                input.pane_id,
                key,
                "move the trust dialog selection to the trusted option",
                input.log,
            )?;
            keys.push(key.to_string());
        }
        // The screen redraws a moment after a key, so the selection is polled for a short while; the Enter below is
        // still sent only after a read shows it.
        let redraw_deadline = self.now() + SELECTION_REDRAW_MS;
        let mut second = check()?;
        // A half-drawn screen can read as no dialog, unknown options or a dialog that is not last; those are waited
        // out. A different path or a wrapped path is not a redraw problem and stops the answer at once.
        let transient = |result: &Check| match result {
            Check::Ok { selected, target } => selected != target,
            Check::Failed(reason) => {
                matches!(
                    reason.as_str(),
                    "no_dialog" | "unknown_options" | "dialog_not_last"
                )
            }
        };
        while !keys.is_empty() && transient(&second) && self.now() < redraw_deadline {
            self.sleep(self.poll_ms());
            second = check()?;
        }
        let (selected, target) = match second {
            Check::Failed(reason) => return Ok(DialogOutcome::Unhandled { reason }),
            Check::Ok { selected, target } => (selected, target),
        };
        if selected != target {
            return Ok(DialogOutcome::Unhandled {
                reason: "selection_not_reached".into(),
            });
        }
        self.send_key(
            input.pane_id,
            "enter",
            "confirm the trusted option",
            input.log,
        )?;
        keys.push("enter".into());
        let deadline = self.now() + input.timeout_ms.unwrap_or(10_000);
        while self.now() < deadline {
            let screen = self.screen(input.pane_id, false, None)?;
            if parse_trust_dialog_of(&entry.kind, &screen).is_none() {
                return Ok(DialogOutcome::Handled { keys });
            }
            self.sleep(self.poll_ms());
        }
        Err(AdapterError::DialogStillOpen(
            "the trust dialog is still open after the answer".into(),
        ))
    }

    /// Reads the blocking permission prompt of a started worker pane. Only a blocked claude agent at its own pane whose
    /// screen is a fixture-proven dialog is captured; nothing is typed.
    pub fn capture_prompt_of(&self, pane_id: &str) -> AdapterResult<CaptureOutcome> {
        let entry = self.assert_typable(pane_id, Action::Dialog)?;
        let Some(agent) = entry.agent.clone() else {
            return Err(AdapterError::Phase("the pane has no agent".into()));
        };
        let refusal = self.relay_preflight(&entry, pane_id)?;
        if refusal.is_none() {
            if let Some(read) = self.read_prompt(&agent, pane_id, &entry.kind)? {
                return Ok(CaptureOutcome::Captured(read.prompt));
            }
        }
        // A dialog that is not a permission prompt may not set Herdr blocked.
        if entry.kind == "claude" && self.not_working(&agent, pane_id)? {
            if let Some(dialog) = self.read_dialog(&agent, pane_id, &entry.kind)? {
                return Ok(CaptureOutcome::Captured(dialog));
            }
        }
        Ok(CaptureOutcome::Refused(
            refusal.unwrap_or(RelayRefusal::PromptUnrecognized),
        ))
    }

    /// Types an answer to a captured prompt. The screen is read again and must hash to `prompt_sha` before the first
    /// key and before each Enter; arrows are sent one at a time and the Enter only after a read shows the target option
    /// selected. `before_type` runs once, after every check that precedes the first key and before it.
    pub fn answer_prompt_of(&self, input: AnswerPromptInput<'_>) -> AdapterResult<RelayOutcome> {
        let AnswerPromptInput {
            pane_id,
            prompt_sha,
            answer,
            before_type,
            log,
        } = input;
        let entry = self.assert_typable(pane_id, Action::Dialog)?;
        let Some(agent) = entry.agent.clone() else {
            return Err(AdapterError::Phase("the pane has no agent".into()));
        };
        let mut keys: Vec<String> = Vec::new();
        if entry.kind == "claude" {
            let screen = self.screen(pane_id, true, None)?;
            if parse_host_prompt(&entry.kind, &screen).is_none() {
                if parse_blocking_dialog(&entry.kind, &screen).is_some() {
                    return self.answer_dialog(
                        &entry,
                        pane_id,
                        prompt_sha,
                        answer,
                        before_type,
                        log,
                        &screen,
                    );
                }
                // Neither a permission prompt nor a dialog: nothing here may be answered.
                return Ok(refuse(RelayRefusal::PromptUnrecognized, &keys));
            }
        }
        if let Some(preflight) = self.relay_preflight(&entry, pane_id)? {
            return Ok(refuse(preflight, &keys));
        }
        let Some(first) = self.read_prompt(&agent, pane_id, &entry.kind)? else {
            return Ok(refuse(RelayRefusal::PromptUnrecognized, &keys));
        };
        if first.prompt.prompt_sha != prompt_sha {
            return Ok(refuse(RelayRefusal::PromptChanged, &keys));
        }
        if let Some(problem) = check_answer(&first.prompt, answer) {
            return Ok(refuse(problem, &keys));
        }
        if let PromptAnswer::Text { text, .. } = answer {
            if relay_text_problem(text).is_some() {
                return Ok(refuse(RelayRefusal::TextRefused, &keys));
            }
        }

        let mut announced = false;
        let mut press = |this: &Adapter,
                         keys: &mut Vec<String>,
                         key: &str,
                         reason: &str|
         -> AdapterResult<()> {
            if !announced {
                announced = true;
                before_type()?;
            }
            this.send_key(pane_id, key, reason, log)?;
            keys.push(key.to_string());
            Ok(())
        };
        let still_blocked = || self.is_blocked(&agent, pane_id);

        let number = match answer {
            PromptAnswer::Esc => {
                if !still_blocked()? {
                    return Ok(refuse(RelayRefusal::NotBlocked, &keys));
                }
                press(
                    self,
                    &mut keys,
                    "esc",
                    "dismiss the worker's permission prompt",
                )?;
                return Ok(RelayOutcome::Typed {
                    keys,
                    input_readable: None,
                });
            }
            PromptAnswer::Option { number } | PromptAnswer::Text { number, .. } => *number,
        };

        let target = (number - 1) as usize;
        let steps = target as i64 - first.selected_index as i64;
        for _ in 0..steps.unsigned_abs() {
            press(
                self,
                &mut keys,
                if steps > 0 { "down" } else { "up" },
                "move the prompt selection to the answer",
            )?;
        }
        // The screen redraws a moment after a key, so the selection is polled for a short while; the Enter below is
        // still sent only after a read shows it.
        let settled = match self.await_prompt(
            &agent,
            pane_id,
            &entry.kind,
            &|read: &PromptRead| read.selected_index == target,
            Some(prompt_sha),
        )? {
            Ok(read) => read,
            Err(reason) => return Ok(refuse(reason, &keys)),
        };
        if !still_blocked()? {
            return Ok(refuse(RelayRefusal::NotBlocked, &keys));
        }

        let text = match answer {
            PromptAnswer::Option { .. } => {
                press(self, &mut keys, "enter", "confirm the selected answer")?;
                return Ok(RelayOutcome::Typed {
                    keys,
                    input_readable: None,
                });
            }
            PromptAnswer::Text { text, .. } => text,
            PromptAnswer::Esc => unreachable!("handled above"),
        };

        let original = settled.options[target].text.clone();
        let Some(field_wording) = text_field_wording(&original) else {
            return Ok(refuse(RelayRefusal::NoTextOption, &keys));
        };
        // Only the target option may differ from the prompt as it was captured.
        let same_except_target = |read: &PromptRead, expected: &str| -> bool {
            read.selected_index == target
                && read.prompt_text == settled.prompt_text
                && read.options.len() == settled.options.len()
                && read.options[target].text == expected
                && read.options.iter().enumerate().all(|(index, entry)| {
                    index == target || entry.text == settled.options[index].text
                })
        };
        press(self, &mut keys, "tab", "open the option's text field")?;
        // Esc would cancel the whole prompt, so a field that is not as expected is left as it is.
        let opened = self.await_prompt(
            &agent,
            pane_id,
            &entry.kind,
            &|read: &PromptRead| {
                read.options.get(target).map(|o| o.text.as_str()) != Some(original.as_str())
            },
            None,
        )?;
        let opened = match opened {
            Ok(read)
                if same_except_target(&read, field_wording)
                    && read.footer.as_deref() == Some("Esc to cancel") =>
            {
                read
            }
            _ => return Ok(refuse(RelayRefusal::TextFieldNotOpen, &keys)),
        };
        let _ = opened;
        if !still_blocked()? {
            return Ok(refuse(RelayRefusal::NotBlocked, &keys));
        }
        log(&KeyLogEntry {
            pane: pane_id.to_string(),
            key: "text".into(),
            reason: format!("type {} bytes into the open text field", text.len()),
        });
        self.run_checked(&strings(&["pane", "send-text", pane_id, text]))?;
        keys.push("text".into());
        let expected = format!("{original}, {}", js_trim_end(text));
        let typed = self.await_prompt(
            &agent,
            pane_id,
            &entry.kind,
            &|read: &PromptRead| same_except_target(read, &expected),
            None,
        )?;
        if typed.is_err() {
            return Ok(refuse(RelayRefusal::TextFieldNotOpen, &keys));
        }
        if !still_blocked()? {
            return Ok(refuse(RelayRefusal::NotBlocked, &keys));
        }
        press(self, &mut keys, "enter", "submit the typed answer")?;
        Ok(RelayOutcome::Typed {
            keys,
            input_readable: None,
        })
    }

    /// Interrupts a working agent with exactly one Esc. Nothing is sent when Herdr does not show the agent working, and
    /// no second key is ever sent.
    pub fn interrupt_working_of(&self, pane_id: &str, log: KeyLogger<'_>) -> AdapterResult<bool> {
        let entry = self.assert_typable(pane_id, Action::Dialog)?;
        let Some(agent) = entry.agent else {
            return Err(AdapterError::Phase("the pane has no agent".into()));
        };
        if self.state_for(&agent, pane_id)? != "working" {
            return Ok(false);
        }
        self.send_key(pane_id, "esc", "interrupt a paused worker", log)?;
        Ok(true)
    }

    /// Answers an Esc-only dialog relay: the hash must match the screen just read, the agent must not be working, and
    /// exactly one Esc is sent. Then the input line is polled for a short while; no second key is ever sent.
    #[allow(clippy::too_many_arguments)]
    fn answer_dialog(
        &self,
        entry: &PaneEntry,
        pane_id: &str,
        prompt_sha: &str,
        answer: &PromptAnswer,
        before_type: &mut dyn FnMut() -> HookResult,
        log: KeyLogger<'_>,
        screen: &str,
    ) -> AdapterResult<RelayOutcome> {
        let agent = entry.agent.clone().expect("checked by the caller");
        if !matches!(answer, PromptAnswer::Esc) {
            return Ok(refuse(RelayRefusal::NoSuchOption, &[]));
        }
        let Some(read) = Self::dialog_of(&agent, pane_id, &entry.kind, screen) else {
            return Ok(refuse(RelayRefusal::PromptUnrecognized, &[]));
        };
        if read.prompt_sha != prompt_sha {
            return Ok(refuse(RelayRefusal::PromptChanged, &[]));
        }
        if !self.not_working(&agent, pane_id)? {
            return Ok(refuse(RelayRefusal::NotBlocked, &[]));
        }
        before_type()?;
        self.send_key(pane_id, "esc", "dismiss the worker's blocking dialog", log)?;
        let deadline = self.now() + SELECTION_REDRAW_MS;
        let input_readable = loop {
            let readable = self.input_of(pane_id)?.is_some();
            if readable || self.now() >= deadline {
                break readable;
            }
            self.sleep(self.poll_ms());
        };
        Ok(RelayOutcome::Typed {
            keys: vec!["esc".into()],
            input_readable: Some(input_readable),
        })
    }

    /// True unless Herdr shows the agent working; a name that points at another pane is not usable here.
    fn not_working(&self, agent: &str, pane_id: &str) -> AdapterResult<bool> {
        match self.state_for(agent, pane_id) {
            Ok(status) => Ok(status != "working"),
            Err(AdapterError::AgentPaneMismatch(_)) => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn dialog_of(agent: &str, pane_id: &str, kind: &str, screen: &str) -> Option<CapturedPrompt> {
        let text = parse_blocking_dialog(kind, screen)?;
        let sha = prompt_hash(&HashInput {
            agent_id: agent,
            pane_id,
            host_kind: kind,
            text: &text,
            options: &[],
            dialog: true,
        });
        Some(CapturedPrompt {
            agent_id: agent.to_string(),
            pane_id: pane_id.to_string(),
            host_kind: kind.to_string(),
            text,
            options: Vec::new(),
            prompt_sha: sha,
            dialog: true,
        })
    }

    fn read_dialog(
        &self,
        agent: &str,
        pane_id: &str,
        kind: &str,
    ) -> AdapterResult<Option<CapturedPrompt>> {
        let screen = self.screen(pane_id, true, None)?;
        Ok(Self::dialog_of(agent, pane_id, kind, &screen))
    }

    /// Why a prompt may not be read at all: another kind of host, or an agent that is not blocked.
    fn relay_preflight(
        &self,
        entry: &PaneEntry,
        pane_id: &str,
    ) -> AdapterResult<Option<RelayRefusal>> {
        if entry.kind != "claude" {
            return Ok(Some(RelayRefusal::UnsupportedHost));
        }
        let agent = entry.agent.as_deref().unwrap_or("");
        Ok(if self.is_blocked(agent, pane_id)? {
            None
        } else {
            Some(RelayRefusal::NotBlocked)
        })
    }

    /// True only when Herdr shows the agent blocked at this very pane; a name that points at another pane is not
    /// blocked here.
    fn is_blocked(&self, agent: &str, pane_id: &str) -> AdapterResult<bool> {
        match self.state_for(agent, pane_id) {
            Ok(status) => Ok(status == "blocked"),
            Err(AdapterError::AgentPaneMismatch(_)) => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn read_prompt(
        &self,
        agent: &str,
        pane_id: &str,
        kind: &str,
    ) -> AdapterResult<Option<PromptRead>> {
        let screen = self.screen(pane_id, true, None)?;
        let Some(parsed) = parse_host_prompt(kind, &screen) else {
            return Ok(None);
        };
        let sha = prompt_hash(&HashInput {
            agent_id: agent,
            pane_id,
            host_kind: kind,
            text: &parsed.text,
            options: &parsed.options,
            dialog: false,
        });
        Ok(Some(PromptRead {
            prompt: CapturedPrompt {
                agent_id: agent.to_string(),
                pane_id: pane_id.to_string(),
                host_kind: kind.to_string(),
                text: parsed.text.clone(),
                options: parsed.options.clone(),
                prompt_sha: sha,
                dialog: false,
            },
            selected_index: parsed.selected_index,
            options: parsed.options,
            prompt_text: parsed.text,
            footer: prompt_footer(&screen),
        }))
    }

    /// Polls the prompt until `done` holds for it. A read that does not parse or does not satisfy `done` is waited out
    /// for a short while; a read whose hash differs from `sha` stops the answer at once.
    fn await_prompt(
        &self,
        agent: &str,
        pane_id: &str,
        kind: &str,
        done: &dyn Fn(&PromptRead) -> bool,
        sha: Option<&str>,
    ) -> AdapterResult<Result<PromptRead, RelayRefusal>> {
        let deadline = self.now() + SELECTION_REDRAW_MS;
        loop {
            let read = self.read_prompt(agent, pane_id, kind)?;
            if let Some(read) = read {
                if let Some(sha) = sha {
                    if read.prompt.prompt_sha != sha {
                        return Ok(Err(RelayRefusal::PromptChanged));
                    }
                }
                if done(&read) {
                    return Ok(Ok(read));
                }
                if self.now() >= deadline {
                    return Ok(Err(RelayRefusal::SelectionNotReached));
                }
            } else if self.now() >= deadline {
                return Ok(Err(RelayRefusal::PromptUnrecognized));
            }
            self.sleep(self.poll_ms());
        }
    }
}

// ------------------------------------------------------------------------------------------------ the traits

impl DriverAdapter for Adapter {
    fn guarded_send(&self, input: GuardedSendInput<'_>) -> AdapterResult<SendOutcome> {
        self.guarded_send_of(input)
    }

    fn wake_pm(&self, input: WakePmInput<'_>) -> AdapterResult<WakeOutcome> {
        self.wake_pm_of(input)
    }

    fn clear_after_deferral(&self, input: ClearInput<'_>) -> AdapterResult<ClearOutcome> {
        self.clear_after_deferral_of(input)
    }
}

impl LauncherAdapter for Adapter {
    fn create_workspace(&self, input: CreateWorkspaceInput<'_>) -> AdapterResult<WorkspaceCreated> {
        self.create_workspace_of(&input)
    }

    fn create_worktree(&self, input: CreateWorktreeInput<'_>) -> AdapterResult<WorktreeCreated> {
        self.create_worktree_of(&input)
    }

    fn create_tab(&self, input: CreateTabInput<'_>) -> AdapterResult<TabCreated> {
        self.create_tab_of(&input)
    }

    fn pane_layout(&self, pane_id: &str) -> AdapterResult<PaneLayoutView> {
        self.pane_layout_of(pane_id)
    }

    fn place_pane(&self, input: PlacePaneInput<'_>) -> AdapterResult<PaneAtPath> {
        self.place_pane_of(&input)
    }

    fn panes_at_path(&self, directory: &str) -> AdapterResult<Vec<PaneAtPath>> {
        self.panes_at_path_of(directory)
    }

    fn prepare_shell(&self, input: PrepareShellInput<'_>) -> AdapterResult<()> {
        self.prepare_shell_pane(&input)
    }

    fn start_agent(&self, input: StartAgentInput<'_>) -> AdapterResult<StartStatus> {
        self.start_agent_in_pane(&input)
    }

    fn answer_trust_dialog(&self, input: TrustDialogInput<'_>) -> AdapterResult<DialogOutcome> {
        self.answer_trust_dialog_of(input)
    }

    fn close_pane(&self, pane_id: &str) -> AdapterResult<()> {
        self.close_pane_of(pane_id)
    }

    fn pane_identity(&self, pane_id: &str) -> AdapterResult<Option<PaneIdentity>> {
        self.pane_identity_of(pane_id)
    }

    fn adopt_pane(&self, input: AdoptPaneInput<'_>) -> AdapterResult<()> {
        self.adopt_pane_of(&input)
    }

    fn adopt_shell_pane(&self, pane_id: &str, workspace_id: Option<&str>) -> AdapterResult<()> {
        self.adopt_shell_pane_of(pane_id, workspace_id)
    }

    fn report_metadata(
        &self,
        target: MetadataTarget<'_>,
        tokens: &BTreeMap<String, String>,
    ) -> AdapterResult<()> {
        self.report_metadata_of(target, tokens)
    }

    fn rename_workspace(&self, workspace_id: &str, label: &str) -> AdapterResult<()> {
        self.rename_workspace_of(workspace_id, label)
    }

    fn rename_tab(&self, tab_id: &str, label: &str) -> AdapterResult<()> {
        self.rename_tab_of(tab_id, label)
    }

    fn forget_pane(&self, pane_id: &str) {
        self.delete_entry(pane_id);
    }

    fn run_in_pane(&self, pane_id: &str, command: &str) -> AdapterResult<()> {
        Adapter::run_in_pane(self, pane_id, command)
    }

    fn write_prompt_file(&self, text: &str) -> AdapterResult<String> {
        self.write_prompt(text)
    }

    fn read_screen(
        &self,
        pane_id: &str,
        ansi: bool,
        lines: Option<usize>,
    ) -> AdapterResult<String> {
        self.screen(pane_id, ansi, lines)
    }

    fn capture_prompt(&self, pane_id: &str) -> AdapterResult<CaptureOutcome> {
        self.capture_prompt_of(pane_id)
    }

    fn answer_prompt(&self, input: AnswerPromptInput<'_>) -> AdapterResult<RelayOutcome> {
        self.answer_prompt_of(input)
    }

    fn interrupt_working(&self, pane_id: &str, log: KeyLogger<'_>) -> AdapterResult<bool> {
        self.interrupt_working_of(pane_id, log)
    }
}
