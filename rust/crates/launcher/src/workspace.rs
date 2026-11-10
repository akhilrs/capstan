//! Workspace mode of the launcher: a worker gets one change folder, `<worktree_dir>/<agent-id>`, that holds one git
//! worktree per repo of its task, all on one branch. The folder is not a git work tree; the worker opens in a hub tab
//! whose directory is the folder and runs git inside each repo.
//!
//! The helpers `add_repo_worktree`, `remove_repo_worktree`, `run_repo_setup` and `rollback_added` are shared with the
//! package that adds a repo to a running worker.

use crate::api::{
    BranchRenameResult, Placement, ReleaseOutcome, RepoAddResult, SpawnOptions, SpawnResult,
    CLEANUP_BUDGET_MS,
};
use crate::kernel::LauncherKernel;
use crate::setup::RepoSetup;
use crate::shared::*;
use crate::spawn::{spawn_failure, TaskFields};
use capstan_config::{
    CleanupWorktrees, RepoConfig, Role, Workspace, Worktree, DEFAULT_WORKTREE_SETUP_TIMEOUT_SECONDS,
};
use capstan_herdr::api::{AdapterError, CreateTabInput, PaneRole};
use capstan_herdr::naming::workspace_label;
use capstan_kernel::integrate::with_suffix;
use serde_json::json;
use std::path::{Path, PathBuf};

/// The refusal text of a host that cannot open a pane at a folder.
pub const HOST_UNSUPPORTED: &str =
    "this host cannot open a worker at a folder; workspace mode needs it";

const NOT_A_WORKTREE: &str = "is not a working tree";

/// A repo a spawn works in and the commit its worktree starts at (`None`: the tip of the repo's main branch).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceRepo {
    pub name: String,
    pub commit: Option<String>,
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

impl LauncherKernel {
    pub(crate) fn workspace(&self) -> Option<&Workspace> {
        self.config.workspace.as_ref()
    }

    fn repo_config(&self, name: &str) -> Option<&RepoConfig> {
        self.workspace().and_then(|ws| ws.repo(name))
    }

    /// The repos of a new workspace worker, in merge order: a reviewer's `repo_bases`, else the repos its package
    /// names, else the default repo. A name that is not in `[repos]` refuses before any worktree is made.
    pub(crate) fn workspace_repos(&self, options: &SpawnOptions) -> OpResult<Vec<WorkspaceRepo>> {
        let Some(ws) = self.workspace() else {
            return Ok(Vec::new());
        };
        let mut chosen: Vec<WorkspaceRepo> = Vec::new();
        if let Some(bases) = options
            .repo_bases
            .as_ref()
            .filter(|bases| !bases.is_empty())
        {
            for base in bases {
                if ws.repo(&base.repo).is_none() {
                    return Err(OpError::launcher(
                        "unknown_repo",
                        format!(
                            "repo_bases names repo {}, which is not in [repos]",
                            base.repo
                        ),
                    ));
                }
                if !chosen.iter().any(|repo| repo.name == base.repo) {
                    chosen.push(WorkspaceRepo {
                        name: base.repo.clone(),
                        commit: Some(base.commit.clone()),
                    });
                }
            }
        } else if let Some((plan, package)) = options
            .task
            .as_deref()
            .and_then(|task| task.split_once('/'))
        {
            for name in self.ledger.package_repos(plan, package)? {
                if ws.repo(&name).is_none() {
                    return Err(OpError::launcher(
                        "unknown_repo",
                        format!(
                            "package {plan}/{package} names repo {name}, which is not in [repos]"
                        ),
                    ));
                }
                if !chosen.iter().any(|repo| repo.name == name) {
                    chosen.push(WorkspaceRepo { name, commit: None });
                }
            }
        }
        if chosen.is_empty() {
            chosen.push(WorkspaceRepo {
                name: ws.default_repo.clone(),
                commit: None,
            });
        }
        chosen.sort_by_key(|repo| {
            ws.merge_order
                .iter()
                .position(|name| name == &repo.name)
                .unwrap_or(usize::MAX)
        });
        Ok(chosen)
    }

