//! `cstan release <agent-id> --discard`: ends an agent when it is still active, then removes its worktrees and deletes its
//! branches in every repo, commits that never landed included. Everything is checked first; a refusal changes nothing.
//!
//! A branch that held commits is deleted at the commit it pointed at, and the answer carries the `git branch <name> <sha>`
//! line that brings it back (the format of `cstan cleanup apply`).

use crate::api::{DiscardRepo, DiscardResult, ADOPT_BUDGET_MS};
use crate::kernel::LauncherKernel;
use crate::shared::*;
use capstan_kernel::cleanup::restore_command;
use serde_json::{json, Value};
use std::path::Path;

/// Branch names that are never deleted, whatever a repo's configuration says.
const ALWAYS_PROTECTED: &[&str] = &["main", "master", "develop"];

/// One repo's worktree and branch the discard removes.
struct Target {
    repo: Option<String>,
    root: String,
    /// The agent's worktree, when one is recorded or the branch is checked out in one of its own.
    worktree: Option<String>,
    branch: String,
    /// The branches of this repo that are never deleted.
    protected: Vec<String>,
}

fn same_path(left: &str, right: &str) -> bool {
    let canonical =
        |p: &str| std::fs::canonicalize(p).unwrap_or_else(|_| Path::new(p).to_path_buf());
    canonical(left) == canonical(right)
}

/// A worktree that is the agent's own by its folder: some component of its path is the agent id.
fn is_agents_path(path: &str, agent_id: &str) -> bool {
    Path::new(path)
        .components()
        .any(|c| c.as_os_str() == agent_id)
}

