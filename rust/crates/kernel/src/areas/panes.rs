//! The agent panes (src/controller/panes.ts).
//!
//! `recordAgentPane` and `recordOrphanPane` carry the Herdr terminal id of migration 0037. `taskNaming`,
//! `agentHasReports`, `activeBranchHolder` and `renameAgentBranch` of the same Node class belong to the links area of
//! the Rust kernel (`areas/links.rs`) and are not here.

use crate::dispatch::{arg, call, Args};
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{execute, exists, js_trim, query_all, query_opt, safe_id};
use crate::kernel::Kernel;
use crate::records::{MutationEvent, MutationOutput};
use crate::types::MutationContext;
use regex::Regex;
use serde_json::{json, Value};
use std::sync::LazyLock;

/// `MAX_TASK_TITLE` of src/task-text.ts.
const MAX_TASK_TITLE: usize = 200;

/// `TASK_REF_PATTERN` of src/task-text.ts.
static TASK_REF_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}(/[A-Za-z0-9][A-Za-z0-9._:-]{0,127})?$")
        .expect("the pattern is valid")
});

static BRANCH_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._/-]{0,99}$").expect("the pattern is valid")
});

static BASE_SHA_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9a-f]{40}$").expect("the pattern is valid"));

/// `/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u`.
static UNPRINTABLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]").expect("the pattern is valid"));

/// `/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]|\s\s/u`.
static UNACCEPTABLE_TITLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]|\s\s").expect("the pattern is valid"));

/// A field of an input object; `None` is JavaScript's `undefined` (the key is absent).
fn field<'a>(input: &'a Value, key: &str) -> Option<&'a Value> {
    input.get(key)
}

/// The text `RegExp.prototype.test` makes of its argument.
fn js_text(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Bool(flag)) => flag.to_string(),
        Some(Value::Number(number)) => number.to_string(),
        Some(_) => "[object Object]".into(),
    }
}

fn text_or_null(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_string)
}

/// `recordAgentPane`.
pub fn record_agent_pane(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let agent_id = safe_id(field(input, "agentId").unwrap_or(&Value::Null), "agent id")?;
    for (key, label) in [("workspaceId", "workspace id"), ("paneId", "pane id")] {
        let value = field(input, key);
        if !matches!(value, Some(Value::Null)) {
            safe_id(value.unwrap_or(&Value::Null), label)?;
        }
    }
    if let Some(terminal_id) = field(input, "terminalId") {
        safe_id(terminal_id, "terminal id")?;
    }
    let worktree_path = field(input, "worktreePath");
    if !matches!(worktree_path, Some(Value::Null)) {
        let Some(path) = worktree_path.and_then(Value::as_str) else {
            return Err(KernelError::type_error(format!(
                "The \"path\" argument must be of type string. Received {}",
                match worktree_path {
                    None => "undefined".to_string(),
                    Some(other) => other.to_string(),
                }
            )));
        };
        if !path.starts_with('/')
            || path.encode_utf16().count() > 1000
            || UNPRINTABLE.is_match(path)
        {
            return Err(KernelError::type_error(
                "worktree path must be absolute and printable",
            ));
        }
    }
    if !matches!(field(input, "branch"), Some(Value::Null))
        && !BRANCH_PATTERN.is_match(&js_text(field(input, "branch")))
    {
        return Err(KernelError::type_error("branch name is not acceptable"));
    }
    if !matches!(field(input, "baseSha"), Some(Value::Null))
        && !BASE_SHA_PATTERN.is_match(&js_text(field(input, "baseSha")))
    {
        return Err(KernelError::type_error(
            "base sha must be 40 lowercase hex characters",
        ));
    }
    if let Some(task_ref) = field(input, "taskRef") {
        if !TASK_REF_PATTERN.is_match(&js_text(Some(task_ref))) {
            return Err(KernelError::type_error("task ref is not acceptable"));
        }
    }
    if let Some(title) = field(input, "taskTitle") {
        let acceptable = title.as_str().is_some_and(|text| {
            text.chars().count() <= MAX_TASK_TITLE
                && js_trim(text) == text
                && !text.is_empty()
                && !UNACCEPTABLE_TITLE.is_match(text)
        });
        if !acceptable {
            return Err(KernelError::type_error("task title is not acceptable"));
        }
    }
    let workspace_id = text_or_null(field(input, "workspaceId"));
    let pane_id = text_or_null(field(input, "paneId"));
    let worktree_path = text_or_null(field(input, "worktreePath"));
    let branch = text_or_null(field(input, "branch"));
    let base_sha = text_or_null(field(input, "baseSha"));
    let task_ref = text_or_null(field(input, "taskRef"));
    let task_title = text_or_null(field(input, "taskTitle"));
    let terminal_id = text_or_null(field(input, "terminalId"));
    kernel.mutate(
        context,
        "agent_pane.record",
        "controller:reconcile",
        input,
        |_| {
            let agent = kernel.agent_row(agent_id)?;
            let Some(agent) = agent.filter(|agent| agent.state == "active") else {
                return Err(KernelError::controller(
                    "a pane is recorded only for an active agent",
                ));
            };
            let now = kernel.now();
            // Node binds `input.branch` as given: an absent one cannot be bound.
            if field(input, "branch").is_none() {
                return Err(KernelError::type_error(
                    "Provided value cannot be bound to SQLite parameter 6.",
                ));
            }
            execute(
                &kernel.database,
                "INSERT INTO agent_panes(project_id, agent_id, workspace_id, pane_id, worktree_path, branch, base_sha, generation, created_at, updated_at, task_ref, task_title, terminal_id)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(project_id, agent_id) DO UPDATE SET
                   workspace_id = excluded.workspace_id, pane_id = excluded.pane_id,
                   worktree_path = excluded.worktree_path, branch = excluded.branch,
                   base_sha = excluded.base_sha, generation = excluded.generation,
                   updated_at = excluded.updated_at,
                   task_ref = COALESCE(excluded.task_ref, agent_panes.task_ref),
                   task_title = COALESCE(excluded.task_title, agent_panes.task_title),
                   terminal_id = excluded.terminal_id",
                rusqlite::params![
                    kernel.project_id,
                    agent_id,
                    workspace_id,
                    pane_id,
                    worktree_path,
                    branch,
                    base_sha,
                    agent.generation,
                    now,
                    now,
                    task_ref,
                    task_title,
                    terminal_id,
                ],
            )?;
            Ok(MutationOutput::new(
                json!({"recorded": true}),
                MutationEvent::new("agent_pane", agent_id, agent.generation)
                    .with_details(json!({"paneId": pane_id, "branch": branch})),
            ))
        },
    )
}

