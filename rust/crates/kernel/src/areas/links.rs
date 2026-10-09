//! The external links (src/controller/links.ts), with the wanted-state decisions of src/nexora.ts as private code. The
//! `taskNaming`, `agentHasReports`, `activeBranchHolder` and `renameAgentBranch` the Node `ControllerCore` takes from its
//! panes area live here too.

use crate::dispatch::{arg, call, Args};
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{execute, exists, query_all, query_opt, safe_id, safe_id_str};
use crate::kernel::Kernel;
use crate::plan_body::{package_naming, package_of_body};
use crate::records::{MutationEvent, MutationOutput};
use crate::types::MutationContext;
use regex::Regex;
use serde_json::{json, Value};
use std::sync::LazyLock;

use super::{integrations, plan_packages, plans};

/// `NEXORA_STATES`: the Nexora statuses the PM writes.
const NEXORA_STATES: [&str; 6] = [
    "backlog",
    "todo",
    "in_progress",
    "in_review",
    "completed",
    "wont_do",
];

const EXTERNAL_REF_KINDS: [&str; 3] = ["requirement", "plan", "package"];

/// `NEXORA_ID_PATTERN`.
static NEXORA_ID_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z][A-Za-z0-9]{0,15}-[0-9]{1,9}$").expect("the pattern is valid")
});

static BRANCH_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._/-]{0,99}$").expect("the pattern is valid")
});

/// `isNexoraState`.
fn is_nexora_state(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|state| NEXORA_STATES.contains(&state))
}

/// `PackageStanding`: first match wins; a confirmed package is finished work that a later cancel does not unwind.
fn standing(facts: &plan_packages::PackageFacts) -> &'static str {
    if facts.confirmed {
        "confirmed"
    } else if facts.cancelled {
        "cancelled"
    } else {
        facts.progress
    }
}

/// `wantedPackageState`.
fn wanted_package_state(facts: &plan_packages::PackageFacts) -> &'static str {
    match standing(facts) {
        "unassigned" => "todo",
        "assigned" | "reported" | "findings" => "in_progress",
        "reviewed" | "integrated" => "in_review",
        "confirmed" => "completed",
        _ => "wont_do",
    }
}

/// `wantedPlanState`: the parent item of a plan; `None` for a superseded plan, which keeps what it had and is not
/// drift-checked.
fn wanted_plan_state(
    state: &str,
    cancelled: bool,
    packages: &[plan_packages::PackageFacts],
) -> Option<&'static str> {
    if state == "superseded" {
        return None;
    }
    let standings: Vec<&str> = packages.iter().map(standing).collect();
    let live: Vec<&str> = standings
        .iter()
        .copied()
        .filter(|s| *s != "cancelled")
        .collect();
    let all_confirmed = !standings.is_empty() && standings.iter().all(|s| *s == "confirmed");
    if cancelled && !all_confirmed {
        return Some("wont_do");
    }
    if state == "approved" && live.is_empty() {
        return Some("wont_do");
    }
    if !live.is_empty() && live.iter().all(|s| *s == "confirmed") {
        return Some("completed");
    }
    if state == "draft" || state == "in_review" || live.iter().all(|s| *s == "unassigned") {
        return Some("todo");
    }
    if live
        .iter()
        .all(|s| matches!(*s, "reviewed" | "integrated" | "confirmed"))
    {
        return Some("in_review");
    }
    Some("in_progress")
}

/// `wantedRequirementState`: the item of a small-tier requirement; `None` while no developer is bound.
fn wanted_requirement_state(
    bound: bool,
    synced_state: &str,
    report_confirmed: bool,
    review_passed: bool,
) -> Option<&'static str> {
    if !bound {
        return None;
    }
    if synced_state == "wont_do" && !report_confirmed {
        return Some("wont_do");
    }
    if report_confirmed {
        return Some("completed");
    }
    if review_passed {
        return Some("in_review");
    }
    Some("in_progress")
}

fn field<'a>(input: &'a Value, key: &str) -> &'a Value {
    input.get(key).unwrap_or(&Value::Null)
}

