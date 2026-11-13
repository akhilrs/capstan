//! Workspace projects (migration 0063): a project may span several repositories. This area keeps the worktrees an agent
//! holds per repo, the commits of a report per repo, the integration sets that gather one integration per repo and the
//! repo a cleanup ran in. It only reads and writes the ledger; git is run by the launcher and the daemon.
//!
//! '' is the one repo of a project without `[repos]`: the root. Every writer here is one `Kernel::mutate`, so a writer
//! that touches several rows (a copy on replace, a set move) is one transaction.

use crate::errors::{KernelError, KernelResult};
use crate::helpers::{execute, query_all, query_opt, safe_id};
use crate::kernel::Kernel;
use crate::plan_body::is_repo_name;
use crate::records::{MutationEvent, MutationOutput};
use crate::types::MutationContext;
use serde_json::{json, Value};

/// The capability of the controller's own bookkeeping, as the other ledger-side writers use.
const CONTROL: &str = "controller:reconcile";

/// The states of an integration set. The column has no CHECK: the values and moves live in `set_integration_state`.
pub const SET_STATES: [&str; 5] = ["running", "merged", "conflicted", "failed", "discarded"];

fn field<'a>(input: &'a Value, key: &str) -> &'a Value {
    input.get(key).unwrap_or(&Value::Null)
}

/// A repo name: '' (the root) or a workspace repo name.
fn repo_arg<'a>(input: &'a Value, key: &str) -> KernelResult<&'a str> {
    match field(input, key).as_str() {
        Some(name) if name.is_empty() || is_repo_name(name) => Ok(name),
        _ => Err(KernelError::type_error(format!(
            "{key} must be a repo name or the empty root repo"
        ))),
    }
}

fn generation_arg(input: &Value, key: &str) -> KernelResult<i64> {
    match field(input, key).as_i64() {
        Some(n) if n >= 1 => Ok(n),
        _ => Err(KernelError::type_error(format!(
            "{key} must be an integer of at least 1"
        ))),
    }
}

fn sha_arg<'a>(input: &'a Value, key: &str) -> KernelResult<&'a str> {
    match field(input, key).as_str() {
        Some(sha)
            if sha.len() == 40 && sha.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) =>
        {
            Ok(sha)
        }
        _ => Err(KernelError::type_error(format!(
            "{key} must be 40 lowercase hex characters"
        ))),
    }
}

fn text_arg<'a>(input: &'a Value, key: &str, max: usize) -> KernelResult<&'a str> {
    match field(input, key).as_str() {
        Some(text)
            if !text.trim().is_empty()
                && text.len() <= max
                && !text.chars().any(char::is_control) =>
        {
            Ok(text)
        }
        _ => Err(KernelError::type_error(format!(
            "{key} must be one line of text of 1-{max} bytes"
        ))),
    }
}

fn current_generation(kernel: &Kernel, agent_id: &str) -> KernelResult<i64> {
    match kernel.agent_row(agent_id)? {
        Some(agent) => Ok(agent.generation),
        None => Err(KernelError::controller(format!(
            "agent {agent_id} does not exist"
        ))),
    }
}

// ------------------------------------------------------------------------------------------------ agent worktrees

fn worktree_value(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    Ok(json!({
        "agentId": row.get::<_, String>("agent_id")?,
        "generation": row.get::<_, i64>("generation")?,
        "repo": row.get::<_, String>("repo")?,
        "worktreePath": row.get::<_, String>("worktree_path")?,
        "branch": row.get::<_, String>("branch")?,
        "baseSha": row.get::<_, String>("base_sha")?,
        "createdAt": row.get::<_, String>("created_at")?,
        "removedAt": row.get::<_, Option<String>>("removed_at")?,
    }))
}

/// The live worktree of an agent generation in one repo.
pub(crate) fn live_worktree(
    kernel: &Kernel,
    agent_id: &str,
    generation: i64,
    repo: &str,
) -> KernelResult<Option<Value>> {
    query_opt(
        &kernel.database,
        "SELECT * FROM agent_worktrees WHERE project_id = ? AND agent_id = ? AND generation = ? AND repo = ? AND removed_at IS NULL",
        rusqlite::params![kernel.project_id, agent_id, generation, repo],
        worktree_value,
    )
}

/// `recordAgentWorktree`: the worktree an agent holds in a repo. A removed row comes back to life (a repo added again);
/// a live row refuses with `already_added`.
pub fn record_agent_worktree(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let agent_id = safe_id(field(input, "agentId"), "agent id")?;
    let generation = generation_arg(input, "generation")?;
    let repo = repo_arg(input, "repo")?;
    let path = text_arg(input, "worktreePath", 4096)?;
    let branch = text_arg(input, "branch", 255)?;
    let base_sha = sha_arg(input, "baseSha")?;
    kernel.mutate(context, "workspace.worktree_record", CONTROL, input, |_| {
        if current_generation(kernel, agent_id)? != generation {
            return Err(KernelError::controller(format!(
                "generation {generation} is not the current generation of {agent_id}"
            )));
        }
        let existing: Option<Option<String>> = query_opt(
            &kernel.database,
            "SELECT removed_at FROM agent_worktrees WHERE project_id = ? AND agent_id = ? AND generation = ? AND repo = ?",
            rusqlite::params![kernel.project_id, agent_id, generation, repo],
            |row| row.get::<_, Option<String>>(0),
        )?;
        let now = kernel.now();
        let revived = match existing {
            Some(None) => {
                return Err(KernelError::controller(format!(
                    "already_added: {agent_id} already has a worktree in repo {repo:?}"
                )))
            }
            Some(Some(_)) => {
                execute(
                    &kernel.database,
                    "UPDATE agent_worktrees SET worktree_path = ?, branch = ?, base_sha = ?, created_at = ?, removed_at = NULL
                     WHERE project_id = ? AND agent_id = ? AND generation = ? AND repo = ?",
                    rusqlite::params![path, branch, base_sha, now, kernel.project_id, agent_id, generation, repo],
                )?;
                true
            }
            None => {
                execute(
                    &kernel.database,
                    "INSERT INTO agent_worktrees(project_id, agent_id, generation, repo, worktree_path, branch, base_sha, created_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    rusqlite::params![kernel.project_id, agent_id, generation, repo, path, branch, base_sha, now],
                )?;
                false
            }
        };
        let event = MutationEvent::new("agent_worktree", agent_id, generation)
            .with_details(json!({"repo": repo, "branch": branch, "revived": revived}));
        Ok(MutationOutput::new(json!({"recorded": true, "revived": revived}), event))
    })
}