/// `clearAgentPane`.
pub fn clear_agent_pane(
    kernel: &Kernel,
    context: &MutationContext,
    agent_id: &str,
) -> KernelResult<Value> {
    crate::helpers::safe_id_str(agent_id, "agent id")?;
    kernel.assert_open()?;
    if !kernel.has_stored_request(context)?
        && !exists(
            &kernel.database,
            "SELECT 1 AS present FROM agent_panes WHERE project_id = ? AND agent_id = ?",
            [&kernel.project_id, agent_id],
        )?
    {
        kernel.authorize(&context.credential, "controller:reconcile")?;
        return Ok(json!({"cleared": false}));
    }
    kernel.mutate(
        context,
        "agent_pane.clear",
        "controller:reconcile",
        &json!({"agentId": agent_id}),
        |_| {
            execute(
                &kernel.database,
                "DELETE FROM agent_panes WHERE project_id = ? AND agent_id = ?",
                [&kernel.project_id, agent_id],
            )?;
            Ok(MutationOutput::new(
                json!({"cleared": true}),
                MutationEvent::new("agent_pane", agent_id, 0)
                    .with_details(json!({"cleared": true})),
            ))
        },
    )
}

/// `recordFallbackPane`.
pub fn record_fallback_pane(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let workspace_id = safe_id(
        field(input, "workspaceId").unwrap_or(&Value::Null),
        "workspace id",
    )?;
    let pane_id = safe_id(field(input, "paneId").unwrap_or(&Value::Null), "pane id")?;
    kernel.mutate(
        context,
        "fallback_pane.record",
        "controller:reconcile",
        input,
        |_| {
            execute(
                &kernel.database,
                "INSERT INTO fallback_panes(project_id, workspace_id, pane_id, created_at) VALUES (?, ?, ?, ?)
                 ON CONFLICT(project_id) DO UPDATE SET workspace_id = excluded.workspace_id, pane_id = excluded.pane_id",
                rusqlite::params![kernel.project_id, workspace_id, pane_id, kernel.now()],
            )?;
            Ok(MutationOutput::new(
                json!({"recorded": true}),
                MutationEvent::new("fallback_pane", &kernel.project_id, 0)
                    .with_details(json!({"paneId": pane_id})),
            ))
        },
    )
}

