//! The cleanup records (migration 0038): an offer or an automatic run of the branch cleanup and what became of each
//! branch it considered. The git half lives in `cleanup.rs`; this area only reads and writes the ledger.

use crate::canonical::sha256;
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{execute, query_all, query_opt, safe_id};
use crate::kernel::Kernel;
use crate::records::{MutationEvent, MutationOutput};
use crate::types::MutationContext;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::LazyLock;

use super::{integrations, message_notices, messages};

static SHA1: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9a-f]{40}$").expect("the pattern is valid"));

const CAPABILITY: &str = "controller:reconcile";

fn field<'a>(input: &'a Value, key: &str) -> &'a Value {
    input.get(key).unwrap_or(&Value::Null)
}

fn one_of<'a>(input: &'a Value, key: &str, allowed: &[&str]) -> KernelResult<&'a str> {
    match field(input, key).as_str() {
        Some(value) if allowed.contains(&value) => Ok(value),
        _ => Err(KernelError::type_error(format!(
            "{key} must be one of {}",
            allowed.join(", ")
        ))),
    }
}

/// A pane the ledger recorded for an agent: where its branch lives.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneRef {
    pub agent_id: String,
    pub active: bool,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
}

/// Every recorded pane with its agent's state.
pub fn pane_refs(kernel: &Kernel) -> KernelResult<Vec<PaneRef>> {
    kernel.assert_open()?;
    query_all(
        &kernel.database,
        "SELECT p.agent_id, a.state, p.branch, p.worktree_path FROM agent_panes p
         JOIN agents a ON a.project_id = p.project_id AND a.agent_id = p.agent_id
         WHERE p.project_id = ? ORDER BY p.agent_id",
        [&kernel.project_id],
        |row| {
            Ok(PaneRef {
                agent_id: row.get(0)?,
                active: row.get::<_, String>(1)? == "active",
                branch: row.get(2)?,
                worktree_path: row.get(3)?,
            })
        },
    )
}

/// The branches of integrations that are not settled yet (running, merged, conflicted); they are not for cleanup.
pub fn unsettled_integration_branches(kernel: &Kernel) -> KernelResult<BTreeSet<String>> {
    kernel.assert_open()?;
    Ok(query_all(
        &kernel.database,
        "SELECT branch FROM integrations WHERE project_id = ? AND state IN ('running', 'merged', 'conflicted')",
        [&kernel.project_id],
        |row| row.get::<_, String>(0),
    )?
    .into_iter()
    .collect())
}

/// The accepted report whose commit is `sha` and that a confirmed integration holds (as a member or by coverage), with
/// that integration; the earliest such integration when several hold it.
pub fn confirmed_link_for_commit(
    kernel: &Kernel,
    sha: &str,
) -> KernelResult<Option<(String, String)>> {
    kernel.assert_open()?;
    let reports = query_all(
        &kernel.database,
        "SELECT report_id FROM agent_reports WHERE project_id = ? AND commit_sha = ? AND state = 'accepted' ORDER BY sequence",
        [&kernel.project_id, sha],
        |row| row.get::<_, String>(0),
    )?;
    for report_id in reports {
        if !integrations::report_in_confirmed_integration(kernel, &report_id)? {
            continue;
        }
        let integration_id = query_opt(
            &kernel.database,
            "SELECT i.integration_id FROM integration_reports ir
             JOIN integrations i ON i.project_id = ir.project_id AND i.integration_id = ir.integration_id
             WHERE ir.project_id = ? AND ir.report_id = ? AND i.state = 'confirmed'
             UNION ALL
             SELECT i.integration_id FROM integration_covered_reports c
             JOIN integrations i ON i.project_id = c.project_id AND i.integration_id = c.integration_id
             WHERE c.project_id = ? AND c.report_id = ? AND i.state = 'confirmed'
             LIMIT 1",
            [
                kernel.project_id.as_str(),
                report_id.as_str(),
                kernel.project_id.as_str(),
                report_id.as_str(),
            ],
            |row| row.get::<_, String>(0),
        )?;
        if let Some(integration_id) = integration_id {
            return Ok(Some((report_id, integration_id)));
        }
    }
    Ok(None)
}