/// A row of `external_links`.
#[derive(Clone, Debug)]
pub(crate) struct LinkRow {
    ref_kind: String,
    ref_id: String,
    system: String,
    external_id: String,
    synced_state: String,
    bound_agent_id: Option<String>,
    bound_at: Option<String>,
    linked_by: String,
    linked_at: String,
    synced_at: String,
}

impl LinkRow {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            ref_kind: row.get("ref_kind")?,
            ref_id: row.get("ref_id")?,
            system: row.get("system")?,
            external_id: row.get("external_id")?,
            synced_state: row.get("synced_state")?,
            bound_agent_id: row.get("bound_agent_id")?,
            bound_at: row.get("bound_at")?,
            linked_by: row.get("linked_by")?,
            linked_at: row.get("linked_at")?,
            synced_at: row.get("synced_at")?,
        })
    }
}

/// What `#wantedState` reads of a link row.
struct LinkFacts<'a> {
    synced_state: &'a str,
    bound_agent_id: Option<&'a str>,
    bound_at: Option<&'a str>,
}

/// The wanted Nexora status of a linked item, derived from ledger facts and never stored; `None` when there is none
/// (unbound requirement, superseded plan).
fn wanted_state(
    kernel: &Kernel,
    ref_kind: &str,
    ref_id: &str,
    link: Option<LinkFacts<'_>>,
) -> KernelResult<Option<&'static str>> {
    if ref_kind == "requirement" {
        let Some(LinkFacts {
            synced_state,
            bound_agent_id: Some(bound_agent_id),
            bound_at: Some(bound_at),
        }) = link
        else {
            return Ok(None);
        };
        // The bound agent and the agents it replaced count, but only reports made since the binding.
        let report = query_opt(
            &kernel.database,
            "WITH RECURSIVE chain(agent_id) AS (
               SELECT ?
               UNION
               SELECT e.entity_id FROM controller_events e JOIN chain c
                 ON json_extract(e.payload_json, '$.details.successorId') = c.agent_id
               WHERE e.project_id = ? AND e.entity_type = 'agent' AND e.to_state = 'replaced')
             SELECT r.report_id FROM agent_reports r
             WHERE r.project_id = ? AND r.agent_id IN (SELECT agent_id FROM chain)
               AND r.state = 'accepted' AND r.created_at >= ?
             ORDER BY r.sequence DESC LIMIT 1",
            [
                bound_agent_id,
                kernel.project_id.as_str(),
                kernel.project_id.as_str(),
                bound_at,
            ],
            |row| row.get::<_, String>(0),
        )?;
        let verdict = match &report {
            None => None,
            Some(report_id) => query_opt(
                &kernel.database,
                "SELECT state FROM reviews WHERE project_id = ? AND subject_report_id = ? AND state IN ('passed', 'findings')
                 ORDER BY sequence DESC LIMIT 1",
                [&kernel.project_id, report_id],
                |row| row.get::<_, String>(0),
            )?,
        };
        let report_confirmed = match &report {
            Some(report_id) => integrations::report_in_confirmed_integration(kernel, report_id)?,
            None => false,
        };
        return Ok(wanted_requirement_state(
            true,
            synced_state,
            report_confirmed,
            verdict.as_deref() == Some("passed"),
        ));
    }
    let plan_id = if ref_kind == "plan" {
        ref_id.to_string()
    } else {
        plan_packages::package_ref(ref_id)?.0
    };
    let Some(plan) = plans::plan_row(kernel, &plan_id)? else {
        return Err(KernelError::controller(format!(
            "plan {plan_id} does not exist"
        )));
    };
    let plan_cancelled = plan.cancelled_at.is_some();
    let rows = plan_packages::plan_package_rows(kernel, &plan_id)?;
    if ref_kind == "plan" {
        let mut facts = Vec::new();
        for row in &rows {
            facts.push(plan_packages::package_facts(
                kernel,
                &plan_id,
                row,
                plan_cancelled,
            )?);
        }
        return Ok(wanted_plan_state(&plan.state, plan_cancelled, &facts));
    }
    let (_, package_id) = plan_packages::package_ref(ref_id)?;
    let Some(row) = rows.iter().find(|r| r.package_id == package_id) else {
        return Err(KernelError::controller(format!(
            "plan {plan_id} has no package {package_id}"
        )));
    };
    Ok(Some(wanted_package_state(&plan_packages::package_facts(
        kernel,
        &plan_id,
        row,
        plan_cancelled,
    )?)))
}