    /// Whether `name` is taken in any of the repos (a branch, a worktree, a remote branch) or by an active agent's
    /// record.
    fn ws_branch_taken(&self, name: &str, agent_id: &str, repos: &[&RepoConfig]) -> OpResult<bool> {
        for repo in repos {
            if self.git.branch_taken_in(&path_text(&repo.root), name)? {
                return Ok(true);
            }
        }
        Ok(self.ledger.active_branch_holder(name, agent_id)?.is_some())
    }

    /// `wanted`, or the first of `wanted-2`, `wanted-3`, ... that is free in every repo.
    pub(crate) fn free_branch_in(
        &self,
        wanted: &str,
        agent_id: &str,
        repos: &[&RepoConfig],
    ) -> OpResult<String> {
        for n in 1..1000 {
            let name = with_suffix(wanted, n);
            if !self.ws_branch_taken(&name, agent_id, repos)? {
                return Ok(name);
            }
        }
        Err(OpError::launcher(
            "invalid_branch",
            format!("no free branch name starting with {wanted}"),
        ))
    }

    fn base_in(&self, repo: &RepoConfig, commit: Option<&str>) -> OpResult<String> {
        if let Some(commit) = commit {
            return Ok(commit.to_string());
        }
        self.git
            .branch_tip_in(&path_text(&repo.root), &repo.main_branch)
            .ok_or_else(|| {
                OpError::launcher(
                    "git_error",
                    format!(
                        "repo {} has no branch {} to start from",
                        repo.name, repo.main_branch
                    ),
                )
            })
    }

    /// `git worktree add -b <branch> <folder>/<repo> <base>`.
    pub(crate) fn add_repo_worktree(
        &self,
        repo: &RepoConfig,
        folder: &Path,
        branch: &str,
        commit: Option<&str>,
    ) -> OpResult<RepoTree> {
        let base = self.base_in(repo, commit)?;
        let path = path_text(&folder.join(&repo.name));
        self.git
            .worktree_add_in(&path_text(&repo.root), &path, branch, &base)
            .map_err(|error| {
                OpError::launcher(
                    "worktree_add_failed",
                    format!("repo {}: {}", repo.name, error.message),
                )
            })?;
        Ok(RepoTree {
            repo: repo.name.clone(),
            path,
            branch: branch.to_string(),
            base_sha: base,
        })
    }

    /// Removes one worktree and, when its branch has not moved from `base_sha`, the branch. `Err` is git's message.
    pub(crate) fn remove_repo_worktree(
        &self,
        repo: &RepoConfig,
        tree: &RepoTree,
    ) -> Result<(), String> {
        let root = path_text(&repo.root);
        if self.git.worktree_present(&tree.path) {
            let removal = self.git.worktree_remove_in(&root, &tree.path);
            if !removal.removed && !removal.stderr.contains(NOT_A_WORKTREE) {
                return Err(removal.stderr);
            }
        }
        self.git.prune_worktrees_in(&root);
        self.git
            .delete_branch_if_in(&root, &tree.branch, &tree.base_sha);
        Ok(())
    }

    /// The setup command of one repo's worktree, in it, with `CAPSTAN_REPO` and the repo's cache entries.
    pub(crate) fn run_repo_setup(
        &self,
        repo: &RepoConfig,
        agent_id: &str,
        tree: &RepoTree,
        budget: &Budget,
    ) -> OpResult<()> {
        let config = self.config.worktree.clone().unwrap_or(Worktree {
            setup: None,
            setup_timeout_seconds: DEFAULT_WORKTREE_SETUP_TIMEOUT_SECONDS,
            teardown: None,
            teardown_timeout_seconds: None,
        });
        let context = RepoSetup {
            name: repo.name.clone(),
            command: repo.setup.clone(),
            cache: repo.cache.clone(),
        };
        self.setup_worktree_for(&config, agent_id, &tree.path, budget, Some(&context))
            .map_err(|error| {
                OpError::launcher(
                    "worktree_setup_failed",
                    format!("repo {}: {}", repo.name, error.message()),
                )
            })
    }

