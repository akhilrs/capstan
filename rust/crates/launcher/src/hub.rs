//! The project workspace, the PM start and restart, and the watch tab (src/launcher/hub.ts).

use crate::api::{HubStatus, LaunchResult, LaunchState, ADOPT_BUDGET_MS};
use crate::kernel::LauncherKernel;
use crate::shared::*;
use capstan_config::prompts::{
    build_role_prompt, Kind, OpenWork, PromptInput, RestartSummary, SummaryIntegration,
    SummaryLink, SummaryMessage, SummaryPlan, WorkerRole,
};
use capstan_config::Role;
use capstan_herdr::api::{
    shell_quote, AdapterError, CreateTabInput, CreateWorkspaceInput, HerdrState, PaneRole,
    PrepareShellInput, StartAgentInput, StartStatus,
};
use capstan_wire::js;
use serde_json::{json, Value};

/// A `PmRestartSummary` of the ledger as the prompt builder takes it.
pub fn restart_summary_of(value: &Value) -> Option<RestartSummary> {
    fn list<'a>(value: &'a Value, key: &str) -> &'a [Value] {
        value
            .get(key)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
    fn text(value: &Value, key: &str) -> Option<String> {
        value.get(key).and_then(Value::as_str).map(str::to_string)
    }
    fn texts(value: &Value, key: &str) -> Vec<String> {
        list(value, key)
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()
    }
    let objective = js::parse(&value.get("objective").unwrap_or(&Value::Null).to_string()).ok()?;
    Some(RestartSummary {
        objective,
        open_work: list(value, "openWork")
            .iter()
            .map(|item| {
                Some(OpenWork {
                    work_item_id: text(item, "workItemId")?,
                    title: text(item, "title")?,
                    role: text(item, "role")?,
                    state: text(item, "state")?,
                    owner: text(item, "owner"),
                    blockers: texts(item, "blockers"),
                })
            })
            .collect::<Option<_>>()?,
        messages: list(value, "messages")
            .iter()
            .map(|item| {
                Some(SummaryMessage {
                    message_id: text(item, "messageId")?,
                    from: text(item, "from")?,
                    body: text(item, "body")?,
                    state: text(item, "state")?,
                })
            })
            .collect::<Option<_>>()?,
        plans: list(value, "plans")
            .iter()
            .map(|item| {
                Some(SummaryPlan {
                    plan_id: text(item, "planId")?,
                    title: text(item, "title")?,
                    tier: text(item, "tier")?,
                    state: text(item, "state")?,
                    packages: item.get("packages").and_then(Value::as_f64)?,
                    signed_off: texts(item, "signedOff"),
                })
            })
            .collect::<Option<_>>()?,
        integrations: list(value, "integrations")
            .iter()
            .map(|item| {
                Some(SummaryIntegration {
                    integration_id: text(item, "integrationId")?,
                    branch: text(item, "branch")?,
                    head_sha: text(item, "headSha"),
                })
            })
            .collect::<Option<_>>()?,
        links: list(value, "links")
            .iter()
            .map(|item| {
                Some(SummaryLink {
                    ref_kind: text(item, "refKind")?,
                    ref_id: text(item, "refId")?,
                    external_id: text(item, "externalId")?,
                    synced_state: text(item, "syncedState")?,
                    wanted: text(item, "wanted"),
                    drift: item.get("drift").and_then(Value::as_bool) == Some(true),
                    bound_agent_id: text(item, "boundAgentId"),
                })
            })
            .collect::<Option<_>>()?,
        truncated: value.get("truncated").and_then(Value::as_bool) == Some(true),
        generated_at: text(value, "generatedAt")?,
    })
}

fn too_large(error: capstan_config::prompts::PromptTooLarge) -> OpError {
    OpError::Kernel {
        name: "RangeError".into(),
        message: error.to_string(),
    }
}

impl LauncherKernel {
    /// The system prompt of the PM, with the recorded summary of a restarted one.
    fn pm_prompt(
        &self,
        role: &Role,
        agent_id: &str,
        summary: Option<RestartSummary>,
    ) -> OpResult<String> {
        let mut input = PromptInput::new(
            &role.name,
            Kind::Pm,
            agent_id,
            self.wait_seconds(role) as f64,
        );
        input.role_prompt = role.prompt_text.clone();
        input.worker_roles = Some(
            self.worker_roles()
                .into_iter()
                .map(|(name, kind)| WorkerRole { name, kind })
                .collect(),
        );
        input.architect = self.architect_prompt();
        input.operator = self.operator_prompt();
        input.researcher = self.researcher_prompt();
        input.prompt_relay_enabled = self.config.prompt_relay.enabled;
        input.nexora = self.nexora_prompt();
        input.restart_summary = summary;
        build_role_prompt(&input).map_err(too_large)
    }

