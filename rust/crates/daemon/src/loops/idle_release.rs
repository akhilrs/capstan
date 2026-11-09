//! Releases a worker that has sat idle with nothing left to do, after warning the PM. The ledger decides who is a
//! candidate and what open work there is (`capstan_kernel::areas::idle_release`); this loop keeps the idle episodes,
//! takes a fresh look at the agent's tool processes before it warns and before it releases, and calls the launcher's
//! release, which closes the pane, removes a worktree without commits and keeps branches that have commits.
//!
//! An idle episode starts when an agent is a candidate with no open work and lasts while its idle-since time stays the
//! same: any activity moves that time, open work, a keep or a pause takes the agent out of the candidates, and both end the
//! episode. The warning goes out `warn` minutes before the release; the release needs the warning to be at least `warn`
//! minutes old and every check to pass again. A new episode needs a new warning.

use super::driver::{DriverLog, Steps};
use super::notifier::Clock;
use capstan_herdr::api::{HerdrAdapter, ProcessActivityProbe};
use capstan_launcher::api::LauncherService;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// How long an agent is left alone after a failed release.
pub const RETRY_AFTER_FAILURE_MS: i64 = 30 * 60 * 1000;
/// How often the loop looks, in production.
pub const TICK_MS: u64 = 60_000;

/// Tells whether a tool process is alive under an agent's pane right now. `Err` means it could not be told; the loop
/// then leaves the agent alone.
pub trait ToolProcessSampler: Send + Sync {
    fn tool_process_alive(&self, agent_id: &str) -> Result<bool, String>;
}

/// The sampler of the daemon: a fresh process sample of the agent's pane.
pub struct PaneSampler {
    pub adapter: Arc<dyn HerdrAdapter>,
    pub probe: Arc<dyn ProcessActivityProbe>,
}

impl ToolProcessSampler for PaneSampler {
    fn tool_process_alive(&self, agent_id: &str) -> Result<bool, String> {
        let Some(pane) = self.adapter.pane_for_agent(agent_id) else {
            return Ok(false);
        };
        self.probe.begin_tick();
        match self.probe.sample(&pane) {
            Ok(sample) => Ok(!sample.processes.is_empty()),
            Err(error) => Err(error.message),
        }
    }
}

pub struct IdleReleaseOptions {
    pub steps: Steps,
    pub launcher: Arc<dyn LauncherService>,
    /// `[agents] idle_release_minutes`; 0 turns the loop off.
    pub idle_minutes: i64,
    /// `[agents] idle_release_warn_minutes`.
    pub warn_minutes: i64,
    pub now: Clock,
    pub log: DriverLog,
    /// The driver's map of agent id to the last child-process activity in milliseconds.
    pub child_activity: Arc<Mutex<HashMap<String, i64>>>,
    pub sampler: Arc<dyn ToolProcessSampler>,
}

/// One idle episode of one agent.
struct Episode {
    idle_since_ms: i64,
    warned_at_ms: Option<i64>,
}

pub struct IdleRelease {
    options: IdleReleaseOptions,
    started_at: i64,
    episodes: HashMap<String, Episode>,
    /// Agents left alone until the time, after a failed release.
    skip_until: HashMap<String, i64>,
    /// The last time the loop's own sample found a tool process alive.
    sampled_alive: HashMap<String, i64>,
    /// True until the first tick has looked at the candidates: only then does a ledger warning stand for a warning of this
    /// loop (it was restarted). Later, a warning in the ledger whose episode ended (open work, activity) no longer counts.
    restoring: bool,
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}

fn kernel_text(error: &capstan_kernel::KernelError) -> String {
    error.to_string()
}

impl IdleRelease {
    pub fn new(options: IdleReleaseOptions) -> Self {
        let started_at = (options.now)();
        Self {
            options,
            started_at,
            episodes: HashMap::new(),
            skip_until: HashMap::new(),
            sampled_alive: HashMap::new(),
            restoring: true,
        }
    }

    fn log(&self, event: &str, details: Value) {
        (self.options.log)(event, details);
    }

    fn now(&self) -> i64 {
        (self.options.now)()
    }

    pub fn tick(&mut self) {
        if let Err(error) = self.tick_once() {
            self.log("idle_release_failed", json!({"error": error}));
        }
    }

    fn child_activity(&self) -> Value {
        let mut merged: HashMap<String, i64> = self
            .options
            .child_activity
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        for (agent, at) in &self.sampled_alive {
            let entry = merged.entry(agent.clone()).or_insert(*at);
            *entry = (*entry).max(*at);
        }
        json!(merged)
    }

    fn candidates(&self, now: i64) -> Result<Vec<Value>, String> {
        let input = json!({
            "idleMinutes": self.options.idle_minutes,
            "graceFloorMs": self.started_at,
            "childActivity": self.child_activity(),
        });
        let found = self
            .options
            .steps
            .read("idleCandidates", move |core| {
                core.idle_candidates(now, &input).map(Value::from)
            })
            .map_err(|e| kernel_text(&e))?;
        Ok(found.as_array().cloned().unwrap_or_default())
    }