/// `linkRecord`: a link with its wanted state and drift flag.
pub(crate) fn link_record(kernel: &Kernel, row: &LinkRow) -> KernelResult<Value> {
    let wanted = wanted_state(
        kernel,
        &row.ref_kind,
        &row.ref_id,
        Some(LinkFacts {
            synced_state: &row.synced_state,
            bound_agent_id: row.bound_agent_id.as_deref(),
            bound_at: row.bound_at.as_deref(),
        }),
    )?;
    Ok(json!({
        "refKind": row.ref_kind,
        "refId": row.ref_id,
        "system": row.system,
        "externalId": row.external_id,
        "syncedState": row.synced_state,
        "boundAgentId": row.bound_agent_id,
        "boundAt": row.bound_at,
        "linkedBy": row.linked_by,
        "linkedAt": row.linked_at,
        "syncedAt": row.synced_at,
        "wanted": wanted,
        "drift": wanted.is_some_and(|w| w != row.synced_state),
    }))
}

/// Every link as `linkRecord` shows it, oldest first.
pub(crate) fn link_records(kernel: &Kernel) -> KernelResult<Vec<Value>> {
    let rows = query_all(
        &kernel.database,
        "SELECT * FROM external_links WHERE project_id = ? ORDER BY linked_at, ref_kind, ref_id",
        [&kernel.project_id],
        LinkRow::from_row,
    )?;
    rows.iter().map(|row| link_record(kernel, row)).collect()
}

/// `linkRow`.
pub(crate) fn link_row(
    kernel: &Kernel,
    ref_kind: &str,
    ref_id: &str,
) -> KernelResult<Option<LinkRow>> {
    query_opt(
        &kernel.database,
        "SELECT * FROM external_links WHERE project_id = ? AND ref_kind = ? AND ref_id = ? AND system = 'nexora'",
        [&kernel.project_id, ref_kind, ref_id],
        LinkRow::from_row,
    )
}

fn assert_link_target(kernel: &Kernel, ref_kind: &str, ref_id: &str) -> KernelResult<()> {
    if ref_kind == "requirement" {
        return Ok(());
    }
    if ref_kind == "plan" {
        if plans::plan_row(kernel, ref_id)?.is_none() {
            return Err(KernelError::controller(format!(
                "plan {ref_id} does not exist"
            )));
        }
        return Ok(());
    }
    let (plan_id, package_id) = plan_packages::package_ref(ref_id)?;
    let found = exists(
        &kernel.database,
        "SELECT 1 AS present FROM plan_packages WHERE project_id = ? AND plan_id = ? AND package_id = ?",
        [&kernel.project_id, &plan_id, &package_id],
    )?;
    if !found {
        return Err(KernelError::controller(format!(
            "plan {plan_id} has no package {package_id}"
        )));
    }
    Ok(())
}

fn ref_kind_of(value: &Value) -> KernelResult<&str> {
    match value.as_str() {
        Some(kind) if EXTERNAL_REF_KINDS.contains(&kind) => Ok(kind),
        _ => Err(KernelError::type_error(format!(
            "the link kind must be one of {}",
            EXTERNAL_REF_KINDS.join(", ")
        ))),
    }
}