/// `removeAgentWorktree`: marks the live worktree of the current generation in a repo as removed.
pub fn remove_agent_worktree(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let agent_id = safe_id(field(input, "agentId"), "agent id")?;
    let repo = repo_arg(input, "repo")?;
    kernel.mutate(context, "workspace.worktree_remove", CONTROL, input, |_| {
        let generation = current_generation(kernel, agent_id)?;
        let changed = execute(
            &kernel.database,
            "UPDATE agent_worktrees SET removed_at = ? WHERE project_id = ? AND agent_id = ? AND generation = ? AND repo = ? AND removed_at IS NULL",
            rusqlite::params![kernel.now(), kernel.project_id, agent_id, generation, repo],
        )?;
        if changed == 0 {
            return Err(KernelError::controller(format!(
                "{agent_id} has no live worktree in repo {repo:?}"
            )));
        }
        let event = MutationEvent::new("agent_worktree", agent_id, generation)
            .with_details(json!({"repo": repo, "removed": true}));
        Ok(MutationOutput::new(json!({"removed": true}), event))
    })
}

/// `removeAgentWorktrees`: marks every live worktree of the current generation as removed, in one transaction. It
/// is the end of a replace: the successor's rows exist, and the predecessor's go together or not at all.
pub fn remove_agent_worktrees(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let agent_id = safe_id(field(input, "agentId"), "agent id")?;
    kernel.mutate(context, "workspace.worktrees_remove", CONTROL, input, |_| {
        let generation = current_generation(kernel, agent_id)?;
        let changed = execute(
            &kernel.database,
            "UPDATE agent_worktrees SET removed_at = ? WHERE project_id = ? AND agent_id = ? AND generation = ? AND removed_at IS NULL",
            rusqlite::params![kernel.now(), kernel.project_id, agent_id, generation],
        )?;
        let event = MutationEvent::new("agent_worktree", agent_id, generation)
            .with_details(json!({"removedAll": changed}));
        Ok(MutationOutput::new(json!({"removed": changed}), event))
    })
}

/// `listAgentWorktrees`: the live worktrees of an agent's current generation, by repo.
pub fn list_agent_worktrees(kernel: &Kernel, agent_id: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    let Some(agent) = kernel.agent_row(agent_id)? else {
        return Ok(json!([]));
    };
    let rows = query_all(
        &kernel.database,
        "SELECT * FROM agent_worktrees WHERE project_id = ? AND agent_id = ? AND generation = ? AND removed_at IS NULL ORDER BY repo",
        rusqlite::params![kernel.project_id, agent_id, agent.generation],
        worktree_value,
    )?;
    Ok(Value::Array(rows))
}

/// `copyAgentWorktrees`: on a replace the live rows of the old generation become rows of the new one, in one transaction.
/// A repo that already has a row in the new generation is left as it is.
pub fn copy_agent_worktrees(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let agent_id = safe_id(field(input, "agentId"), "agent id")?;
    let from = generation_arg(input, "fromGeneration")?;
    let to = generation_arg(input, "toGeneration")?;
    if to <= from {
        return Err(KernelError::type_error(
            "toGeneration must be greater than fromGeneration",
        ));
    }
    kernel.mutate(context, "workspace.worktrees_copy", CONTROL, input, |_| {
        if current_generation(kernel, agent_id)? != to {
            return Err(KernelError::controller(format!(
                "generation {to} is not the current generation of {agent_id}"
            )));
        }
        let copied = execute(
            &kernel.database,
            "INSERT INTO agent_worktrees(project_id, agent_id, generation, repo, worktree_path, branch, base_sha, created_at)
             SELECT project_id, agent_id, ?, repo, worktree_path, branch, base_sha, created_at FROM agent_worktrees o
             WHERE o.project_id = ? AND o.agent_id = ? AND o.generation = ? AND o.removed_at IS NULL
               AND NOT EXISTS (SELECT 1 FROM agent_worktrees n WHERE n.project_id = o.project_id AND n.agent_id = o.agent_id AND n.generation = ? AND n.repo = o.repo)",
            rusqlite::params![to, kernel.project_id, agent_id, from, to],
        )?;
        let event = MutationEvent::new("agent_worktree", agent_id, to)
            .with_details(json!({"from": from, "to": to, "copied": copied}));
        Ok(MutationOutput::new(json!({"copied": copied}), event))
    })
}

// ------------------------------------------------------------------------------------------------- report commits

/// One commit of a report, as the `commits` input of `recordAgentReport` states it. The commits must name listed
/// repos; the root '' is never a listed repo in workspace mode.
#[derive(Clone, Debug)]
pub(crate) struct ReportCommit {
    pub repo: String,
    pub commit_sha: String,
    pub branch: String,
    pub base_sha: String,
}

/// Reads the optional `commits` of a report: `None` when absent; else 1-32 entries sorted by repo, unique, the first
/// with the report's own commit.
pub(crate) fn parse_report_commits(
    input: &Value,
    commit_sha: &str,
) -> KernelResult<Option<Vec<ReportCommit>>> {
    let Some(value) = input.get("commits") else {
        return Ok(None);
    };
    let bad = |why: &str| KernelError::type_error(format!("commits {why}"));
    let items = value
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= 32)
        .ok_or_else(|| bad("must be a list of 1-32 commits"))?;
    let mut commits: Vec<ReportCommit> = Vec::new();
    for item in items {
        let repo = match item.get("repo").and_then(Value::as_str) {
            Some(name) if is_repo_name(name) => name,
            _ => return Err(bad("entries need a repo name")),
        };
        let sha = sha_arg(item, "commitSha").map_err(|_| bad("entries need a 40 hex commitSha"))?;
        let base = sha_arg(item, "baseSha").map_err(|_| bad("entries need a 40 hex baseSha"))?;
        let branch = text_arg(item, "branch", 255).map_err(|_| bad("entries need a branch"))?;
        commits.push(ReportCommit {
            repo: repo.to_string(),
            commit_sha: sha.to_string(),
            branch: branch.to_string(),
            base_sha: base.to_string(),
        });
    }
    for pair in commits.windows(2) {
        if pair[0].repo == pair[1].repo {
            return Err(bad("name a repo more than once"));
        }
        if pair[0].repo > pair[1].repo {
            return Err(bad("must be sorted by repo"));
        }
    }
    if commits[0].commit_sha != commit_sha {
        return Err(bad("must start with the report's own commit"));
    }
    Ok(Some(commits))
}