    /// Removes the worktrees (and new branches) this operation made; the folder goes when it is empty. The trees whose
    /// removal failed come back with git's message.
    pub(crate) fn rollback_added(
        &self,
        added: &[RepoTree],
        folder: &Path,
    ) -> Vec<(RepoTree, String)> {
        let mut failed = Vec::new();
        for tree in added.iter().rev() {
            let Some(repo) = self.repo_config(&tree.repo) else {
                failed.push((tree.clone(), "the repo is not in [repos]".to_string()));
                continue;
            };
            if let Err(message) = self.remove_repo_worktree(repo, tree) {
                failed.push((tree.clone(), message));
            }
        }
        if failed.is_empty() {
            let _ = std::fs::remove_dir(folder);
        }
        failed
    }

    fn in_use(folder: &Path, why: &str) -> OpError {
        OpError::launcher(
            "change_folder_in_use",
            format!("the change folder {} is in use: {why}", folder.display()),
        )
    }

    /// A leftover change folder of this agent id (a spawn that crashed) is reused when no live pane holds it and none
    /// of its worktrees has a commit; its worktrees are pruned. Otherwise it is `change_folder_in_use`.
    fn prepare_change_folder(&self, ws: &Workspace, folder: &Path) -> OpResult<()> {
        if !folder.exists() {
            return Ok(());
        }
        let panes = self
            .adapter
            .panes_at_path(&path_text(folder))
            .map_err(|error| {
                Self::in_use(folder, &format!("its panes cannot be listed ({error:?})"))
            })?;
        if !panes.is_empty() {
            return Err(Self::in_use(folder, "a pane is open in it"));
        }
        let entries =
            std::fs::read_dir(folder).map_err(|error| Self::in_use(folder, &error.to_string()))?;
        let mut found: Vec<(&RepoConfig, PathBuf)> = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(repo) = ws.repo(&name) else {
                return Err(Self::in_use(
                    folder,
                    &format!("it holds {name}, which is not a repo"),
                ));
            };
            let tip = self
                .git
                .branch_tip_in(&path_text(&repo.root), &repo.main_branch);
            let Some(tip) = tip else {
                return Err(Self::in_use(
                    folder,
                    &format!("repo {name} has no {}", repo.main_branch),
                ));
            };
            let counted = crate::git::run_git(
                &[
                    "-C",
                    &path_text(&entry.path()),
                    "rev-list",
                    "--count",
                    "HEAD",
                    "--not",
                    &tip,
                ],
                None,
            );
            if !counted.ok() || counted.stdout.trim() != "0" {
                return Err(Self::in_use(folder, &format!("repo {name} has commits")));
            }
            found.push((repo, entry.path()));
        }
        for (repo, path) in &found {
            let root = path_text(&repo.root);
            let head = crate::git::run_git(
                &["-C", &path_text(path), "rev-parse", "--abbrev-ref", "HEAD"],
                None,
            );
            let tip = crate::git::run_git(&["-C", &path_text(path), "rev-parse", "HEAD"], None);
            let removal = self.git.worktree_remove_in(&root, &path_text(path));
            if !removal.removed {
                return Err(Self::in_use(
                    folder,
                    &format!("repo {} could not be pruned: {}", repo.name, removal.stderr),
                ));
            }
            self.git.prune_worktrees_in(&root);
            if head.ok() && tip.ok() {
                self.git
                    .delete_branch_if_in(&root, head.stdout.trim(), tip.stdout.trim());
            }
        }
        std::fs::remove_dir(folder).map_err(|error| Self::in_use(folder, &error.to_string()))?;
        Ok(())
    }

    /// The sentences a workspace worker gets after its role prompt.
    fn workspace_block(folder: &str, branch: &str, trees: &[RepoTree]) -> String {
        let mut text = format!(
            "Workspace: your change folder is {folder}. It is not a git work tree. It holds one worktree per repo, all on the branch {branch}:\n"
        );
        for tree in trees {
            text.push_str(&format!("- {}: {}\n", tree.repo, tree.path));
        }
        text.push_str(
            "Run git inside each repo (git -C <repo path> ...), not in the change folder, and read each repo's CLAUDE.md before you change it. cstan works from the change folder.",
        );
        text
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn_workspace_steps(
        &self,
        role: &Role,
        agent_id: &str,
        credential: &str,
        options: &SpawnOptions,
        task_fields: &TaskFields,
        budget: &Budget,
        hub_workspace: Option<&str>,
        info: &mut CleanupInfo,
        step: &mut &'static str,
        repos: Option<&[WorkspaceRepo]>,
        continued: Option<&[RepoTree]>,
    ) -> OpResult<SpawnResult> {
        let ws = self.workspace().cloned().ok_or_else(|| {
            OpError::launcher("not_workspace", "the configuration has no [repos]")
        })?;
        let folder: PathBuf = match continued.and_then(|trees| trees.first()) {
            Some(tree) => Path::new(&tree.path)
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| ws.worktree_dir.join(agent_id)),
            None => ws.worktree_dir.join(agent_id),
        };
        let mut added: Vec<RepoTree> = Vec::new();
        let mut recorded = false;
        let attempt = self.ws_make(
            role,
            agent_id,
            credential,
            options,
            task_fields,
            budget,
            hub_workspace,
            info,
            step,
            repos.unwrap_or(&[]),
            continued,
            &ws,
            &folder,
            &mut added,
            &mut recorded,
        );
        match attempt {
            Ok(result) => Ok(result),
            Err(error) => {
                let failure = spawn_failure(error, step);
                if recorded || added.is_empty() {
                    return Err(failure);
                }
                let failed = self.rollback_added(&added, &folder);
                info.worktrees.retain(|tree| !added.contains(tree));
                if failed.is_empty() {
                    return Err(failure);
                }
                let notes: Vec<String> = failed
                    .iter()
                    .map(|(tree, why)| format!("repo {} ({}): {why}", tree.repo, tree.path))
                    .collect();
                info.worktrees
                    .extend(failed.into_iter().map(|(tree, _)| tree));
                match failure {
                    OpError::Launcher(inner) => Err(OpError::launcher(
                        inner.code,
                        format!(
                            "{}; a worktree could not be removed: {}",
                            inner.message,
                            notes.join("; ")
                        ),
                    )),
                    other => Err(other),
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn ws_make(
        &self,
        role: &Role,
        agent_id: &str,
        credential: &str,
        options: &SpawnOptions,
        task_fields: &TaskFields,
        budget: &Budget,
        hub_workspace: Option<&str>,
        info: &mut CleanupInfo,
        step: &mut &'static str,
        repos: &[WorkspaceRepo],
        continued: Option<&[RepoTree]>,
        ws: &Workspace,
        folder: &Path,
        added: &mut Vec<RepoTree>,
        recorded: &mut bool,
    ) -> OpResult<SpawnResult> {
        let folder_text = path_text(folder);
        info.folder = Some(folder_text.clone());
        let hub_workspace = hub_workspace.ok_or_else(|| {
            AdapterError::InvalidArgument("the hub workspace id is not recorded".into())
        })?;
        let trees: Vec<RepoTree> = match continued {
            Some(trees) => trees.to_vec(),
            None => {
                let configs: Vec<&RepoConfig> = repos
                    .iter()
                    .filter_map(|repo| ws.repo(&repo.name))
                    .collect();
                let branch = match &options.branch {
                    Some(branch) => {
                        if self.ws_branch_taken(branch, agent_id, &configs)? {
                            return Err(OpError::launcher(
                                "branch_in_use",
                                format!("{branch} still exists; it cannot be continued"),
                            ));
                        }
                        branch.clone()
                    }
                    None => {
                        let wanted = self.wanted_branch(agent_id, role, options)?;
                        self.free_branch_in(&wanted, agent_id, &configs)?
                    }
                };
                info.branch = Some(branch.clone());
                if !self.git.branch_name_valid(&branch) {
                    return Err(OpError::launcher(
                        "invalid_branch",
                        format!("git does not accept the branch name {branch}"),
                    ));
                }
                budget.check("creating the worktrees")?;
                *step = "worktree";
                self.prepare_change_folder(ws, folder)?;
                std::fs::create_dir_all(folder).map_err(|error| {
                    OpError::launcher(
                        "worktree_add_failed",
                        format!("the change folder {folder_text} could not be made: {error}"),
                    )
                })?;
                for repo in repos {
                    let config = ws.repo(&repo.name).ok_or_else(|| {
                        OpError::launcher(
                            "unknown_repo",
                            format!("repo {} is not in [repos]", repo.name),
                        )
                    })?;
                    let tree =
                        self.add_repo_worktree(config, folder, &branch, repo.commit.as_deref())?;
                    added.push(tree.clone());
                    info.worktrees.push(tree);
                }
                *step = "setup";
                for tree in added.clone() {
                    if let Some(config) = ws.repo(&tree.repo) {
                        self.run_repo_setup(config, agent_id, &tree, budget)?;
                    }
                }
                added.clone()
            }
        };
        let Some(first) = trees.first() else {
            return Err(OpError::launcher("no_worktree", "the worker has no repo"));
        };
        let branch = first.branch.clone();
        let base_sha = first.base_sha.clone();
        info.base_sha = Some(base_sha.clone());
        info.branch = Some(branch.clone());
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
        *step = "tab";
        budget.check("opening the worker tab")?;
        let tab = match self.adapter.create_tab(CreateTabInput {
            workspace_id: hub_workspace,
            cwd: &folder_text,
            label: &workspace_label(&self.project, agent_id),
            role: PaneRole::Worker,
            record_folder: true,
        }) {
            Ok(tab) => tab,
            Err(AdapterError::UnsupportedHost(_)) => {
                return Err(OpError::launcher("host_unsupported", HOST_UNSUPPORTED))
            }
            Err(error) => return Err(error.into()),
        };
        info.pane_id = Some(tab.pane_id.clone());
        info.pane_created_here = true;
        let terminal_id = self.terminal_of(&tab.pane_id, agent_id);
        let mut row = json!({
            "agentId": agent_id,
            "workspaceId": hub_workspace,
            "paneId": tab.pane_id,
            "worktreePath": folder_text,
            "branch": branch,
            "baseSha": base_sha,
        });
        task_fields.add_to(&mut row);
        if let Some(terminal) = &terminal_id {
            row["terminalId"] = json!(terminal);
        }
        self.ledger.record_agent_pane(row)?;
        // The rows come only after every repo succeeded.
        let generation = self
            .agent_record(agent_id)?
            .map(|agent| agent.generation)
            .unwrap_or(1);
        let mut written: Vec<&RepoTree> = Vec::new();
        for tree in &trees {
            if let Err(error) = self
                .ledger
                .record_agent_worktree(agent_id, generation, tree)
            {
                for done in &written {
                    let _ = self.ledger.remove_agent_worktree(agent_id, &done.repo);
                }
                return Err(error.into());
            }
            written.push(tree);
        }
        *recorded = true;
        info.worktrees.clear();
        *step = "start";
        budget.check("starting the worker")?;
        let mut prompt_text = self.worker_prompt(role, agent_id, options)?;
        prompt_text.push_str("\n\n");
        prompt_text.push_str(&Self::workspace_block(&folder_text, &branch, &trees));
        let prompt_file = self.adapter.write_prompt_file(&prompt_text)?;
        let tree_paths: Vec<String> = trees.iter().map(|tree| tree.path.clone()).collect();
        let args = self.arguments_for(
            role,
            &prompt_text,
            &prompt_file,
            Some(&folder_text),
            &tree_paths,
        )?;
        let (state, hint) =
            self.start_worker_pane(role, agent_id, credential, &tab.pane_id, &args, step)?;
        let placement_note = (self.config.layout.spawn == "pane")
            .then(|| "a workspace worker opens in a tab of its own".to_string());
        Ok(SpawnResult {
            state,
            agent_id: agent_id.to_string(),
            pane_id: tab.pane_id,
            worktree_path: folder_text,
            branch,
            placement: Placement::Tab,
            placement_note,
            hint,
            missing_env: None,
            warning: None,
        })
    }

    /// The release of a workspace worker: the worktrees with no commit after their base (and no report) go with their
    /// new branches, those with commits stay for the integration, and the folder goes when it is empty. A removal that
    /// fails keeps the row and the pane row, so the next start retries it.
    pub(crate) fn release_workspace(
        &self,
        agent_id: &str,
        info: &CleanupInfo,
        pane_closed: Option<bool>,
    ) -> OpResult<ReleaseOutcome> {
        let mut trees = self.ledger.agent_worktrees(agent_id)?;
        for tree in &info.worktrees {
            if !trees.iter().any(|known| known.repo == tree.repo) {
                trees.push(tree.clone());
            }
        }
        let budget = self.budget(CLEANUP_BUDGET_MS);
        let keep_all =
            info.keep_worktrees || self.config.cleanup.worktrees == CleanupWorktrees::Keep;
        let reported = !keep_all && self.ledger.agent_has_reports(agent_id)?;
        let mut failed = false;
        let mut all_removed = true;
        let mut any_branch_kept = false;
        for tree in &trees {
            let Some(repo) = self.repo_config(&tree.repo) else {
                self.log(
                    "worktree_repo_unknown",
                    json!({"agentId": agent_id, "repo": tree.repo, "worktreePath": tree.path}),
                );
                failed = true;
                continue;
            };
            let root = path_text(&repo.root);
            if keep_all {
                if !info.keep_worktrees {
                    let _ = self.ledger.remove_agent_worktree(agent_id, &tree.repo);
                }
                all_removed = false;
                any_branch_kept = true;
                continue;
            }
            // A replace hands the tree to its successor; a row of another active agent at this path keeps it.
            match self.ledger.worktree_held_by_other(agent_id, &tree.path) {
                Ok(false) => {}
                Ok(true) => {
                    self.log(
                        "worktree_held_by_other",
                        json!({"agentId": agent_id, "repo": tree.repo, "worktreePath": tree.path}),
                    );
                    let _ = self.ledger.remove_agent_worktree(agent_id, &tree.repo);
                    all_removed = false;
                    any_branch_kept = true;
                    continue;
                }
                Err(error) => {
                    self.log(
                        "worktree_holder_unknown",
                        json!({"agentId": agent_id, "repo": tree.repo, "error": OpError::from(error).js_string()}),
                    );
                    failed = true;
                    continue;
                }
            }
            let present = self.git.worktree_present(&tree.path);
            let moved = self
                .git
                .branch_tip_in(&root, &tree.branch)
                .is_some_and(|tip| tip != tree.base_sha);
            if present && (moved || reported) {
                self.log(
                    "worktree_kept",
                    json!({"agentId": agent_id, "repo": tree.repo, "worktreePath": tree.path, "reason": if moved { "commits" } else { "reported" }}),
                );
                let _ = self.ledger.remove_agent_worktree(agent_id, &tree.repo);
                all_removed = false;
                any_branch_kept = true;
                continue;
            }
            if present {
                self.teardown_worktree(agent_id, &tree.path, &budget);
            }
            if let Err(stderr) = self.remove_repo_worktree(repo, tree) {
                self.log(
                    "worktree_kept",
                    json!({"agentId": agent_id, "repo": tree.repo, "worktreePath": tree.path, "stderr": stderr}),
                );
                failed = true;
                continue;
            }
            if self.git.branch_tip_in(&root, &tree.branch).is_some() {
                any_branch_kept = true;
            }
            let _ = self.ledger.remove_agent_worktree(agent_id, &tree.repo);
        }
        if failed {
            return Ok(ReleaseOutcome {
                pane_closed,
                worktree_removed: Some(false),
                branch_kept: Some(true),
            });
        }
        if !trees.is_empty() && all_removed && !info.keep_worktrees {
            if let Some(folder) = info.worktree_path.as_ref().or(info.folder.as_ref()) {
                let _ = std::fs::remove_dir(folder);
            }
        }
        if let Err(error) = self.ledger.clear_agent_pane(agent_id) {
            self.log(
                "pane_row_not_cleared",
                json!({"agentId": agent_id, "error": OpError::from(error).js_string()}),
            );
        }
        Ok(ReleaseOutcome {
            pane_closed,
            worktree_removed: if trees.is_empty() {
                None
            } else {
                Some(all_removed)
            },
            branch_kept: if trees.is_empty() {
                info.branch.as_ref().map(|_| true)
            } else {
                Some(any_branch_kept)
            },
        })
    }

    /// `cstan repo add`: one more worktree for a running workspace worker, on its branch, at the tip of the repo's
    /// main branch. The launcher turn is the only lock: a live row is checked before any git, so a second add waits
    /// and answers `already_added` without touching the disk. All or nothing: a failure removes what was made.
    pub fn repo_add(&self, agent_id: &str, repo_name: &str) -> OpResult<RepoAddResult> {
        self.run(|budget| self.repo_add_in_turn(agent_id, repo_name, budget))
    }

    fn repo_add_in_turn(
        &self,
        agent_id: &str,
        repo_name: &str,
        budget: &Budget,
    ) -> OpResult<RepoAddResult> {
        let ws = self.workspace().cloned().ok_or_else(|| {
            OpError::launcher("not_workspace", "the configuration has no [repos]")
        })?;
        let not_a_worker = || {
            OpError::launcher(
                "not_a_worker",
                format!("{agent_id} has no worktree to add to; only a running worker adds a repo"),
            )
        };
        let agent = self
            .agent_record(agent_id)?
            .filter(|agent| agent.active())
            .ok_or_else(not_a_worker)?;
        let live = self.ledger.agent_worktrees(agent_id)?;
        let Some(first) = live.first() else {
            return Err(not_a_worker());
        };
        let Some(config) = ws.repo(repo_name) else {
            return Err(OpError::launcher(
                "unknown_repo",
                format!("repo {repo_name} is not in [repos]"),
            ));
        };
        let task = self
            .ledger
            .agent_panes()?
            .into_iter()
            .find(|row| row.agent_id == agent_id)
            .and_then(|row| row.task_ref);
        if let Some((plan, package)) = task.as_deref().and_then(|task| task.split_once('/')) {
            let mut allowed = self.ledger.package_repos(plan, package)?;
            if allowed.is_empty() {
                allowed.push(ws.default_repo.clone());
            }
            if !allowed.iter().any(|name| name == repo_name) {
                return Err(OpError::launcher(
                    "not_in_package",
                    format!("repo {repo_name} is not in package {plan}/{package}"),
                ));
            }
        }
        if live.iter().any(|tree| tree.repo == repo_name) {
            return Err(OpError::launcher(
                "already_added",
                format!("{agent_id} already has a worktree in repo {repo_name}"),
            ));
        }
        let branch = first.branch.clone();
        let folder = Path::new(&first.path)
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| {
                OpError::launcher(
                    "repo_unusable",
                    "the change folder of the worker is unknown",
                )
            })?;
        let root = path_text(&config.root);
        let unusable =
            |why: String| OpError::launcher("repo_unusable", format!("repo {repo_name}: {why}"));
        if self.git.branch_tip_in(&root, &config.main_branch).is_none() {
            return Err(unusable(format!(
                "it has no branch {} to start from",
                config.main_branch
            )));
        }
        let taken = self
            .git
            .branch_taken_in(&root, &branch)
            .map_err(|error| unusable(error.message))?;
        if taken {
            return Err(OpError::launcher(
                "branch_taken",
                format!(
                    "branch {branch} is taken in repo {repo_name}; rename your branch in every repo (links rename without repos) or ask the PM"
                ),
            ));
        }
        let tree = self
            .add_repo_worktree(config, &folder, &branch, None)
            .map_err(|error| unusable(error.message()))?;
        let undo = |error: OpError| {
            let failed = self.rollback_added(std::slice::from_ref(&tree), &folder);
            match (failed.first(), error) {
                (Some((_, why)), OpError::Launcher(inner)) => OpError::launcher(
                    inner.code,
                    format!(
                        "{}; the worktree {} could not be removed: {why}",
                        inner.message, tree.path
                    ),
                ),
                (_, error) => error,
            }
        };
        if let Err(error) = self.run_repo_setup(config, agent_id, &tree, budget) {
            return Err(undo(OpError::launcher("setup_failed", error.message())));
        }
        if let Err(error) = self
            .ledger
            .record_agent_worktree(agent_id, agent.generation, &tree)
        {
            let error = OpError::from(error);
            let code = if error.message().contains("already_added") {
                "already_added"
            } else {
                "repo_unusable"
            };
            return Err(undo(OpError::launcher(code, error.message())));
        }
        self.log(
            "repo_added",
            json!({"agentId": agent_id, "repo": repo_name, "worktreePath": tree.path, "branch": branch}),
        );
        Ok(RepoAddResult {
            agent_id: agent_id.to_string(),
            repo: tree.repo,
            worktree_path: tree.path,
            branch: tree.branch,
            base_sha: tree.base_sha,
        })
    }

    /// `rename_branch_for_task` in workspace mode: `git branch -m` repo by repo in merge order, only where the branch
    /// has no commit after its base; a failure renames the earlier repos back and says which repo failed. The ledger
    /// rename runs once, after every git rename succeeded.
    pub(crate) fn rename_workspace_branch(
        &self,
        agent_id: &str,
        current: &str,
        wanted: &str,
        trees: &[RepoTree],
    ) -> OpResult<BranchRenameResult> {
        let kept = |why: &str| BranchRenameResult {
            branch: Some(current.to_string()),
            renamed: false,
            note: Some(format!("branch kept: {current} ({why})")),
        };
        let Some(ws) = self.workspace() else {
            return Ok(kept("the configuration has no [repos]"));
        };
        let mut ordered: Vec<(&RepoConfig, &RepoTree)> = Vec::new();
        for name in &ws.merge_order {
            if let Some(tree) = trees.iter().find(|tree| &tree.repo == name) {
                if let Some(repo) = ws.repo(name) {
                    ordered.push((repo, tree));
                }
            }
        }
        if ordered.len() != trees.len() {
            return Ok(kept("a repo of the worker is not in [repos]"));
        }
        for (repo, tree) in &ordered {
            match self.git.branch_tip_in(&path_text(&repo.root), current) {
                Some(tip) if tip == tree.base_sha => {}
                _ => return Ok(kept("it already has commits")),
            }
        }
        let configs: Vec<&RepoConfig> = ordered.iter().map(|(repo, _)| *repo).collect();
        let name = self.free_branch_in(wanted, agent_id, &configs)?;
        if !self.git.branch_name_valid(&name) {
            return Ok(kept("git does not accept the new name"));
        }
        let mut renamed: Vec<&RepoConfig> = Vec::new();
        for (repo, _) in &ordered {
            let moved = self
                .git
                .rename_branch_in(&path_text(&repo.root), current, &name);
            if !moved.removed {
                let mut why = format!(
                    "repo {}: {}",
                    repo.name,
                    if moved.stderr.is_empty() {
                        "git refused the rename"
                    } else {
                        &moved.stderr
                    }
                );
                let stuck = self.rename_back(&renamed, &name, current);
                if !stuck.is_empty() {
                    why.push_str(&format!("; could not rename back: {}", stuck.join("; ")));
                }
                return Ok(kept(&why));
            }
            renamed.push(repo);
        }
        if let Err(error) = self.ledger.rename_agent_branch(agent_id, current, &name) {
            let mut why = OpError::from(error).message();
            let stuck = self.rename_back(&renamed, &name, current);
            if !stuck.is_empty() {
                why.push_str(&format!("; could not rename back: {}", stuck.join("; ")));
            }
            return Ok(kept(&why));
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
    }

    /// Renames the branch back in the repos already renamed; the repos where that failed come back by name.
    fn rename_back(&self, renamed: &[&RepoConfig], from: &str, to: &str) -> Vec<String> {
        renamed
            .iter()
            .filter_map(|repo| {
                let back = self.git.rename_branch_in(&path_text(&repo.root), from, to);
                (!back.removed).then(|| format!("repo {} ({})", repo.name, back.stderr))
            })
            .collect()
    }
}
