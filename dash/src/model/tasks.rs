//! Who works on what: the header entries and the per-agent task lookups from `status.activeTasks` or `status.plans`.
use std::collections::HashMap;

use indexmap::IndexMap;
use serde_json::Value;

use super::build::{
    get, is_plain_object, is_str, is_true, list, num, num_str, slice_units, text, text_of,
};
use super::types::TaskEntry;

const ACTIVE_PLAN_STATES: [&str; 3] = ["draft", "in_review", "approved"];

/// What the status says about who works on what, as lookups by agent id.
pub(super) struct TaskIndex {
    /// Header entries; `None` when the status has neither `activeTasks` nor `plans`.
    pub entries: Option<Vec<TaskEntry>>,
    /// Agent id -> `plan-19 · plan`, with ` +k` when it is the Architect of k more plans.
    pub architect: HashMap<String, String>,
    /// Agent id -> `plan-19/dash-ui PM-114 <title>`.
    pub assignee: HashMap<String, String>,
    /// Agent id -> `<nexora id or ref id> <title>`.
    pub requirement: HashMap<String, String>,
    /// `plan/package` -> the package label.
    pub package_by_ref: HashMap<String, String>,
    /// Requirement ref id -> label.
    pub requirement_by_ref: HashMap<String, String>,
    /// Built from `status.plans` alone (an older daemon): a pane's own task fields come before the Architect.
    pub from_plans: bool,
}

fn set_first(map: &mut HashMap<String, String>, key: String, value: &str) {
    if !key.is_empty() {
        map.entry(key).or_insert_with(|| value.to_string());
    }
}

/// `a b`, or whichever part is present; empty when neither is.
fn join_parts(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `(done/total)` from two counters, or "" when either is not a count.
fn progress_of(done: Option<&Value>, total: Option<&Value>) -> String {
    let as_count = |v: Option<&Value>| v.and_then(Value::as_f64).filter(|n| n.is_finite());
    match (as_count(done), as_count(total)) {
        (Some(d), Some(t)) => format!(
            "({}/{})",
            d.floor().max(0.0) as i64,
            t.floor().max(0.0) as i64
        ),
        _ => String::new(),
    }
}

fn add_architect(plans_of: &mut IndexMap<String, Vec<String>>, agent_id: String, plan_id: &str) {
    if !agent_id.is_empty() {
        plans_of
            .entry(agent_id)
            .or_default()
            .push(plan_id.to_string());
    }
}

fn architect_labels(plans_of: &IndexMap<String, Vec<String>>) -> HashMap<String, String> {
    plans_of
        .iter()
        .map(|(agent, ids)| {
            let more = if ids.len() > 1 {
                format!(" +{}", ids.len() - 1)
            } else {
                String::new()
            };
            (agent.clone(), format!("{} · plan{more}", ids[0]))
        })
        .collect()
}

/// The header entries and the per-agent task lookups from `status.activeTasks`. A status without it (an older daemon)
/// falls back to `status.plans`; with neither, or no open plan, `entries` is `None`. A malformed field is skipped.
pub(super) fn task_index_of(status: &Value) -> TaskIndex {
    let mut plans_of: IndexMap<String, Vec<String>> = IndexMap::new();
    let mut assignee = HashMap::new();
    let mut requirement = HashMap::new();
    let mut package_by_ref = HashMap::new();
    let mut requirement_by_ref = HashMap::new();
    let mut entries: Vec<TaskEntry> = Vec::new();

    if let Some(active) = get(status, "activeTasks").filter(|v| is_plain_object(v)) {
        for plan in list(get(active, "plans")) {
            let plan_id = text_of(plan, "planId");
            if plan_id.is_empty() {
                continue;
            }
            entries.push(TaskEntry {
                label: join_parts(&[
                    &plan_id,
                    &text_of(plan, "title"),
                    &progress_of(get(plan, "done"), get(plan, "total")),
                ]),
                id: plan_id.clone(),
            });
            add_architect(&mut plans_of, text_of(plan, "architectAgentId"), &plan_id);
            for pkg in list(get(plan, "packages")) {
                let package_id = text_of(pkg, "packageId");
                if package_id.is_empty() {
                    continue;
                }
                let reference = format!("{plan_id}/{package_id}");
                let label = join_parts(&[
                    &reference,
                    &text_of(pkg, "nexoraId"),
                    &text_of(pkg, "title"),
                ]);
                package_by_ref
                    .entry(reference)
                    .or_insert_with(|| label.clone());
                set_first(&mut assignee, text_of(pkg, "assigneeAgentId"), &label);
            }
        }
        for req in list(get(active, "requirements")) {
            let ref_id = text_of(req, "refId");
            let name = Some(text_of(req, "nexoraId"))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| ref_id.clone());
            if name.is_empty() {
                continue;
            }
            let label = join_parts(&[&name, &text_of(req, "title")]);
            entries.push(TaskEntry {
                id: name,
                label: label.clone(),
            });
            if !ref_id.is_empty() {
                requirement_by_ref.insert(ref_id, label.clone());
            }
            if let Some(Value::Array(agent_ids)) = get(req, "agentIds") {
                for agent_id in agent_ids {
                    set_first(&mut requirement, text(Some(agent_id)), &label);
                }
            }
        }
        return finish(
            Some(entries),
            false,
            &plans_of,
            assignee,
            requirement,
            package_by_ref,
            requirement_by_ref,
        );
    }

    if !matches!(get(status, "plans"), Some(Value::Array(_))) {
        return finish(
            None,
            false,
            &plans_of,
            assignee,
            requirement,
            package_by_ref,
            requirement_by_ref,
        );
    }
    for plan in list(get(status, "plans")) {
        let plan_id = text_of(plan, "planId");
        if plan_id.is_empty()
            || is_true(get(plan, "cancelled"))
            || !ACTIVE_PLAN_STATES.contains(&text_of(plan, "state").as_str())
            || !list(get(plan, "signoffs")).is_empty()
        {
            continue;
        }
        add_architect(&mut plans_of, text_of(plan, "architectAgentId"), &plan_id);
        let counts = get(plan, "packages").filter(|v| is_plain_object(v));
        let total: f64 = counts
            .and_then(Value::as_object)
            .map(|o| {
                o.iter()
                    .filter(|(state, _)| state.as_str() != "cancelled")
                    .map(|(_, n)| num(Some(n)))
                    .sum()
            })
            .unwrap_or(0.0);
        let count_of = |key: &str| counts.map_or(0.0, |c| num(get(c, key)));
        let done = count_of("reviewed") + count_of("integrated");
        let progress = if done.is_finite() && total.is_finite() {
            format!(
                "({}/{})",
                num_str(done.floor().max(0.0)),
                num_str(total.floor().max(0.0))
            )
        } else {
            String::new()
        };
        entries.push(TaskEntry {
            label: join_parts(&[&plan_id, &text_of(plan, "title"), &progress]),
            id: plan_id,
        });
    }
    let found = (!entries.is_empty()).then_some(entries);
    finish(
        found,
        true,
        &plans_of,
        assignee,
        requirement,
        package_by_ref,
        requirement_by_ref,
    )
}