fn strings(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

impl LauncherKernel {
    /// `release --discard`.
    pub fn release_discard(&self, agent_id: &str) -> OpResult<DiscardResult> {
        self.run(|_| self.discard_in_turn(agent_id))
    }

    /// The repos and branches the ledger records for an agent, current or gone.
    fn discard_targets(&self, agent_id: &str, refs: &Value) -> OpResult<Vec<Target>> {
        let mut targets: Vec<Target> = Vec::new();
        if let Some(ws) = self.workspace() {
            let rows = refs
                .get("worktrees")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for row in rows {
                let (Some(repo), Some(branch)) = (text(&row, "repo"), text(&row, "branch")) else {
                    continue;
                };
                if targets
                    .iter()
                    .any(|t| t.repo.as_deref() == Some(&repo) && t.branch == branch)
                {
                    continue;
                }
                let Some(config) = ws.repo(&repo) else {
                    return Err(OpError::launcher(
                        "unknown_repo",
                        format!("repo {repo} of {agent_id} is not in [repos]; nothing was changed"),
                    ));
                };
                let root = config.root.to_string_lossy().to_string();
                let mut protected = vec![config.main_branch.clone()];
                protected.extend(ALWAYS_PROTECTED.iter().map(|b| b.to_string()));
                protected.extend(self.git.head_branch_in(&root));
                targets.push(Target {
                    repo: Some(repo),
                    root,
                    worktree: text(&row, "worktreePath"),
                    branch,
                    protected,
                });
            }
            return Ok(targets);
        }
        let mut protected: Vec<String> = ALWAYS_PROTECTED.iter().map(|b| b.to_string()).collect();
        protected.extend(self.git.head_branch_in(&self.root));
        let pane = refs.get("pane").filter(|p| !p.is_null());
        let mut branches: Vec<(String, Option<String>)> = Vec::new();
        if let Some(pane) = pane {
            if let Some(branch) = text(pane, "branch") {
                branches.push((branch, text(pane, "worktreePath")));
            }
        }
        for branch in strings(refs, "reportBranches") {
            if !branches.iter().any(|(b, _)| *b == branch) {
                branches.push((branch, None));
            }
        }
        for (branch, worktree) in branches {
            targets.push(Target {
                repo: None,
                root: self.root.clone(),
                worktree,
                branch,
                protected: protected.clone(),
            });
        }
        Ok(targets)
    }

    /// The refusals, as (code, text), for the targets as they stand; empty when the discard may go on. Sets each
    /// target's worktree when the branch is checked out in one that is the agent's own.
    fn discard_problems(
        &self,
        agent_id: &str,
        refs: &Value,
        targets: &mut [Target],
    ) -> OpResult<Vec<(&'static str, String)>> {
        let mut problems: Vec<(&'static str, String)> = Vec::new();
        let held = strings(refs, "integrations");
        let integration_branches = refs
            .get("integrationBranches")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for target in targets.iter_mut() {
            let name = match &target.repo {
                Some(repo) => format!("{} in repo {repo}", target.branch),
                None => target.branch.clone(),
            };
            if target.protected.contains(&target.branch) {
                problems.push((
                    "protected_branch",
                    format!("{name} is the main branch of its repo and is never deleted"),
                ));
                continue;
            }
            if let Some(id) = held.first() {
                problems.push((
                    "in_integration",
                    format!(
                        "{name} is part of the unsettled integration {id} (running, or merged and not confirmed)"
                    ),
                ));
            }
            for entry in &integration_branches {
                if text(entry, "branch").as_deref() == Some(target.branch.as_str()) {
                    problems.push((
                        "in_integration",
                        format!(
                            "{name} is the branch of the unsettled integration {}",
                            text(entry, "integrationId").unwrap_or_default()
                        ),
                    ));
                }
            }
            match self.git.worktree_by_branch_in(&target.root, &target.branch) {
                Err(error) => {
                    return Err(OpError::launcher(
                        "git_error",
                        format!("{name}: {}", error.message),
                    ))
                }
                Ok(Some(found)) => {
                    let recorded = target
                        .worktree
                        .as_deref()
                        .is_some_and(|w| same_path(w, &found));
                    if recorded || is_agents_path(&found, agent_id) {
                        target.worktree = Some(found);
                    } else {
                        problems.push((
                            "branch_in_use",
                            format!("{name} is checked out in {found}, which is not a worktree of {agent_id}"),
                        ));
                    }
                }
                Ok(None) => {}
            }
            if let Some(path) = &target.worktree {
                if self.git.worktree_present(path) && self.git.worktree_dirty_count(path) != Some(0)
                {
                    problems.push((
                        "worktree_dirty",
                        format!(
                            "{} worktree {path} has uncommitted changes",
                            target.repo.as_deref().unwrap_or("the")
                        ),
                    ));
                }
            }
        }
        Ok(problems)
    }

    fn discard_in_turn(&self, agent_id: &str) -> OpResult<DiscardResult> {
        self.adopt_now(&self.budget(ADOPT_BUDGET_MS))?;
        let Some(agent) = self.agent_record(agent_id)? else {
            return Err(OpError::launcher(
                "unknown_agent",
                format!("there is no agent {agent_id}"),
            ));
        };
        if agent.kind == "PM" {
            return Err(OpError::launcher(
                "kind_not_releasable",
                "the PM is restarted with cstan pm restart, not released",
            ));
        }
        let refs = self.ledger.discard_refs(agent_id)?;
        let mut targets = self.discard_targets(agent_id, &refs)?;
        let problems = self.discard_problems(agent_id, &refs, &mut targets)?;
        if let Some((code, _)) = problems.first() {
            let all: Vec<String> = problems.iter().map(|(_, text)| text.clone()).collect();
            return Err(OpError::launcher(
                *code,
                format!(
                    "{agent_id} was not discarded and nothing was changed: {}",
                    all.join("; ")
                ),
            ));
        }
        if targets.is_empty() && !agent.active() {
            return Err(OpError::launcher(
                "nothing_to_discard",
                format!("{agent_id} is not active and no worktree or branch is recorded for it"),
            ));
        }
        // The commit each branch points at now, so a branch the release itself removes is still answered with it.
        let before: Vec<Option<String>> = targets
            .iter()
            .map(|t| self.git.branch_tip_in(&t.root, &t.branch))
            .collect();
        let mut cancelled = Vec::new();
        let released = agent.active();
        if released {
            cancelled = self.release_in_turn(agent_id)?.cancelled_message_ids;
        }
        let mut repos = Vec::new();
        let mut entries = Vec::new();
        for (target, tip_before) in targets.iter().zip(before) {
            let mut note: Option<String> = None;
            let mut worktree_removed = target.worktree.is_none();
            if let Some(path) = &target.worktree {
                if self.git.worktree_present(path) {
                    let removal = self.git.worktree_remove_in(&target.root, path);
                    worktree_removed = removal.removed;
                    if !removal.removed {
                        note = Some(format!("the worktree was not removed: {}", removal.stderr));
                    }
                } else {
                    worktree_removed = true;
                }
                self.git.prune_worktrees_in(&target.root);
            }
            let mut sha = tip_before.clone();
            let mut branch_deleted = false;
            if worktree_removed {
                match self.git.branch_tip_in(&target.root, &target.branch) {
                    Some(tip) => {
                        branch_deleted =
                            self.git
                                .delete_branch_if_in(&target.root, &target.branch, &tip);
                        sha = Some(tip);
                        if !branch_deleted {
                            note = Some(
                                "the branch moved while it was being deleted and was kept".into(),
                            );
                        }
                    }
                    // The release removed it (it held no commits): the answer still names the commit.
                    None => branch_deleted = tip_before.is_some(),
                }
            }
            let restore = match (&sha, branch_deleted) {
                (Some(sha), true) => Some(restore_command(&target.branch, sha)),
                _ => None,
            };
            if restore.is_some() || worktree_removed {
                entries.push(json!({
                    "repo": target.repo,
                    "worktreePath": target.worktree,
                    "branch": target.branch,
                    "sha": sha,
                    "branchDeleted": branch_deleted,
                }));
            }
            repos.push(DiscardRepo {
                repo: target.repo.clone(),
                worktree_path: target.worktree.clone(),
                worktree_removed,
                branch: target.branch.clone(),
                sha,
                branch_deleted,
                restore,
                note,
            });
        }
        // What was done is ledgered like a release; a failure to write it never undoes the deletion, so the restore lines
        // are in the log as well as in the answer.
        self.log(
            "release_discard",
            json!({"agentId": agent_id, "released": released, "repos": entries}),
        );
        if let Err(error) = self
            .ledger
            .record_release_discard(agent_id, Value::Array(entries))
        {
            self.log(
                "release_discard_not_recorded",
                json!({"agentId": agent_id, "error": OpError::from(error).js_string()}),
            );
        }
        for target in &targets {
            if let Some(repo) = &target.repo {
                let _ = self.ledger.remove_agent_worktree(agent_id, repo);
            }
        }
        let _ = self.ledger.clear_agent_pane(agent_id);
        // The agent's change folder in workspace mode: removed only when empty.
        for target in &targets {
            if let Some(parent) = target
                .worktree
                .as_deref()
                .and_then(|w| Path::new(w).parent())
            {
                if parent.file_name().is_some_and(|n| n == agent_id) {
                    let _ = std::fs::remove_dir(parent);
                }
            }
        }
        Ok(DiscardResult {
            agent_id: agent_id.to_string(),
            released,
            repos,
            cancelled_message_ids: cancelled,
        })
    }
}