/// Whether an integration is running.
pub fn integration_running(kernel: &Kernel) -> KernelResult<bool> {
    kernel.assert_open()?;
    crate::helpers::exists(
        &kernel.database,
        "SELECT 1 AS present FROM integrations WHERE project_id = ? AND state = 'running'",
        [&kernel.project_id],
    )
}

fn cleanup_record(kernel: &Kernel, cleanup_id: &str) -> KernelResult<Value> {
    let Some(mut record) = query_opt(
        &kernel.database,
        "SELECT cleanup_id, sequence, trigger, integration_id, requested_by, mode, state, message_id, created_at, completed_at
         FROM cleanups WHERE project_id = ? AND cleanup_id = ?",
        [&kernel.project_id, cleanup_id],
        |row| {
            Ok(json!({
                "cleanupId": row.get::<_, String>(0)?,
                "sequence": row.get::<_, i64>(1)?,
                "trigger": row.get::<_, String>(2)?,
                "integrationId": row.get::<_, Option<String>>(3)?,
                "requestedBy": row.get::<_, String>(4)?,
                "mode": row.get::<_, String>(5)?,
                "state": row.get::<_, String>(6)?,
                "messageId": row.get::<_, Option<String>>(7)?,
                "createdAt": row.get::<_, String>(8)?,
                "completedAt": row.get::<_, Option<String>>(9)?,
            }))
        },
    )?
    else {
        return Ok(Value::Null);
    };
    let entries = query_all(
        &kernel.database,
        "SELECT branch, sha, class, report_id, integration_id, worktree_path, outcome, reason, decided_at
         FROM cleanup_branches WHERE project_id = ? AND cleanup_id = ? ORDER BY branch",
        [&kernel.project_id, cleanup_id],
        |row| {
            Ok(json!({
                "branch": row.get::<_, String>(0)?,
                "sha": row.get::<_, String>(1)?,
                "class": row.get::<_, String>(2)?,
                "reportId": row.get::<_, Option<String>>(3)?,
                "integrationId": row.get::<_, Option<String>>(4)?,
                "worktreePath": row.get::<_, Option<String>>(5)?,
                "outcome": row.get::<_, String>(6)?,
                "reason": row.get::<_, Option<String>>(7)?,
                "decidedAt": row.get::<_, Option<String>>(8)?,
            }))
        },
    )?;
    record["entries"] = Value::Array(entries);
    Ok(record)
}

/// One cleanup with its branch entries; null when there is none.
pub fn cleanup(kernel: &Kernel, cleanup_id: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    cleanup_record(kernel, cleanup_id)
}

/// The cleanups, newest first.
pub fn cleanups(kernel: &Kernel, credential: &str, limit: Option<i64>) -> KernelResult<Value> {
    kernel.authorize(credential, CAPABILITY)?;
    let ids = query_all(
        &kernel.database,
        "SELECT cleanup_id FROM cleanups WHERE project_id = ? ORDER BY sequence DESC LIMIT ?",
        rusqlite::params![kernel.project_id, limit.unwrap_or(20)],
        |row| row.get::<_, String>(0),
    )?;
    let mut records = Vec::new();
    for id in ids {
        records.push(cleanup_record(kernel, &id)?);
    }
    Ok(Value::Array(records))
}

fn queue_notice(
    kernel: &Kernel,
    body: &str,
    now: &str,
    action_needed: bool,
    kind: &str,
) -> KernelResult<Option<String>> {
    let Some(parties) = message_notices::notice_parties(kernel)? else {
        return Ok(None);
    };
    Ok(Some(messages::insert_queued_message(
        kernel,
        &parties.controller_actor_id,
        &parties.pm,
        body,
        &sha256(body),
        now,
        action_needed,
        messages::class_for_kind(kernel, kind),
    )?))
}

