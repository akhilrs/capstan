//! Status, observation of panes, prompt relay and re-adoption after a daemon restart (src/launcher/observe.ts).

use crate::api::{AnswerPromptRequest, LauncherStatus, ObserveResult, ADOPT_BUDGET_MS};
use crate::kernel::LauncherKernel;
use crate::shared::*;
use crate::text::sanitize_screen;
use capstan_herdr::api::{
    AdapterError, AdoptPaneInput, AnswerPromptInput, CaptureOutcome, KeyLogEntry, PaneRole,
    RelayOutcome,
};
use capstan_herdr::claude_args::build_agent_environment;
use capstan_herdr::naming::workspace_label;
use serde_json::json;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

fn key_entry(agent_id: &str, entry: &KeyLogEntry) -> serde_json::Value {
    json!({"agentId": agent_id, "kind": "key", "pane": entry.pane, "key": entry.key, "reason": entry.reason})
}

fn resolve(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    for component in absolute.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

impl LauncherKernel {
    pub fn launcher_status(&self) -> OpResult<LauncherStatus> {
        // A worktree that could not be removed keeps its ledger row, so it is reported from the ledger and survives a
        // daemon restart.
        let mut leftovers: Vec<(String, String, Option<String>)> = Vec::new();
        for row in self.ledger.agent_panes()? {
            if self
                .agent_record(&row.agent_id)?
                .is_some_and(|a| a.state == "ended")
            {
                let reason = if row.worktree_path.is_none() {
                    "a record of an ended agent is waiting to be cleaned up"
                } else {
                    "git could not remove the worktree"
                };
                leftovers.push((row.agent_id.clone(), reason.to_string(), row.worktree_path));
            }
        }
        // An entry for an agent that has since ended is no longer a problem.
        let mut cleanup_failed: Vec<(String, String, Option<String>)> = Vec::new();
        for entry in self.cleanup_failures() {
            if self
                .agent_record(&entry.agent_id)?
                .is_some_and(|a| a.active())
            {
                cleanup_failed.push((entry.agent_id, entry.reason, None));
            }
        }
        cleanup_failed.extend(leftovers);
        Ok(LauncherStatus {
            cleanup_failed,
            orphan_panes: self.ledger.orphan_panes()?,
        })
    }

    /// Reads the blocking permission prompt of an active worker; nothing is typed.
    pub fn capture_prompt(&self, agent_id: &str) -> OpResult<CaptureOutcome> {
        let pane = self.active_pane(agent_id)?;
        Ok(self.adapter.capture_prompt(&pane)?)
    }

    /// Types an answer to the worker's prompt when the screen still hashes to `prompt_sha`; every key sent is logged.
    pub fn answer_prompt(
        &self,
        agent_id: &str,
        request: AnswerPromptRequest<'_>,
    ) -> OpResult<RelayOutcome> {
        let pane_id = self.active_pane(agent_id)?;
        let log = |entry: &KeyLogEntry| self.log("prompt_relay_key", key_entry(agent_id, entry));
        Ok(self.adapter.answer_prompt(AnswerPromptInput {
            pane_id: &pane_id,
            prompt_sha: request.prompt_sha,
            answer: request.answer,
            before_type: request.before_type,
            log: &log,
        })?)
    }

    /// Sends one Esc to a worker that Herdr shows working, so a pause takes hold at once. Needs `[prompt_relay]`, whose key
    /// log it shares. True when the Esc was sent.
    pub fn interrupt(&self, agent_id: &str) -> OpResult<bool> {
        if !self.config.prompt_relay.enabled {
            return Err(OpError::launcher(
                "not_configured",
                "--interrupt needs [prompt_relay] enabled = true in capstan.toml",
            ));
        }
        let pane_id = self.active_pane(agent_id)?;
        let log = |entry: &KeyLogEntry| self.log("prompt_relay_key", key_entry(agent_id, entry));
        Ok(self.adapter.interrupt_working(&pane_id, &log)?)
    }

    /// The recent screen of an active agent. A read: it does not wait for other launcher operations.
    pub fn observe_agent(&self, agent_id: &str, lines: usize) -> OpResult<ObserveResult> {
        let agent = self.agent_record(agent_id)?;
        let Some(agent) = agent.filter(|a| a.active()) else {
            return Err(OpError::launcher(
                "agent_not_active",
                "the agent is not active",
            ));
        };
        let pane_id = match self.adapter.pane_for_agent(agent_id) {
            Some(pane) => Some(pane),
            None => self
                .ledger
                .agent_panes()?
                .into_iter()
                .find(|row| row.agent_id == agent_id)
                .and_then(|row| row.pane_id),
        };
        let Some(pane_id) = pane_id else {
            return Err(OpError::launcher(
                "no_pane",
                "the agent has no pane recorded",
            ));
        };
        let screen = match self.adapter.read_screen(&pane_id, false, Some(lines)) {
            Ok(screen) => screen,
            Err(error) => {
                self.log(
                    "observe_failed",
                    json!({"agentId": agent_id, "error": OpError::from(error).js_string()}),
                );
                return Err(OpError::launcher(
                    "pane_unreadable",
                    "the agent's pane could not be read",
                ));
            }
        };
        let agent_status = self
            .adapter
            .agent_observation(agent_id)
            .ok()
            .map(|state| state.as_str().to_string());
        Ok(ObserveResult {
            agent_id: agent_id.to_string(),
            role_name: agent.role_name,
            kind: agent.kind,
            state: agent.state,
            agent_status,
            text: sanitize_screen(&screen),
        })
    }

    /// The environment an approved Operator command runs in: the setup command's filtered environment, with every CAPSTAN_
    /// variable absent and no `cstan` wrapper directory on PATH.
    pub fn operator_environment(&self) -> BTreeMap<String, String> {
        let wrapper_directory = resolve(&Path::new(&self.root).join(".capstan").join("bin"));
        let base_path = self
            .base_environment
            .get("PATH")
            .cloned()
            .unwrap_or_default();
        let kept: Vec<&str> = base_path
            .split(':')
            .filter(|entry| !entry.is_empty() && resolve(Path::new(entry)) != wrapper_directory)
            .collect();
        let mut environment =
            build_agent_environment(&self.base_environment, &[], &self.config.env_pass)
                .unwrap_or_default();
        environment.retain(|name, _| !name.starts_with("CAPSTAN_"));
        if kept.is_empty() {
            environment.remove("PATH");
        } else {
            environment.insert("PATH".into(), kept.join(":"));
        }
        environment
    }

    /// Re-registers the panes recorded before a daemon restart; the daemon runs it in the background before it starts the
    /// driver.
    pub fn adopt_all(&self) -> OpResult<()> {
        self.run(|_| self.adopt_now(&self.budget(ADOPT_BUDGET_MS)))
    }

    pub fn adopt_now(&self, budget: &Budget) -> OpResult<()> {
        for (agent_id, pane_id, terminal_id) in self.ledger.orphan_pane_terminals()? {
            let attempt = || -> OpResult<()> {
                match &terminal_id {
                    // Without a terminal id an orphan cannot be told apart from the PM's new pane (same agent token): it
                    // is never closed, only dropped once Herdr no longer has it.
                    None => {
                        if self.adapter.pane_identity(&pane_id)?.is_some() {
                            return Ok(());
                        }
                    }
                    Some(terminal) => {
                        self.close_owned(&pane_id, &agent_id, Some(terminal))?;
                    }
                }
                self.ledger.clear_orphan_pane(&pane_id)?;
                Ok(())
            };
            // Still open; it stays listed.
            let _ = attempt();
        }
        let rows = self.ledger.agent_panes()?;
        let seen: HashSet<String> = rows.iter().map(|r| r.agent_id.clone()).collect();
        for row in &rows {
            if !self.within(budget) {
                self.log("adopt_skipped", json!({"agentId": row.agent_id}));
                continue;
            }
            let agent = self.agent_record(&row.agent_id)?;
            let Some(agent) = agent.filter(|a| a.active()) else {
                self.release_resources(
                    &row.agent_id,
                    &CleanupInfo {
                        pane_id: row.pane_id.clone(),
                        branch: row.branch.clone(),
                        base_sha: row.base_sha.clone(),
                        worktree_path: row.worktree_path.clone(),
                        ..CleanupInfo::default()
                    },
                )?;
                continue;
            };
            let Some(pane_id) = &row.pane_id else {
                self.cleanup_agent(
                    &row.agent_id,
                    &CleanupInfo {
                        branch: row.branch.clone(),
                        base_sha: row.base_sha.clone(),
                        worktree_path: row.worktree_path.clone(),
                        ..CleanupInfo::default()
                    },
                    None,
                    None,
                )?;
                continue;
            };
            if self.adapter.pane_entry(pane_id).is_some() {
                continue;
            }
            let adopted = (|| -> OpResult<()> {
                self.adapter.adopt_pane(AdoptPaneInput {
                    pane_id,
                    role: if agent.kind == "PM" {
                        PaneRole::Pm
                    } else {
                        PaneRole::Worker
                    },
                    agent: &row.agent_id,
                    workspace_id: row.workspace_id.as_deref(),
                    worktree_path: row.worktree_path.as_deref(),
                })?;
                // A worker placed as a tab or pane shares the PM's workspace, and a PM in the project workspace shares the
                // hub's name, so only a PM's own old workspace is relabelled here.
                let label = if agent.kind == "PM" {
                    let hub_workspace =
                        self.ledger.fallback_pane()?.map(|(workspace, _)| workspace);
                    let differs = match hub_workspace {
                        Some(hub) => row.workspace_id != hub,
                        None => true,
                    };
                    differs.then(|| workspace_label(&self.project, &agent.role_name))
                } else {
                    None
                };
                self.describe(
                    pane_id,
                    row.workspace_id.as_deref(),
                    &row.agent_id,
                    &agent.role_name,
                    label.as_deref(),
                );
                Ok(())
            })();
            match adopted {
                Ok(()) => {}
                Err(error)
                    if matches!(
                        error.adapter(),
                        Some(AdapterError::PaneGone(_) | AdapterError::AgentPaneMismatch(_))
                    ) =>
                {
                    self.log("pane_lost", json!({"agentId": row.agent_id}));
                    // Read before the row is cleared: the pane id may now be another agent's, and only this terminal id
                    // tells.
                    let terminal_id = self.ledger.pane_terminal_id(&row.agent_id)?;
                    self.ledger.clear_agent_pane(&row.agent_id)?;
                    if agent.kind != "PM" {
                        let moved = self.interrupted_move(row.workspace_id.as_deref())?;
                        self.cleanup_agent(
                            &row.agent_id,
                            &CleanupInfo {
                                pane_id: Some(pane_id.clone()),
                                terminal_id: Some(terminal_id),
                                move_may_have_happened: moved,
                                worktree_path: row.worktree_path.clone(),
                                branch: row.branch.clone(),
                                base_sha: row.base_sha.clone(),
                                ..CleanupInfo::default()
                            },
                            Some("found_dead_at_start"),
                            Some(row.branch.as_deref()),
                        )?;
                    }
                }
                Err(error) => self.log(
                    "adopt_failed",
                    json!({"agentId": row.agent_id, "error": error.js_string()}),
                ),
            }
        }
        for agent in self.active_agents()? {
            if agent.kind != "PM" && !seen.contains(&agent.agent_id) {
                self.log("crashed_spawn", json!({"agentId": agent.agent_id}));
                self.cleanup_agent(&agent.agent_id, &CleanupInfo::default(), None, None)?;
            }
        }
        if let Some((workspace_id, pane_id)) = self.ledger.fallback_pane()? {
            if self.adapter.pane_entry(&pane_id).is_none() {
                match self
                    .adapter
                    .adopt_shell_pane(&pane_id, workspace_id.as_deref())
                {
                    Ok(()) => self.label_hub(workspace_id.as_deref()),
                    // A gone watch pane keeps its row: the workspace id in it lets the next hub check make the watch tab
                    // again in the same workspace.
                    Err(AdapterError::PaneGone(_)) => {
                        self.log("fallback_pane_gone", json!({"paneId": pane_id}))
                    }
                    Err(error) => self.log(
                        "fallback_adopt_failed",
                        json!({"error": OpError::from(error).js_string()}),
                    ),
                }
            }
        }
        Ok(())
    }
}
