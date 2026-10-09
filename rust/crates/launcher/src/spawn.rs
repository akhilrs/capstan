//! Spawns a worker into its own worktree and names its branch (src/launcher/spawn.ts), and where the worker's pane goes in
//! the PM's tab (src/layout.ts).

use crate::api::{BranchRenameResult, Placement, SpawnOptions, SpawnResult, ADOPT_BUDGET_MS};
use crate::kernel::LauncherKernel;
use crate::shared::*;
use crate::text::{is_task_ref, normalize_task_title, one_line};
use capstan_config::prompts::{build_role_prompt, Kind, PromptInput};
use capstan_config::Role;
use capstan_herdr::api::{
    AdapterError, CreateWorktreeInput, DialogOutcome, LayoutPane, PaneAtPath, PlacePaneInput,
    SplitDirection, StartAgentInput, StartStatus, TrustDialogInput,
};
use capstan_herdr::naming::workspace_label;
use capstan_kernel::integrate::{slugify, with_suffix};
use serde_json::{json, Value};
use std::cmp::Ordering;
use std::sync::atomic::Ordering as AtomicOrdering;

// ------------------------------------------------------------------------------------------------ layout

/// Where a new worker pane goes inside the PM's tab. Pure: it reads pane sizes (in terminal cells) and returns the pane to
/// split, the direction and the share the target keeps, or nothing when no split leaves both parts at the minimum size.
/// The PM pane stays on the left at full height: the first worker is split off to its right, later workers stack down in
/// that column, and the PM pane is never split down.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub target_pane_id: String,
    pub direction: SplitDirection,
    /// The fraction of the target pane that the target keeps.
    pub keep: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct PlacementLimits {
    pub pm_width_percent: i64,
    pub min_columns: i64,
    pub min_rows: i64,
}

#[derive(PartialEq, Eq)]
enum Part {
    Text(String),
    Number(String),
}

fn parts(id: &str) -> Vec<Part> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut digits = String::new();
    for c in id.chars() {
        if c.is_ascii_digit() {
            if !text.is_empty() || out.is_empty() {
                out.push(Part::Text(std::mem::take(&mut text)));
            }
            digits.push(c);
        } else {
            if !digits.is_empty() {
                out.push(Part::Number(
                    std::mem::take(&mut digits)
                        .trim_start_matches('0')
                        .to_string(),
                ));
            }
            text.push(c);
        }
    }
    if !digits.is_empty() {
        out.push(Part::Number(digits.trim_start_matches('0').to_string()));
        out.push(Part::Text(String::new()));
    } else {
        out.push(Part::Text(text));
    }
    out
}

/// Compares ids such as w2:p10 and w2:p2 by their numeric parts (any length, leading zeros allowed), so p10 sorts after p2;
/// equal numbers fall back to the plain text, so the order is always fixed.
pub fn compare_pane_ids(a: &str, b: &str) -> Ordering {
    let left = parts(a);
    let right = parts(b);
    for (x, y) in left.iter().zip(right.iter()) {
        if x == y {
            continue;
        }
        return match (x, y) {
            (Part::Number(x), Part::Number(y)) => x.len().cmp(&y.len()).then_with(|| x.cmp(y)),
            (Part::Text(x), Part::Text(y)) => x.cmp(y),
            (Part::Text(_), Part::Number(_)) => Ordering::Less,
            (Part::Number(_), Part::Text(_)) => Ordering::Greater,
        };
    }
    left.len().cmp(&right.len()).then_with(|| a.cmp(b))
}

fn usable(pane: &LayoutPane) -> bool {
    pane.width > 0 && pane.height > 0
}