    pub fn start_pm(
        &self,
        role: &Role,
        agent_id: &str,
        credential: &str,
        budget: &Budget,
        hub: &Hub,
        summary: Option<&Value>,
    ) -> OpResult<LaunchResult> {
        budget.check("the PM prompt")?;
        let summary = summary.and_then(restart_summary_of);
        let prompt_text = self.pm_prompt(role, agent_id, summary)?;
        let prompt_file = self.adapter.write_prompt_file(&prompt_text)?;
        let Some(workspace_id) = hub.workspace_id.clone() else {
            return Err(OpError::launcher(
                "hub_failed",
                "the project workspace is not open, so the PM has nowhere to start",
            ));
        };
        // The hub's root pane is free for the PM only in the call that made the hub; a later start gets a tab of its own.
        let pane_id = match &hub.free_pm_pane {
            Some(pane) => pane.clone(),
            None => {
                self.adapter
                    .create_tab(CreateTabInput {
                        workspace_id: &workspace_id,
                        cwd: &self.root,
                        label: "pm",
                        role: PaneRole::Pm,
                    })?
                    .pane_id
            }
        };
        let started = (|| -> OpResult<LaunchResult> {
            budget.check("starting the PM")?;
            let args = self.arguments(role, &prompt_text, &prompt_file, None)?;
            let environment = self.environment(Some(credential), None)?;
            let kind = self.host_kind(role)?;
            let status = self.adapter.start_agent(StartAgentInput {
                name: agent_id,
                kind: &kind,
                pane_id: &pane_id,
                args: &args,
                timeout_ms: Some(START_TIMEOUT_MS),
                environment: Some(&environment),
            })?;
            self.agents_started
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let terminal_id = self.terminal_of(&pane_id, agent_id);
            let mut pane = json!({
                "agentId": agent_id,
                "workspaceId": workspace_id,
                "paneId": pane_id,
                "worktreePath": null,
                "branch": null,
                "baseSha": null,
            });
            if let Some(terminal) = &terminal_id {
                pane["terminalId"] = json!(terminal);
            }
            self.ledger.record_agent_pane(pane)?;
            self.describe(&pane_id, Some(&workspace_id), agent_id, &role.name, None);
            Ok(match status {
                StartStatus::Started => LaunchResult {
                    state: Some(LaunchState::Started),
                    agent_id: Some(agent_id.to_string()),
                    pane_id: Some(pane_id.clone()),
                    ..LaunchResult::default()
                },
                StartStatus::BlockedAtStartup => LaunchResult {
                    state: Some(LaunchState::Blocked),
                    agent_id: Some(agent_id.to_string()),
                    pane_id: Some(pane_id.clone()),
                    hint: Some(
                        "answer the trust dialog in the PM pane; the controller never answers it"
                            .into(),
                    ),
                    ..LaunchResult::default()
                },
            })
        })();
        match started {
            Ok(result) => Ok(result),
            Err(error) => {
                if let Err(close_error) = self.close(&pane_id) {
                    self.log(
                        "pane_not_closed",
                        json!({"paneId": pane_id, "error": close_error.js_string()}),
                    );
                }
                Err(error)
            }
        }
    }

    /// The project workspace carries the project's name, so its PM and the worker worktrees under it read as one group;
    /// display only.
    pub fn label_hub(&self, workspace_id: Option<&str>) {
        let Some(workspace_id) = workspace_id else {
            return;
        };
        let attempt = || -> Result<(), AdapterError> {
            self.adapter.rename_workspace(workspace_id, &self.project)?;
            let tokens =
                std::collections::BTreeMap::from([("project".to_string(), self.project.clone())]);
            self.adapter.report_metadata(
                capstan_herdr::api::MetadataTarget::Workspace(workspace_id),
                &tokens,
            )
        };
        if let Err(error) = attempt() {
            self.log(
                "describe_failed",
                json!({"agentId": "watch", "error": OpError::from(error).js_string()}),
            );
        }
    }

