//! The controller answers a worker's permission prompt while an approval window is open (plan 44).
//!
//! Each tick ends a window past its cap. With `[prompt_relay]` enabled and a window open, every blocked worker (never the
//! PM) is looked at once per prompt: a prompt whose action cannot be read whole goes to the PM; a command or a path is
//! checked against the approval floor (a hit tells the PM and answers nothing); what is clear, or could not be parsed and
//! is passed on as unchecked, is answered with the one option that allows once. The controller never presses Esc, types
//! text or picks an option that widens permissions.
//!
//! The live prompt is read again after the row is typing and before the first key, and the adapter reads it a third
//! time and compares the hash before it sends; the gap left is one pane read to one send-keys call.

use super::driver::DriverLog;
use crate::deps::{new_context, Deps};
use crate::handlers::shared::CommandResponse;
use crate::relay_answer::{attempts_of, record_capture, run_answer, AnswerMode, Attempts};
use capstan_herdr::api::{CaptureOutcome, CapturedPrompt, HerdrState, PromptAction, RelayRefusal};
use capstan_herdr::prompt_relay::{allow_once_option, prompt_action};
use capstan_launcher::api::LauncherService;
use capstan_operator::floor::{check_floor, FloorAction, FloorContext, FloorVerdict};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// How many prompt hashes are remembered as tried.
const TRIED_LIMIT: usize = 512;

pub struct AutoAnswerOptions {
    pub deps: Deps,
    pub log: DriverLog,
}

pub struct AutoAnswer {
    deps: Deps,
    log: DriverLog,
    attempts: Arc<Attempts>,
    /// `agent|sha` of the prompts already looked at: a prompt is attempted once, whatever came of it.
    tried: VecDeque<String>,
}

impl AutoAnswer {
    pub fn new(options: AutoAnswerOptions) -> Self {
        let attempts = attempts_of(&options.deps);
        Self {
            deps: options.deps,
            log: options.log,
            attempts,
            tried: VecDeque::new(),
        }
    }

    fn log(&self, event: &str, details: Value) {
        (self.log)(event, details);
    }

    fn mutate<F>(&self, f: F) -> Result<Value, capstan_kernel::KernelError>
    where
        F: FnOnce(
                &capstan_kernel::Core,
                &capstan_kernel::types::MutationContext,
            ) -> capstan_kernel::KernelResult<Value>
            + Send
            + 'static,
    {
        let credential = self.deps.credential().to_string();
        self.deps.kernel.run(move |core| {
            let context = new_context(core, &credential)?;
            f(core, &context)
        })
    }