pub fn choose_placement(
    panes: &[LayoutPane],
    pm_pane_id: &str,
    limits: &PlacementLimits,
) -> Option<Choice> {
    let pm = panes.iter().find(|pane| pane.pane_id == pm_pane_id)?;
    if !usable(pm) {
        return None;
    }
    let mut workers: Vec<&LayoutPane> = panes
        .iter()
        .filter(|pane| pane.pane_id != pm_pane_id && usable(pane))
        .collect();
    workers.sort_by(|a, b| {
        b.height
            .cmp(&a.height)
            .then_with(|| compare_pane_ids(&a.pane_id, &b.pane_id))
    });
    if workers.is_empty() {
        let keep = limits.pm_width_percent as f64 / 100.0;
        let kept = (pm.width as f64 * keep).floor() as i64;
        let given = pm.width - kept;
        return (kept >= limits.min_columns && given >= limits.min_columns).then(|| Choice {
            target_pane_id: pm.pane_id.clone(),
            direction: SplitDirection::Right,
            keep,
        });
    }
    workers
        .into_iter()
        .find(|pane| pane.width >= limits.min_columns && pane.height / 2 >= limits.min_rows)
        .map(|pane| Choice {
            target_pane_id: pane.pane_id.clone(),
            direction: SplitDirection::Down,
            keep: 0.5,
        })
}

enum Placed {
    Placed(PaneAtPath),
    Note(String),
}

/// The task ref and title a spawn stores on every pane row it writes, normalized; absent ones are left out.
#[derive(Clone, Debug, Default)]
pub struct TaskFields {
    pub task_ref: Option<String>,
    pub task_title: Option<String>,
}

impl TaskFields {
    fn add_to(&self, row: &mut Value) {
        if let Some(task_ref) = &self.task_ref {
            row["taskRef"] = json!(task_ref);
        }
        if let Some(title) = &self.task_title {
            row["taskTitle"] = json!(title);
        }
    }
}