/// `clearFallbackPane`.
pub fn clear_fallback_pane(kernel: &Kernel, context: &MutationContext) -> KernelResult<Value> {
    kernel.assert_open()?;
    if !kernel.has_stored_request(context)?
        && !exists(
            &kernel.database,
            "SELECT 1 AS present FROM fallback_panes WHERE project_id = ?",
            [&kernel.project_id],
        )?
    {
        kernel.authorize(&context.credential, "controller:reconcile")?;
        return Ok(json!({"cleared": false}));
    }
    kernel.mutate(
        context,
        "fallback_pane.clear",
        "controller:reconcile",
        &json!({}),
        |_| {
            execute(
                &kernel.database,
                "DELETE FROM fallback_panes WHERE project_id = ?",
                [&kernel.project_id],
            )?;
            Ok(MutationOutput::new(
                json!({"cleared": true}),
                MutationEvent::new("fallback_pane", &kernel.project_id, 0)
                    .with_details(json!({"cleared": true})),
            ))
        },
    )
}

/// `recordOrphanPane`: a live pane of a replaced PM that could not be closed, kept so a daemon restart still knows it.
pub fn record_orphan_pane(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let pane_id = safe_id(field(input, "paneId").unwrap_or(&Value::Null), "pane id")?;
    let agent_id = safe_id(field(input, "agentId").unwrap_or(&Value::Null), "agent id")?;
    if let Some(terminal_id) = field(input, "terminalId") {
        safe_id(terminal_id, "terminal id")?;
    }
    let terminal_id = text_or_null(field(input, "terminalId"));
    kernel.mutate(
        context,
        "orphan_pane.record",
        "controller:reconcile",
        input,
        |_| {
            if kernel.agent_row(agent_id)?.is_none() {
                return Err(KernelError::controller(
                    "an orphan pane belongs to a known agent",
                ));
            }
            execute(
                &kernel.database,
                "INSERT OR IGNORE INTO orphan_panes(project_id, pane_id, agent_id, created_at, terminal_id) VALUES (?, ?, ?, ?, ?)",
                rusqlite::params![
                    kernel.project_id,
                    pane_id,
                    agent_id,
                    kernel.now(),
                    terminal_id
                ],
            )?;
            Ok(MutationOutput::new(
                json!({"recorded": true}),
                MutationEvent::new("orphan_pane", pane_id, 0)
                    .with_details(json!({"agentId": agent_id})),
            ))
        },
    )
}

/// `clearOrphanPane`.
pub fn clear_orphan_pane(
    kernel: &Kernel,
    context: &MutationContext,
    pane_id: &str,
) -> KernelResult<Value> {
    crate::helpers::safe_id_str(pane_id, "pane id")?;
    kernel.assert_open()?;
    if !kernel.has_stored_request(context)?
        && !exists(
            &kernel.database,
            "SELECT 1 AS present FROM orphan_panes WHERE project_id = ? AND pane_id = ?",
            [&kernel.project_id, pane_id],
        )?
    {
        kernel.authorize(&context.credential, "controller:reconcile")?;
        return Ok(json!({"cleared": false}));
    }
    kernel.mutate(
        context,
        "orphan_pane.clear",
        "controller:reconcile",
        &json!({"paneId": pane_id}),
        |_| {
            execute(
                &kernel.database,
                "DELETE FROM orphan_panes WHERE project_id = ? AND pane_id = ?",
                [&kernel.project_id, pane_id],
            )?;
            Ok(MutationOutput::new(
                json!({"cleared": true}),
                MutationEvent::new("orphan_pane", pane_id, 0)
                    .with_details(json!({"cleared": true})),
            ))
        },
    )
}

/// `orphanPanes`: the orphan panes without their terminal ids.
pub fn orphan_panes(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    let rows = orphan_rows(kernel, credential)?;
    Ok(Value::Array(
        rows.into_iter()
            .map(|(pane_id, agent_id, _)| json!({"paneId": pane_id, "agentId": agent_id}))
            .collect(),
    ))
}