/// Writes the rows of `commits` for a report (inside the report's transaction).
pub(crate) fn insert_report_commits(
    kernel: &Kernel,
    report_id: &str,
    commits: &[ReportCommit],
) -> KernelResult<()> {
    for commit in commits {
        execute(
            &kernel.database,
            "INSERT INTO report_commits(project_id, report_id, repo, commit_sha, branch, base_sha) VALUES (?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                kernel.project_id,
                report_id,
                commit.repo,
                commit.commit_sha,
                commit.branch,
                commit.base_sha
            ],
        )?;
    }
    Ok(())
}

/// Whether the project has any workspace row (a report commit or an integration set). A single-repo project has none, so
/// callers skip the per-repo lookups. One existence probe.
pub fn has_workspace_rows(kernel: &Kernel) -> KernelResult<bool> {
    kernel.assert_open()?;
    let found = query_all(
        &kernel.database,
        "SELECT 1 WHERE EXISTS (SELECT 1 FROM report_commits WHERE project_id = ?1) OR EXISTS (SELECT 1 FROM integration_sets WHERE project_id = ?1)",
        [&kernel.project_id],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(!found.is_empty())
}

/// `reportCommits`: the commits a report named, by repo; empty for a report without any.
pub fn report_commits(kernel: &Kernel, report_id: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    let rows = query_all(
        &kernel.database,
        "SELECT repo, commit_sha, branch, base_sha FROM report_commits WHERE project_id = ? AND report_id = ? ORDER BY repo",
        [&kernel.project_id, report_id],
        |row| {
            Ok(json!({
                "repo": row.get::<_, String>(0)?,
                "commitSha": row.get::<_, String>(1)?,
                "branch": row.get::<_, String>(2)?,
                "baseSha": row.get::<_, String>(3)?,
            }))
        },
    )?;
    Ok(Value::Array(rows))
}

// -------------------------------------------------------------------------------------------- integration sets

fn set_error(set_id: &str, what: impl std::fmt::Display) -> KernelError {
    KernelError::controller(format!("integration set {set_id}: {what}"))
}

struct SetRow {
    sequence: i64,
    branch: String,
    state: String,
    reason: Option<String>,
    reset_at: Option<String>,
    created_by: String,
    created_at: String,
}

fn set_row(kernel: &Kernel, set_id: &str) -> KernelResult<Option<SetRow>> {
    query_opt(
        &kernel.database,
        "SELECT sequence, branch, state, reason, reset_at, created_by, created_at FROM integration_sets WHERE project_id = ? AND set_id = ?",
        [&kernel.project_id, set_id],
        |row| {
            Ok(SetRow {
                sequence: row.get(0)?,
                branch: row.get(1)?,
                state: row.get(2)?,
                reason: row.get(3)?,
                reset_at: row.get(4)?,
                created_by: row.get(5)?,
                created_at: row.get(6)?,
            })
        },
    )
}

fn require_set(kernel: &Kernel, set_id: &str) -> KernelResult<SetRow> {
    set_row(kernel, set_id)?.ok_or_else(|| set_error(set_id, "does not exist"))
}

/// A member of a set: repo, position, integration (None until attached or after a detach) and attempt.
struct Member {
    repo: String,
    position: i64,
    integration_id: Option<String>,
    attempt: i64,
}

fn members_of(kernel: &Kernel, set_id: &str) -> KernelResult<Vec<Member>> {
    query_all(
        &kernel.database,
        "SELECT repo, position, integration_id, attempt FROM integration_set_members WHERE project_id = ? AND set_id = ? ORDER BY position",
        [&kernel.project_id, set_id],
        |row| {
            Ok(Member {
                repo: row.get(0)?,
                position: row.get(1)?,
                integration_id: row.get(2)?,
                attempt: row.get(3)?,
            })
        },
    )
}

fn set_value(kernel: &Kernel, set_id: &str, row: &SetRow) -> KernelResult<Value> {
    let members: Vec<Value> = members_of(kernel, set_id)?
        .into_iter()
        .map(|m| {
            json!({"repo": m.repo, "position": m.position, "integrationId": m.integration_id, "attempt": m.attempt})
        })
        .collect();
    Ok(json!({
        "setId": set_id,
        "sequence": row.sequence,
        "branch": row.branch,
        "state": row.state,
        "reason": row.reason,
        "resetAt": row.reset_at,
        "createdBy": row.created_by,
        "createdAt": row.created_at,
        "members": members,
    }))
}

/// `integrationSet`: one set with its members; null when there is none.
pub fn integration_set(kernel: &Kernel, set_id: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    match set_row(kernel, set_id)? {
        Some(row) => set_value(kernel, set_id, &row),
        None => Ok(Value::Null),
    }
}

/// `setOfIntegration`: the set an integration is a member of.
pub fn set_of_integration(kernel: &Kernel, integration_id: &str) -> KernelResult<Option<String>> {
    kernel.assert_open()?;
    query_opt(
        &kernel.database,
        "SELECT set_id FROM integration_set_members WHERE project_id = ? AND integration_id = ?",
        [&kernel.project_id, integration_id],
        |row| row.get::<_, String>(0),
    )
}

/// `repoOfIntegration`: the repo of an integration that belongs to a set; `None` for any other (the root repo).
pub fn repo_of_integration(kernel: &Kernel, integration_id: &str) -> KernelResult<Option<String>> {
    kernel.assert_open()?;
    query_opt(
        &kernel.database,
        "SELECT repo FROM integration_set_members WHERE project_id = ? AND integration_id = ?",
        [&kernel.project_id, integration_id],
        |row| row.get::<_, String>(0),
    )
}

/// `createIntegrationSet`: a running set of `repos` (in the order given, which is the merge order), no integrations yet.
pub fn create_integration_set(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let branch = text_arg(input, "branch", 255)?;
    let repos: Vec<&str> = match field(input, "repos").as_array() {
        Some(items) if !items.is_empty() && items.len() <= 32 => items
            .iter()
            .map(|v| match v.as_str() {
                Some(name) if is_repo_name(name) => Ok(name),
                _ => Err(KernelError::type_error("repos must be repo names")),
            })
            .collect::<KernelResult<Vec<_>>>()?,
        _ => {
            return Err(KernelError::type_error(
                "repos must be a list of 1-32 repo names",
            ))
        }
    };
    for (i, repo) in repos.iter().enumerate() {
        if repos[..i].contains(repo) {
            return Err(KernelError::type_error(format!(
                "repos lists {repo} more than once"
            )));
        }
    }
    kernel.mutate(context, "workspace.set_create", CONTROL, input, |actor| {
        let set_id = kernel.env.uuid();
        let sequence = query_opt(
            &kernel.database,
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM integration_sets WHERE project_id = ?",
            [&kernel.project_id],
            |row| row.get::<_, i64>(0),
        )?
        .unwrap_or(1);
        execute(
            &kernel.database,
            "INSERT INTO integration_sets(project_id, set_id, sequence, branch, state, created_by, created_at) VALUES (?, ?, ?, ?, 'running', ?, ?)",
            rusqlite::params![kernel.project_id, set_id, sequence, branch, actor.actor_id, kernel.now()],
        )?;
        for (position, repo) in repos.iter().enumerate() {
            execute(
                &kernel.database,
                "INSERT INTO integration_set_members(project_id, set_id, repo, position, attempt) VALUES (?, ?, ?, ?, 0)",
                rusqlite::params![kernel.project_id, set_id, repo, position as i64],
            )?;
        }
        let row = require_set(kernel, &set_id)?;
        let mut event = MutationEvent::new("integration_set", &set_id, 0)
            .with_details(json!({"branch": branch, "repos": repos}));
        event.to_state = Some("running".into());
        Ok(MutationOutput::new(set_value(kernel, &set_id, &row)?, event))
    })
}

/// The moves a set state may make (a move to `discarded` also needs every member discarded or detached).
///
/// - `running` -> `merged`/`conflicted`/`failed`: the set merge and the set gate settle a running set.
/// - `merged` -> `failed`: a verify or review failure after the merge; a merged set never becomes `conflicted`.
/// - `failed` -> `merged`/`conflicted`/`failed`: a redo, or a new reason, after a failure.
/// - `conflicted` -> `merged`/`failed`/`conflicted`: redo and `integrate discard --repo` recompute the
///   outcome once a conflicted member is redone or dropped.
/// - any open state -> `discarded`: `integrate discard` of the whole set.
fn move_allowed(from: &str, to: &str) -> bool {
    matches!(
        (from, to),
        ("running", "merged" | "conflicted" | "failed")
            | ("merged", "failed")
            | ("failed", "merged" | "conflicted" | "failed")
            | ("conflicted", "merged" | "failed" | "conflicted")
            | ("running" | "merged" | "conflicted" | "failed", "discarded")
    )
}

/// `setIntegrationState`: moves a set to `state` (with a `reason` for `failed` and `conflicted`).
pub fn set_integration_state(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let set_id = safe_id(field(input, "setId"), "set id")?;
    let state = field(input, "state")
        .as_str()
        .ok_or_else(|| KernelError::type_error("state must be text"))?;
    let reason = match field(input, "reason") {
        Value::Null => None,
        _ => Some(text_arg(input, "reason", 500)?),
    };
    kernel.mutate(context, "workspace.set_state", CONTROL, input, |_| {
        let row = require_set(kernel, set_id)?;
        if !SET_STATES.contains(&state) {
            return Err(set_error(set_id, format!("unknown state {state:?}")));
        }
        if matches!(state, "failed" | "conflicted") && reason.is_none() {
            return Err(set_error(
                set_id,
                format!("needs a reason to move to {state}"),
            ));
        }
        if !move_allowed(&row.state, state) {
            return Err(set_error(
                set_id,
                format!("cannot move from {} to {state}", row.state),
            ));
        }
        if state == "discarded" {
            for member in members_of(kernel, set_id)? {
                let Some(integration_id) = member.integration_id else {
                    continue;
                };
                let discarded = query_opt(
                    &kernel.database,
                    "SELECT state FROM integrations WHERE project_id = ? AND integration_id = ?",
                    [&kernel.project_id, &integration_id],
                    |r| r.get::<_, String>(0),
                )?
                .is_some_and(|s| s == "discarded");
                if !discarded {
                    return Err(set_error(
                        set_id,
                        format!(
                            "cannot be discarded while the integration of {} is not",
                            member.repo
                        ),
                    ));
                }
            }
        }
        execute(
            &kernel.database,
            "UPDATE integration_sets SET state = ?, reason = ? WHERE project_id = ? AND set_id = ?",
            rusqlite::params![state, reason, kernel.project_id, set_id],
        )?;
        let event = MutationEvent::new("integration_set", set_id, 0)
            .transition(&row.state, state)
            .with_details(json!({"reason": reason}));
        Ok(MutationOutput::new(
            json!({"setId": set_id, "state": state}),
            event,
        ))
    })
}

/// `attachSetMember`: the integration created for a repo of a set; the member's attempt grows by one.
pub fn attach_set_member(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let set_id = safe_id(field(input, "setId"), "set id")?;
    let repo = repo_arg(input, "repo")?;
    let integration_id = safe_id(field(input, "integrationId"), "integration id")?;
    kernel.mutate(context, "workspace.set_attach", CONTROL, input, |_| {
        require_set(kernel, set_id)?;
        let member = members_of(kernel, set_id)?
            .into_iter()
            .find(|m| m.repo == repo)
            .ok_or_else(|| set_error(set_id, format!("has no member for repo {repo:?}")))?;
        if let Some(live) = member.integration_id {
            return Err(set_error(
                set_id,
                format!("member {repo:?} already has integration {live}; detach it first"),
            ));
        }
        if crate::areas::integrations::integration_row(kernel, integration_id)?.is_none() {
            return Err(set_error(set_id, format!("integration {integration_id} does not exist")));
        }
        if let Some(other) = set_of_integration(kernel, integration_id)? {
            return Err(set_error(
                set_id,
                format!("integration {integration_id} already belongs to set {other}"),
            ));
        }
        execute(
            &kernel.database,
            "UPDATE integration_set_members SET integration_id = ?, attempt = attempt + 1 WHERE project_id = ? AND set_id = ? AND repo = ?",
            rusqlite::params![integration_id, kernel.project_id, set_id, repo],
        )?;
        let event = MutationEvent::new("integration_set", set_id, 0)
            .with_details(json!({"repo": repo, "integrationId": integration_id, "attempt": member.attempt + 1}));
        Ok(MutationOutput::new(json!({"attempt": member.attempt + 1}), event))
    })
}

/// `detachSetMember`: the member's integration is let go (it stays an integration; the member waits for a new one).
pub fn detach_set_member(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let set_id = safe_id(field(input, "setId"), "set id")?;
    let repo = repo_arg(input, "repo")?;
    kernel.mutate(context, "workspace.set_detach", CONTROL, input, |_| {
        require_set(kernel, set_id)?;
        let member = members_of(kernel, set_id)?
            .into_iter()
            .find(|m| m.repo == repo)
            .ok_or_else(|| set_error(set_id, format!("has no member for repo {repo:?}")))?;
        let Some(detached) = member.integration_id else {
            return Err(set_error(set_id, format!("member {repo:?} has no integration to detach")));
        };
        execute(
            &kernel.database,
            "UPDATE integration_set_members SET integration_id = NULL WHERE project_id = ? AND set_id = ? AND repo = ?",
            rusqlite::params![kernel.project_id, set_id, repo],
        )?;
        let event = MutationEvent::new("integration_set", set_id, 0)
            .with_details(json!({"repo": repo, "detached": detached}));
        Ok(MutationOutput::new(json!({"detached": detached}), event))
    })
}

/// `removeSetMember`: an unconfirmed member leaves its set (its repo already has the change on main). Its row is deleted, the
/// other members keep their positions, and the set's runs and reviews no longer count (`reset_set`). Refuses a confirmed
/// or unknown member.
pub fn remove_set_member(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let set_id = safe_id(field(input, "setId"), "set id")?;
    let repo = repo_arg(input, "repo")?;
    kernel.mutate(
        context,
        "workspace.set_member_remove",
        CONTROL,
        input,
        |_| {
            require_set(kernel, set_id)?;
            let member = members_of(kernel, set_id)?
                .into_iter()
                .find(|m| m.repo == repo)
                .ok_or_else(|| set_error(set_id, format!("has no member for repo {repo:?}")))?;
            if let Some(integration_id) = &member.integration_id {
                let confirmed = query_opt(
                    &kernel.database,
                    "SELECT state FROM integrations WHERE project_id = ? AND integration_id = ?",
                    [&kernel.project_id, integration_id],
                    |r| r.get::<_, String>(0),
                )?
                .is_some_and(|s| s == "confirmed");
                if confirmed {
                    return Err(set_error(
                        set_id,
                        format!("member {repo:?} is confirmed and cannot be removed"),
                    ));
                }
            }
            execute(
            &kernel.database,
            "DELETE FROM integration_set_members WHERE project_id = ? AND set_id = ? AND repo = ?",
            rusqlite::params![kernel.project_id, set_id, repo],
        )?;
            let now = kernel.now();
            execute(
                &kernel.database,
                "UPDATE integration_sets SET reset_at = ? WHERE project_id = ? AND set_id = ?",
                [&now, &kernel.project_id, set_id],
            )?;
            let event = MutationEvent::new("integration_set", set_id, 0).with_details(
                json!({"repo": repo, "removed": member.integration_id, "resetAt": now}),
            );
            Ok(MutationOutput::new(
                json!({"setId": set_id, "repo": repo, "resetAt": now}),
                event,
            ))
        },
    )
}

/// `resetSet`: a member was redone or discarded; runs and reviews before now no longer count for the set's gates.
pub fn reset_set(kernel: &Kernel, context: &MutationContext, input: &Value) -> KernelResult<Value> {
    let set_id = safe_id(field(input, "setId"), "set id")?;
    kernel.mutate(context, "workspace.set_reset", CONTROL, input, |_| {
        require_set(kernel, set_id)?;
        let now = kernel.now();
        execute(
            &kernel.database,
            "UPDATE integration_sets SET reset_at = ? WHERE project_id = ? AND set_id = ?",
            [&now, &kernel.project_id, set_id],
        )?;
        let event =
            MutationEvent::new("integration_set", set_id, 0).with_details(json!({"resetAt": now}));
        Ok(MutationOutput::new(json!({"resetAt": now}), event))
    })
}

/// `setComplete`: whether every member has a merged (or confirmed) integration. Refuses an unknown set.
pub fn set_complete(kernel: &Kernel, set_id: &str) -> KernelResult<bool> {
    kernel.assert_open()?;
    require_set(kernel, set_id)?;
    let members = members_of(kernel, set_id)?;
    if members.is_empty() {
        return Ok(false);
    }
    for member in members {
        let Some(integration_id) = member.integration_id else {
            return Ok(false);
        };
        let done = query_opt(
            &kernel.database,
            "SELECT state FROM integrations WHERE project_id = ? AND integration_id = ?",
            [&kernel.project_id, &integration_id],
            |r| r.get::<_, String>(0),
        )?
        .is_some_and(|s| s == "merged" || s == "confirmed");
        if !done {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `setConfirmed`: whether every live member of a set is confirmed. A member whose integration was discarded (and not
/// redone) is dropped and skipped; a member with no integration yet (a row only) blocks. A set with no live member is not
/// confirmed. Refuses an unknown set.
pub fn set_confirmed(kernel: &Kernel, set_id: &str) -> KernelResult<bool> {
    kernel.assert_open()?;
    require_set(kernel, set_id)?;
    let mut live = 0;
    for member in members_of(kernel, set_id)? {
        let Some(integration_id) = member.integration_id else {
            return Ok(false);
        };
        let state = query_opt(
            &kernel.database,
            "SELECT state FROM integrations WHERE project_id = ? AND integration_id = ?",
            [&kernel.project_id, &integration_id],
            |r| r.get::<_, String>(0),
        )?;
        match state.as_deref() {
            Some("discarded") => {}
            Some("confirmed") => live += 1,
            _ => return Ok(false),
        }
    }
    Ok(live > 0)
}

/// `setRunning`: the ids of the sets in state `running`, oldest first.
pub fn set_running(kernel: &Kernel) -> KernelResult<Vec<String>> {
    kernel.assert_open()?;
    query_all(
        &kernel.database,
        "SELECT set_id FROM integration_sets WHERE project_id = ? AND state = 'running' ORDER BY sequence",
        [&kernel.project_id],
        |row| row.get::<_, String>(0),
    )
}

/// One member of a set as the gates read it: its repo, integration, head commit and whether it is confirmed (settled with
/// the member flag: its code is on main).
pub(crate) struct MemberView {
    pub repo: String,
    pub integration_id: Option<String>,
    pub head: Option<String>,
    pub confirmed: bool,
}

/// `member_views`: the members of a set in position order, without a dropped one (its integration was discarded and not
/// redone: no runs count for it and it is never the review's subject). Refuses an unknown set.
pub(crate) fn member_views(kernel: &Kernel, set_id: &str) -> KernelResult<Vec<MemberView>> {
    require_set(kernel, set_id)?;
    let mut views = Vec::new();
    for member in members_of(kernel, set_id)? {
        let row = match &member.integration_id {
            Some(id) => crate::areas::integrations::integration_row(kernel, id)?,
            None => None,
        };
        if row.as_ref().is_some_and(|i| i.state == "discarded") {
            continue;
        }
        views.push(MemberView {
            repo: member.repo,
            integration_id: member.integration_id,
            confirmed: row.as_ref().is_some_and(|i| i.state == "confirmed"),
            head: row.and_then(|i| i.head_sha),
        });
    }
    Ok(views)
}

/// The instant a set's gates count from ('' before any reset).
pub(crate) fn set_reset_at(kernel: &Kernel, set_id: &str) -> KernelResult<String> {
    Ok(require_set(kernel, set_id)?.reset_at.unwrap_or_default())
}

/// `setGate`: the verify gate of every member of a set under `commands_sha`, and the latest review of the set's first
/// member. Runs queued and reviews started before the set's `reset_at` are ignored, except that a confirmed member counts
/// its newest passed run whatever its time (its code is on main). A member run is one of its own integration, repo and
/// commit. An empty `commands_sha` accepts a run under any commands.
pub fn set_gate(kernel: &Kernel, set_id: &str, commands_sha: &str) -> KernelResult<Value> {
    set_gate_by(kernel, set_id, &|_| commands_sha.to_string())
}

/// `setGate` with each repo's own commands sha (`sha_of` gives it; '' accepts any commands).
pub fn set_gate_by(
    kernel: &Kernel,
    set_id: &str,
    sha_of: &dyn Fn(&str) -> String,
) -> KernelResult<Value> {
    kernel.assert_open()?;
    let row = require_set(kernel, set_id)?;
    let since = row.reset_at.clone().unwrap_or_default();
    let mut members = Vec::new();
    let mut all_passed = true;
    let mut first_subject: Option<String> = None;
    for member in member_views(kernel, set_id)? {
        if first_subject.is_none() {
            first_subject = member.integration_id.clone();
        }
        let sha = sha_of(&member.repo);
        let (code, run_id) = match (&member.integration_id, &member.head) {
            (Some(id), Some(commit)) => {
                let runs = query_all(
                    &kernel.database,
                    "SELECT run_id, state, queued_at FROM verify_runs WHERE project_id = ? AND subject_integration_id = ? AND repo = ? AND commit_sha = ? AND (? = '' OR commands_sha = ?) ORDER BY sequence DESC",
                    rusqlite::params![kernel.project_id, id, member.repo, commit, sha, sha],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)),
                )?;
                let counted: Vec<&(String, String, String)> = runs
                    .iter()
                    .filter(|(_, state, queued)| {
                        queued.as_str() >= since.as_str() || (member.confirmed && state == "passed")
                    })
                    .collect();
                if let Some((run, _, _)) = counted.iter().find(|(_, s, _)| s == "passed") {
                    ("passed", Some(run.clone()))
                } else if let Some((run, _, _)) = counted
                    .iter()
                    .find(|(_, s, _)| s == "queued" || s == "running")
                {
                    ("verify_running", Some(run.clone()))
                } else if let Some((run, _, _)) = counted.first() {
                    ("verify_failed", Some(run.clone()))
                } else {
                    ("verify_missing", None)
                }
            }
            _ => ("verify_missing", None),
        };
        all_passed &= code == "passed";
        members.push(json!({
            "repo": member.repo,
            "integrationId": member.integration_id,
            "commitSha": member.head,
            "confirmed": member.confirmed,
            "code": code,
            "runId": run_id,
        }));
    }
    let review = match &first_subject {
        Some(subject) => query_opt(
            &kernel.database,
            "SELECT review_id, state FROM reviews WHERE project_id = ? AND subject_integration_id = ? AND created_at >= ? ORDER BY sequence DESC LIMIT 1",
            rusqlite::params![kernel.project_id, subject, since],
            |r| Ok(json!({"reviewId": r.get::<_, String>(0)?, "state": r.get::<_, String>(1)?})),
        )?,
        None => None,
    };
    Ok(json!({
        "setId": set_id,
        "state": row.state,
        "resetAt": row.reset_at,
        "ok": all_passed && !members.is_empty(),
        "members": members,
        "reviewSubjectId": first_subject,
        "review": review,
    }))
}

/// `setOfSubject`: the set a subject id names, whether it is the set's own id or a member's integration id.
pub fn set_of_subject(kernel: &Kernel, subject_id: &str) -> KernelResult<Option<String>> {
    kernel.assert_open()?;
    if set_row(kernel, subject_id)?.is_some() {
        return Ok(Some(subject_id.to_string()));
    }
    set_of_integration(kernel, subject_id)
}

// ------------------------------------------------------------------------------------------------- cleanup repos

/// `recordCleanupRepo`: the repo a cleanup ran in (once per cleanup).
pub fn record_cleanup_repo(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let cleanup_id = safe_id(field(input, "cleanupId"), "cleanup id")?;
    let repo = repo_arg(input, "repo")?;
    kernel.mutate(context, "workspace.cleanup_repo", CONTROL, input, |_| {
        let exists = query_opt(
            &kernel.database,
            "SELECT 1 FROM cleanups WHERE project_id = ? AND cleanup_id = ?",
            [&kernel.project_id, cleanup_id],
            |r| r.get::<_, i64>(0),
        )?
        .is_some();
        if !exists {
            return Err(KernelError::controller(format!(
                "cleanup {cleanup_id} does not exist"
            )));
        }
        execute(
            &kernel.database,
            "INSERT INTO cleanup_repos(project_id, cleanup_id, repo) VALUES (?, ?, ?)",
            [&kernel.project_id, cleanup_id, repo],
        )
        .map_err(|_| KernelError::controller(format!("cleanup {cleanup_id} already has a repo")))?;
        let event =
            MutationEvent::new("cleanup", cleanup_id, 0).with_details(json!({"repo": repo}));
        Ok(MutationOutput::new(json!({"repo": repo}), event))
    })
}

/// `cleanupRepo`: the repo of a cleanup; '' (the root) for one without a row.
pub fn cleanup_repo(kernel: &Kernel, cleanup_id: &str) -> KernelResult<String> {
    kernel.assert_open()?;
    Ok(query_opt(
        &kernel.database,
        "SELECT repo FROM cleanup_repos WHERE project_id = ? AND cleanup_id = ?",
        [&kernel.project_id, cleanup_id],
        |row| row.get::<_, String>(0),
    )?
    .unwrap_or_default())
}

// ------------------------------------------------------------------------------------- read views (status, feed)

/// The most members a set view lists (a set holds at most this many repos).
pub const SET_VIEW_MEMBERS: usize = 32;

/// One member row as the views read it: the member and the state of its integration (`None` before it exists).
struct ViewRow {
    repo: String,
    position: i64,
    attempt: i64,
    integration_id: Option<String>,
    state: Option<String>,
    branch: Option<String>,
    head_sha: Option<String>,
}

/// The members of a set as a view lists them, in merge order, and what the set as a whole is: whether every live member is
/// confirmed and which integration is the review subject (the first live member that has one, as `set_gate` picks it).
/// A member with no integration yet reads `running`; a member whose integration was discarded in a set that goes on is
/// `dropped` (outside the gate, review and confirm).
fn member_values(set_state: &str, rows: &[ViewRow]) -> (Vec<Value>, bool, Option<String>) {
    let mut live = 0;
    let mut confirmed = true;
    let mut subject: Option<String> = None;
    let mut dropped_flags = Vec::new();
    for row in rows {
        let discarded = row.state.as_deref() == Some("discarded");
        dropped_flags.push(discarded && set_state != "discarded");
        if discarded {
            continue;
        }
        if subject.is_none() {
            subject.clone_from(&row.integration_id);
        }
        if row.state.as_deref() == Some("confirmed") {
            live += 1;
        } else {
            confirmed = false;
        }
    }
    let members = rows
        .iter()
        .zip(&dropped_flags)
        .map(|(row, dropped)| {
            json!({
                "repo": row.repo,
                "position": row.position,
                "attempt": row.attempt,
                "integrationId": row.integration_id,
                "state": row.state.clone().unwrap_or_else(|| "running".to_string()),
                "branch": row.branch,
                "headSha": row.head_sha,
                "dropped": dropped,
            })
        })
        .collect();
    (members, confirmed && live > 0, subject)
}

fn view_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ViewRow> {
    Ok(ViewRow {
        repo: row.get(0)?,
        position: row.get(1)?,
        attempt: row.get(2)?,
        integration_id: row.get(3)?,
        state: row.get(4)?,
        branch: row.get(5)?,
        head_sha: row.get(6)?,
    })
}

/// The members of a set with their integrations: one statement through the set's index, at most [`SET_VIEW_MEMBERS`] rows.
pub const SET_VIEW_SQL: &str = "SELECT m.repo, m.position, m.attempt, m.integration_id, i.state, i.branch, i.head_sha
       FROM integration_set_members m
       LEFT JOIN integrations i ON i.project_id = m.project_id AND i.integration_id = m.integration_id
      WHERE m.project_id = ?1 AND m.set_id = ?2 ORDER BY m.position LIMIT 32";

/// `setMembersView`: the members of a set as the status, the feed and the events list them, plus whether the set is
/// confirmed. Empty for an unknown set.
pub fn set_members_view(
    kernel: &Kernel,
    set_id: &str,
    set_state: &str,
) -> KernelResult<(Vec<Value>, bool, Option<String>)> {
    let rows = query_all(
        &kernel.database,
        SET_VIEW_SQL,
        [&kernel.project_id, set_id],
        view_row,
    )?;
    Ok(member_values(set_state, &rows))
}

/// `integrationSetView`: the `set` member of an integration that belongs to a set (`null` for any other): the set, this
/// member's place in it and every member. Reads two statements and at most [`SET_VIEW_MEMBERS`] rows.
pub fn integration_set_view(kernel: &Kernel, integration_id: &str) -> KernelResult<Option<Value>> {
    kernel.assert_open()?;
    let Some((set_id, repo, position, attempt, state)) = query_opt(
        &kernel.database,
        "SELECT m.set_id, m.repo, m.position, m.attempt, s.state
           FROM integration_set_members m
           JOIN integration_sets s ON s.project_id = m.project_id AND s.set_id = m.set_id
          WHERE m.project_id = ? AND m.integration_id = ?",
        [&kernel.project_id, integration_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
            ))
        },
    )?
    else {
        return Ok(None);
    };
    let (members, _, subject) = set_members_view(kernel, &set_id, &state)?;
    let dropped = members
        .iter()
        .any(|m| m["repo"] == repo.as_str() && m["dropped"] == true);
    Ok(Some(json!({
        "setId": set_id,
        "state": state,
        "repo": repo,
        "position": position,
        "attempt": attempt,
        "reviewSubject": subject.as_deref() == Some(integration_id),
        "dropped": dropped,
        "members": members,
    })))
}

/// `integrationSetsView`: the newest `limit` sets for the status, each with its members in merge order.
pub fn integration_sets_view(kernel: &Kernel, limit: i64) -> KernelResult<Value> {
    kernel.assert_open()?;
    let sets = query_all(
        &kernel.database,
        "SELECT set_id, state, branch FROM integration_sets WHERE project_id = ? ORDER BY sequence DESC LIMIT ?",
        rusqlite::params![kernel.project_id, limit],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    )?;
    let mut out = Vec::new();
    for (set_id, state, branch) in sets {
        let (members, confirmed, _) = set_members_view(kernel, &set_id, &state)?;
        out.push(json!({
            "setId": set_id,
            "state": state,
            "branch": branch,
            "confirmed": confirmed,
            "members": members,
        }));
    }
    Ok(Value::Array(out))
}

/// The most open cleanup offers the feed lists.
pub const CLEANUP_OFFERS_MAX: i64 = 64;

/// `cleanupOffersView`: the open (`offered`) cleanups of a workspace, newest first, each `{cleanupId, repo, branches,
/// worktrees}`: the branches still offered and how many of them have a worktree. Only a cleanup with a repo row is listed,
/// so a single-repo project (which has none) gets an empty list. One statement.
pub fn cleanup_offers_view(kernel: &Kernel) -> KernelResult<Value> {
    kernel.assert_open()?;
    let rows = query_all(
        &kernel.database,
        "SELECT c.cleanup_id, r.repo,
                (SELECT COUNT(*) FROM cleanup_branches b WHERE b.project_id = c.project_id AND b.cleanup_id = c.cleanup_id AND b.outcome = 'offered'),
                (SELECT COUNT(*) FROM cleanup_branches b WHERE b.project_id = c.project_id AND b.cleanup_id = c.cleanup_id AND b.outcome = 'offered' AND b.worktree_path IS NOT NULL)
           FROM cleanups c JOIN cleanup_repos r ON r.project_id = c.project_id AND r.cleanup_id = c.cleanup_id
          WHERE c.project_id = ? AND c.state = 'offered'
          ORDER BY c.sequence DESC LIMIT ?",
        rusqlite::params![kernel.project_id, CLEANUP_OFFERS_MAX],
        |row| {
            Ok(json!({
                "cleanupId": row.get::<_, String>(0)?,
                "repo": row.get::<_, String>(1)?,
                "branches": row.get::<_, i64>(2)?,
                "worktrees": row.get::<_, i64>(3)?,
            }))
        },
    )?;
    Ok(Value::Array(rows))
}

/// `setVerifyView`: the verify part of the status's newest `limit` sets, in the order of [`integration_sets_view`]: the
/// gate as the review and merge read it (`set_gate_by`: each repo's own commands sha, the set's `reset_at`) and each live
/// member's verify. Runs change without a `state_version` bump, so the status keeps this apart from the sets.
/// `[{setId, gate, verify: [{repo, verify}]}]`; `gate` is `null` for a set with no live member, else
/// `{state: passed|waiting|failed, repos}` (`failed` names the repos whose counted runs failed, else `waiting` those
/// still running or without a run); a member's `verify` is `null` without a counted run, else
/// `{state, commitSha, lastPassed}` (`lastPassed` only when an earlier commit passed).
pub fn set_verify_view(
    kernel: &Kernel,
    limit: i64,
    sha_of: &dyn Fn(&str) -> String,
) -> KernelResult<Value> {
    kernel.assert_open()?;
    let sets = query_all(
        &kernel.database,
        "SELECT set_id FROM integration_sets WHERE project_id = ? ORDER BY sequence DESC LIMIT ?",
        rusqlite::params![kernel.project_id, limit],
        |row| row.get::<_, String>(0),
    )?;
    let mut out = Vec::new();
    for set_id in sets {
        let gate = set_gate_by(kernel, &set_id, sha_of)?;
        let (mut failed, mut waiting) = (Vec::new(), Vec::new());
        let mut verify = Vec::new();
        for member in gate["members"].as_array().into_iter().flatten() {
            let repo = member["repo"].as_str().unwrap_or_default().to_string();
            let state = match member["code"].as_str() {
                Some("passed") => Some("passed"),
                Some("verify_running") => {
                    waiting.push(repo.clone());
                    Some("running")
                }
                Some("verify_failed") => {
                    failed.push(repo.clone());
                    Some("failed")
                }
                _ => {
                    waiting.push(repo.clone());
                    None
                }
            };
            let last_passed = match (&member["integrationId"], state) {
                (Value::String(id), Some(s)) if s != "passed" => query_opt(
                    &kernel.database,
                    "SELECT commit_sha FROM verify_runs WHERE project_id = ? AND subject_integration_id = ? AND state = 'passed'
                      ORDER BY sequence DESC LIMIT 1",
                    [&kernel.project_id, id],
                    |r| r.get::<_, String>(0),
                )?
                .filter(|sha| member["commitSha"].as_str() != Some(sha.as_str())),
                _ => None,
            };
            verify.push(json!({
                "repo": repo,
                "verify": state.map_or(Value::Null, |state| json!({
                    "state": state, "commitSha": member["commitSha"], "lastPassed": last_passed,
                })),
            }));
        }
        let gate = if verify.is_empty() {
            Value::Null
        } else if !failed.is_empty() {
            json!({"state": "failed", "repos": failed})
        } else if !waiting.is_empty() {
            json!({"state": "waiting", "repos": waiting})
        } else {
            json!({"state": "passed", "repos": []})
        };
        out.push(json!({"setId": set_id, "gate": gate, "verify": verify}));
    }
    Ok(Value::Array(out))
}

/// `agentWorktreeMembers`: the `worktrees` member of an agent in the status: `[{repo, path, branch}]` from its live
/// worktrees (current generation), and the change folder (the folder that holds them). Empty for an agent with none.
pub(crate) fn agent_worktree_members(
    kernel: &Kernel,
) -> KernelResult<std::collections::BTreeMap<String, Value>> {
    kernel.assert_open()?;
    let rows = query_all(
        &kernel.database,
        "SELECT w.agent_id, w.repo, w.worktree_path, w.branch
           FROM agent_worktrees w
           JOIN agents a ON a.project_id = w.project_id AND a.agent_id = w.agent_id AND a.generation = w.generation
          WHERE w.project_id = ? AND w.removed_at IS NULL ORDER BY w.agent_id, w.repo",
        [&kernel.project_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        },
    )?;
    let mut out: std::collections::BTreeMap<String, Vec<(String, String, String)>> =
        std::collections::BTreeMap::new();
    for (agent, repo, path, branch) in rows {
        out.entry(agent).or_default().push((repo, path, branch));
    }
    Ok(out
        .into_iter()
        .map(|(agent, trees)| {
            let folder = std::path::Path::new(&trees[0].1)
                .parent()
                .map(|p| p.to_string_lossy().into_owned());
            let list: Vec<Value> = trees
                .into_iter()
                .map(|(repo, path, branch)| json!({"repo": repo, "path": path, "branch": branch}))
                .collect();
            (agent, json!({"worktrees": list, "changeFolder": folder}))
        })
        .collect())
}