    fn open_work(&self, agent_id: &str) -> Result<Vec<String>, String> {
        let id = agent_id.to_string();
        let work = self
            .options
            .steps
            .read("openWork", move |core| core.open_work(&id).map(Value::from))
            .map_err(|e| kernel_text(&e))?;
        Ok(work
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// A fresh look at the agent's tool processes. True means the agent is busy (a process is alive) or could not be
    /// judged; the time of a live process is remembered so the idle time moves.
    fn busy_now(&mut self, agent_id: &str) -> bool {
        match self.options.sampler.tool_process_alive(agent_id) {
            Ok(false) => false,
            Ok(true) => {
                let now = self.now();
                self.sampled_alive.insert(agent_id.to_string(), now);
                true
            }
            Err(error) => {
                self.log(
                    "idle_release_sample_failed",
                    json!({"agentId": agent_id, "error": error}),
                );
                true
            }
        }
    }

    fn tick_once(&mut self) -> Result<(), String> {
        let idle_ms = self.options.idle_minutes * 60_000;
        if idle_ms <= 0 {
            return Ok(());
        }
        let warn_ms = self
            .options
            .warn_minutes
            .clamp(1, self.options.idle_minutes)
            * 60_000;
        let now = self.now();
        let candidates = self.candidates(now)?;
        let restoring = std::mem::replace(&mut self.restoring, false);
        let present: HashSet<String> = candidates
            .iter()
            .map(|c| text(c, "agentId").to_string())
            .collect();
        // An agent that is no longer a candidate (activity that made it busy, a keep, a pause, a release by hand) ends its
        // episode.
        let gone: Vec<String> = self
            .episodes
            .keys()
            .filter(|id| !present.contains(*id))
            .cloned()
            .collect();
        for id in gone {
            self.end_episode(&id);
        }
        self.skip_until.retain(|_, until| *until > now);
        self.sampled_alive
            .retain(|id, at| present.contains(id) && now - *at < idle_ms);
        for candidate in candidates {
            let id = text(&candidate, "agentId").to_string();
            let since = candidate["idleSinceMs"].as_i64().unwrap_or(now);
            if self.skip_until.contains_key(&id) {
                continue;
            }
            if !self.open_work(&id)?.is_empty() {
                self.end_episode(&id);
                continue;
            }
            // A new idle-since time is new activity: a new episode.
            if self
                .episodes
                .get(&id)
                .is_some_and(|e| e.idle_since_ms != since)
            {
                self.end_episode(&id);
            }
            if !self.episodes.contains_key(&id) {
                let warned = if restoring {
                    self.restored_warning(&id, &candidate)?
                } else {
                    None
                };
                self.episodes.insert(
                    id.clone(),
                    Episode {
                        idle_since_ms: since,
                        warned_at_ms: warned,
                    },
                );
            }
            let warned_at = self.episodes[&id].warned_at_ms;
            let release_at = since + idle_ms;
            match warned_at {
                None if now >= release_at - warn_ms => {
                    if self.busy_now(&id) {
                        self.end_episode(&id);
                        continue;
                    }
                    self.warn(&candidate, since, release_at.max(now + warn_ms), now)?;
                }
                Some(at) if now >= release_at && now >= at + warn_ms => {
                    // Everything is checked again, as late as possible.
                    if !self.open_work(&id)?.is_empty() || self.busy_now(&id) {
                        self.end_episode(&id);
                        continue;
                    }
                    self.release(&candidate, since, at, now)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Ends the episode of an agent that was not released. When it had been warned, the ledger gets an episode-end row,
    /// so the warning does not count for a later episode with the same idle-since time.
    fn end_episode(&mut self, agent_id: &str) {
        let Some(episode) = self.episodes.remove(agent_id) else {
            return;
        };
        if episode.warned_at_ms.is_none() {
            return;
        }
        let input = json!({"agentId": agent_id});
        if let Err(error) = self
            .options
            .steps
            .mutate("recordIdleEpisodeEnd", move |core, context| {
                core.record_idle_episode_end(context, &input)
            })
        {
            self.log(
                "idle_episode_end_record_failed",
                json!({"agentId": agent_id, "error": kernel_text(&error)}),
            );
        }
    }

    /// The warning of this very episode when the ledger has one (the loop was restarted since).
    fn restored_warning(&self, agent_id: &str, candidate: &Value) -> Result<Option<i64>, String> {
        let id = agent_id.to_string();
        let found = self
            .options
            .steps
            .read("lastIdleWarning", move |core| {
                core.last_idle_warning(&id)
                    .map(|v| v.unwrap_or(Value::Null))
            })
            .map_err(|e| kernel_text(&e))?;
        if found["idleSince"] == candidate["idleSince"] {
            if let Some(warned) = found["warnedAt"].as_str() {
                return Ok(Some(capstan_kernel::areas::messaging::parse_iso_ms(warned)));
            }
        }
        Ok(None)
    }

    /// The one active PM, if any.
    fn pm(&self) -> Result<Option<String>, String> {
        let agents = self
            .options
            .steps
            .read("activeAgents", |core| core.active_agents())
            .map_err(|e| kernel_text(&e))?;
        Ok(agents
            .as_array()
            .into_iter()
            .flatten()
            .find(|a| a["kind"] == "PM")
            .and_then(|a| a["agentId"].as_str().map(str::to_string)))
    }

    /// An info message to the PM; false when there is no PM or the send failed (logged).
    fn tell_pm(&self, body: String) -> Result<bool, String> {
        let Some(pm) = self.pm()? else {
            self.log(
                "idle_release_note_skipped",
                json!({"reason": "no active PM"}),
            );
            return Ok(false);
        };
        let sent = self
            .options
            .steps
            .mutate("enqueueMessage", move |core, context| {
                core.enqueue_message(
                    context,
                    &json!({"recipientAgentId": pm, "body": body, "class": "info"}),
                )
            });
        match sent {
            Ok(_) => Ok(true),
            Err(error) => {
                self.log(
                    "idle_release_note_failed",
                    json!({"error": kernel_text(&error)}),
                );
                Ok(false)
            }
        }
    }

    fn warn(
        &mut self,
        candidate: &Value,
        since: i64,
        release_at: i64,
        now: i64,
    ) -> Result<(), String> {
        let id = text(candidate, "agentId").to_string();
        let role = text(candidate, "role").to_string();
        let since_iso = capstan_ledger::iso_from_millis(since);
        let release_iso = capstan_ledger::iso_from_millis(release_at);
        let body = format!(
            "{id} ({role}) has been idle since {since_iso} and has no open work; it will be released at {release_iso}. To keep it: cstan keep {id} [hours], or give it work."
        );
        if !self.tell_pm(body)? {
            // No note went out, so no warning counts; the next tick tries again.
            return Ok(());
        }
        let input =
            json!({"agentId": id, "role": role, "idleSince": since_iso, "releaseAt": release_iso});
        if let Err(error) = self
            .options
            .steps
            .mutate("recordIdleWarning", move |core, context| {
                core.record_idle_warning(context, &input)
            })
        {
            self.log(
                "idle_warning_record_failed",
                json!({"agentId": id, "error": kernel_text(&error)}),
            );
        }
        if let Some(episode) = self.episodes.get_mut(&id) {
            episode.warned_at_ms = Some(now);
        }
        self.log(
            "idle_release_warned",
            json!({"agentId": id, "idleSince": since_iso, "releaseAt": release_iso}),
        );
        Ok(())
    }

    fn release(
        &mut self,
        candidate: &Value,
        since: i64,
        warned_at: i64,
        now: i64,
    ) -> Result<(), String> {
        let id = text(candidate, "agentId").to_string();
        let since_iso = capstan_ledger::iso_from_millis(since);
        match self.options.launcher.release(&id) {
            Ok(result) => {
                let kept: Vec<String> = if result.outcome.branch_kept == Some(true) {
                    result.branch.iter().cloned().collect()
                } else {
                    Vec::new()
                };
                self.episodes.remove(&id);
                let minutes = (now - since) / 60_000;
                let input = json!({
                    "agentId": id,
                    "idleSince": since_iso,
                    "idleMinutes": minutes,
                    "warnedAt": capstan_ledger::iso_from_millis(warned_at),
                    "keptBranches": kept,
                });
                if let Err(error) = self
                    .options
                    .steps
                    .mutate("recordAutoRelease", move |core, context| {
                        core.record_auto_release(context, &input)
                    })
                {
                    self.log(
                        "idle_release_record_failed",
                        json!({"agentId": id, "error": kernel_text(&error)}),
                    );
                }
                self.log(
                    "idle_released",
                    json!({"agentId": id, "idleSince": since_iso, "branchesKept": kept}),
                );
                let list = if kept.is_empty() {
                    "none".to_string()
                } else {
                    kept.join(", ")
                };
                self.tell_pm(format!(
                    "{id} was released after {minutes} minutes idle; branches kept: {list}"
                ))?;
            }
            Err(error) => {
                self.skip_until
                    .insert(id.clone(), self.now() + RETRY_AFTER_FAILURE_MS);
                self.log(
                    "idle_release_failed",
                    json!({"agentId": id, "error": format!("LauncherError: {}", error.message)}),
                );
                self.tell_pm(format!(
                    "{id} could not be released after being idle: {}. It will be tried again in 30 minutes.",
                    error.message
                ))?;
            }
        }
        Ok(())
    }
}
