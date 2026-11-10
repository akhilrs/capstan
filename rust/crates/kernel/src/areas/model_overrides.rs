//! Model overrides: the model the user or the PM chose for a plan or a requirement (optionally per role kind), and the
//! model each agent was started with.
//!
//! An override is read when a worker is started; the controller only stores and answers. Setting and clearing follow
//! `bindRequirement`: `plan:write`, and only the PM or the operator.

use crate::errors::{KernelError, KernelResult};
use crate::helpers::{execute, query_all, query_opt, safe_id, safe_id_str};
use crate::kernel::Kernel;
use crate::records::{MutationEvent, MutationOutput};
use crate::types::MutationContext;
use serde_json::{json, Value};

use super::{links, plans, warm_reviewers};

const ROLE_KINDS: [&str; 3] = ["Developer", "Verifier", "Supervisor"];
pub const MODEL_SOURCES: [&str; 6] = ["spawn", "task", "plan", "role", "host", "defaults"];
const REFUSED: &str = "only the user, the PM or the operator may set a model override";

fn field<'a>(input: &'a Value, key: &str) -> &'a Value {
    input.get(key).unwrap_or(&Value::Null)
}

fn scope_type_of(input: &Value) -> KernelResult<&str> {
    match field(input, "scopeType").as_str() {
        Some(kind @ ("plan" | "requirement")) => Ok(kind),
        _ => Err(KernelError::type_error(
            "the scope type must be plan or requirement",
        )),
    }
}

/// The role kind of an input: null or absent is the all-kinds row (stored as the empty text).
fn role_kind_of(input: &Value) -> KernelResult<&str> {
    match field(input, "roleKind") {
        Value::Null => Ok(""),
        Value::String(kind) if ROLE_KINDS.contains(&kind.as_str()) => Ok(kind.as_str()),
        _ => Err(KernelError::type_error(
            "the role kind must be null, Developer, Verifier or Supervisor",
        )),
    }
}

fn model_of(value: &Value) -> KernelResult<&str> {
    match value.as_str() {
        Some(model) if !model.is_empty() => Ok(model),
        _ => Err(KernelError::type_error(
            "the model must be a non-empty text",
        )),
    }
}

/// Validates the `modelOverrides` list of an `openPlan` input: `[{roleKind, model}]`; absent or null is empty.
pub fn parse_plan_overrides(input: &Value) -> KernelResult<Vec<(String, String)>> {
    let list = match input.get("modelOverrides") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(list)) => list,
        Some(_) => {
            return Err(KernelError::type_error(
                "modelOverrides must be a list of {roleKind, model}",
            ))
        }
    };
    let mut parsed: Vec<(String, String)> = Vec::new();
    for entry in list {
        let kind = role_kind_of(entry)?.to_string();
        let model = model_of(field(entry, "model"))?.to_string();
        if parsed.iter().any(|(known, _)| *known == kind) {
            return Err(KernelError::type_error(
                "modelOverrides names a role kind twice",
            ));
        }
        parsed.push((kind, model));
    }
    Ok(parsed)
}

pub fn require_setter(role: &str) -> KernelResult<()> {
    if role != "PM" && role != "operator" {
        return Err(KernelError::controller(REFUSED));
    }
    Ok(())
}

fn stored(
    kernel: &Kernel,
    scope_type: &str,
    scope_id: &str,
    role_kind: &str,
) -> KernelResult<Option<String>> {
    query_opt(
        &kernel.database,
        "SELECT model FROM model_overrides WHERE project_id = ? AND scope_type = ? AND scope_id = ? AND role_kind = ?",
        [&kernel.project_id, scope_type, scope_id, role_kind],
        |row| row.get::<_, String>(0),
    )
}