fn is_sha40(text: &str) -> bool {
    text.len() == 40 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn kind_of(role: &Role) -> Kind {
    match role.kind.as_str() {
        "PM" => Kind::Pm,
        "Verifier" => Kind::Verifier,
        "Supervisor" => Kind::Supervisor,
        _ => Kind::Developer,
    }
}

impl LauncherKernel {
    /// `baseSha` makes the worker's worktree and branch start at that commit (a review) instead of the project's HEAD. The
    /// branch is named after `task` (a `<plan-id>/<package-id>` or a requirement ref id), after `review_target` (a
    /// reviewer), or is `branch` itself (a replacement continues its predecessor's); otherwise it is an ad-hoc `chore/`
    /// name.
    pub fn spawn_worker(&self, role_name: &str, options: &SpawnOptions) -> OpResult<SpawnResult> {
        self.ledger.assert_run_not_paused("spawn")?;
        if let Some(base) = &options.base_sha {
            if !is_sha40(base) {
                return Err(OpError::launcher(
                    "invalid_base",
                    "the base commit must be a full lowercase id",
                ));
            }
        }
        let task_fields = self.task_fields(options)?;
        let (mut result, missing) = self
            .run_starting(|budget| self.spawn_in_turn(role_name, options, &task_fields, budget))?;
        if let Some((names, warning)) = missing {
            result.missing_env = Some(names);
            result.warning = Some(warning);
        }
        Ok(result)
    }

    fn spawn_in_turn(
        &self,
        role_name: &str,
        options: &SpawnOptions,
        task_fields: &TaskFields,
        budget: &Budget,
    ) -> OpResult<SpawnResult> {
        self.adopt_now(&self.budget(ADOPT_BUDGET_MS))?;
        let Some(role) = self.role_named(role_name) else {
            return Err(OpError::launcher(
                "unknown_role",
                format!("the configuration has no role {role_name}"),
            ));
        };
        if role.kind == "PM" {
            return Err(OpError::launcher(
                "kind_not_spawnable",
                "a PM is launched, not spawned",
            ));
        }
        if !self.config.operator.enabled && self.is_operator_role(&role.name, &role.kind) {
            return Err(OpError::launcher(
                "operator_disabled",
                format!(
                    "the role {} is the Operator and [operator] is not enabled",
                    role.name
                ),
            ));
        }
        self.assert_role_synced(role)?;
        // The Supervisor watches the workers and does not take one of their places; the Architect and the Operator do not
        // either unless the configuration says they count.
        let architect_counts = self.config.architect.count_toward_worker_limit;
        let operator_counts = self.config.operator.count_toward_worker_limit;
        let exempt = |name: &str, kind: &str| -> bool {
            kind == "Supervisor"
                || (!architect_counts && self.is_architect_role(name, kind))
                || (!operator_counts
                    && self.config.operator.enabled
                    && self.is_operator_role(name, kind))
        };
        let workers: Vec<AgentRec> = self
            .active_agents()?
            .into_iter()
            .filter(|a| a.kind != "PM" && !exempt(&a.role_name, &a.kind))
            .collect();
        let limit = self.config.limits.max_workers;
        if !exempt(&role.name, &role.kind) && workers.len() as i64 >= limit {
            let failures = self.cleanup_failures();
            let stuck: Vec<String> = workers
                .iter()
                .filter_map(|a| {
                    failures
                        .iter()
                        .find(|c| c.agent_id == a.agent_id)
                        .map(|c| format!("{}: {}", a.agent_id, c.reason))
                })
                .collect();
            return Err(OpError::launcher(
                "worker_limit",
                format!(
                    "{} of {limit} workers are active ({}); release one with cstan release <agent-id>{}",
                    workers.len(),
                    workers
                        .iter()
                        .map(|a| a.agent_id.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    if stuck.is_empty() {
                        String::new()
                    } else {
                        format!("; a cleanup failed for {}", stuck.join("; "))
                    }
                ),
            ));
        }
        // Fails with the git requirement's message before an agent record or a hub exists.
        self.git.head_sha()?;
        let pm = self.active_agents()?.into_iter().find(|a| a.kind == "PM");
        let pm_pane = match &pm {
            Some(pm) => self
                .ledger
                .agent_panes()?
                .into_iter()
                .find(|r| r.agent_id == pm.agent_id),
            None => None,
        };
        let Some(pm_pane) = pm_pane.filter(|p| p.workspace_id.is_some()) else {
            return Err(OpError::launcher(
                "pm_not_launched",
                "launch the PM first with cstan start",
            ));
        };
        // Herdr refuses to close a pane whose workspace has worktree children, so worktrees hang under the long-lived hub
        // workspace, never under the PM's.
        if self.ensure_hub_without_pm(budget)? == crate::api::HubStatus::Failed {
            return Err(OpError::launcher(
                "hub_unavailable",
                "the hub workspace could not be opened",
            ));
        }
        let Some((hub_workspace, _)) = self.ledger.fallback_pane()? else {
            return Err(OpError::launcher(
                "hub_unavailable",
                "the hub workspace could not be opened",
            ));
        };
        budget.check("creating the agent")?;
        let (agent_id, credential) = self.create_agent(role)?;
        let mut info = CleanupInfo::default();
        let mut step = "worktree";
        let attempt = self.spawn_steps(
            role,
            &agent_id,
            &credential,
            options,
            task_fields,
            budget,
            hub_workspace.as_deref(),
            pm_pane.pane_id.as_deref(),
            &mut info,
            &mut step,
        );
        match attempt {
            Ok(result) => Ok(result),
            Err(error) => {
                let failure = spawn_failure(error, step);
                self.log(
                    "spawn_failed",
                    json!({
                        "agentId": agent_id,
                        "step": step,
                        "error": one_line(&failure.message(), MAX_NOTE_LENGTH),
                    }),
                );
                self.cleanup_agent(&agent_id, &info, None, None)?;
                Err(failure)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_steps(
        &self,
        role: &Role,
        agent_id: &str,
        credential: &str,
        options: &SpawnOptions,
        task_fields: &TaskFields,
        budget: &Budget,
        hub_workspace: Option<&str>,
        pm_pane_id: Option<&str>,
        info: &mut CleanupInfo,
        step: &mut &'static str,
    ) -> OpResult<SpawnResult> {
        let branch = self.spawn_branch(agent_id, role, options)?;
        info.branch = Some(branch.clone());
        if !self.git.branch_name_valid(&branch) {
            return Err(OpError::launcher(
                "invalid_branch",
                format!("git does not accept the branch name {branch}"),
            ));
        }
        let base_sha = match &options.base_sha {
            Some(base) => base.clone(),
            None => self.git.head_sha()?,
        };
        info.base_sha = Some(base_sha.clone());
        let mut row = json!({
            "agentId": agent_id,
            "workspaceId": null,
            "paneId": null,
            "worktreePath": null,
            "branch": branch,
            "baseSha": base_sha,
        });
        task_fields.add_to(&mut row);
        self.ledger.record_agent_pane(row)?;
        budget.check("creating the worktree")?;
        let hub_workspace = hub_workspace.ok_or_else(|| {
            AdapterError::InvalidArgument("the hub workspace id is not recorded".into())
        })?;
        let tree = self.adapter.create_worktree(CreateWorktreeInput {
            workspace_id: hub_workspace,
            branch: &branch,
            label: &workspace_label(&self.project, agent_id),
            base: Some(&base_sha),
        })?;
        info.worktree_path = Some(tree.path.clone());
        info.pane_id = Some(tree.pane_id.clone());
        info.pane_created_here = true;
        let terminal_id = self.terminal_of(&tree.pane_id, agent_id);
        let mut row = json!({
            "agentId": agent_id,
            "workspaceId": tree.workspace_id,
            "paneId": tree.pane_id,
            "worktreePath": tree.path,
            "branch": branch,
            "baseSha": base_sha,
        });
        task_fields.add_to(&mut row);
        if let Some(terminal) = &terminal_id {
            row["terminalId"] = json!(terminal);
        }
        self.ledger.record_agent_pane(row)?;
        if let Some(worktree) = &self.config.worktree {
            *step = "setup";
            self.setup_worktree(worktree, agent_id, &tree.path, budget)?;
        }
        let mut pane_id = tree.pane_id.clone();
        let mut placement = Placement::Tab;
        let mut placement_note: Option<String> = None;
        if self.config.layout.spawn == "pane" {
            *step = "place";
            budget.check("placing the worker pane")?;
            info.move_may_have_happened = true;
            let outcome = match pm_pane_id {
                None => Placed::Note("the PM has no recorded pane".into()),
                Some(pm_pane) => self.place_worker_pane(&tree.pane_id, &tree.path, pm_pane)?,
            };
            match outcome {
                Placed::Placed(placed) => {
                    pane_id = placed.pane_id.clone();
                    placement = Placement::Pane;
                    info.pane_id = Some(pane_id.clone());
                    let placed_terminal = self.terminal_of(&pane_id, agent_id);
                    let mut row = json!({
                        "agentId": agent_id,
                        "workspaceId": placed.workspace_id,
                        "paneId": pane_id,
                        "worktreePath": tree.path,
                        "branch": branch,
                        "baseSha": base_sha,
                    });
                    task_fields.add_to(&mut row);
                    if let Some(terminal) = &placed_terminal {
                        row["terminalId"] = json!(terminal);
                    }
                    self.ledger.record_agent_pane(row)?;
                    info.move_may_have_happened = false;
                }
                Placed::Note(note) => {
                    placement_note = Some(note);
                    info.move_may_have_happened = false;
                }
            }
        }
        *step = "start";
        budget.check("starting the worker")?;
        let prompt_text = self.worker_prompt(role, agent_id, options)?;
        let prompt_file = self.adapter.write_prompt_file(&prompt_text)?;
        let args = self.arguments(role, &prompt_text, &prompt_file, Some(&tree.path))?;
        let environment = self.environment(Some(credential), None)?;
        let kind = self.host_kind(role)?;
        let started = self.start_with_retry(agent_id, &kind, &pane_id, &args, &environment)?;
        self.agents_started.fetch_add(1, AtomicOrdering::SeqCst);
        // The worker's workspace is gone once its pane is placed, and the PM's and the watch workspace already carry the
        // project token.
        self.describe(&pane_id, None, agent_id, &role.name, None);
        let result = |state: &str, hint: Option<String>| SpawnResult {
            state: state.to_string(),
            agent_id: agent_id.to_string(),
            pane_id: pane_id.clone(),
            worktree_path: tree.path.clone(),
            branch: branch.clone(),
            placement,
            placement_note: placement_note.clone(),
            hint,
            missing_env: None,
            warning: None,
        };
        if started == StartStatus::Started {
            return Ok(result("started", None));
        }
        *step = "trust";
        let log = |entry: &capstan_herdr::api::KeyLogEntry| {
            self.log(
                "trust_dialog_key",
                json!({"kind": "key", "pane": entry.pane, "key": entry.key, "reason": entry.reason}),
            )
        };
        let answered = self.adapter.answer_trust_dialog(TrustDialogInput {
            pane_id: &pane_id,
            log: &log,
            timeout_ms: Some(DIALOG_TIMEOUT_MS),
        })?;
        Ok(match answered {
            DialogOutcome::Handled { .. } => result("started", None),
            DialogOutcome::Unhandled { reason } => result(
                "blocked",
                Some(format!(
                    "the trust dialog was left alone ({reason}); answer it in the worker pane"
                )),
            ),
        })
    }

    fn worker_prompt(
        &self,
        role: &Role,
        agent_id: &str,
        options: &SpawnOptions,
    ) -> OpResult<String> {
        let mut input = PromptInput::new(
            &role.name,
            kind_of(role),
            agent_id,
            self.wait_seconds(role) as f64,
        );
        input.role_prompt = role.prompt_text.clone();
        if let Some(architect) = self.architect_prompt() {
            input.architect = Some(architect);
            input.is_architect = self.is_architect_role(&role.name, &role.kind);
        }
        if let Some(operator) = self.operator_prompt() {
            input.operator = Some(operator);
            input.is_operator = self.is_operator_role(&role.name, &role.kind);
        }
        if let Some(researcher) = self.researcher_prompt() {
            input.researcher = Some(researcher);
            input.is_researcher = self.is_researcher_role(&role.name, &role.kind);
        }
        input.nexora = self.nexora_prompt();
        input.replacement_seed = options.seed.clone();
        build_role_prompt(&input).map_err(|error| OpError::Kernel {
            name: "RangeError".into(),
            message: error.to_string(),
        })
    }

    /// The task ref and title a spawn stores on every pane row it writes, normalized; keys are omitted when absent.
    pub fn task_fields(&self, options: &SpawnOptions) -> OpResult<TaskFields> {
        let task_ref = options.task.as_ref().or(options.record_task_ref.as_ref());
        let title = options
            .title
            .as_ref()
            .or(options.record_task_title.as_ref());
        let invalid = |message: &str| OpError::launcher("invalid_task", message);
        let mut fields = TaskFields::default();
        if let Some(task_ref) = task_ref {
            if !is_task_ref(task_ref) {
                return Err(invalid(
                    "the task must be <plan-id>/<package-id> or a requirement ref id",
                ));
            }
            fields.task_ref = Some(task_ref.clone());
        }
        if let Some(title) = title {
            fields.task_title = Some(normalize_task_title(title).map_err(invalid)?);
        }
        Ok(fields)
    }

    /// The branch a new worker gets; a name that is taken gets `-2`, `-3`, ...
    pub fn spawn_branch(
        &self,
        agent_id: &str,
        role: &Role,
        options: &SpawnOptions,
    ) -> OpResult<String> {
        if let Some(branch) = &options.branch {
            if self.git.branch_tip(branch).is_some()
                || self.git.worktree_by_branch(branch)?.is_some()
                || self
                    .ledger
                    .active_branch_holder(branch, agent_id)?
                    .is_some()
            {
                return Err(OpError::launcher(
                    "branch_in_use",
                    format!("{branch} still exists; it cannot be continued"),
                ));
            }
            return Ok(branch.clone());
        }
        let wanted = if let Some(target) = &options.review_target {
            review_branch_name(agent_id, target)
        } else if let Some(task) = &options.task {
            self.task_branch(
                &role.name,
                &role.kind,
                task,
                options.type_.as_deref(),
                options.title.as_deref(),
            )?
        } else {
            ad_hoc_branch_name(
                agent_id,
                &slugify(options.title.as_deref().unwrap_or(&role.name), 40),
            )
        };
        self.free_branch(&wanted, agent_id)
    }

    pub fn task_branch(
        &self,
        role_name: &str,
        role_kind: &str,
        task: &str,
        type_: Option<&str>,
        title: Option<&str>,
    ) -> OpResult<String> {
        let (task_id, naming_type, naming_title) = self
            .ledger
            .task_naming(task)
            .map_err(|error| OpError::launcher("unknown_task", OpError::from(error).message()))?;
        let kind = type_
            .map(str::to_string)
            .or(naming_type)
            .unwrap_or_else(|| {
                if self.is_researcher_role(role_name, role_kind) {
                    "docs".to_string()
                } else {
                    "feat".to_string()
                }
            });
        Ok(worker_branch_name(
            &kind,
            &task_id,
            title.unwrap_or(&naming_title),
        ))
    }

    /// `wanted`, or the first of `wanted-2`, `wanted-3`, ... that no branch, worktree or active agent's record holds.
    pub fn free_branch(&self, wanted: &str, agent_id: &str) -> OpResult<String> {
        for n in 1..1000 {
            let name = with_suffix(wanted, n);
            if self.git.branch_tip(&name).is_none()
                && self.git.worktree_by_branch(&name)?.is_none()
                && self.ledger.active_branch_holder(&name, agent_id)?.is_none()
            {
                return Ok(name);
            }
        }
        Err(OpError::launcher(
            "invalid_branch",
            format!("no free branch name starting with {wanted}"),
        ))
    }

    /// Gives an assignee's branch the name of its task when it still has no commit after its base and no report; otherwise
    /// the branch is kept. Never fails the assignment: the answer says what happened.
    pub fn rename_branch_for_task(
        &self,
        agent_id: &str,
        task: &str,
    ) -> OpResult<BranchRenameResult> {
        self.run(|_| {
            let row = self
                .ledger
                .agent_panes()?
                .into_iter()
                .find(|candidate| candidate.agent_id == agent_id);
            let agent = self.agent_record(agent_id)?;
            let (Some(row), Some(agent)) = (row, agent) else {
                return Ok(BranchRenameResult {
                    branch: None,
                    renamed: false,
                    note: Some("no branch recorded".into()),
                });
            };
            let Some(current) = row.branch.clone() else {
                return Ok(BranchRenameResult {
                    branch: None,
                    renamed: false,
                    note: Some("no branch recorded".into()),
                });
            };
            let kept = |why: &str| BranchRenameResult {
                branch: Some(current.clone()),
                renamed: false,
                note: Some(format!("branch kept: {current} ({why})")),
            };
            // Only an ad-hoc chore/<agent-id>-<slug> name is replaced; a task-based name from spawn (or an older one) stays.
            let ad_hoc_prefix = ad_hoc_branch_prefix(agent_id);
            let Some(spawn_slug) = current.strip_prefix(&ad_hoc_prefix) else {
                return Ok(kept("it already has a task-based name"));
            };
            // The spawn's title lives on in the ad-hoc slug; a slug that is just the role name carries no title.
            let title = if spawn_slug.is_empty() || spawn_slug == slugify(&agent.role_name, 40) {
                None
            } else {
                Some(spawn_slug)
            };
            let wanted = match self.task_branch(&agent.role_name, &agent.kind, task, None, title) {
                Ok(wanted) => wanted,
                Err(error) => return Ok(kept(&error.message())),
            };
            if current == wanted {
                return Ok(BranchRenameResult {
                    branch: Some(current.clone()),
                    renamed: false,
                    note: None,
                });
            }
            if self.ledger.agent_has_reports(agent_id)? {
                return Ok(kept("the agent has reported"));
            }
            let tip = self.git.branch_tip(&current);
            match (&tip, &row.base_sha) {
                (Some(tip), Some(base)) if tip == base => {}
                _ => return Ok(kept("it already has commits")),
            }
            let name = self.free_branch(&wanted, agent_id)?;
            if !self.git.branch_name_valid(&name) {
                return Ok(kept("git does not accept the new name"));
            }
            let moved = self.git.rename_branch(&current, &name);
            if !moved.removed {
                return Ok(kept(if moved.stderr.is_empty() {
                    "git refused the rename"
                } else {
                    &moved.stderr
                }));
            }
            if let Err(error) = self.ledger.rename_agent_branch(agent_id, &current, &name) {
                self.git.rename_branch(&name, &current);
                return Ok(kept(&OpError::from(error).message()));
            }
            self.log(
                "branch_renamed",
                json!({"agentId": agent_id, "from": current, "to": name}),
            );
            Ok(BranchRenameResult {
                branch: Some(name),
                renamed: true,
                note: None,
            })
        })
    }

    /// Starts a worker's agent. A pane whose shell was not ready yet may be ready a moment later, so those two failures are
    /// tried again; the agent is not running when they are thrown. Every other error is final.
    fn start_with_retry(
        &self,
        agent_id: &str,
        kind: &str,
        pane_id: &str,
        args: &[String],
        environment: &std::collections::BTreeMap<String, String>,
    ) -> OpResult<StartStatus> {
        let mut earlier: Option<AdapterError> = None;
        let mut attempt = 1;
        loop {
            let started = self.adapter.start_agent(StartAgentInput {
                name: agent_id,
                kind,
                pane_id,
                args,
                timeout_ms: Some(START_TIMEOUT_MS),
                environment: Some(environment),
            });
            let error = match started {
                Ok(status) => return Ok(status),
                Err(error) => error,
            };
            // A pane the failed attempt tainted refuses the next one; the first failure is the real one.
            if matches!(error, AdapterError::Phase(_)) {
                if let Some(first) = earlier {
                    return Err(first.into());
                }
            }
            let retryable = matches!(
                error,
                AdapterError::PromptUnrecognized(_) | AdapterError::ShellNotReady(_)
            ) && self
                .adapter
                .pane_entry(pane_id)
                .is_none_or(|entry| entry.agent.is_none());
            if !retryable || attempt >= START_ATTEMPTS {
                return Err(error.into());
            }
            self.log(
                "start_retry",
                json!({
                    "agentId": agent_id,
                    "attempt": attempt,
                    "error": one_line(&OpError::from(error.clone()).js_string(), MAX_NOTE_LENGTH),
                }),
            );
            earlier = Some(error);
            self.sleep(START_RETRY_DELAY_MS);
            attempt += 1;
        }
    }

    /// Splits a new worker pane into the PM's tab, or says why it stays a tab. A pane that was lost in the move is an
    /// error, not a fallback.
    fn place_worker_pane(
        &self,
        pane_id: &str,
        worktree_path: &str,
        pm_pane_id: &str,
    ) -> OpResult<Placed> {
        let layout_config = &self.config.layout;
        let attempt = || -> OpResult<Placed> {
            let layout = self.adapter.pane_layout(pm_pane_id)?;
            if layout.zoomed {
                return Ok(Placed::Note("the PM's tab has a zoomed pane".into()));
            }
            let mut live: std::collections::HashSet<String> = std::collections::HashSet::new();
            for row in self.ledger.agent_panes()? {
                let Some(row_pane) = &row.pane_id else {
                    continue;
                };
                let agent = self.agent_record(&row.agent_id)?;
                let counts = agent.as_ref().is_some_and(|agent| {
                    agent.active()
                        && (agent.kind == "PM"
                            || !self
                                .cleanup_failures()
                                .iter()
                                .any(|c| c.agent_id == row.agent_id))
                });
                if counts && self.adapter.pane_entry(row_pane).is_some() {
                    live.insert(row_pane.clone());
                }
            }
            let candidates: Vec<LayoutPane> = layout
                .panes
                .iter()
                .filter(|pane| live.contains(&pane.pane_id))
                .cloned()
                .collect();
            let limits = PlacementLimits {
                pm_width_percent: layout_config.pm_width_percent,
                min_columns: layout_config.min_pane_columns,
                min_rows: layout_config.min_pane_rows,
            };
            let Some(choice) = choose_placement(&candidates, pm_pane_id, &limits) else {
                return Ok(Placed::Note(format!(
                    "no pane has room for a split of at least {} columns by {} rows",
                    layout_config.min_pane_columns, layout_config.min_pane_rows
                )));
            };
            let placed = self.adapter.place_pane(PlacePaneInput {
                pane_id,
                tab_id: &layout.tab_id,
                target_pane_id: &choice.target_pane_id,
                direction: choice.direction,
                keep: choice.keep,
                worktree_path,
            })?;
            Ok(Placed::Placed(placed))
        };
        match attempt() {
            Ok(placed) => Ok(placed),
            Err(error) if matches!(error.adapter(), Some(AdapterError::PaneLost(_))) => Err(error),
            Err(error) => {
                self.log(
                    "placement_failed",
                    json!({"paneId": pane_id, "error": error.js_string()}),
                );
                let reason = one_line(&error.message(), MAX_NOTE_LENGTH);
                Ok(Placed::Note(format!(
                    "the pane could not be placed{}",
                    if reason.is_empty() {
                        String::new()
                    } else {
                        format!(" ({reason})")
                    }
                )))
            }
        }
    }
}

/// A LauncherError passes unchanged; anything else names the failing step, with the adapter's own code when it has one.
pub fn spawn_failure(error: OpError, step: &str) -> OpError {
    if matches!(error, OpError::Launcher(_)) {
        return error;
    }
    let message = one_line(&error.message(), MAX_NOTE_LENGTH);
    let code = match error.adapter().and_then(AdapterError::herdr_code) {
        Some(code)
            if !code.is_empty()
                && code
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') =>
        {
            code.to_string()
        }
        _ => "spawn_failed".to_string(),
    };
    OpError::launcher(code, format!("step {step}: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str, width: i64, height: i64) -> LayoutPane {
        LayoutPane {
            pane_id: id.into(),
            width,
            height,
        }
    }

    const LIMITS: PlacementLimits = PlacementLimits {
        pm_width_percent: 60,
        min_columns: 60,
        min_rows: 12,
    };

    #[test]
    fn pane_ids_compare_by_their_numbers() {
        assert_eq!(compare_pane_ids("w2:p10", "w2:p2"), Ordering::Greater);
        assert_eq!(compare_pane_ids("w2:p2", "w2:p10"), Ordering::Less);
        assert_eq!(compare_pane_ids("w2:p02", "w2:p2"), Ordering::Less);
        assert_eq!(compare_pane_ids("w2:p2", "w2:p2"), Ordering::Equal);
        assert_eq!(compare_pane_ids("a", "a1"), Ordering::Less);
    }

    #[test]
    fn the_first_worker_goes_to_the_right_of_the_pm() {
        let panes = [pane("p1", 200, 50)];
        let choice = choose_placement(&panes, "p1", &LIMITS).unwrap();
        assert_eq!(choice.target_pane_id, "p1");
        assert_eq!(choice.direction, SplitDirection::Right);
        assert_eq!(choice.keep, 0.6);
        assert_eq!(
            choose_placement(&[pane("p1", 100, 50)], "p1", &LIMITS),
            None
        );
    }

    #[test]
    fn later_workers_stack_down_in_the_tallest_pane_with_room() {
        let panes = [pane("p1", 120, 50), pane("p2", 80, 20), pane("p3", 80, 30)];
        let choice = choose_placement(&panes, "p1", &LIMITS).unwrap();
        assert_eq!(choice.target_pane_id, "p3");
        assert_eq!(choice.direction, SplitDirection::Down);
        assert_eq!(choice.keep, 0.5);
        assert_eq!(
            choose_placement(&[pane("p1", 120, 50), pane("p2", 80, 20)], "p1", &LIMITS),
            None
        );
    }

    #[test]
    fn a_failed_step_names_itself_and_keeps_the_herdr_code() {
        let herdr = OpError::Adapter(AdapterError::Herdr(capstan_herdr::api::HerdrError::new(
            "workspace_not_found",
            "no such workspace",
        )));
        let failure = spawn_failure(herdr, "worktree");
        assert_eq!(
            failure,
            OpError::launcher("workspace_not_found", "step worktree: no such workspace")
        );
        let other = spawn_failure(
            OpError::Adapter(AdapterError::ShellNotReady("slow".into())),
            "start",
        );
        assert_eq!(other, OpError::launcher("spawn_failed", "step start: slow"));
        let own = OpError::launcher("worker_limit", "x");
        assert_eq!(spawn_failure(own.clone(), "start"), own);
    }
}