/// Records an offer (or the start of an automatic run): the cleanup and its entries. An older open offer is
/// superseded by a new `ask` offer. A `notice` is queued to the PM as an action-needed message.
pub fn record_offer(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let cleanup_id = safe_id(field(input, "cleanupId"), "cleanup id")?;
    let requested_by = safe_id(field(input, "requestedBy"), "requester")?;
    let trigger = one_of(input, "trigger", &["confirm", "manual"])?;
    let mode = one_of(input, "mode", &["ask", "auto"])?;
    let integration_id = match field(input, "integrationId") {
        Value::Null => None,
        value => Some(safe_id(value, "integration id")?),
    };
    let entries = match field(input, "entries").as_array() {
        Some(entries) if !entries.is_empty() => entries,
        _ => {
            return Err(KernelError::type_error(
                "a cleanup takes at least one branch",
            ))
        }
    };
    for entry in entries {
        let sha_ok = field(entry, "sha")
            .as_str()
            .is_some_and(|s| SHA1.is_match(s));
        let branch_ok = field(entry, "branch")
            .as_str()
            .is_some_and(|b| !b.is_empty());
        if !sha_ok || !branch_ok {
            return Err(KernelError::type_error(
                "a cleanup branch needs a name and a full lowercase sha1",
            ));
        }
        one_of(entry, "class", &["landed", "landed_by_content", "unlanded"])?;
    }
    let notice = field(input, "notice").as_str();
    kernel.mutate(
        context,
        "cleanup.offer",
        CAPABILITY,
        input,
        |_| {
            let now = kernel.now();
            let mut superseded: Vec<String> = Vec::new();
            if mode == "ask" {
                superseded = query_all(
                    &kernel.database,
                    "SELECT cleanup_id FROM cleanups WHERE project_id = ? AND state = 'offered' ORDER BY sequence",
                    [&kernel.project_id],
                    |row| row.get::<_, String>(0),
                )?;
                for old in &superseded {
                    execute(
                        &kernel.database,
                        "UPDATE cleanup_branches SET outcome = 'skipped', reason = 'superseded', decided_at = ?
                         WHERE project_id = ? AND cleanup_id = ? AND outcome = 'offered'",
                        [&now, &kernel.project_id, old],
                    )?;
                    execute(
                        &kernel.database,
                        "UPDATE cleanups SET state = 'superseded', completed_at = ? WHERE project_id = ? AND cleanup_id = ?",
                        [&now, &kernel.project_id, old],
                    )?;
                }
            }
            let sequence = query_opt(
                &kernel.database,
                "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM cleanups WHERE project_id = ?",
                [&kernel.project_id],
                |row| row.get::<_, i64>(0),
            )?
            .unwrap_or(1);
            let message_id = match notice {
                Some(body) if mode == "ask" => queue_notice(kernel, body, &now, true, "cleanup_offer")?,
                _ => None,
            };
            execute(
                &kernel.database,
                "INSERT INTO cleanups(project_id, cleanup_id, sequence, trigger, integration_id, requested_by, mode, state, message_id, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, 'offered', ?, ?)",
                rusqlite::params![
                    kernel.project_id,
                    cleanup_id,
                    sequence,
                    trigger,
                    integration_id,
                    requested_by,
                    mode,
                    message_id,
                    now
                ],
            )?;
            for entry in entries {
                execute(
                    &kernel.database,
                    "INSERT INTO cleanup_branches(project_id, cleanup_id, branch, sha, class, report_id, integration_id, worktree_path, outcome)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'offered')",
                    rusqlite::params![
                        kernel.project_id,
                        cleanup_id,
                        field(entry, "branch").as_str(),
                        field(entry, "sha").as_str(),
                        field(entry, "class").as_str(),
                        field(entry, "reportId").as_str(),
                        field(entry, "integrationId").as_str(),
                        field(entry, "worktreePath").as_str()
                    ],
                )?;
            }
            let mut event = MutationEvent::new("cleanup", cleanup_id, 0).with_details(json!({
                "trigger": trigger, "mode": mode, "integrationId": integration_id,
                "branches": entries.len(), "superseded": superseded,
            }));
            event.to_state = Some("offered".into());
            Ok(MutationOutput::new(cleanup_record(kernel, cleanup_id)?, event))
        },
    )
}