/// `linkExternal`: records what the PM wrote to Nexora for a requirement, a plan or a package.
pub fn link_external(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let ref_kind = ref_kind_of(field(input, "refKind"))?;
    let ref_id_value = field(input, "refId");
    if ref_kind == "package" {
        match ref_id_value.as_str() {
            Some(text) => {
                plan_packages::package_ref(text)?;
            }
            None => {
                return Err(KernelError::type_error(
                    "Cannot read properties of undefined (reading 'split')",
                ))
            }
        }
    } else {
        safe_id(ref_id_value, "ref id")?;
    }
    let ref_id = ref_id_value.as_str().unwrap_or_default();
    let external_id = match field(input, "externalId").as_str() {
        Some(id) if NEXORA_ID_PATTERN.is_match(id) => id,
        _ => {
            return Err(KernelError::type_error(
                "the external id must look like PM-47",
            ))
        }
    };
    let synced_state: Option<&str> = match input.get("syncedState") {
        None => None,
        Some(value) if is_nexora_state(value) => value.as_str(),
        Some(_) => {
            return Err(KernelError::type_error(
                "the synced state is not a Nexora status",
            ))
        }
    };
    kernel.mutate(context, "link.external", "plan:write", input, |actor| {
        if actor.role != "PM" && actor.role != "operator" {
            return Err(KernelError::controller(
                "only the PM or the operator records a link",
            ));
        }
        assert_link_target(kernel, ref_kind, ref_id)?;
        let now = kernel.now();
        let existing = link_row(kernel, ref_kind, ref_id)?;
        match existing {
            None => {
                execute(
                    &kernel.database,
                    "INSERT INTO external_links(project_id, ref_kind, ref_id, system, external_id, synced_state, linked_by, linked_at, synced_at)
                     VALUES (?, ?, ?, 'nexora', ?, ?, ?, ?, ?)",
                    rusqlite::params![
                        kernel.project_id,
                        ref_kind,
                        ref_id,
                        external_id,
                        synced_state.unwrap_or("todo"),
                        actor.actor_id,
                        now,
                        now
                    ],
                )?;
            }
            Some(existing) if existing.external_id != external_id => {
                return Err(KernelError::controller(format!(
                    "link_conflict: {ref_kind} {ref_id} is linked to {}, not {external_id}",
                    existing.external_id
                )));
            }
            Some(existing) => {
                execute(
                    &kernel.database,
                    "UPDATE external_links SET synced_state = ?, synced_at = ? WHERE project_id = ? AND ref_kind = ? AND ref_id = ? AND system = 'nexora'",
                    [
                        synced_state.unwrap_or(existing.synced_state.as_str()),
                        now.as_str(),
                        kernel.project_id.as_str(),
                        ref_kind,
                        ref_id,
                    ],
                )?;
            }
        }
        let row = link_row(kernel, ref_kind, ref_id)?
            .ok_or_else(|| KernelError::controller("the link row is missing"))?;
        Ok(MutationOutput::new(
            link_record(kernel, &row)?,
            MutationEvent::new("external_link", &format!("{ref_kind}:{ref_id}"), 0)
                .with_details(json!({"externalId": external_id, "syncedState": synced_state})),
        ))
    })
}

/// `bindRequirement`: binds the developer whose reports drive a small-tier requirement's wanted state.
pub fn bind_requirement(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let ref_id = safe_id(field(input, "refId"), "requirement ref id")?;
    let agent_id = safe_id(field(input, "agentId"), "agent id")?;
    kernel.mutate(context, "link.bind", "plan:write", input, |actor| {
        if actor.role != "PM" && actor.role != "operator" {
            return Err(KernelError::controller(
                "only the PM or the operator binds a requirement",
            ));
        }
        if link_row(kernel, "requirement", ref_id)?.is_none() {
            return Err(KernelError::controller(format!(
                "requirement {ref_id} is not linked; run link first"
            )));
        }
        let agent = match kernel.agent_row(agent_id)? {
            Some(agent) if agent.state == "active" && agent.kind == "Developer" => agent,
            _ => {
                return Err(KernelError::controller(format!(
                    "{agent_id} is not an active developer-kind agent"
                )))
            }
        };
        execute(
            &kernel.database,
            "UPDATE external_links SET bound_agent_id = ?, bound_at = ? WHERE project_id = ? AND ref_kind = 'requirement' AND ref_id = ? AND system = 'nexora'",
            [
                agent.agent_id.as_str(),
                kernel.now().as_str(),
                kernel.project_id.as_str(),
                ref_id,
            ],
        )?;
        let row = link_row(kernel, "requirement", ref_id)?
            .ok_or_else(|| KernelError::controller("the link row is missing"))?;
        Ok(MutationOutput::new(
            link_record(kernel, &row)?,
            MutationEvent::new("external_link", &format!("requirement:{ref_id}"), 0)
                .with_details(json!({"boundAgentId": agent.agent_id})),
        ))
    })
}