/// Writes the plan's overrides inside the caller's mutation.
pub fn write_plan_overrides(
    kernel: &Kernel,
    plan_id: &str,
    overrides: &[(String, String)],
    set_by: &str,
    now: &str,
) -> KernelResult<()> {
    for (kind, model) in overrides {
        execute(
            &kernel.database,
            "INSERT INTO model_overrides(project_id, scope_type, scope_id, role_kind, model, set_by, set_at) VALUES (?, 'plan', ?, ?, ?, ?, ?)",
            [&kernel.project_id, plan_id, kind, model, set_by, now],
        )?;
    }
    Ok(())
}

fn check_scope(kernel: &Kernel, scope_type: &str, scope_id: &str) -> KernelResult<()> {
    if scope_type == "plan" {
        super::plan_archive::refuse_if_archived(kernel, scope_id)?;
        match plans::plan_row(kernel, scope_id)? {
            None => Err(KernelError::controller(format!(
                "plan {scope_id} does not exist"
            ))),
            Some(plan) if plan.cancelled_at.is_some() => Err(KernelError::controller(format!(
                "plan_cancelled: {scope_id} was cancelled"
            ))),
            Some(plan) if plan.state == "superseded" => Err(KernelError::controller(format!(
                "plan_superseded: plan {scope_id} was superseded"
            ))),
            Some(_) => Ok(()),
        }
    } else if links::link_row(kernel, "requirement", scope_id)?.is_none() {
        Err(KernelError::controller(format!(
            "requirement {scope_id} is not linked; run link first"
        )))
    } else {
        Ok(())
    }
}

fn kind_value(role_kind: &str) -> Value {
    if role_kind.is_empty() {
        Value::Null
    } else {
        json!(role_kind)
    }
}

/// `setModelOverride`.
pub fn set_model_override(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let scope_type = scope_type_of(input)?;
    let scope_id = safe_id(field(input, "scopeId"), "scope id")?;
    let role_kind = role_kind_of(input)?;
    let model = model_of(field(input, "model"))?;
    let payload = json!({"scopeType": scope_type, "scopeId": scope_id, "roleKind": kind_value(role_kind), "model": model});
    kernel.mutate(context, "model.override.set", "plan:write", &payload, |actor| {
        require_setter(&actor.role)?;
        check_scope(kernel, scope_type, scope_id)?;
        let previous = stored(kernel, scope_type, scope_id, role_kind)?;
        let now = kernel.now();
        execute(
            &kernel.database,
            "INSERT INTO model_overrides(project_id, scope_type, scope_id, role_kind, model, set_by, set_at) VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(project_id, scope_type, scope_id, role_kind) DO UPDATE SET model = excluded.model, set_by = excluded.set_by, set_at = excluded.set_at",
            [&kernel.project_id, scope_type, scope_id, role_kind, model, actor.actor_id.as_str(), now.as_str()],
        )?;
        Ok(MutationOutput::new(
            json!({"scopeType": scope_type, "scopeId": scope_id, "roleKind": kind_value(role_kind), "model": model,
                   "previous": previous, "setBy": actor.actor_id, "setAt": now}),
            MutationEvent::new("model_override", &format!("{scope_type}:{scope_id}"), 0)
                .with_details(json!({"roleKind": kind_value(role_kind), "model": model, "previous": previous})),
        ))
    })
}

/// `clearModelOverride`.
pub fn clear_model_override(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let scope_type = scope_type_of(input)?;
    let scope_id = safe_id(field(input, "scopeId"), "scope id")?;
    let role_kind = role_kind_of(input)?;
    let payload =
        json!({"scopeType": scope_type, "scopeId": scope_id, "roleKind": kind_value(role_kind)});
    kernel.mutate(context, "model.override.clear", "plan:write", &payload, |actor| {
        require_setter(&actor.role)?;
        if scope_type == "plan" {
            super::plan_archive::refuse_if_archived(kernel, scope_id)?;
        }
        let previous = stored(kernel, scope_type, scope_id, role_kind)?;
        if previous.is_some() {
            execute(
                &kernel.database,
                "DELETE FROM model_overrides WHERE project_id = ? AND scope_type = ? AND scope_id = ? AND role_kind = ?",
                [&kernel.project_id, scope_type, scope_id, role_kind],
            )?;
        }
        Ok(MutationOutput::new(
            json!({"cleared": previous.is_some(), "previous": previous}),
            MutationEvent::new("model_override", &format!("{scope_type}:{scope_id}"), 0)
                .with_details(json!({"roleKind": kind_value(role_kind), "previous": previous})),
        ))
    })
}