    /// The watch shell in a tab of the project workspace; it stays the recorded fallback pane.
    pub fn open_watch_tab(&self, workspace_id: &str) -> OpResult<String> {
        let tab = self.adapter.create_tab(CreateTabInput {
            workspace_id,
            cwd: &self.root,
            label: "watch",
            role: PaneRole::Worker,
        })?;
        let prepared = (|| -> OpResult<()> {
            let environment = self.environment(None, None)?;
            self.adapter.prepare_shell(PrepareShellInput {
                pane_id: &tab.pane_id,
                environment: &environment,
                timeout_ms: None,
            })?;
            self.adapter.run_in_pane(
                &tab.pane_id,
                &format!(
                    "cd {} && exec {} status --watch",
                    shell_quote(&self.root),
                    self_invocation(&self.node, &self.cli_path, &self.site)
                ),
            )?;
            self.ledger
                .record_fallback_pane(workspace_id, &tab.pane_id)?;
            Ok(())
        })();
        if let Err(error) = prepared {
            // A tab with no recorded watch pane would be made again at every check.
            if let Err(close_error) = self.close(&tab.pane_id) {
                self.log(
                    "pane_not_closed",
                    json!({"paneId": tab.pane_id, "error": close_error.js_string()}),
                );
            }
            return Err(error);
        }
        Ok(tab.pane_id)
    }

    pub fn ensure_hub_without_pm(&self, budget: &Budget) -> OpResult<HubStatus> {
        let hub = self.ensure_hub(budget)?;
        self.drop_unused_root(&hub);
        Ok(hub.status)
    }

    /// A hub made while a PM is already running has an empty root pane no PM will take; it goes, and the workspace keeps
    /// its watch tab.
    pub fn drop_unused_root(&self, hub: &Hub) {
        let Some(pane) = &hub.free_pm_pane else {
            return;
        };
        if let Err(error) = self.close(pane) {
            self.log(
                "pane_not_closed",
                json!({"paneId": pane, "error": error.js_string()}),
            );
        }
    }

    fn failed_hub() -> Hub {
        Hub {
            status: HubStatus::Failed,
            workspace_id: None,
            free_pm_pane: None,
        }
    }

    /// The project workspace: tab 1 is the PM's, a `watch` tab keeps the workspace open when a PM pane is replaced, and
    /// worker worktrees hang under it.
    pub fn ensure_hub(&self, budget: &Budget) -> OpResult<Hub> {
        if let Some((workspace_id, pane_id)) = self.ledger.fallback_pane()? {
            if self.adapter.pane_entry(&pane_id).is_some() {
                return Ok(Hub {
                    status: HubStatus::Present,
                    workspace_id,
                    free_pm_pane: None,
                });
            }
            // Re-adopt before opening another hub: a second one would hide the worktrees that hang under the first.
            match self
                .adapter
                .adopt_shell_pane(&pane_id, workspace_id.as_deref())
            {
                Ok(()) => {
                    self.label_hub(workspace_id.as_deref());
                    return Ok(Hub {
                        status: HubStatus::Present,
                        workspace_id,
                        free_pm_pane: None,
                    });
                }
                Err(AdapterError::PaneGone(_)) => {}
                Err(error) => {
                    self.log(
                        "hub_adopt_failed",
                        json!({"error": OpError::from(error).js_string()}),
                    );
                    return Ok(Self::failed_hub());
                }
            }
            // The watch pane is gone; its workspace may not be, and closing a PM pane in a workspace with worktree
            // children needs another tab there. Only a workspace that Herdr says is gone is replaced: any other failure
            // leaves the row alone, so no second hub hides the worktrees under the first.
            if let Some(workspace) = &workspace_id {
                let opened = budget
                    .check("opening the watch tab")
                    .and_then(|()| self.open_watch_tab(workspace));
                match opened {
                    Ok(_) => {
                        return Ok(Hub {
                            status: HubStatus::Opened,
                            workspace_id,
                            free_pm_pane: None,
                        })
                    }
                    Err(error) => {
                        self.log("watch_tab_failed", json!({"error": error.js_string()}));
                        if !error.herdr_code_is("workspace_not_found") {
                            return Ok(Self::failed_hub());
                        }
                    }
                }
            }
            self.ledger.clear_fallback_pane()?;
        }
        let mut created: Option<capstan_herdr::api::WorkspaceCreated> = None;
        let attempt = (|| -> OpResult<Hub> {
            budget.check("opening the project workspace")?;
            let workspace = self.adapter.create_workspace(CreateWorkspaceInput {
                cwd: &self.root,
                label: &self.project,
                role: PaneRole::Pm,
            })?;
            created = Some(workspace.clone());
            self.label_hub(Some(&workspace.workspace_id));
            if let Err(error) = self.adapter.rename_tab(&workspace.tab_id, "pm") {
                self.log(
                    "describe_failed",
                    json!({"agentId": "pm", "error": OpError::from(error).js_string()}),
                );
            }
            self.open_watch_tab(&workspace.workspace_id)?;
            Ok(Hub {
                status: HubStatus::Opened,
                workspace_id: Some(workspace.workspace_id),
                free_pm_pane: Some(workspace.pane_id),
            })
        })();
        match attempt {
            Ok(hub) => Ok(hub),
            Err(error) => {
                self.log("fallback_pane_failed", json!({"error": error.js_string()}));
                if let Some(workspace) = &created {
                    if let Err(close_error) = self.close(&workspace.pane_id) {
                        self.log(
                            "pane_not_closed",
                            json!({"paneId": workspace.pane_id, "error": close_error.js_string()}),
                        );
                    }
                }
                // A configuration fault (a colon in the project path, say) must reach the operator as itself, not as an
                // unopened workspace.
                if matches!(error, OpError::Launcher(_)) {
                    return Err(error);
                }
                Ok(Self::failed_hub())
            }
        }
    }