/// `taskNaming`: the naming keys of a task a worker branch is named after.
pub fn task_naming(kernel: &Kernel, credential: &str, reference: &str) -> KernelResult<Value> {
    kernel.authorize(credential, "plan:read")?;
    let Some(slash) = reference.find('/') else {
        safe_id_str(reference, "requirement ref id")?;
        let link = link_row(kernel, "requirement", reference)?;
        return Ok(json!({
            "kind": "requirement",
            "taskId": link.map_or_else(|| reference.to_string(), |l| l.external_id),
            "type": null,
            "title": reference,
        }));
    };
    let plan_id = &reference[..slash];
    let package_id = &reference[slash + 1..];
    safe_id_str(plan_id, "plan id")?;
    safe_id_str(package_id, "package id")?;
    let Some(plan) = plans::plan_row(kernel, plan_id)? else {
        return Err(KernelError::controller(format!(
            "plan {plan_id} does not exist"
        )));
    };
    let revision = query_opt(
        &kernel.database,
        "SELECT body_json FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
        rusqlite::params![kernel.project_id, plan_id, plan.approved_revision],
        |row| row.get::<_, String>(0),
    )?;
    let view = revision
        .as_ref()
        .and_then(|body| package_of_body(body, package_id));
    let (Some(revision), Some(view)) = (revision, view) else {
        return Err(KernelError::controller(format!(
            "plan {plan_id} has no approved package {package_id}"
        )));
    };
    let link = link_row(kernel, "package", &format!("{plan_id}/{package_id}"))?;
    Ok(json!({
        "kind": "package",
        "taskId": link.map_or_else(|| format!("{plan_id}-{package_id}"), |l| l.external_id),
        "type": package_naming(&revision, package_id).kind,
        "title": view.title,
    }))
}

/// `agentHasReports`: whether the agent has made any report, accepted or not.
pub fn agent_has_reports(kernel: &Kernel, agent_id: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    safe_id_str(agent_id, "agent id")?;
    Ok(Value::Bool(exists(
        &kernel.database,
        "SELECT 1 AS present FROM agent_reports WHERE project_id = ? AND agent_id = ? LIMIT 1",
        [&kernel.project_id, agent_id],
    )?))
}

/// `activeBranchHolder`: the active agent whose recorded branch is `branch`, other than `except_agent_id`; null when none.
pub fn active_branch_holder(
    kernel: &Kernel,
    branch: &str,
    except_agent_id: Option<&str>,
) -> KernelResult<Value> {
    kernel.assert_open()?;
    let holder = query_opt(
        &kernel.database,
        "SELECT p.agent_id FROM agent_panes p JOIN agents a ON a.project_id = p.project_id AND a.agent_id = p.agent_id
         WHERE p.project_id = ? AND p.branch = ? AND a.state = 'active' AND p.agent_id <> ? LIMIT 1",
        [
            kernel.project_id.as_str(),
            branch,
            except_agent_id.unwrap_or(""),
        ],
        |row| row.get::<_, String>(0),
    )?;
    Ok(holder.map_or(Value::Null, Value::String))
}