    pub fn tick(&mut self) {
        // Only a window past its cap writes anything: an idle tick leaves the ledger as it is.
        let due = self
            .deps
            .kernel
            .run(|core| core.list_approval_windows(Some(1)))
            .ok()
            .and_then(|list| list.get(0).cloned())
            .is_some_and(|window| window["endedAt"].is_null() && window["remainingSeconds"] == 0);
        if due {
            match self.mutate(|core, context| core.expire_approval_windows(context)) {
                Ok(Value::Array(ended)) if !ended.is_empty() => {
                    self.log("approval_window_expired", json!({"windows": ended}));
                }
                Ok(_) => {}
                Err(error) => self.log(
                    "approval_window_expire_failed",
                    json!({"error": error.to_string()}),
                ),
            }
        }
        if self.attempts.disabled() {
            return;
        }
        let (Some(config), Some(launcher)) = (
            self.deps.options.capstan.clone(),
            self.deps.launcher.clone(),
        ) else {
            return;
        };
        if !config.prompt_relay.enabled {
            return;
        }
        let window = match self.deps.kernel.run(|core| core.active_approval_window()) {
            Ok(Value::Null) => return,
            Ok(window) => window,
            Err(error) => {
                self.log(
                    "approval_window_read_failed",
                    json!({"error": error.to_string()}),
                );
                return;
            }
        };
        let Some(window_id) = window["windowId"].as_str().map(str::to_string) else {
            return;
        };
        let credential = self.deps.credential().to_string();
        let listed = self.deps.kernel.run(move |core| {
            Ok(json!({
                "agents": core.active_agents()?,
                "panes": core.agent_panes(&credential)?,
            }))
        });
        let listed = match listed {
            Ok(listed) => listed,
            Err(error) => {
                self.log("auto_answer_failed", json!({"error": error.to_string()}));
                return;
            }
        };
        let mut worktrees: HashMap<String, String> = HashMap::new();
        for pane in listed["panes"].as_array().into_iter().flatten() {
            if let (Some(agent), Some(path)) =
                (pane["agentId"].as_str(), pane["worktreePath"].as_str())
            {
                worktrees.insert(agent.to_string(), path.to_string());
            }
        }
        let mut protected_roots: Vec<String> = worktrees.values().cloned().collect();
        protected_roots.push(
            self.deps
                .options
                .workspace_root
                .to_string_lossy()
                .into_owned(),
        );
        for agent in listed["agents"].as_array().into_iter().flatten() {
            let (Some(agent_id), Some(kind)) = (agent["agentId"].as_str(), agent["kind"].as_str())
            else {
                continue;
            };
            if kind == "PM" || agent["state"] != "active" {
                continue;
            }
            // Herdr shows the agent blocked before its pane is read.
            if let Some(adapter) = &self.deps.adapter {
                if !matches!(adapter.agent_observation(agent_id), Ok(HerdrState::Blocked)) {
                    self.forget(agent_id);
                    continue;
                }
            }
            let Some(worktree) = worktrees.get(agent_id) else {
                continue;
            };
            let floor = FloorContext {
                entries: config.operator.approval_floor.clone(),
                own_root: worktree.clone(),
                protected_roots: protected_roots.clone(),
            };
            let now = self.deps.kernel.now_ms();
            if !self.attempts.start(agent_id, now) {
                continue;
            }
            self.attempt(
                launcher.as_ref(),
                &window_id,
                agent_id,
                &floor,
                config.operator.auto_window_tool_prompts,
            );
            self.attempts.finish(agent_id);
        }
    }

    /// An agent that is no longer at a prompt may meet the same prompt again later (the same command twice): that is a new
    /// attempt.
    fn forget(&mut self, agent_id: &str) {
        let prefix = format!("{agent_id}|");
        self.tried.retain(|key| !key.starts_with(&prefix));
    }