    pub fn launch_pm(&self) -> OpResult<LaunchResult> {
        let (mut result, missing) = self.run_starting(|budget| {
            self.adopt_now(&self.budget(ADOPT_BUDGET_MS))?;
            let role = self.pm_role()?;
            let active: Vec<AgentRec> = self
                .active_agents()?
                .into_iter()
                .filter(|a| a.kind == "PM")
                .collect();
            if active.len() > 1 {
                return Err(OpError::launcher(
                    "pm_exists",
                    "more than one PM agent is active",
                ));
            }
            if let Some(existing) = active.first() {
                let Some(pane_id) = self.adapter.pane_for_agent(&existing.agent_id) else {
                    return Ok(LaunchResult {
                        state: Some(LaunchState::NeedsRestart),
                        agent_id: Some(existing.agent_id.clone()),
                        hint: Some("the PM has no live pane; run cstan pm restart".into()),
                        ..LaunchResult::default()
                    });
                };
                let mut state = LaunchState::Running;
                match self.adapter.agent_observation(&existing.agent_id) {
                    Ok(HerdrState::Blocked) => state = LaunchState::Blocked,
                    Ok(_) => {}
                    Err(error) => self.log(
                        "observe_failed",
                        json!({"agentId": existing.agent_id, "error": OpError::from(error).js_string()}),
                    ),
                }
                return Ok(LaunchResult {
                    state: Some(state),
                    agent_id: Some(existing.agent_id.clone()),
                    pane_id: Some(pane_id),
                    generation: Some(existing.generation),
                    hub: Some(self.ensure_hub_without_pm(budget)?),
                    ..LaunchResult::default()
                });
            }
            let (agent_id, credential) = self.create_agent(role)?;
            let mut hub_status = HubStatus::Failed;
            let mut made_hub: Option<Hub> = None;
            let attempt = (|| -> OpResult<LaunchResult> {
                let hub = self.ensure_hub(budget)?;
                made_hub = Some(hub.clone());
                hub_status = hub.status;
                if hub.status == HubStatus::Failed {
                    return Err(OpError::launcher(
                        "hub_unavailable",
                        "the project workspace could not be opened; check the Herdr session named in capstan.toml",
                    ));
                }
                self.start_pm(role, &agent_id, &credential, budget, &hub, None)
            })();
            match attempt {
                Ok(result) => Ok(LaunchResult {
                    hub: Some(hub_status),
                    ..result
                }),
                Err(error) => {
                    self.cleanup_agent(&agent_id, &CleanupInfo::default(), None, None)?;
                    // A start that failed before it took the new workspace's root pane leaves it empty.
                    if let Some(hub) = &made_hub {
                        if hub
                            .free_pm_pane
                            .as_ref()
                            .is_some_and(|pane| self.adapter.pane_entry(pane).is_some())
                        {
                            self.drop_unused_root(hub);
                        }
                    }
                    Ok(LaunchResult {
                        state: Some(LaunchState::Failed),
                        agent_id: Some(agent_id),
                        reason: Some(error.message()),
                        step: Some("start".into()),
                        ..LaunchResult::default()
                    })
                }
            }
        })?;
        if let Some((names, warning)) = missing {
            result.missing_env = Some(names);
            result.warning = Some(warning);
        }
        Ok(result)
    }