/// `renameAgentBranch`: records that an active agent's branch was renamed in git.
pub fn rename_agent_branch(
    kernel: &Kernel,
    context: &MutationContext,
    input: &Value,
) -> KernelResult<Value> {
    let agent_id = safe_id(field(input, "agentId"), "agent id")?;
    for key in ["from", "to"] {
        let acceptable = field(input, key)
            .as_str()
            .is_some_and(|name| BRANCH_PATTERN.is_match(name));
        if !acceptable {
            return Err(KernelError::type_error("branch name is not acceptable"));
        }
    }
    let from = field(input, "from").as_str().unwrap_or_default();
    let to = field(input, "to").as_str().unwrap_or_default();
    kernel.mutate(
        context,
        "agent_pane.rename_branch",
        "controller:reconcile",
        input,
        |_| {
            let agent = kernel.agent_row(agent_id)?;
            let Some(agent) = agent.filter(|a| a.state == "active") else {
                return Err(KernelError::controller(
                    "a branch is renamed for an active agent",
                ));
            };
            let branch = query_opt(
                &kernel.database,
                "SELECT branch FROM agent_panes WHERE project_id = ? AND agent_id = ?",
                [&kernel.project_id, agent_id],
                |row| row.get::<_, Option<String>>(0),
            )?
            .flatten();
            if branch.as_deref() != Some(from) {
                return Err(KernelError::controller(format!(
                    "{agent_id} does not have the recorded branch {from}"
                )));
            }
            execute(
                &kernel.database,
                "UPDATE agent_panes SET branch = ?, updated_at = ? WHERE project_id = ? AND agent_id = ?",
                [to, kernel.now().as_str(), kernel.project_id.as_str(), agent_id],
            )?;
            let mut event = MutationEvent::new("agent_pane", agent_id, agent.generation)
                .with_details(json!({"from": from, "to": to}));
            event.to_state = Some("branch_renamed".into());
            Ok(MutationOutput::new(json!({"renamed": true}), event))
        },
    )
}

/// `externalLinks`: every link with its wanted state and drift flag, oldest first.
pub fn external_links(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    kernel.authorize(credential, "plan:read")?;
    Ok(Value::Array(link_records(kernel)?))
}

/// `wantedNexoraState`: the wanted Nexora status of one ref; null when the ledger has none.
pub fn wanted_nexora_state(
    kernel: &Kernel,
    credential: &str,
    ref_kind: &Value,
    ref_id: &str,
) -> KernelResult<Value> {
    kernel.authorize(credential, "plan:read")?;
    let ref_kind = ref_kind_of(ref_kind)?;
    if ref_kind == "package" {
        plan_packages::package_ref(ref_id)?;
    } else {
        safe_id_str(ref_id, "ref id")?;
    }
    let link = link_row(kernel, ref_kind, ref_id)?;
    let wanted = wanted_state(
        kernel,
        ref_kind,
        ref_id,
        link.as_ref().map(|link| LinkFacts {
            synced_state: &link.synced_state,
            bound_agent_id: link.bound_agent_id.as_deref(),
            bound_at: link.bound_at.as_deref(),
        }),
    )?;
    Ok(wanted.map_or(Value::Null, |w| Value::String(w.to_string())))
}