/// `orphanPaneTerminals`: the orphan panes with the Herdr terminal id each had when it was recorded (null when unknown).
pub fn orphan_pane_terminals(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    let rows = orphan_rows(kernel, credential)?;
    Ok(Value::Array(
        rows.into_iter()
            .map(|(pane_id, agent_id, terminal_id)| {
                json!({"paneId": pane_id, "agentId": agent_id, "terminalId": terminal_id})
            })
            .collect(),
    ))
}

type OrphanRow = (String, String, Option<String>);

fn orphan_rows(kernel: &Kernel, credential: &str) -> KernelResult<Vec<OrphanRow>> {
    kernel.authorize(credential, "controller:reconcile")?;
    query_all(
        &kernel.database,
        "SELECT pane_id, agent_id, terminal_id FROM orphan_panes WHERE project_id = ? ORDER BY created_at, pane_id",
        [&kernel.project_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
}

/// `agentPanes`.
pub fn agent_panes(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    kernel.authorize(credential, "controller:reconcile")?;
    let rows = query_all(
        &kernel.database,
        "SELECT agent_id, workspace_id, pane_id, worktree_path, branch, base_sha, generation, task_ref, task_title FROM agent_panes WHERE project_id = ? ORDER BY agent_id",
        [&kernel.project_id],
        |row| {
            Ok(json!({
                "agentId": row.get::<_, String>(0)?,
                "workspaceId": row.get::<_, Option<String>>(1)?,
                "paneId": row.get::<_, Option<String>>(2)?,
                "worktreePath": row.get::<_, Option<String>>(3)?,
                "branch": row.get::<_, Option<String>>(4)?,
                "baseSha": row.get::<_, Option<String>>(5)?,
                "generation": row.get::<_, i64>(6)?,
                "taskRef": row.get::<_, Option<String>>(7)?,
                "taskTitle": row.get::<_, Option<String>>(8)?,
            }))
        },
    )?;
    Ok(Value::Array(rows))
}

/// `paneTerminalId`: the Herdr terminal id recorded with an agent's pane; null when there is no row or it was not known.
pub fn pane_terminal_id(kernel: &Kernel, credential: &str, agent_id: &str) -> KernelResult<Value> {
    kernel.authorize(credential, "controller:reconcile")?;
    let terminal_id = query_opt(
        &kernel.database,
        "SELECT terminal_id FROM agent_panes WHERE project_id = ? AND agent_id = ?",
        [&kernel.project_id, agent_id],
        |row| row.get::<_, Option<String>>(0),
    )?
    .flatten();
    Ok(json!(terminal_id))
}

/// `fallbackPane`: the recorded fallback pane, null when there is none.
pub fn fallback_pane(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    kernel.authorize(credential, "controller:reconcile")?;
    let row = query_opt(
        &kernel.database,
        "SELECT workspace_id, pane_id FROM fallback_panes WHERE project_id = ?",
        [&kernel.project_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    Ok(match row {
        Some((workspace_id, pane_id)) => json!({"workspaceId": workspace_id, "paneId": pane_id}),
        None => Value::Null,
    })
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "recordAgentPane" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(record_agent_pane(kernel, &context, input)))
        }
        "clearAgentPane" => {
            let context = arg!(a.ctx(0));
            let agent_id = arg!(a.str(1, "agentId"));
            Some(call(clear_agent_pane(kernel, &context, agent_id)))
        }
        "recordFallbackPane" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(record_fallback_pane(kernel, &context, input)))
        }
        "clearFallbackPane" => {
            let context = arg!(a.ctx(0));
            Some(call(clear_fallback_pane(kernel, &context)))
        }
        "recordOrphanPane" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(record_orphan_pane(kernel, &context, input)))
        }
        "clearOrphanPane" => {
            let context = arg!(a.ctx(0));
            let pane_id = arg!(a.str(1, "paneId"));
            Some(call(clear_orphan_pane(kernel, &context, pane_id)))
        }
        "orphanPanes" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(orphan_panes(kernel, credential)))
        }
        "orphanPaneTerminals" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(orphan_pane_terminals(kernel, credential)))
        }
        "agentPanes" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(agent_panes(kernel, credential)))
        }
        "paneTerminalId" => {
            let credential = arg!(a.str(0, "credential"));
            let agent_id = arg!(a.str(1, "agentId"));
            Some(call(pane_terminal_id(kernel, credential, agent_id)))
        }
        "fallbackPane" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(fallback_pane(kernel, credential)))
        }
        _ => None,
    }
}
