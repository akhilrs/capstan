//! `build_dash_model`: one operator `status` response in, display rows out, with the semantics of `buildDashModel`.
//!
//! The status is untrusted JSON, so every read mirrors what JavaScript does with the same value: a missing or mistyped
//! field reads as `undefined`, `String(value)` stands in for `text`, and nothing here panics on a malformed field.
use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::helpers::{is_worker_kind, TASK_NONE, WORKING_WINDOW_MS};
use super::pipeline::{label_integration, label_report, label_review, stage_of, true_counts};
use super::tasks::{pane_task, review_subjects, task_index_of};
use super::text::clean;
use super::time::parse_iso_ms;
use super::types::*;
use super::waiting::waiting_items;

/// A property of an object; `None` for a missing property and for a value that is not an object.
pub(super) fn get<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_object().and_then(|o| o.get(key))
}

/// `value ?? undefined` for a property: JSON `null` and a missing property both read as nothing.
pub(super) fn field<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    get(value, key).filter(|v| !v.is_null())
}

/// JavaScript's `typeof x === "object" && x !== null`: objects and arrays.
pub(super) fn is_objectish(value: &Value) -> bool {
    value.is_object() || value.is_array()
}

/// A plain object that is not an array.
pub(super) fn is_plain_object(value: &Value) -> bool {
    value.is_object()
}

/// JavaScript truthiness of a JSON value.
pub(super) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

pub(super) fn is_true(value: Option<&Value>) -> bool {
    value == Some(&Value::Bool(true))
}

pub(super) fn is_str(value: Option<&Value>, expected: &str) -> bool {
    value.and_then(Value::as_str) == Some(expected)
}

/// `list`: the object-like elements of an array; anything else is empty.
pub(super) fn list(value: Option<&Value>) -> Vec<&Value> {
    match value {
        Some(Value::Array(items)) => items.iter().filter(|v| is_objectish(v)).collect(),
        _ => Vec::new(),
    }
}

/// `num`: a finite number, else 0.
pub(super) fn num(value: Option<&Value>) -> f64 {
    value
        .and_then(Value::as_f64)
        .filter(|f| f.is_finite())
        .unwrap_or(0.0)
}

/// A finite number as JavaScript prints it.
pub(super) fn num_str(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// `String(value)` for a JSON value.
fn js_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.as_f64().map(num_str).unwrap_or_default(),
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(js_string).collect::<Vec<_>>().join(","),
        Value::Object(_) => "[object Object]".to_string(),
    }
}

/// `text`: `""` for nothing, else the cleaned `String(value)`.
pub(super) fn text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(v) => clean(&js_string(v)),
    }
}

/// `textOrNull`.
pub(super) fn text_or_null(value: Option<&Value>) -> Option<String> {
    match value {
        None | Some(Value::Null) => None,
        Some(v) => Some(clean(&js_string(v))),
    }
}

/// `text(row[key])`.
pub(super) fn text_of(row: &Value, key: &str) -> String {
    text(get(row, key))
}

/// `s.slice(0, n)` counted in UTF-16 code units, as JavaScript does.
pub(super) fn slice_units(s: &str, n: usize) -> String {
    let units: Vec<u16> = s.encode_utf16().take(n).collect();
    String::from_utf16_lossy(&units)
}

fn bool_str(b: bool) -> &'static str {
    if b {
        "true"
    } else {
        "false"
    }
}

fn opt_str(s: &Option<String>) -> &str {
    s.as_deref().unwrap_or("")
}

/// `Date.parse` ordering of two timestamps newest first; an unreadable one sorts as `-Infinity`.
fn newest_first(x: Option<i64>, y: Option<i64>) -> std::cmp::Ordering {
    let key = |t: Option<i64>| t.map_or(f64::NEG_INFINITY, |v| v as f64);
    let diff = key(y) - key(x);
    if diff.is_nan() || diff == 0.0 {
        std::cmp::Ordering::Equal
    } else if diff < 0.0 {
        std::cmp::Ordering::Less
    } else {
        std::cmp::Ordering::Greater
    }
}

fn supervision_of(status: &Value) -> Option<SupervisionHeader> {
    let rec = field(status, "supervisionState").filter(|v| is_objectish(v))?;
    let supervisor = field(rec, "supervisor").filter(|v| truthy(v));
    let check = field(rec, "lastCheck").filter(|v| truthy(v));
    Some(SupervisionHeader {
        enabled: is_true(get(rec, "enabled")),
        supervisor: supervisor.map(|s| SupervisorRef {
            id: text_of(s, "agentId"),
            state: text_of(s, "state"),
        }),
        last_check: check.map(|c| LastCheck {
            state: text_of(c, "state"),
            queued_at: text_of(c, "queuedAt"),
            acked_at: text_or_null(get(c, "ackedAt")),
        }),
        open_findings: num(get(rec, "openFindings")) as i64,
    })
}