/// `syncDrift`: the links whose last synced state differs from the wanted one.
pub fn sync_drift(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    kernel.authorize(credential, "plan:read")?;
    Ok(Value::Array(
        link_records(kernel)?
            .into_iter()
            .filter(|link| link["drift"] == true)
            .collect(),
    ))
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "linkExternal" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(link_external(kernel, &context, input)))
        }
        "bindRequirement" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(bind_requirement(kernel, &context, input)))
        }
        "taskNaming" => {
            let credential = arg!(a.str(0, "credential"));
            let reference = arg!(a.str(1, "ref"));
            Some(call(task_naming(kernel, credential, reference)))
        }
        "agentHasReports" => {
            let agent_id = arg!(a.str(0, "agentId"));
            Some(call(agent_has_reports(kernel, agent_id)))
        }
        "activeBranchHolder" => {
            let branch = arg!(a.str(0, "branch"));
            let except_agent_id = arg!(a.opt_str(1, "exceptAgentId"));
            Some(call(active_branch_holder(kernel, branch, except_agent_id)))
        }
        "renameAgentBranch" => {
            let context = arg!(a.ctx(0));
            let input = a.value(1);
            Some(call(rename_agent_branch(kernel, &context, input)))
        }
        "externalLinks" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(external_links(kernel, credential)))
        }
        "wantedNexoraState" => {
            let credential = arg!(a.str(0, "credential"));
            let ref_kind = a.value(1);
            let ref_id = arg!(a.str(2, "refId"));
            Some(call(wanted_nexora_state(
                kernel, credential, ref_kind, ref_id,
            )))
        }
        "syncDrift" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(sync_drift(kernel, credential)))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(
        progress: &'static str,
        cancelled: bool,
        confirmed: bool,
    ) -> plan_packages::PackageFacts {
        plan_packages::PackageFacts {
            progress,
            cancelled,
            confirmed,
        }
    }

    #[test]
    fn a_package_is_wanted_by_its_standing() {
        let cases = [
            (facts("unassigned", false, false), "todo"),
            (facts("assigned", false, false), "in_progress"),
            (facts("reported", false, false), "in_progress"),
            (facts("findings", false, false), "in_progress"),
            (facts("reviewed", false, false), "in_review"),
            (facts("integrated", false, false), "in_review"),
            (facts("integrated", false, true), "completed"),
            (facts("assigned", true, false), "wont_do"),
            // finished work is not unwound by a later cancel
            (facts("integrated", true, true), "completed"),
        ];
        for (case, wanted) in cases {
            assert_eq!(wanted_package_state(&case), wanted);
        }
    }

    #[test]
    fn a_plan_is_wanted_by_its_live_packages() {
        let done = || facts("integrated", false, true);
        let open = || facts("assigned", false, false);
        assert_eq!(wanted_plan_state("superseded", false, &[]), None);
        assert_eq!(wanted_plan_state("draft", false, &[]), Some("todo"));
        assert_eq!(
            wanted_plan_state("in_review", false, &[open()]),
            Some("todo")
        );
        assert_eq!(wanted_plan_state("approved", false, &[]), Some("wont_do"));
        assert_eq!(
            wanted_plan_state("approved", false, &[done()]),
            Some("completed")
        );
        assert_eq!(
            wanted_plan_state("approved", false, &[done(), facts("assigned", true, false)]),
            Some("completed")
        );
        assert_eq!(
            wanted_plan_state("approved", true, &[done(), open()]),
            Some("wont_do")
        );
        assert_eq!(
            wanted_plan_state("approved", true, &[done()]),
            Some("completed")
        );
        assert_eq!(
            wanted_plan_state("approved", false, &[facts("unassigned", false, false)]),
            Some("todo")
        );
        assert_eq!(
            wanted_plan_state(
                "approved",
                false,
                &[facts("reviewed", false, false), done()]
            ),
            Some("in_review")
        );
        assert_eq!(
            wanted_plan_state(
                "approved",
                false,
                &[facts("reviewed", false, false), open()]
            ),
            Some("in_progress")
        );
    }

    #[test]
    fn a_requirement_is_wanted_once_a_developer_is_bound() {
        assert_eq!(wanted_requirement_state(false, "todo", true, true), None);
        assert_eq!(
            wanted_requirement_state(true, "wont_do", false, true),
            Some("wont_do")
        );
        assert_eq!(
            wanted_requirement_state(true, "wont_do", true, true),
            Some("completed")
        );
        assert_eq!(
            wanted_requirement_state(true, "todo", false, true),
            Some("in_review")
        );
        assert_eq!(
            wanted_requirement_state(true, "todo", false, false),
            Some("in_progress")
        );
    }

    #[test]
    fn nexora_states_are_the_six_the_pm_writes() {
        for state in NEXORA_STATES {
            assert!(is_nexora_state(&json!(state)));
        }
        assert!(!is_nexora_state(&json!("done")));
        assert!(!is_nexora_state(&Value::Null));
        assert!(NEXORA_ID_PATTERN.is_match("PM-47"));
        assert!(!NEXORA_ID_PATTERN.is_match("pm47"));
    }
}