/// `modelOverrides`: the rows of one scope ordered by role kind (the all-kinds row first).
pub fn model_overrides(kernel: &Kernel, scope_type: &str, scope_id: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    if !matches!(scope_type, "plan" | "requirement") {
        return Err(KernelError::type_error(
            "the scope type must be plan or requirement",
        ));
    }
    safe_id_str(scope_id, "scope id")?;
    let rows = query_all(
        &kernel.database,
        "SELECT role_kind, model, set_by, set_at FROM model_overrides WHERE project_id = ? AND scope_type = ? AND scope_id = ? ORDER BY role_kind",
        [&kernel.project_id, scope_type, scope_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        },
    )?;
    Ok(Value::Array(
        rows.into_iter()
            .map(|(kind, model, set_by, set_at)| {
                json!({"roleKind": kind_value(&kind), "model": model, "setBy": set_by, "setAt": set_at})
            })
            .collect(),
    ))
}

/// `modelOverrideFor`: the first scope with a row wins; inside a scope the kind row beats the all-kinds row.
pub fn model_override_for(
    kernel: &Kernel,
    scopes: &[(&str, &str)],
    kind: &str,
) -> KernelResult<Option<Value>> {
    kernel.assert_open()?;
    for (scope_type, scope_id) in scopes {
        let found = query_opt(
            &kernel.database,
            "SELECT model FROM model_overrides WHERE project_id = ? AND scope_type = ? AND scope_id = ? AND role_kind IN (?, '')
             ORDER BY role_kind DESC LIMIT 1",
            rusqlite::params![kernel.project_id, scope_type, scope_id, kind],
            |row| row.get::<_, String>(0),
        )?;
        if let Some(model) = found {
            let source = if *scope_type == "requirement" {
                "task"
            } else {
                "plan"
            };
            return Ok(Some(
                json!({"model": model, "source": source, "scopeId": scope_id}),
            ));
        }
    }
    Ok(None)
}

/// `modelScopeOfSubject`: the scope whose override applies to the worker of a review subject.
pub fn model_scope_of_subject(
    kernel: &Kernel,
    subject_id: &str,
) -> KernelResult<Option<(String, String)>> {
    kernel.assert_open()?;
    safe_id_str(subject_id, "report or integration id")?;
    if let Some((plan_id, _)) = warm_reviewers::plan_of_subject(kernel, subject_id)? {
        return Ok(Some(("plan".to_string(), plan_id)));
    }
    let task_ref = query_opt(
        &kernel.database,
        "SELECT p.task_ref FROM agent_reports r JOIN agent_panes p ON p.project_id = r.project_id AND p.agent_id = r.agent_id
          WHERE r.project_id = ? AND r.report_id = ?",
        [&kernel.project_id, subject_id],
        |row| row.get::<_, Option<String>>(0),
    )?
    .flatten();
    Ok(task_ref.map(|reference| match reference.split_once('/') {
        Some((plan_id, _)) => ("plan".to_string(), plan_id.to_string()),
        None => ("requirement".to_string(), reference),
    }))
}

/// `agentModel`: the model an agent was started with.
pub fn agent_model(kernel: &Kernel, agent_id: &str) -> KernelResult<Option<Value>> {
    kernel.assert_open()?;
    safe_id_str(agent_id, "agent id")?;
    query_opt(
        &kernel.database,
        "SELECT model, source, scope_id FROM agent_models WHERE project_id = ? AND agent_id = ?",
        [&kernel.project_id, agent_id],
        |row| {
            Ok(
                json!({"model": row.get::<_, String>(0)?, "source": row.get::<_, String>(1)?, "scopeId": row.get::<_, Option<String>>(2)?}),
            )
        },
    )
}