    fn attempt(
        &mut self,
        launcher: &dyn LauncherService,
        window_id: &str,
        agent_id: &str,
        floor: &FloorContext,
        tool_prompts: bool,
    ) {
        let captured = match launcher.capture_prompt(agent_id) {
            Ok(CaptureOutcome::Captured(captured)) => captured,
            Ok(CaptureOutcome::Refused(reason)) => {
                if reason == RelayRefusal::NotBlocked {
                    self.forget(agent_id);
                }
                return;
            }
            Err(error) => {
                self.log(
                    "auto_answer_capture_failed",
                    json!({"agentId": agent_id, "error": format!("{}: {}", error.code, error.message)}),
                );
                return;
            }
        };
        let key = format!("{agent_id}|{}", captured.prompt_sha);
        if self.tried.contains(&key) {
            return;
        }
        self.tried.push_back(key);
        if self.tried.len() > TRIED_LIMIT {
            self.tried.pop_front();
        }
        // A prompt whose action cannot be read whole is the PM's.
        let Some(action) = prompt_action(&captured) else {
            return;
        };
        let (verdict, described) = match &action {
            PromptAction::Command { text } => (
                check_floor(
                    &FloorAction::Command {
                        text: text.clone(),
                        cwd: floor.own_root.clone(),
                    },
                    floor,
                ),
                format!("Bash: {text}"),
            ),
            PromptAction::Write { path } => (
                check_floor(
                    &FloorAction::Write {
                        path: path.clone(),
                        cwd: floor.own_root.clone(),
                    },
                    floor,
                ),
                format!("write {path}"),
            ),
            PromptAction::Other { header } => {
                if !tool_prompts {
                    return;
                }
                (FloorVerdict::Clear, format!("tool prompt: {header}"))
            }
        };
        let option = allow_once_option(&captured);
        if option.is_none() && !matches!(verdict, FloorVerdict::Floor { .. }) {
            return;
        }
        let record = match record_capture(
            &self.deps,
            self.deps.credential(),
            &captured,
            Some(window_id),
        ) {
            Ok(record) => record,
            Err(error) => {
                self.log(
                    "auto_answer_capture_failed",
                    json!({"agentId": agent_id, "error": error.to_string()}),
                );
                return;
            }
        };
        let relay_id = record["relayId"].as_str().unwrap_or("").to_string();
        let unchecked = match verdict {
            FloorVerdict::Floor { entry, detail } => {
                self.floor_hit(window_id, &relay_id, agent_id, &described, &entry, &detail);
                return;
            }
            FloorVerdict::Unchecked { reason } => Some(reason),
            FloorVerdict::Clear => None,
        };
        let Some(number) = option else {
            return;
        };
        let recheck = |stored: &Value| -> Result<(), String> {
            recheck_live(launcher, agent_id, stored, number)
        };
        let response = run_answer(
            &self.deps,
            launcher,
            self.deps.credential(),
            &relay_id,
            record["hash12"].as_str().unwrap_or(""),
            &crate::handlers::shared::ParsedAnswer::Option { number },
            &AnswerMode::Auto {
                window_id,
                unchecked: unchecked.as_deref(),
                recheck: &recheck,
            },
        );
        match response {
            CommandResponse::Ok(_) => self.log(
                "auto_answered",
                json!({"agentId": agent_id, "relayId": relay_id, "windowId": window_id, "option": number, "unchecked": unchecked}),
            ),
            CommandResponse::Err { message, .. } => self.log(
                "auto_answer_refused",
                json!({"agentId": agent_id, "relayId": relay_id, "windowId": window_id, "reason": message}),
            ),
        }
    }

    fn floor_hit(
        &self,
        window_id: &str,
        relay_id: &str,
        agent_id: &str,
        action: &str,
        entry: &str,
        detail: &str,
    ) {
        let input = json!({
            "windowId": window_id,
            "relayId": relay_id,
            "agentId": agent_id,
            "action": action,
            "entry": entry,
            "detail": detail,
        });
        let queued = self.mutate({
            let input = input.clone();
            move |core, context| core.queue_floor_hit_notice(context, &input)
        });
        match queued {
            Ok(result) => self.log(
                "auto_answer_floor_hit",
                json!({"agentId": agent_id, "relayId": relay_id, "entry": entry, "queued": result["queued"]}),
            ),
            Err(error) => self.log(
                "auto_answer_floor_notice_failed",
                json!({"agentId": agent_id, "relayId": relay_id, "error": error.to_string()}),
            ),
        }
    }
}

/// The live pane is read and parsed again: it must still be blocked at the prompt that was captured (same hash), and
/// option `number` must be the one that allows once as the live text reads, whatever the stored options say.
fn recheck_live(
    launcher: &dyn LauncherService,
    agent_id: &str,
    stored: &Value,
    number: i64,
) -> Result<(), String> {
    let live: CapturedPrompt = match launcher.capture_prompt(agent_id) {
        Ok(CaptureOutcome::Captured(live)) => live,
        Ok(CaptureOutcome::Refused(reason)) => {
            return Err(format!("the live prompt was refused: {}", reason.as_str()))
        }
        Err(error) => return Err(format!("the live prompt could not be read: {}", error.code)),
    };
    if stored["promptSha"].as_str() != Some(live.prompt_sha.as_str()) {
        return Err("the live prompt is no longer the captured one".into());
    }
    if allow_once_option(&live) != Some(number) {
        return Err(format!(
            "option {number} does not allow once on the live screen"
        ));
    }
    Ok(())
}