/// Full auto and grants from the Operator part of the status; nothing when there is none.
fn operator_header(operator: Option<&Value>) -> (Option<i64>, Option<i64>) {
    let operator = operator.unwrap_or(&Value::Null);
    let full = field(operator, "fullAuto");
    let minutes = full
        .filter(|f| is_true(get(f, "on")))
        .map(|f| (num(get(f, "remainingSeconds")) / 60.0).ceil() as i64);
    let grants = list(get(operator, "grants")).len();
    (minutes, (grants > 0).then_some(grants as i64))
}

/// One operator `status` response to display rows.
pub fn build_dash_model(status: &Value, now_ms: i64, worker_limit: Option<i64>) -> DashModel {
    let empty = Value::Object(Default::default());
    let run = field(status, "run").unwrap_or(&empty);
    let supervision = supervision_of(status);

    let raw_messages = list(get(status, "messages"));
    let mut stuck: HashMap<String, String> = HashMap::new();
    for entry in list(get(status, "stuck")) {
        stuck.insert(text_of(entry, "messageId"), text_of(entry, "reason"));
    }
    let id_set = |key: &str| -> HashSet<String> {
        match get(status, key) {
            Some(Value::Array(items)) => items.iter().map(|v| text(Some(v))).collect(),
            _ => HashSet::new(),
        }
    };
    let stalled = id_set("stalledAgentIds");
    let lost = id_set("lostAgentIds");
    let pause = field(status, "pause").unwrap_or(&empty);
    let run_pause = field(pause, "run");
    let mut paused_at_of: HashMap<String, String> = HashMap::new();
    for entry in list(get(pause, "agents")) {
        paused_at_of.insert(text_of(entry, "agentId"), text_of(entry, "pausedAt"));
    }
    let panes = list(get(status, "panes"));
    let mut pane_of: HashMap<String, String> = HashMap::new();
    let mut pane_records: HashMap<String, &Value> = HashMap::new();
    for pane in &panes {
        if get(pane, "paneId").is_some_and(truthy) {
            pane_of.insert(text_of(pane, "agentId"), text_of(pane, "paneId"));
        }
        pane_records.insert(text_of(pane, "agentId"), pane);
    }
    let task_index = task_index_of(status);
    let subjects = review_subjects(&list(get(status, "reviews")));
    let agent_records = list(get(status, "agents"));
    let active_agents: HashSet<String> = agent_records
        .iter()
        .filter(|a| is_str(get(a, "state"), "active"))
        .map(|a| text_of(a, "agentId"))
        .collect();

    let mut depth: HashMap<String, f64> = HashMap::new();
    let mut in_flight: HashSet<String> = HashSet::new();
    let mut stuck_recipients: HashSet<String> = HashSet::new();
    for m in &raw_messages {
        let to = text_of(m, "recipientAgentId");
        *depth.entry(to.clone()).or_insert(0.0) += 1.0;
        if is_str(get(m, "state"), "sent") || is_str(get(m, "state"), "unacked") {
            in_flight.insert(to.clone());
        }
        if stuck.contains_key(&text_of(m, "messageId")) {
            stuck_recipients.insert(to);
        }
    }
    let findings = list(get(status, "agentFindings"));
    let escalated_targets: HashSet<String> = findings
        .iter()
        .filter(|f| is_str(get(f, "state"), "escalated"))
        .map(|f| text_of(f, "targetAgentId"))
        .collect();

    let all_agents: Vec<AgentRow> = agent_records
        .iter()
        .map(|a| {
            let agent_id = text_of(a, "agentId");
            let state = text_of(a, "state");
            let last_activity_at = text_of(a, "lastActivityAt");
            let last = parse_iso_ms(&last_activity_at);
            let working = state == "active"
                && (in_flight.contains(&agent_id)
                    || last.is_some_and(|l| (now_ms - l) < WORKING_WINDOW_MS));
            let kind = text_of(a, "kind");
            let blocked =
                stuck_recipients.contains(&agent_id) || escalated_targets.contains(&agent_id);
            let paused_at = paused_at_of
                .get(&agent_id)
                .cloned()
                .or_else(|| match run_pause {
                    Some(r) if kind != "PM" => Some(text_of(r, "pausedAt")),
                    _ => None,
                });
            let task = if state == "active" {
                let pane_task_of = || pane_task(pane_records.get(&agent_id).copied(), &task_index);
                task_index
                    .assignee
                    .get(&agent_id)
                    .cloned()
                    .or_else(|| {
                        if task_index.from_plans {
                            pane_task_of().or_else(|| task_index.architect.get(&agent_id).cloned())
                        } else {
                            task_index
                                .architect
                                .get(&agent_id)
                                .cloned()
                                .or_else(|| task_index.requirement.get(&agent_id).cloned())
                                .or_else(pane_task_of)
                        }
                    })
                    .or_else(|| subjects.get(&agent_id).cloned())
                    .unwrap_or_else(|| TASK_NONE.to_string())
            } else {
                TASK_NONE.to_string()
            };
            let generation = num(get(a, "generation"));
            let stalled_flag = stalled.contains(&agent_id);
            let lost_flag = lost.contains(&agent_id);
            let queue_depth = depth.get(&agent_id).copied().unwrap_or(0.0);
            let fingerprint = [
                state.clone(),
                task.clone(),
                num_str(generation),
                bool_str(working).to_string(),
                bool_str(stalled_flag).to_string(),
                bool_str(lost_flag).to_string(),
                bool_str(blocked).to_string(),
                bool_str(paused_at.is_some()).to_string(),
                num_str(queue_depth),
            ]
            .join("|");
            AgentRow {
                id: agent_id.clone(),
                role_name: text_of(a, "roleName"),
                kind,
                generation: generation as i64,
                state,
                last_activity_at,
                working,
                stalled: stalled_flag,
                lost: lost_flag,
                blocked,
                paused_at,
                pane_id: pane_of.get(&agent_id).cloned(),
                queue_depth: queue_depth as i64,
                task,
                fingerprint,
                agent_id,
            }
        })
        .collect();

    let attention = |a: &AgentRow| u8::from(!(a.lost || a.stalled || a.blocked));
    let mut active: Vec<AgentRow> = all_agents
        .iter()
        .filter(|a| a.state == "active")
        .cloned()
        .collect();
    active.sort_by_key(attention);
    let mut ended: Vec<(usize, Option<i64>, AgentRow)> = all_agents
        .iter()
        .filter(|a| a.state != "active")
        .cloned()
        .enumerate()
        .map(|(index, a)| (index, parse_iso_ms(&a.last_activity_at), a))
        .collect();
    ended.sort_by(|x, y| newest_first(x.1, y.1).then(y.0.cmp(&x.0)));

    let mut messages: Vec<MessageRow> = raw_messages
        .iter()
        .map(|m| {
            let message_id = text_of(m, "messageId");
            let state = text_of(m, "state");
            let problem = match stuck.get(&message_id) {
                Some(reason) => Some(reason.clone()),
                None if matches!(state.as_str(), "failed" | "expired" | "unacked") => {
                    Some(text_or_null(get(m, "stateReason")).unwrap_or_else(|| state.clone()))
                }
                None => None,
            };
            let recipient = text_of(m, "recipientAgentId");
            let deferred_reason = text_or_null(get(m, "deferredReason"));
            let state_reason = text_or_null(get(m, "stateReason"));
            let notified = field(m, "lastNotifiedAt").is_some();
            let fingerprint = [
                state.as_str(),
                opt_str(&deferred_reason),
                opt_str(&state_reason),
                bool_str(notified),
                opt_str(&problem),
            ]
            .join("|");
            MessageRow {
                id: message_id.clone(),
                message_id,
                recipient_active: active_agents.contains(&recipient),
                recipient_agent_id: recipient,
                state,
                sequence: num(get(m, "sequence")) as i64,
                queued_at: text_of(m, "queuedAt"),
                deferred_reason,
                state_reason,
                notified,
                problem,
                fingerprint,
            }
        })
        .collect();
    messages.sort_by_key(|m| u8::from(m.problem.is_none()));

    let mut finding_rows: Vec<FindingRow> = findings
        .iter()
        .filter(|f| is_str(get(f, "state"), "open") || is_str(get(f, "state"), "escalated"))
        .map(|f| {
            let target = text_of(f, "targetAgentId");
            let target_state = if active_agents.contains(&target) {
                "active"
            } else if agent_records
                .iter()
                .any(|a| text_of(a, "agentId") == target)
            {
                "ended"
            } else {
                "unknown"
            };
            let interventions = num(get(f, "interventions"));
            let state_reason = text_or_null(get(f, "stateReason"));
            let state = text_of(f, "state");
            let finding_id = text_of(f, "findingId");
            let fingerprint = [
                state.clone(),
                num_str(interventions),
                opt_str(&state_reason).to_string(),
                target_state.to_string(),
            ]
            .join("|");
            FindingRow {
                id: finding_id.clone(),
                finding_id,
                target_agent_id: target,
                severity: text_of(f, "severity"),
                needs_operator: is_str(get(f, "state"), "escalated"),
                state,
                interventions: interventions as i64,
                state_reason,
                target_state: target_state.to_string(),
                stale: target_state != "active",
                fingerprint,
            }
        })
        .collect();
    // Live findings first (escalated before open), then stale ones in the same order.
    finding_rows.sort_by_key(|f| (f.stale, !f.needs_operator));

    let clears = list(get(status, "inputClears"));
    let orphan_panes = list(get(status, "orphanPanes"))
        .iter()
        .map(|o| {
            format!("{} {}", text_of(o, "agentId"), text_of(o, "paneId"))
                .trim()
                .to_string()
        })
        .collect();
    let cleanup_failed = list(get(status, "cleanupFailed"))
        .iter()
        .map(|c| format!("{}: {}", text_of(c, "agentId"), text_of(c, "reason")))
        .collect();

    let work = list(get(status, "work"))
        .iter()
        .map(|w| {
            let state = text_of(w, "state");
            let work_item_id = text_of(w, "workItemId");
            WorkRow {
                id: work_item_id.clone(),
                work_item_id,
                title: text_of(w, "title"),
                owner: text_or_null(get(w, "owner")),
                fingerprint: state.clone(),
                state,
            }
        })
        .collect();

    let pm_mail = field(status, "pmMail")
        .filter(|v| is_objectish(v))
        .map(|p| PmMailHeader {
            pending: num(get(p, "pending")) as i64,
            oldest_age_seconds: num(get(p, "oldestAgeSeconds")),
            stale: is_true(get(p, "stale")),
        });
    let (full_auto_minutes, grants) = operator_header(get(status, "operator"));
    let pipeline_counts = get(status, "pipelineCounts");
    let working_count = active.iter().filter(|a| a.working).count() as i64;
    let workers = active.iter().filter(|a| is_worker_kind(&a.kind)).count() as i64;
    let unresolved = messages.len() as i64;

    DashModel {
        header: Header {
            tasks: task_index.entries.clone(),
            pm_mail,
            project_id: text_of(status, "projectId"),
            run_state: text_of(run, "state"),
            run_pause: run_pause.map(|r| RunPause {
                paused_at: text_of(r, "pausedAt"),
                reason: text_of(r, "reason"),
                actor_id: text_of(r, "actorId"),
            }),
            supervision,
            workers,
            worker_limit,
            full_auto_minutes,
            grants,
        },
        agents: active
            .into_iter()
            .chain(ended.into_iter().map(|(_, _, a)| a))
            .collect(),
        pipeline: Pipeline {
            reports: stage_of(
                &list(get(status, "reports")),
                "report",
                "reportId",
                label_report,
                true_counts(pipeline_counts, "reports"),
            ),
            reviews: stage_of(
                &list(get(status, "reviews")),
                "review",
                "reviewId",
                label_review,
                true_counts(pipeline_counts, "reviews"),
            ),
            integrations: stage_of(
                &list(get(status, "integrations")),
                "integration",
                "integrationId",
                label_integration,
                true_counts(pipeline_counts, "integrations"),
            ),
        },
        queue: Queue {
            messages,
            truncated: is_true(get(status, "messagesTruncated")),
            input_clears: clears
                .iter()
                .take(3)
                .map(|c| {
                    format!(
                        "message {} {}",
                        slice_units(&text_of(c, "messageId"), 8),
                        text_of(c, "recordedAt")
                    )
                })
                .collect(),
            input_clear_count: clears.len() as i64,
            cleanup_failed,
            orphan_panes,
        },
        findings: finding_rows,
        work,
        waiting: waiting_items(status, pause),
        counts: Counts {
            unresolved,
            working: working_count,
        },
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::super::helpers::visible_agents;
    use super::*;

    const NOW: i64 = 1_790_942_400_000;

    fn iso(seconds_ago: i64) -> String {
        let ms = NOW - seconds_ago * 1000;
        let days = ms.div_euclid(86_400_000);
        let rest = ms.rem_euclid(86_400_000) / 1000;
        // 2026-10-02 is day 20_728 since the epoch.
        let day = 2 + (days - 20_728);
        format!(
            "2026-10-{day:02}T{:02}:{:02}:{:02}.000Z",
            rest / 3600,
            rest % 3600 / 60,
            rest % 60
        )
    }

    fn agent(id: &str, kind: &str, state: &str, ago: i64) -> Value {
        json!({"agentId": id, "roleName": kind.to_lowercase(), "kind": kind, "generation": 1,
               "state": state, "lastActivityAt": iso(ago)})
    }

    fn build(status: Value) -> DashModel {
        build_dash_model(&status, NOW, Some(3))
    }

    fn ids(model: &DashModel) -> Vec<&str> {
        model.agents.iter().map(|a| a.id.as_str()).collect()
    }

    #[test]
    fn malformed_status_never_panics() {
        for status in [
            json!(null),
            json!(1),
            json!("x"),
            json!([]),
            json!({}),
            json!({"agents": "x", "messages": [1, null, "a", []], "run": 5, "pause": "x", "plans": {}, "activeTasks": []}),
            json!({"agents": [null, 1, {"agentId": [1, [2]], "state": {}, "generation": "x"}],
                   "pipelineCounts": {"reports": {"a": -1}, "reviews": [], "integrations": {"x": "y"}},
                   "operator": "x", "pmMail": [], "supervisionState": [],
                   "pause": {"run": "x", "agents": [[], {}]}, "awaitingConfirm": [{}], "pendingProposals": [{}]}),
        ] {
            let model = build_dash_model(&status, NOW, None);
            serde_json::to_value(&model).expect("serialises");
        }
        let empty = build(json!({}));
        assert!(empty.agents.is_empty() && empty.waiting.is_empty());
        assert_eq!(empty.header.supervision, None);
        assert_eq!(empty.header.tasks, None);
    }

    #[test]
    fn working_needs_an_active_agent_with_recent_activity_or_mail_in_flight() {
        let model = build(json!({
            "agents": [agent("a", "Developer", "active", 5), agent("b", "Developer", "active", 31),
                       agent("c", "Developer", "active", 600), agent("d", "Developer", "ended", 1)],
            "messages": [{"messageId": "m", "recipientAgentId": "c", "state": "sent", "queuedAt": iso(9)}],
        }));
        let working: Vec<bool> = model.agents.iter().map(|a| a.working).collect();
        assert_eq!(working, [true, false, true, false]);
        assert_eq!(model.counts.working, 2);
        assert_eq!(model.header.workers, 3);
        assert_eq!(model.agents[2].queue_depth, 1);
    }

    #[test]
    fn unreadable_activity_time_is_not_working() {
        let mut a = agent("a", "Developer", "active", 1);
        a["lastActivityAt"] = json!("soon");
        assert!(!build(json!({"agents": [a]})).agents[0].working);
    }

    #[test]
    fn attention_agents_sort_first_and_the_rest_keep_order() {
        let model = build(json!({
            "agents": [agent("a", "Developer", "active", 1), agent("b", "Developer", "active", 1),
                       agent("c", "Developer", "active", 1)],
            "stalledAgentIds": ["c"], "lostAgentIds": ["b"],
        }));
        assert_eq!(ids(&model), ["b", "c", "a"]);
    }

    #[test]
    fn ended_agents_follow_active_ones_newest_first_unreadable_last() {
        let mut unreadable = agent("u", "Developer", "ended", 1);
        unreadable["lastActivityAt"] = json!("never");
        let model = build(json!({
            "agents": [agent("e1", "Developer", "ended", 500), agent("act", "Developer", "active", 1),
                       agent("e2", "Developer", "ended", 100), unreadable, agent("e3", "Developer", "ended", 100)],
        }));
        // Equal times keep the later row first; the unreadable one sorts last.
        assert_eq!(ids(&model), ["act", "e3", "e2", "e1", "u"]);
    }

    #[test]
    fn ended_filter_shows_the_latest_five_and_every_flagged_one() {
        let mut agents = vec![agent("act", "Developer", "active", 1)];
        for i in 0..8 {
            agents.push(agent(&format!("e{i}"), "Developer", "ended", 100 + i));
        }
        let model = build(json!({"agents": agents, "lostAgentIds": ["e7"]}));
        let shown = visible_agents(&model, false);
        assert_eq!(shown.len(), 1 + 5 + 1);
        assert!(shown.iter().any(|a| a.id == "e7"));
        assert_eq!(visible_agents(&model, true).len(), 9);
    }

    #[test]
    fn a_blocked_agent_has_a_stuck_message_or_an_escalated_finding() {
        let model = build(json!({
            "agents": [agent("a", "Developer", "active", 1), agent("b", "Developer", "active", 1),
                       agent("c", "Developer", "active", 1)],
            "messages": [{"messageId": "m1", "recipientAgentId": "a", "state": "queued"}],
            "stuck": [{"messageId": "m1", "reason": "pane gone"}],
            "agentFindings": [{"findingId": "f", "targetAgentId": "b", "state": "escalated"}],
        }));
        let blocked: Vec<bool> = model.agents.iter().map(|a| a.blocked).collect();
        assert_eq!(blocked, [true, true, false]);
        assert_eq!(
            model.queue.messages[0].problem.as_deref(),
            Some("pane gone")
        );
    }

    #[test]
    fn delivery_problems_come_from_stuck_or_failed_states_and_sort_first() {
        let model = build(json!({
            "messages": [
                {"messageId": "ok", "state": "queued"},
                {"messageId": "bad", "state": "failed", "stateReason": "no pane"},
                {"messageId": "gone", "state": "expired"},
                {"messageId": "late", "state": "unacked", "lastNotifiedAt": iso(5)},
            ],
        }));
        let rows = &model.queue.messages;
        assert_eq!(
            rows.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["bad", "gone", "late", "ok"]
        );
        assert_eq!(rows[0].problem.as_deref(), Some("no pane"));
        assert_eq!(rows[1].problem.as_deref(), Some("expired"));
        assert!(rows[2].notified && !rows[3].notified);
        assert_eq!(model.counts.unresolved, 4);
    }

    #[test]
    fn findings_keep_open_and_escalated_live_before_stale_escalated_before_open() {
        let model = build(json!({
            "agents": [agent("live", "Developer", "active", 1), agent("done", "Developer", "ended", 1)],
            "agentFindings": [
                {"findingId": "f1", "targetAgentId": "done", "state": "escalated"},
                {"findingId": "f2", "targetAgentId": "live", "state": "open"},
                {"findingId": "f3", "targetAgentId": "nobody", "state": "open"},
                {"findingId": "f4", "targetAgentId": "live", "state": "escalated"},
                {"findingId": "f5", "targetAgentId": "live", "state": "closed"},
            ],
        }));
        let rows: Vec<(&str, &str, bool)> = model
            .findings
            .iter()
            .map(|f| (f.id.as_str(), f.target_state.as_str(), f.stale))
            .collect();
        assert_eq!(
            rows,
            [
                ("f4", "active", false),
                ("f2", "active", false),
                ("f1", "ended", true),
                ("f3", "unknown", true)
            ]
        );
    }

    #[test]
    fn pipeline_totals_use_true_counts_and_fall_back_to_the_capped_list() {
        let reports: Vec<Value> = (0..25)
            .map(|i| json!({"reportId": format!("r{i}"), "agentId": "d", "state": "accepted", "commitSha": "0123456789", "createdAt": iso(i)}))
            .collect();
        let model = build(json!({
            "reports": reports,
            "reviews": [{"reviewId": "v", "state": "started", "reviewerAgentId": "rv", "authorAgentId": "d", "round": 2, "createdAt": iso(1)}],
            "pipelineCounts": {"reports": {"accepted": 120, "pending": 0, "failed": 3.9}, "reviews": {"a": -1}},
        }));
        let r = &model.pipeline.reports;
        assert_eq!((r.total, r.capped, r.items.len()), (123, false, 20));
        assert_eq!(r.counts.get("accepted"), Some(&120));
        assert_eq!(r.counts.get("failed"), Some(&3));
        assert!(!r.counts.contains_key("pending"));
        assert_eq!(r.items[0].label, "d 0123456");
        assert_eq!(r.items[0].id, "report:r0");
        let v = &model.pipeline.reviews;
        assert_eq!((v.total, v.capped), (1, false));
        assert_eq!(v.items[0].label, "rv -> d r2");
        assert_eq!(v.items[0].fingerprint, "started|rv -> d r2");
        let i = &model.pipeline.integrations;
        assert_eq!((i.total, i.capped, i.items.len()), (0, false, 0));
        let fallback = build(
            json!({"reports": (0..20).map(|i| json!({"reportId": i, "state": "x"})).collect::<Vec<_>>()}),
        );
        assert!(fallback.pipeline.reports.capped);
        assert_eq!(fallback.pipeline.reports.items[0].label, " -");
    }

    #[test]
    fn an_integration_review_names_the_integration() {
        let model = build(json!({
            "agents": [agent("rv", "Verifier", "active", 1)],
            "reviews": [{"reviewId": "v", "state": "started", "reviewerAgentId": "rv", "integrationId": "0123456789abcdef", "round": 1}],
        }));
        assert_eq!(model.pipeline.reviews.items[0].label, "rv -> 01234567 r1");
        assert_eq!(model.agents[0].task, "review 01234567 r1");
    }

    #[test]
    fn header_carries_run_pause_pm_mail_full_auto_and_grants() {
        let model = build(json!({
            "projectId": "p", "run": {"state": "paused"},
            "pause": {"run": {"pausedAt": iso(60), "reason": "ops", "actorId": "me"}, "agents": [{"agentId": "d", "pausedAt": iso(10), "reason": "x"}]},
            "agents": [agent("pm", "PM", "active", 1), agent("d", "Developer", "active", 1), agent("e", "Developer", "active", 1)],
            "pmMail": {"pending": 2, "oldestAgeSeconds": 12.5, "stale": true},
            "operator": {"fullAuto": {"on": true, "remainingSeconds": 61}, "grants": [{}, {}, 3]},
        }));
        let h = &model.header;
        assert_eq!(
            (h.project_id.as_str(), h.run_state.as_str()),
            ("p", "paused")
        );
        assert_eq!(h.run_pause.as_ref().map(|r| r.reason.as_str()), Some("ops"));
        assert_eq!(
            h.pm_mail,
            Some(PmMailHeader {
                pending: 2,
                oldest_age_seconds: 12.5,
                stale: true
            })
        );
        assert_eq!((h.full_auto_minutes, h.grants), (Some(2), Some(2)));
        let paused: Vec<bool> = model.agents.iter().map(|a| a.paused_at.is_some()).collect();
        // The PM is not paused by the run pause; `d` is paused on its own; `e` by the run.
        assert_eq!(paused, [false, true, true]);
        assert_eq!(h.workers, 2);
        assert_eq!(h.worker_limit, Some(3));
    }

    #[test]
    fn text_loses_control_characters_and_non_strings_print_like_javascript() {
        let model = build(
            json!({"projectId": "a\u{1b}b\u{202e}c", "work": [{"workItemId": 7, "title": [1, null, "x"], "owner": true}]}),
        );
        assert_eq!(model.header.project_id, "a b c");
        assert_eq!(model.work[0].id, "7");
        assert_eq!(model.work[0].title, "1,,x");
        assert_eq!(model.work[0].owner.as_deref(), Some("true"));
    }

    #[test]
    fn leftovers_are_carried_through() {
        let model = build(json!({
            "messagesTruncated": true,
            "inputClears": [{"messageId": "0123456789", "recordedAt": "t1"}, {}, {}, {}],
            "cleanupFailed": [{"agentId": "a", "reason": "r"}],
            "orphanPanes": [{"agentId": "", "paneId": "p9"}],
        }));
        let q = &model.queue;
        assert!(q.truncated);
        assert_eq!((q.input_clears.len(), q.input_clear_count), (3, 4));
        assert_eq!(q.input_clears[0], "message 01234567 t1");
        assert_eq!(q.cleanup_failed, ["a: r"]);
        assert_eq!(q.orphan_panes, ["p9"]);
    }

    mod tasks {
        use super::*;

        fn active_tasks() -> Value {
            json!({
                "agents": [agent("arch", "Architect", "active", 1), agent("dev", "Developer", "active", 1),
                           agent("pane", "Developer", "active", 1), agent("req", "Developer", "active", 1),
                           agent("none", "Developer", "active", 1), agent("old", "Developer", "ended", 1)],
                "activeTasks": {
                    "plans": [{"planId": "plan-1", "title": "Ship", "done": 2.7, "total": 5, "architectAgentId": "arch",
                               "packages": [{"packageId": "ui", "nexoraId": "PM-1", "title": "UI", "assigneeAgentId": "dev"}, {"packageId": ""}]},
                              {"planId": "plan-2", "architectAgentId": "arch"}, {"planId": ""}],
                    "requirements": [{"refId": "r1", "title": "Req", "agentIds": ["req", null]}, {"nexoraId": "PM-9"}, {}],
                },
                "panes": [{"agentId": "pane", "paneId": "p1", "taskRef": "plan-1/ui"}, {"agentId": "none", "paneId": "p2", "taskRef": "x", "taskTitle": "T"}],
            })
        }

        #[test]
        fn active_tasks_give_header_entries_and_every_agents_task() {
            let model = build(active_tasks());
            let entries: Vec<(&str, &str)> = model
                .header
                .tasks
                .as_ref()
                .unwrap()
                .iter()
                .map(|t| (t.id.as_str(), t.label.as_str()))
                .collect();
            assert_eq!(
                entries,
                [
                    ("plan-1", "plan-1 Ship (2/5)"),
                    ("plan-2", "plan-2"),
                    ("r1", "r1 Req"),
                    ("PM-9", "PM-9")
                ]
            );
            let tasks: Vec<&str> = model.agents.iter().map(|a| a.task.as_str()).collect();
            assert_eq!(
                tasks,
                [
                    "plan-1 · plan +1",
                    "plan-1/ui PM-1 UI",
                    "plan-1/ui PM-1 UI",
                    "r1 Req",
                    "x T",
                    "-"
                ]
            );
        }

        #[test]
        fn empty_active_tasks_is_idle_not_missing() {
            assert_eq!(build(json!({"activeTasks": {}})).header.tasks, Some(vec![]));
            assert_eq!(build(json!({"activeTasks": []})).header.tasks, None);
        }

        #[test]
        fn plans_alone_fall_back_to_open_plans_and_pane_fields_win_over_the_architect() {
            let model = build(json!({
                "agents": [agent("arch", "Architect", "active", 1)],
                "plans": [
                    {"planId": "p1", "title": "A", "state": "approved", "architectAgentId": "arch", "packages": {"reviewed": 1, "integrated": 1, "draft": 3, "cancelled": 9}},
                    {"planId": "p2", "state": "draft", "cancelled": true},
                    {"planId": "p3", "state": "done"},
                    {"planId": "p4", "state": "draft", "signoffs": [{}]},
                ],
                "panes": [{"agentId": "arch", "taskRef": "r7"}],
            }));
            let entries = model.header.tasks.as_ref().unwrap();
            assert_eq!(
                (entries.len(), entries[0].label.as_str()),
                (1, "p1 A (2/5)")
            );
            assert_eq!(model.agents[0].task, "r7");
            let no_pane = build(
                json!({"agents": [agent("arch", "Architect", "active", 1)], "plans": [{"planId": "p1", "state": "draft", "architectAgentId": "arch"}]}),
            );
            assert_eq!(no_pane.agents[0].task, "p1 · plan");
        }

        #[test]
        fn no_open_plan_hides_the_header() {
            assert_eq!(
                build(json!({"plans": [{"planId": "p", "state": "done"}]}))
                    .header
                    .tasks,
                None
            );
            assert_eq!(build(json!({"plans": []})).header.tasks, None);
        }

        #[test]
        fn malformed_task_data_falls_back_without_panicking() {
            let model = build(json!({
                "agents": [agent("a", "Developer", "active", 1)],
                "activeTasks": {"plans": [7, {"planId": 5, "done": "x", "total": null, "packages": "x"}, {"planId": "p", "packages": [1, {"packageId": 3}]}],
                                "requirements": "x"},
                "panes": [{"agentId": "a", "taskRef": {"x": 1}}],
            }));
            let entries = model.header.tasks.unwrap();
            assert_eq!(
                entries.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
                ["5", "p"]
            );
            assert_eq!(model.agents[0].task, "[object Object]");
        }

        #[test]
        fn a_task_change_changes_the_row_fingerprint() {
            let a = build(active_tasks());
            let mut changed = active_tasks();
            changed["activeTasks"]["plans"][0]["packages"][0]["title"] = json!("Other");
            let b = build(changed);
            assert_ne!(a.agents[1].fingerprint, b.agents[1].fingerprint);
        }

        #[test]
        fn plan_labels_lose_control_characters() {
            let model =
                build(json!({"activeTasks": {"plans": [{"planId": "p", "title": "a\nb\u{7}c"}]}}));
            assert_eq!(model.header.tasks.unwrap()[0].label, "p a b c");
        }
    }

    mod waiting {
        use super::*;

        #[test]
        fn confirms_proposals_and_pauses_wait_in_that_order() {
            let model = build(json!({
                "awaitingConfirm": [{"integrationId": "0123456789abcdef", "createdAt": iso(200), "reviewState": "passed"}, {"integrationId": "ab"}],
                "pendingProposals": [{"proposalId": "p-7", "kind": "add-role", "proposer": "pm-1", "reason": "need", "createdAt": iso(40)}],
                "pause": {"run": {"reason": "ops", "pausedAt": iso(90)}, "agents": [{"agentId": "d", "pausedAt": iso(9), "reason": "stalled"}]},
            }));
            let w: Vec<(&str, &str, Option<&str>)> = model
                .waiting
                .iter()
                .map(|i| (i.id.as_str(), i.label.as_str(), i.since.as_deref()))
                .collect();
            assert_eq!(w.len(), 5);
            assert_eq!(w[0].0, "integration:0123456789abcdef");
            assert_eq!(
                w[0].1,
                "integration 01234567 merged, review passed: cstan integrate confirm pending"
            );
            assert_eq!(
                w[1].1,
                "integration ab merged, review none: cstan integrate confirm pending"
            );
            assert_eq!(w[1].2, None);
            assert_eq!(
                (w[2].0, w[2].1),
                ("proposal:p-7", "proposal p-7 add-role by pm-1: need")
            );
            assert_eq!((w[3].0, w[3].1), ("run-paused", "run paused: ops"));
            assert_eq!((w[4].0, w[4].1), ("agent-paused:d", "d paused: stalled"));
            assert_eq!(model.waiting[3].kind, WaitingKind::RunPaused);
        }

        #[test]
        fn nothing_waiting_is_empty_and_malformed_sources_add_nothing() {
            assert!(build(json!({})).waiting.is_empty());
            let model = build(json!({
                "awaitingConfirm": "x", "pendingProposals": {"a": 1}, "pause": {"run": "x", "agents": 3},
            }));
            assert!(model.waiting.is_empty());
            let nulls = build(
                json!({"awaitingConfirm": [null, 1], "pause": {"run": null, "agents": [null]}}),
            );
            assert!(nulls.waiting.is_empty());
        }
    }
}