/// Records what became of one branch of an open cleanup: `deleted` or `skipped` (with the reason).
pub fn record_branch_outcome(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let cleanup_id = safe_id(field(input, "cleanupId"), "cleanup id")?;
    let outcome = one_of(input, "outcome", &["deleted", "skipped"])?;
    let Some(branch) = field(input, "branch").as_str() else {
        return Err(KernelError::type_error("the branch must be text"));
    };
    let reason = field(input, "reason").as_str();
    if (outcome == "skipped") != reason.is_some() {
        return Err(KernelError::type_error(
            "a skipped branch needs a reason, a deleted one has none",
        ));
    }
    kernel.mutate(context, "cleanup.branch", CAPABILITY, input, |_| {
        let state = query_opt(
            &kernel.database,
            "SELECT state FROM cleanups WHERE project_id = ? AND cleanup_id = ?",
            [&kernel.project_id, cleanup_id],
            |row| row.get::<_, String>(0),
        )?;
        if state.as_deref() != Some("offered") {
            return Err(KernelError::controller("the cleanup is not open"));
        }
        let now = kernel.now();
        let changed = execute(
            &kernel.database,
            "UPDATE cleanup_branches SET outcome = ?, reason = ?, decided_at = ?
             WHERE project_id = ? AND cleanup_id = ? AND branch = ? AND outcome = 'offered'",
            rusqlite::params![outcome, reason, now, kernel.project_id, cleanup_id, branch],
        )?;
        if changed != 1 {
            return Err(KernelError::controller(
                "the branch is not an open entry of the cleanup",
            ));
        }
        let event = MutationEvent::new("cleanup", cleanup_id, 0)
            .transition("offered", outcome)
            .with_details(json!({"branch": branch, "reason": reason}));
        Ok(MutationOutput::new(
            cleanup_record(kernel, cleanup_id)?,
            event,
        ))
    })
}

/// Closes an open cleanup as `applied` or `declined`; entries still open are skipped (`not_selected`, `declined`).
/// A `notice` is queued to the PM (not action-needed).
pub fn complete_cleanup(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let cleanup_id = safe_id(field(input, "cleanupId"), "cleanup id")?;
    let state = one_of(input, "state", &["applied", "declined"])?;
    let notice = field(input, "notice").as_str();
    kernel.mutate(context, "cleanup.complete", CAPABILITY, input, |_| {
        let current = query_opt(
            &kernel.database,
            "SELECT state FROM cleanups WHERE project_id = ? AND cleanup_id = ?",
            [&kernel.project_id, cleanup_id],
            |row| row.get::<_, String>(0),
        )?;
        if current.as_deref() != Some("offered") {
            return Err(KernelError::controller(format!(
                "the cleanup is {}, not open",
                current.as_deref().unwrap_or("unknown")
            )));
        }
        let now = kernel.now();
        let reason = if state == "applied" { "not_selected" } else { "declined" };
        execute(
            &kernel.database,
            "UPDATE cleanup_branches SET outcome = 'skipped', reason = ?, decided_at = ?
             WHERE project_id = ? AND cleanup_id = ? AND outcome = 'offered'",
            [reason, &now, &kernel.project_id, cleanup_id],
        )?;
        execute(
            &kernel.database,
            "UPDATE cleanups SET state = ?, completed_at = ? WHERE project_id = ? AND cleanup_id = ?",
            [state, &now, &kernel.project_id, cleanup_id],
        )?;
        if let Some(body) = notice {
            queue_notice(kernel, body, &now, false, "cleanup_done")?;
        }
        let event = MutationEvent::new("cleanup", cleanup_id, 0).transition("offered", state);
        Ok(MutationOutput::new(cleanup_record(kernel, cleanup_id)?, event))
    })
}