    pub fn restart_pm(&self) -> OpResult<LaunchResult> {
        let (mut result, missing) = self.run_starting(|budget| {
            self.adopt_now(&self.budget(ADOPT_BUDGET_MS))?;
            let active: Vec<AgentRec> = self
                .active_agents()?
                .into_iter()
                .filter(|a| a.kind == "PM")
                .collect();
            if active.is_empty() {
                return Err(OpError::launcher(
                    "no_pm",
                    "no PM agent is active; run cstan start",
                ));
            }
            if active.len() > 1 {
                return Err(OpError::launcher(
                    "pm_ambiguous",
                    "more than one PM agent is active",
                ));
            }
            let agent = &active[0];
            let Some(role) = self.role_named(&agent.role_name) else {
                return Err(OpError::launcher(
                    "unknown_role",
                    format!("the configuration has no role {}", agent.role_name),
                ));
            };
            // The project workspace must exist, with its watch tab, before the old PM pane is closed: Herdr refuses to
            // close the last pane of a workspace that has worktree children.
            let hub = self.ensure_hub(budget)?;
            if hub.status == HubStatus::Failed {
                return Err(OpError::launcher(
                    "hub_unavailable",
                    "the project workspace could not be opened",
                ));
            }
            let replaced = self.ledger.restart_agent_generation(&agent.agent_id)?;
            let generation = replaced.get("generation").and_then(Value::as_i64).unwrap_or(0);
            let credential = replaced
                .get("credential")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let summary = replaced.get("summary").cloned().unwrap_or(Value::Null);
            // The recorded pane counts even when adoption did not register it, so a live old PM is closed or listed,
            // never silently dropped.
            let old_pane = match self.adapter.pane_for_agent(&agent.agent_id) {
                Some(pane) => Some(pane),
                None => self
                    .ledger
                    .agent_panes()?
                    .into_iter()
                    .find(|row| row.agent_id == agent.agent_id)
                    .and_then(|row| row.pane_id),
            };
            if let Some(old_pane) = old_pane {
                let terminal_id = self.ledger.pane_terminal_id(&agent.agent_id)?;
                // The old pane id may have been given to another pane since; only the PM's own pane is closed.
                if let Err(error) =
                    self.close_owned(&old_pane, &agent.agent_id, terminal_id.as_deref())
                {
                    self.ledger.record_orphan_pane(
                        &agent.agent_id,
                        &old_pane,
                        terminal_id.as_deref(),
                    )?;
                    self.adapter.forget_pane(&old_pane);
                    self.log(
                        "old_pane_not_closed",
                        json!({"paneId": old_pane, "error": error.js_string()}),
                    );
                }
            }
            self.ledger.clear_agent_pane(&agent.agent_id)?;
            if summary.get("truncated").and_then(Value::as_bool) != Some(true) {
                let listed: std::collections::HashSet<&str> = summary
                    .get("messages")
                    .and_then(Value::as_array)
                    .map(|messages| {
                        messages
                            .iter()
                            .filter_map(|m| m.get("messageId").and_then(Value::as_str))
                            .collect()
                    })
                    .unwrap_or_default();
                let cancelled = crate::kernel::strings(replaced.get("cancelledMessageIds"));
                if cancelled.iter().any(|id| !listed.contains(id.as_str())) {
                    self.log("summary_incomplete", json!({"agentId": agent.agent_id}));
                }
            }
            let started = self.start_pm(role, &agent.agent_id, &credential, budget, &hub, Some(&summary));
            match started {
                Ok(started) => {
                    if let Some(latest) = self.ledger.pm_restart_sequences(&agent.agent_id)?.last() {
                        self.ledger
                            .mark_pm_restarts_consumed(&agent.agent_id, *latest)?;
                    }
                    Ok(LaunchResult {
                        generation: Some(generation),
                        hub: Some(hub.status),
                        ..started
                    })
                }
                Err(error) => Ok(LaunchResult {
                    state: Some(LaunchState::Failed),
                    agent_id: Some(agent.agent_id.clone()),
                    generation: Some(generation),
                    step: Some("start".into()),
                    reason: Some(error.message()),
                    hint: Some("the generation was replaced and its summary is recorded; run cstan pm restart again".into()),
                    ..LaunchResult::default()
                }),
            }
        })?;
        if let Some((names, warning)) = missing {
            result.missing_env = Some(names);
            result.warning = Some(warning);
        }
        Ok(result)
    }
}