#[allow(clippy::too_many_arguments)]
fn finish(
    entries: Option<Vec<TaskEntry>>,
    from_plans: bool,
    plans_of: &IndexMap<String, Vec<String>>,
    assignee: HashMap<String, String>,
    requirement: HashMap<String, String>,
    package_by_ref: HashMap<String, String>,
    requirement_by_ref: HashMap<String, String>,
) -> TaskIndex {
    TaskIndex {
        entries,
        architect: architect_labels(plans_of),
        assignee,
        requirement,
        package_by_ref,
        requirement_by_ref,
        from_plans,
    }
}

/// The task a pane was spawned with: the package or requirement it names, else its ref and spawn title.
pub(super) fn pane_task(pane: Option<&Value>, index: &TaskIndex) -> Option<String> {
    let pane = pane?;
    let reference = text_of(pane, "taskRef");
    if reference.is_empty() {
        return None;
    }
    let known = if reference.contains('/') {
        index.package_by_ref.get(&reference)
    } else {
        index.requirement_by_ref.get(&reference)
    };
    known.cloned().or_else(|| {
        Some(join_parts(&[&reference, &text_of(pane, "taskTitle")])).filter(|s| !s.is_empty())
    })
}

/// `review <author or integration id> r<round>` by reviewer, for each started review.
pub(super) fn review_subjects(reviews: &[&Value]) -> HashMap<String, String> {
    let mut subjects = HashMap::new();
    for r in reviews {
        if !is_str(get(r, "state"), "started") {
            continue;
        }
        let subject = Some(text_of(r, "authorAgentId"))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| slice_units(&text_of(r, "integrationId"), 8));
        if !subject.is_empty() {
            let label = format!("review {subject} r{}", num_str(num(get(r, "round"))));
            set_first(&mut subjects, text_of(r, "reviewerAgentId"), &label);
        }
    }
    subjects
}
