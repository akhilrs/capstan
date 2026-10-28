//! The paged snapshot (`areas::snapshot_feed`): the waiting items, agents, plans and run with their request ids and content
//! hashes, the bounds of a page, and the convergence of a model that applies the effects of the feed events to the snapshot.
//! Every ledger is built through the kernel's mutation functions on a `SeededEnv` kernel.

use crate::event_feed::{prompt_sha, Rig, BASE, COMMIT};
use capstan_kernel::areas::event_feed::{item_key, parse_item_key, KEY_KINDS};
use capstan_kernel::areas::snapshot_feed::{
    agent_content_hash, agents_page, agents_page_counted, content_hash, content_hash_counted,
    design_hash_input, message_hash_input, plans_page, snapshot_head, snapshot_item, waiting_page,
    waiting_page_counted, AgentItem, Page, PlanItem, WaitingItem, MAX_PAGE_BYTES,
};
use capstan_kernel::canonical::sha256;
use capstan_kernel::errors::KernelError;
use capstan_kernel::Core;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;

// ------------------------------------------------------------------------------------------------------------- model

/// What a client holds: the run and the items of each list, by item key.
#[derive(Clone, Debug, Default, PartialEq)]
struct Model {
    run: Value,
    agents: BTreeMap<String, Value>,
    plans: BTreeMap<String, Value>,
    waiting: BTreeMap<String, Value>,
}

fn value_of<T: serde::Serialize>(item: &T) -> Value {
    serde_json::to_value(item).unwrap()
}

/// Every page of a list, following `next`: the calls made and the items in order.
fn read_all<T: Clone>(
    mut call: impl FnMut(Option<&str>) -> Page<T>,
) -> (usize, Vec<T>, Vec<Option<String>>) {
    let mut after: Option<String> = None;
    let mut items = Vec::new();
    let mut nexts = Vec::new();
    let mut calls = 0;
    loop {
        let page = call(after.as_deref());
        calls += 1;
        items.extend(page.items.iter().cloned());
        nexts.push(page.next.clone());
        match page.next {
            Some(next) => after = Some(next),
            None => return (calls, items, nexts),
        }
        assert!(calls < 10_000, "paging does not end");
    }
}

fn all_waiting(core: &Core, kind: Option<&str>) -> Vec<WaitingItem> {
    read_all(|after| waiting_page(core, after, 100, kind).unwrap()).1
}

fn all_agents(core: &Core) -> Vec<AgentItem> {
    read_all(|after| agents_page(core, after, 100).unwrap()).1
}

fn all_plans(core: &Core) -> Vec<PlanItem> {
    read_all(|after| plans_page(core, after, 100).unwrap()).1
}

fn keyed<T: serde::Serialize>(items: &[T]) -> BTreeMap<String, Value> {
    items
        .iter()
        .map(|item| {
            let value = value_of(item);
            (value["key"].as_str().unwrap().to_string(), value)
        })
        .collect()
}

/// A fresh snapshot: the run and every page of every list.
fn fresh(core: &Core) -> Model {
    Model {
        run: value_of(&snapshot_head(core).unwrap().run),
        agents: keyed(&all_agents(core)),
        plans: keyed(&all_plans(core)),
        waiting: keyed(&all_waiting(core, None)),
    }
}

/// The items of a whole kind of a section (what a bare-kind invalidate asks for).
fn kind_items(core: &Core, section: &str, kind: &str) -> Vec<Value> {
    match section {
        "waiting" => all_waiting(core, Some(kind)).iter().map(value_of).collect(),
        "plans" => all_plans(core).iter().map(value_of).collect(),
        other => panic!("no bare kind {kind} in {other}"),
    }
}

fn count(model: &Model, kind: &str) -> usize {
    model.waiting.values().filter(|w| w["kind"] == kind).count()
}

fn is_kind_key(section: &str, key: &str) -> bool {
    KEY_KINDS.iter().any(|kind| {
        kind.section == section && kind.kind == key && section != "agents" && section != "run"
    })
}

/// Folds `body.effects` of `events` into `model`, reading nothing else of the events: an `invalidate` of an item asks
/// `read_item` for it (`None` removes it), one of a bare kind asks `read_kind` for every item of the kind, an `upsert` of
/// an agent builds the item from its fields. Returns the `(section, key)` of every invalidate.
fn apply_effects(
    model: &mut Model,
    events: &[Value],
    read_item: &dyn Fn(&str, &str) -> Option<Value>,
    read_kind: &dyn Fn(&str, &str) -> Vec<Value>,
) -> Vec<(String, String)> {
    let mut invalidated = Vec::new();
    for event in events {
        for effect in event["body"]["effects"].as_array().unwrap_or(&Vec::new()) {
            let section = effect["section"].as_str().unwrap();
            let key = effect["key"].as_str().unwrap();
            match effect["op"].as_str().unwrap() {
                "upsert" => {
                    assert_eq!(section, "agents");
                    let fields = &effect["fields"];
                    let id = fields["agentId"].as_str().unwrap();
                    let hash = agent_content_hash(
                        id,
                        fields["generation"].as_i64().unwrap(),
                        fields["state"].as_str().unwrap(),
                        fields["paused"].as_bool().unwrap(),
                        None,
                    );
                    model.agents.insert(
                        key.to_string(),
                        json!({
                            "key": key, "agentId": id, "kind": fields["kind"], "role": fields["role"],
                            "generation": fields["generation"], "state": fields["state"],
                            "paused": fields["paused"], "task": null,
                            "requestId": format!("agent:{id}"), "contentHash": hash,
                        }),
                    );
                }
                "invalidate" => {
                    invalidated.push((section.to_string(), key.to_string()));
                    if is_kind_key(section, key) {
                        let items = read_kind(section, key);
                        let map = match section {
                            "waiting" => &mut model.waiting,
                            "plans" => &mut model.plans,
                            _ => unreachable!(),
                        };
                        if section == "waiting" {
                            map.retain(|k, _| !k.starts_with(&format!("{key}:")));
                        } else {
                            map.clear();
                        }
                        for item in items {
                            map.insert(item["key"].as_str().unwrap().to_string(), item);
                        }
                        continue;
                    }
                    let read = read_item(section, key);
                    if section == "run" {
                        model.run = read.expect("the run is always listed");
                        continue;
                    }
                    let map = match section {
                        "waiting" => &mut model.waiting,
                        "agents" => &mut model.agents,
                        "plans" => &mut model.plans,
                        other => panic!("unknown section {other}"),
                    };
                    match read {
                        Some(item) => {
                            map.insert(key.to_string(), item);
                        }
                        None => {
                            map.remove(key);
                        }
                    }
                }
                other => panic!("unknown op {other}"),
            }
        }
    }
    invalidated
}

/// Runs one mutation (write-once columns checked by `Rig::run`), applies the effects of its events to the model and
/// checks the model against a fresh snapshot.
fn step<R>(
    rig: &Rig,
    model: &mut Model,
    invalidated: &mut Vec<(String, String)>,
    label: &str,
    mutation: impl FnOnce(&Rig) -> R,
) -> (R, Vec<Value>) {
    let (result, events) = rig.run(mutation);
    settle(rig, model, invalidated, label, &events);
    (result, events)
}

fn settle(
    rig: &Rig,
    model: &mut Model,
    invalidated: &mut Vec<(String, String)>,
    label: &str,
    events: &[Value],
) {
    let core = &rig.core;
    invalidated.extend(apply_effects(
        model,
        events,
        &|section, key| snapshot_item(core, section, key).unwrap(),
        &|section, kind| kind_items(core, section, kind),
    ));
    assert_eq!(
        *model,
        fresh(core),
        "the model differs from a snapshot after {label}"
    );
}

// ------------------------------------------------------------------------------------------------------ the scripted run

const TIMERS: fn() -> Value = || {
    json!({
        "maxDeferralSeconds": 0.001, "maxBusyDeferralSeconds": 0.001, "pmAckTimeoutSeconds": 0.001,
        "pmNotifyAfterSeconds": 0.001, "notifyIntervalSeconds": 0.001, "stallAfterSeconds": 0.001,
        "workerAckTimeoutSeconds": 0.001, "pmWakeAfterSeconds": 0.001, "pmWakeIntervalSeconds": 0.001,
    })
};

#[test]
fn a_model_that_applies_the_effects_equals_a_fresh_snapshot_after_every_step_of_a_run() {
    let mut rig = Rig::new();
    let mut model = fresh(&rig.core);
    let mut invalidated = Vec::new();
    let m = &mut model;
    let i = &mut invalidated;

    // An agent_pane task write.
    step(&rig, m, i, "agent_pane task write", |r| {
        r.owner(
            "recordAgentPane",
            json!([{"agentId": "dev-1", "workspaceId": "ws-1", "paneId": "pane-dev-1", "worktreePath": "/tmp/wt-dev-1",
                "branch": "feat/a", "baseSha": BASE, "taskRef": "PM-1", "taskTitle": "Build the \"first\" thing"}]),
        );
    });
    assert_eq!(m.agents["dev-1"]["task"], "Build the \"first\" thing");
    // An agent replacement.
    step(&rig, m, i, "agent replacement", |r| {
        r.core
            .replace_agent_generation(&r.ctx("owner"), "rev-1")
            .unwrap();
    });
    assert_eq!(m.agents["rev-1"]["generation"], 2);
    // A message: queued, sent, acked.
    let (id, _) = step(&rig, m, i, "message enqueue", |r| {
        r.enqueue("dev-1", "please look", true)
    });
    assert_eq!(m.waiting.len(), 1);
    step(&rig, m, i, "message sent", |r| {
        r.core.record_sent(&r.ctx("owner"), &id).unwrap()
    });
    step(&rig, m, i, "message ack", |r| {
        r.core.ack_message(&r.ctx("dev-1"), &id).unwrap()
    });
    assert!(m.waiting.is_empty());
    // A message resolved with cancel.
    let (second, _) = step(&rig, m, i, "second message", |r| {
        r.enqueue("dev-2", "second", true)
    });
    step(&rig, m, i, "message.resolve cancel", |r| {
        r.core
            .resolve_message(
                &r.ctx("owner"),
                &second,
                &json!("cancel"),
                Some("no longer needed"),
            )
            .unwrap()
    });
    assert!(m.waiting.is_empty());
    // The run paused and resumed.
    step(&rig, m, i, "run pause", |r| {
        r.core
            .transition_run(&r.ctx("owner"), &json!("paused"), Some("maintenance"))
            .unwrap();
    });
    assert_eq!(m.run["paused"], true);
    step(&rig, m, i, "run resume", |r| {
        r.core
            .transition_run(&r.ctx("owner"), &json!("active"), Some("back"))
            .unwrap();
    });
    assert_eq!(m.run["paused"], false);
    // An agent registered (three mutations, one public event), then paused and resumed.
    let head = rig.head();
    let before = rig.write_once_state();
    rig.add_agent("dev-3", "developer", "Developer");
    let events = rig.since(head);
    for (key, columns) in &before {
        assert_eq!(rig.write_once_state().get(key), Some(columns));
    }
    settle(&rig, m, i, "agent.register", &events);
    assert_eq!(m.agents["dev-3"]["paused"], false);
    step(&rig, m, i, "agent pause", |r| {
        r.core
            .pause_agent(
                &r.ctx("pm-1"),
                &json!({"agentId": "dev-3", "reason": "hold on"}),
            )
            .unwrap();
    });
    assert_eq!(m.agents["dev-3"]["paused"], true);
    step(&rig, m, i, "agent resume", |r| {
        r.core
            .resume_agent(
                &r.ctx("pm-1"),
                &json!({"agentId": "dev-3", "reason": "go on"}),
            )
            .unwrap();
    });
    // A proposal proposed and decided; another cancelled.
    let (proposal, _) = step(&rig, m, i, "propose", |r| r.propose("ls -la", "look"));
    assert!(m.waiting.contains_key("op:1"));
    step(&rig, m, i, "decide", |r| r.decide(&proposal, "approve"));
    assert_eq!(count(m, "op"), 0);
    // A capture superseded by the next.
    rig.relay_on();
    step(&rig, m, i, "capture", |r| {
        r.capture("dev-1", "Allow Bash(ls)?")
    });
    step(&rig, m, i, "superseding capture", |r| {
        r.capture("dev-1", "Allow Bash(pwd)?")
    });
    assert_eq!(count(m, "prompt"), 1);
    // A batch expiry.
    rig.core
        .configure_prompt_relay(&json!({"enabled": true, "captureTtlSeconds": 0.001}))
        .unwrap();
    step(&rig, m, i, "second capture", |r| {
        r.capture("dev-2", "Allow dev-2?")
    });
    step(&rig, m, i, "third capture", |r| {
        r.capture("dev-3", "Allow dev-3?")
    });
    step(&rig, m, i, "batch expiry", |r| {
        r.core.expire_prompt_captures(&r.ctx("owner")).unwrap()
    });
    assert_eq!(
        count(m, "prompt"),
        1,
        "only the capture of dev-1 (long ttl) is left"
    );
    // A message that expires.
    let (late, _) = step(&rig, m, i, "message to expire", |r| {
        r.enqueue("dev-2", "waits", true)
    });
    step(&rig, m, i, "defer", |r| {
        r.core
            .record_deferral(&r.ctx("owner"), &late, &json!("agent_busy"))
            .unwrap()
    });
    step(&rig, m, i, "message expiry", |r| {
        r.core
            .advance_messaging(&r.ctx("owner"), &TIMERS())
            .unwrap();
    });
    assert!(m
        .waiting
        .values()
        .any(|w| w["requestId"] == format!("message:{late}")));
    // A design package: plan, assign, report, decision.
    rig.pane("dev-2", "feat/ui");
    let (plan, _) = step(&rig, m, i, "plan open and submit", |r| r.approved_plan());
    assert!(m.plans.contains_key("1"));
    step(&rig, m, i, "plan assign", |r| {
        r.assign(&plan, "ui", "dev-2")
    });
    assert_eq!(m.plans["1"]["packages"]["assigned"], 1);
    step(&rig, m, i, "design report", |r| {
        r.report_as("dev-2", "feat/ui", COMMIT, "designed")
    });
    assert!(m.waiting.contains_key("design:1/ui"));
    assert_eq!(m.plans["1"]["packages"]["reported"], 1);
    step(&rig, m, i, "design decision", |r| {
        r.core
            .record_design_decision(
                &r.ctx("pm-1"),
                &json!({"planId": plan, "packageId": "ui", "decision": "approved", "link": "https://example.test/ui", "askedUser": "ok?"}),
            )
            .unwrap()
    });
    assert!(!m.waiting.contains_key("design:1/ui"));
    step(&rig, m, i, "second assign", |r| {
        r.assign(&plan, "docs", "dev-3")
    });
    assert_eq!(m.plans["1"]["packages"]["assigned"], 2);
    // An agent ended.
    step(&rig, m, i, "agent end", |r| {
        r.core.end_agent(&r.ctx("owner"), "dev-3", None).unwrap();
    });
    assert!(!m.agents.contains_key("dev-3"));

    // Every invalidate key the feed emitted resolves: an item key through snapshot_item, a bare kind through KEY_KINDS.
    assert!(!invalidated.is_empty());
    for (section, key) in &invalidated {
        if is_kind_key(section, key) {
            continue;
        }
        snapshot_item(&rig.core, section, key)
            .unwrap_or_else(|e| panic!("{section}/{key} does not resolve: {e}"));
    }
    // Every ledger row of an entity of the snapshot yields an effect (ordinary messages are not in it).
    let events = rig.since(0);
    for (seq, entity, action, _from, _to) in
        capstan_kernel::areas::event_feed::ledger_rows(&rig.core).unwrap()
    {
        let in_snapshot = matches!(
            entity.as_str(),
            "prompt_relay"
                | "operator_proposal"
                | "agent"
                | "agent_pane"
                | "run_control"
                | "integration"
                | "agent_report"
        ) || (entity == "plan" && action == "design.decide");
        let skipped = capstan_kernel::areas::event_feed::SKIPPED_ACTIONS.contains(&action.as_str());
        if !in_snapshot || skipped {
            continue;
        }
        let covered = events.iter().any(|e| {
            e["seq"].as_i64() == Some(seq)
                && e["body"]["effects"]
                    .as_array()
                    .is_some_and(|list| !list.is_empty())
        });
        assert!(
            covered,
            "ledger row {seq} ({entity} {action}) yields no effect"
        );
    }
    // Agents hold a key built from KEY_KINDS and no stalled field.
    for agent in m.agents.values() {
        assert!(agent.get("stalled").is_none());
    }
}

#[test]
fn a_merged_integration_waits_and_moves_the_integrated_count_of_its_plan() {
    let rig = Rig::new();
    let mut model = fresh(&rig.core);
    let mut invalidated = Vec::new();
    let (m, i) = (&mut model, &mut invalidated);
    step(&rig, m, i, "pane", |r| r.pane("dev-1", "feat/only"));
    let plan = rig.approved_plan_of(
        r#"{"summary":"s","packages":[
        {"id":"only","title":"Only","owns":["src/only"],"acceptance":["works"],"estimate_hours":1}],
        "risks":[],"integration_order":["only"]}"#,
    );
    settle(&rig, m, i, "plan", &rig.since(0));
    step(&rig, m, i, "assign", |r| r.assign(&plan, "only", "dev-1"));
    let (report, _) = step(&rig, m, i, "report", |r| {
        r.report_as("dev-1", "feat/only", COMMIT, "work written")
    });
    let report_id = report["record"]["reportId"].as_str().unwrap().to_string();
    step(&rig, m, i, "review", |r| {
        r.review(&report_id, "pass", "fine")
    });
    step(&rig, m, i, "begin", |r| {
        r.begin_integration("int-1", &[&report_id])
    });
    assert!(
        m.waiting.values().all(|w| w["kind"] != "integration"),
        "a running integration does not wait"
    );
    step(&rig, m, i, "finish", |r| {
        r.finish_integration(
            "int-1",
            json!({"kind": "merged", "headSha": "3".repeat(40)}),
        )
    });
    assert_eq!(m.waiting["integration:1"]["requestId"], "integration:int-1");
    assert_eq!(m.plans["1"]["packages"]["integrated"], 1);
    let item = &m.waiting["integration:1"];
    assert_eq!(
        content_hash(&rig.core, "integration:int-1")
            .unwrap()
            .as_deref(),
        item["contentHash"].as_str()
    );
    step(&rig, m, i, "discard", |r| {
        r.core
            .settle_integration(
                &r.ctx("owner"),
                &json!({"integrationId": "int-1", "outcome": "discarded"}),
            )
            .unwrap()
    });
    assert!(m.waiting.values().all(|w| w["kind"] != "integration"));
    assert_eq!(m.plans["1"]["packages"]["integrated"], 0);
    assert_eq!(content_hash(&rig.core, "integration:int-1").unwrap(), None);
    assert_eq!(
        snapshot_item(&rig.core, "waiting", "integration:1").unwrap(),
        None
    );
}

// --------------------------------------------------------------------------------------------------------------- bounds

#[test]
fn an_agent_with_2500_pauses_is_paged_and_hashed_within_the_row_budget() {
    let rig = Rig::new();
    let db = rig.db();
    let (project, actor): (String, String) = db
        .query_row(
            "SELECT project_id, actor_id FROM agents WHERE agent_id = 'pm-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    db.execute_batch("BEGIN").unwrap();
    for n in 0..2500 {
        db.execute(
            "INSERT INTO pauses(project_id, pause_id, scope, agent_id, reason, actor_id, paused_at, resumed_at, resume_reason, resumed_by)
             VALUES (?, ?, 'agent', 'dev-1', 'r', ?, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:01.000Z', 'back', ?)",
            rusqlite::params![project, format!("old-{n:05}"), actor, actor],
        )
        .unwrap();
    }
    db.execute_batch("COMMIT").unwrap();
    drop(db);
    rig.core
        .pause_agent(
            &rig.ctx("pm-1"),
            &json!({"agentId": "dev-1", "reason": "now"}),
        )
        .unwrap();
    let (page, reads) = agents_page_counted(&rig.core, None, 100, 2000).unwrap();
    assert!(reads <= 2000, "{reads}");
    let dev = page.items.iter().find(|a| a.agent_id == "dev-1").unwrap();
    assert!(dev.paused);
    let (hash, lookups) = content_hash_counted(&rig.core, "agent:dev-1").unwrap();
    assert_eq!(hash.as_deref(), Some(dev.content_hash.as_str()));
    assert!(
        lookups <= 2,
        "the hash is one pause lookup beside the agent row: {lookups}"
    );
}

#[test]
fn a_waiting_page_stops_at_the_row_budget_and_at_the_item_limit_and_resumes_after_the_last_row_examined(
) {
    let rig = Rig::new();
    for n in 0..2500 {
        rig.enqueue("dev-1", &format!("plain {n}"), false);
    }
    let wanted: Vec<String> = (0..150)
        .map(|n| rig.enqueue("dev-2", &format!("act {n}"), true))
        .collect();
    let (first, reads) = waiting_page_counted(&rig.core, None, 100, None, 2000).unwrap();
    // The probes for the end of the list are inside the budget: 2000 reads in all, the rest examined rows.
    assert!((1960..=2000).contains(&reads), "{reads}");
    assert_eq!(first.items.len(), 0);
    let examined: i64 = first
        .next
        .as_deref()
        .unwrap()
        .strip_prefix("message:")
        .unwrap()
        .parse()
        .unwrap();
    assert!((1960..=2000).contains(&examined), "{examined}");
    // A mutation between two calls is served by the next one, which continues from `next`.
    let late = rig.enqueue("dev-2", "late", true);
    let second = waiting_page(&rig.core, first.next.as_deref(), 100, None).unwrap();
    assert_eq!(second.items.len(), 100);
    assert!(second.next.is_some());
    let third = waiting_page(&rig.core, second.next.as_deref(), 100, None).unwrap();
    assert_eq!(third.items.len(), 51, "50 and the late message");
    assert_eq!(third.next, None);
    let mut seen: Vec<String> = second
        .items
        .iter()
        .chain(&third.items)
        .map(|i| i.request_id.clone())
        .collect();
    assert_eq!(seen.len(), 151);
    assert_eq!(seen.last().unwrap(), &format!("message:{late}"));
    seen.pop();
    let want: Vec<String> = wanted.iter().map(|id| format!("message:{id}")).collect();
    assert_eq!(seen, want);
    // A page that ends in a skipped candidate resumes exactly after it: each item once overall.
    let (calls, items, _) = read_all(|after| waiting_page(&rig.core, after, 100, None).unwrap());
    assert_eq!(items.len(), 151);
    assert_eq!(calls, 3);
}

#[test]
fn three_hundred_waiting_agents_and_eighty_plans_of_maximum_text_page_in_order_within_the_byte_budget(
) {
    let mut rig = Rig::new();
    let quotes = "\"".repeat(1500);
    let slashes = "\\".repeat(1500);
    let mut ids = Vec::new();
    let proposal = rig.propose(&quotes, &slashes[..1000]);
    for n in 0..299 {
        let text = if n % 2 == 0 { &quotes } else { &slashes };
        let to = ["dev-1", "dev-2", "arch-1"][n / 100];
        ids.push(rig.enqueue(to, text, true));
    }
    for n in 0..295 {
        rig.add_agent(&format!("w{n:03}"), "developer", "Developer");
    }
    for n in 0..80 {
        let title: String = if n % 2 == 0 {
            "\"".repeat(200)
        } else {
            "\\".repeat(200)
        };
        rig.core
            .open_plan(&rig.ctx("pm-1"), &json!({"tier": "normal", "title": title}))
            .unwrap();
    }
    for n in 0..300 {
        let task = if n % 2 == 0 {
            "\"".repeat(200)
        } else {
            "\\".repeat(200)
        };
        let agent = if n < 5 {
            ["pm-1", "dev-1", "dev-2", "arch-1", "rev-1"][n].to_string()
        } else {
            format!("w{:03}", n - 5)
        };
        rig.owner(
            "recordAgentPane",
            json!([{"agentId": agent, "workspaceId": "ws-1", "paneId": format!("p{n}"), "worktreePath": "/tmp/wt",
                "branch": format!("b{n}"), "baseSha": BASE, "taskRef": format!("T-{n}"), "taskTitle": task}]),
        );
    }
    let bytes = |page: &Value| serde_json::to_string(page).unwrap().len();
    // waiting
    let mut calls = 0;
    let mut after: Option<String> = None;
    let mut keys = Vec::new();
    loop {
        let page = waiting_page(&rig.core, after.as_deref(), 100, None).unwrap();
        assert!(bytes(&value_of(&page)) <= MAX_PAGE_BYTES);
        calls += 1;
        keys.extend(page.items.iter().map(|i| i.key.clone()));
        // An item decided between the pages hides or repeats no other: the first message of arch-1 is still ahead.
        if calls == 1 {
            rig.core
                .resolve_message(&rig.ctx("owner"), &ids[200], &json!("cancel"), Some("gone"))
                .unwrap();
        }
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    assert_eq!(calls, 3);
    // 299 messages, the proposal and the notice it queued for the PM; the one cancelled between pages is gone.
    assert_eq!(
        keys.len(),
        300,
        "the one decided between pages is gone, no other"
    );
    let _ = proposal;
    let mut sorted = keys.clone();
    sorted.sort_by_key(|k| k.rsplit(':').next().unwrap().parse::<i64>().unwrap());
    assert_eq!(keys, sorted);
    let mut unique = keys.clone();
    unique.dedup();
    assert_eq!(unique, keys);
    // agents
    let (calls, agents, _) = read_all(|after| {
        let page = agents_page(&rig.core, after, 100).unwrap();
        assert!(bytes(&value_of(&page)) <= MAX_PAGE_BYTES);
        page
    });
    assert_eq!(agents.len(), 300);
    assert_eq!(calls, 3);
    let ids: Vec<&str> = agents.iter().map(|a| a.agent_id.as_str()).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted);
    // plans
    let (calls, plans, _) = read_all(|after| {
        let page = plans_page(&rig.core, after, 100).unwrap();
        assert!(bytes(&value_of(&page)) <= MAX_PAGE_BYTES);
        page
    });
    assert_eq!((calls, plans.len()), (1, 80));
    assert_eq!(
        plans.iter().map(|p| p.sequence).collect::<Vec<_>>(),
        (1..=80).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------------------------------- waiting and hashes

fn one(core: &Core, kind: &str) -> Vec<WaitingItem> {
    all_waiting(core, Some(kind))
}

#[test]
fn proposals_prompts_designs_and_messages_wait_with_their_hashes_and_leave_when_decided() {
    let rig = Rig::new();
    rig.relay_on();
    // A pending proposal: its hash is command_sha.
    let proposal = rig.propose("ls -la", "look around");
    let items = one(&rig.core, "op");
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].content_hash,
        proposal["commandSha"].as_str().unwrap()
    );
    assert_eq!(
        items[0].request_id,
        format!("op:{}", proposal["proposalId"].as_str().unwrap())
    );
    assert_eq!(items[0].key, "op:1");
    assert_eq!(items[0].summary, "ls -la");
    assert!(!items[0].needs_terminal);
    // A captured prompt: its hash is prompt_sha; every option takes text or widens permissions: needs the terminal.
    let options = json!([
        {"number": 1, "text": "Yes, always", "acceptsText": false, "widensPermissions": true},
        {"number": 2, "text": "Type it", "acceptsText": true, "widensPermissions": false},
    ]);
    let relay = rig.capture_with("dev-1", "Allow Bash(ls)?", &options);
    let prompts = one(&rig.core, "prompt");
    assert_eq!(
        prompts[0].content_hash,
        prompt_sha("dev-1", "pane-1", "claude", "Allow Bash(ls)?", &options)
    );
    assert_eq!(
        prompts[0].request_id,
        format!("prompt:{}", relay["relayId"].as_str().unwrap())
    );
    assert!(prompts[0].needs_terminal);
    rig.capture("dev-2", "Allow Bash(pwd)?");
    let plain: Vec<_> = one(&rig.core, "prompt")
        .into_iter()
        .filter(|p| p.summary.contains("pwd"))
        .collect();
    assert!(
        !plain[0].needs_terminal,
        "an option that neither widens nor takes text can be answered"
    );
    // A design package with a report and no decision.
    rig.pane("dev-1", "feat/ui");
    let plan = rig.approved_plan();
    rig.assign(&plan, "ui", "dev-1");
    assert!(one(&rig.core, "design").is_empty(), "no report yet");
    let report = rig.report_as("dev-1", "feat/ui", COMMIT, "designed");
    let report_id = report["record"]["reportId"].as_str().unwrap().to_string();
    let designs = one(&rig.core, "design");
    assert_eq!(designs.len(), 1);
    assert_eq!(designs[0].key, "design:1/ui");
    assert_eq!(designs[0].request_id, format!("design:{plan}/ui"));
    assert_eq!(
        designs[0].content_hash,
        sha256(&design_hash_input(&report_id, COMMIT, None))
    );
    // A message that needs action.
    let message = rig.enqueue("dev-1", "look at this", true);
    let messages: Vec<WaitingItem> = one(&rig.core, "message")
        .into_iter()
        .filter(|m| m.request_id == format!("message:{message}"))
        .collect();
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].content_hash,
        sha256(&message_hash_input(&message, "look at this"))
    );
    // The paged hash is content_hash while nothing changes.
    for item in all_waiting(&rig.core, None) {
        assert_eq!(
            content_hash(&rig.core, &item.request_id)
                .unwrap()
                .as_deref(),
            Some(item.content_hash.as_str())
        );
        let again = snapshot_item(&rig.core, "waiting", &item.key)
            .unwrap()
            .unwrap();
        assert_eq!(again, value_of(&item));
        // The key and the request id name the same row.
        let (kind, id) = parse_item_key("waiting", &item.key).unwrap();
        assert_eq!(kind, item.kind);
        assert_eq!(item.key, item_key("waiting", kind, &id));
    }
    // A new report changes the hash of the design item.
    rig.report_as("dev-1", "feat/ui", &"2".repeat(40), "designed again");
    let changed = &one(&rig.core, "design")[0];
    assert_ne!(changed.content_hash, designs[0].content_hash);
    // Decisions remove the items.
    rig.decide(&proposal, "approve");
    assert!(one(&rig.core, "op").is_empty());
    assert_eq!(content_hash(&rig.core, &items[0].request_id).unwrap(), None);
    assert_eq!(snapshot_item(&rig.core, "waiting", "op:1").unwrap(), None);
    rig.core
        .record_design_decision(
            &rig.ctx("pm-1"),
            &json!({"planId": plan, "packageId": "ui", "decision": "changes", "askedUser": "ok?"}),
        )
        .ok();
    rig.core
        .resolve_message(&rig.ctx("owner"), &message, &json!("cancel"), Some("no"))
        .unwrap();
    assert!(one(&rig.core, "message")
        .iter()
        .all(|m| m.request_id != messages[0].request_id));
    assert_eq!(
        content_hash(&rig.core, &messages[0].request_id).unwrap(),
        None
    );
}

#[test]
fn expired_and_failed_messages_wait_and_acked_or_cancelled_ones_do_not() {
    let rig = Rig::new();
    let failed = rig.enqueue("dev-1", "will fail", true);
    rig.core
        .record_failure(&rig.ctx("owner"), &failed, "pane gone")
        .unwrap();
    let cancelled = rig.enqueue("arch-1", "will be cancelled", true);
    rig.core
        .resolve_message(&rig.ctx("owner"), &cancelled, &json!("cancel"), Some("no"))
        .unwrap();
    let expired = rig.enqueue("dev-2", "will expire", true);
    rig.core
        .record_deferral(&rig.ctx("owner"), &expired, &json!("agent_busy"))
        .unwrap();
    rig.core
        .advance_messaging(&rig.ctx("owner"), &TIMERS())
        .unwrap();
    let acked = rig.enqueue("rev-1", "will be acked", true);
    rig.core.record_sent(&rig.ctx("owner"), &acked).unwrap();
    rig.core.ack_message(&rig.ctx("rev-1"), &acked).unwrap();
    let waiting: Vec<String> = one(&rig.core, "message")
        .into_iter()
        .map(|i| i.request_id)
        .collect();
    assert!(waiting.contains(&format!("message:{failed}")));
    assert!(
        waiting.contains(&format!("message:{expired}")),
        "{waiting:?}"
    );
    assert!(!waiting.contains(&format!("message:{cancelled}")));
    assert!(!waiting.contains(&format!("message:{acked}")));
}

#[test]
fn pause_resume_pause_gives_distinct_hashes_to_the_agent_and_to_the_run() {
    let rig = Rig::new();
    let agent_hash = || content_hash(&rig.core, "agent:dev-1").unwrap().unwrap();
    let on_page = || {
        all_agents(&rig.core)
            .into_iter()
            .find(|a| a.agent_id == "dev-1")
            .unwrap()
    };
    let unpaused = agent_hash();
    let pause = |reason: &str| {
        rig.core
            .pause_agent(
                &rig.ctx("pm-1"),
                &json!({"agentId": "dev-1", "reason": reason}),
            )
            .unwrap();
    };
    pause("first");
    let first = agent_hash();
    assert_eq!(on_page().content_hash, first);
    rig.core
        .resume_agent(
            &rig.ctx("pm-1"),
            &json!({"agentId": "dev-1", "reason": "back"}),
        )
        .unwrap();
    assert_eq!(
        agent_hash(),
        unpaused,
        "a resumed agent shows what it showed before"
    );
    pause("second");
    let second = agent_hash();
    assert_ne!(first, second);
    assert_ne!(first, unpaused);
    assert_ne!(second, unpaused);
    // The run.
    let run_hash = || content_hash(&rig.core, "run").unwrap().unwrap();
    let transition = |to: &str| {
        rig.core
            .transition_run(&rig.ctx("owner"), &json!(to), Some("why"))
            .unwrap();
    };
    transition("paused");
    let one = run_hash();
    assert_eq!(snapshot_head(&rig.core).unwrap().run.content_hash, one);
    transition("active");
    let two = run_hash();
    transition("paused");
    let three = run_hash();
    assert!(one != two && two != three && one != three);
}

#[test]
fn the_design_and_message_hashes_match_the_golden_inputs() {
    fn golden(name: &str, input: &str) {
        let path: PathBuf = [
            env!("CARGO_MANIFEST_DIR"),
            "tests",
            "rust_only",
            "fixtures",
            "snapshot_feed",
            name,
        ]
        .iter()
        .collect();
        let value = json!({"input": input, "sha256": sha256(input)});
        let text = format!("{}\n", serde_json::to_string_pretty(&value).unwrap());
        if std::env::var("CAPSTAN_BLESS").as_deref() == Ok("1") {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &text).unwrap();
            return;
        }
        let expected = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("missing fixture {}", path.display()));
        assert_eq!(text, expected, "fixture {name} differs");
        // The expected hash is stated in the fixture, not only recomputed.
        let parsed: Value = serde_json::from_str(&expected).unwrap();
        assert_eq!(parsed["input"], input);
        assert_eq!(parsed["sha256"], sha256(parsed["input"].as_str().unwrap()));
    }
    golden(
        "design.json",
        &design_hash_input(
            "report-1",
            &"a".repeat(40),
            Some("https://example.test/\"x\""),
        ),
    );
    golden(
        "design_no_link.json",
        &design_hash_input("report-1", &"a".repeat(40), None),
    );
    golden(
        "message.json",
        &message_hash_input("msg-1", "say \"hi\"\\\nüñí"),
    );
    // Keys sorted, no whitespace, string escaping as serde_json writes it.
    assert_eq!(
        design_hash_input("r", "c", None),
        r#"{"commit":"c","link":null,"reportId":"r"}"#
    );
    assert_eq!(message_hash_input("m", "b"), r#"{"body":"b","id":"m"}"#);
}

// -------------------------------------------------------------------------------------------- shapes, handoff, inputs

#[test]
fn every_page_and_item_serializes_to_the_fields_the_plan_fixes() {
    let rig = Rig::new();
    rig.propose("ls", "look");
    rig.pane("dev-1", "feat/x");
    rig.approved_plan();
    let fields = |value: &Value| {
        let mut keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        keys.join(",")
    };
    let waiting = value_of(&waiting_page(&rig.core, None, 10, None).unwrap());
    assert_eq!(fields(&waiting), "head,items,next");
    assert_eq!(
        fields(&waiting["items"][0]),
        "contentHash,key,kind,needsTerminal,requestId,since,summary"
    );
    let agents = value_of(&agents_page(&rig.core, None, 10).unwrap());
    assert_eq!(
        fields(&agents["items"][0]),
        "agentId,contentHash,generation,key,kind,paused,requestId,role,state,task"
    );
    let plans = value_of(&plans_page(&rig.core, None, 10).unwrap());
    assert_eq!(
        fields(&plans["items"][0]),
        "key,packages,planId,sequence,state,title"
    );
    assert_eq!(
        fields(&plans["items"][0]["packages"]),
        "assigned,integrated,reported,total"
    );
    let head = value_of(&snapshot_head(&rig.core).unwrap());
    assert_eq!(fields(&head), "epoch,head,run");
    assert_eq!(fields(&head["run"]), "contentHash,paused,requestId,state");
    assert_eq!(head["run"]["requestId"], "run");
    // The agent request id and the keys.
    assert_eq!(
        agents["items"][0]["requestId"],
        format!("agent:{}", agents["items"][0]["agentId"].as_str().unwrap())
    );
    assert_eq!(agents["items"][0]["key"], agents["items"][0]["agentId"]);
    for kind in KEY_KINDS.iter().filter(|k| k.section == "waiting") {
        assert!(!kind.kind.is_empty());
    }
}

#[test]
fn a_model_built_from_pages_and_the_events_after_the_first_head_equals_a_fresh_snapshot() {
    let rig = Rig::new();
    rig.relay_on();
    let first = snapshot_head(&rig.core).unwrap();
    // Mutations between the head and each page call.
    let proposal = rig.propose("ls", "look");
    let mut model = Model {
        run: value_of(&first.run),
        ..Model::default()
    };
    let page = waiting_page(&rig.core, None, 100, None).unwrap();
    assert!(page.head >= first.head);
    assert_eq!(page.head, rig.head());
    model.waiting = keyed(&page.items);
    rig.enqueue("dev-1", "act", true);
    let page = agents_page(&rig.core, None, 100).unwrap();
    assert_eq!(page.head, rig.head());
    model.agents = keyed(&page.items);
    rig.core
        .pause_agent(
            &rig.ctx("pm-1"),
            &json!({"agentId": "dev-2", "reason": "hold"}),
        )
        .unwrap();
    let page = plans_page(&rig.core, None, 100).unwrap();
    model.plans = keyed(&page.items);
    rig.decide(&proposal, "approve");
    rig.capture("dev-1", "Allow?");
    // The events after the first head, whatever the pages already showed.
    let events = rig.since(first.head);
    let core = &rig.core;
    apply_effects(
        &mut model,
        &events,
        &|s, k| snapshot_item(core, s, k).unwrap(),
        &|s, k| kind_items(core, s, k),
    );
    assert_eq!(model, fresh(core));
}

#[test]
fn bad_inputs_are_invalid_and_a_decided_proposal_is_removed_by_the_effects() {
    let rig = Rig::new();
    let proposal = rig.propose("ls", "look");
    for result in [
        waiting_page(&rig.core, None, 10, Some("nope")).map(|_| ()),
        waiting_page(&rig.core, Some("garbage"), 10, None).map(|_| ()),
        waiting_page(&rig.core, Some("op:01"), 10, None).map(|_| ()),
        waiting_page(&rig.core, None, 0, None).map(|_| ()),
        waiting_page(&rig.core, None, 101, None).map(|_| ()),
        agents_page(&rig.core, None, 0).map(|_| ()),
        plans_page(&rig.core, Some("x"), 10).map(|_| ()),
        snapshot_item(&rig.core, "waiting", "design").map(|_| ()),
        snapshot_item(&rig.core, "nowhere", "1").map(|_| ()),
        content_hash(&rig.core, "bogus").map(|_| ()),
    ] {
        let error = result.expect_err("invalid input");
        assert!(matches!(error, KernelError::Type(_)), "{error}");
    }
    assert!(waiting_page(&rig.core, None, 100, None).is_ok());
    assert!(waiting_page(&rig.core, None, 1, Some("op")).is_ok());
    // hash equality of agents and the run with the items
    let agent = &all_agents(&rig.core)[0];
    assert_eq!(
        content_hash(&rig.core, &agent.request_id)
            .unwrap()
            .as_deref(),
        Some(agent.content_hash.as_str())
    );
    assert_eq!(
        content_hash(&rig.core, "run").unwrap().unwrap(),
        snapshot_head(&rig.core).unwrap().run.content_hash
    );
    // The decided proposal: snapshot_item says None and the effects remove it.
    let mut model = fresh(&rig.core);
    assert!(model.waiting.contains_key("op:1"));
    let head = rig.head();
    rig.decide(&proposal, "deny");
    assert_eq!(snapshot_item(&rig.core, "waiting", "op:1").unwrap(), None);
    let core = &rig.core;
    apply_effects(
        &mut model,
        &rig.since(head),
        &|s, k| snapshot_item(core, s, k).unwrap(),
        &|s, k| kind_items(core, s, k),
    );
    assert!(!model.waiting.contains_key("op:1"));
    assert_eq!(model, fresh(core));
    let _ = (COMMIT, MAX_PAGE_BYTES);
}

// ------------------------------------------------------------------------------------------- replay and query plans

/// Applies every event of the ledger, in order, to a model taken before the first one.
fn replay_equals_fresh(rig: &Rig, start: Model, label: &str) {
    let mut model = start;
    let core = &rig.core;
    apply_effects(
        &mut model,
        &rig.since(0),
        &|s, k| snapshot_item(core, s, k).unwrap(),
        &|s, k| kind_items(core, s, k),
    );
    assert_eq!(
        model,
        fresh(core),
        "the replay of {label} differs from a snapshot"
    );
}

#[test]
fn pauses_and_run_pauses_converge_step_by_step_and_on_replay_with_their_hashes() {
    let rig = Rig::new();
    let start = fresh(&rig.core);
    let mut model = start.clone();
    let mut invalidated = Vec::new();
    let (m, i) = (&mut model, &mut invalidated);
    let mut hashes = Vec::new();
    for (n, pause) in [true, false, true].into_iter().enumerate() {
        step(&rig, m, i, "agent pause or resume", |r| {
            let input = json!({"agentId": "dev-1", "reason": format!("agent {n}")});
            if pause {
                r.core.pause_agent(&r.ctx("pm-1"), &input).unwrap();
            } else {
                r.core.resume_agent(&r.ctx("pm-1"), &input).unwrap();
            }
        });
        hashes.push(
            m.agents["dev-1"]["contentHash"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    assert_ne!(hashes[0], hashes[2], "two pauses differ");
    assert_ne!(hashes[0], hashes[1]);
    let mut runs = Vec::new();
    for to in ["paused", "active", "paused"] {
        step(&rig, m, i, "run transition", |r| {
            r.core
                .transition_run(&r.ctx("owner"), &json!(to), Some("run"))
                .unwrap();
        });
        runs.push(m.run["contentHash"].as_str().unwrap().to_string());
    }
    assert!(runs[0] != runs[1] && runs[1] != runs[2] && runs[0] != runs[2]);
    replay_equals_fresh(&rig, start, "the pauses");
}

#[test]
fn a_design_report_reassigned_to_another_agent_converges_step_by_step_and_on_replay() {
    let rig = Rig::new();
    let start = fresh(&rig.core);
    let mut model = start.clone();
    let mut invalidated = Vec::new();
    let (m, i) = (&mut model, &mut invalidated);
    rig.pane("dev-1", "feat/ui");
    rig.pane("dev-2", "feat/ui2");
    let (plan, events) = step(&rig, m, i, "plan", |r| r.approved_plan());
    // The notice of the approval is a message that no event of its own announces.
    assert!(events.iter().all(|e| e["type"] != "message.action_needed"));
    assert!(count(m, "message") >= 1, "the notice waits");
    step(&rig, m, i, "assign to A", |r| {
        r.assign(&plan, "ui", "dev-1")
    });
    step(&rig, m, i, "report of A", |r| {
        r.report_as("dev-1", "feat/ui", COMMIT, "designed")
    });
    assert_eq!(count(m, "design"), 1);
    let before = m.waiting["design:1/ui"]["contentHash"].clone();
    // The package is handed to B: the report of A no longer counts for it.
    step(&rig, m, i, "A ends", |r| {
        r.core.end_agent(&r.ctx("owner"), "dev-1", None).unwrap();
    });
    step(&rig, m, i, "reassign to B", |r| {
        r.assign(&plan, "ui", "dev-2")
    });
    assert_eq!(
        count(m, "design"),
        0,
        "{:?}",
        m.waiting.keys().collect::<Vec<_>>()
    );
    step(&rig, m, i, "report of B", |r| {
        r.report_as("dev-2", "feat/ui2", &"2".repeat(40), "designed again")
    });
    assert_eq!(count(m, "design"), 1);
    assert_ne!(m.waiting["design:1/ui"]["contentHash"], before);
    replay_equals_fresh(&rig, start, "the design report");
}

#[test]
fn the_message_page_reads_by_the_sequence_index_with_no_sort() {
    let rig = Rig::new();
    let db = rig.db();
    let mut statement = db
        .prepare(&format!(
            "EXPLAIN QUERY PLAN {}",
            capstan_kernel::areas::snapshot_feed::message_window_sql()
        ))
        .unwrap();
    let plan: Vec<String> = statement
        .query_map(rusqlite::params!["p", 0, 100], |row| {
            row.get::<_, String>(3)
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let plan = plan.join(" | ");
    assert!(!plan.contains("TEMP B-TREE"), "{plan}");
    assert!(plan.contains("sqlite_autoindex_messages"), "{plan}");
}

// ---------------------------------------------------------------------------------------------- controller notices

#[test]
fn a_cleanup_offer_and_a_delivery_problem_notice_converge_without_a_row_of_their_own() {
    let rig = Rig::new();
    let start = fresh(&rig.core);
    let mut model = start.clone();
    let mut invalidated = Vec::new();
    let (m, i) = (&mut model, &mut invalidated);
    let (_, events) = step(&rig, m, i, "cleanup offer", |r| {
        capstan_kernel::areas::cleanups::record_offer(
            r.core.kernel(),
            &r.ctx("owner"),
            &json!({
                "cleanupId": "c-1", "requestedBy": "pm-1", "trigger": "manual", "mode": "ask",
                "entries": [{"branch": "feat/x", "sha": "1".repeat(40), "class": "landed"}],
                "notice": "Branches can be cleaned up. ACTION NEEDED",
            }),
        )
        .unwrap();
    });
    assert!(events.iter().all(|e| e["type"] != "message.action_needed"));
    assert!(events
        .iter()
        .any(|e| e["type"] == "snapshot.invalidated" && e["body"]["key"] == "message"));
    assert!(m
        .waiting
        .values()
        .any(|w| w["summary"].as_str().unwrap().contains("ACTION NEEDED")));
    // A delivery problem: a failed message the PM has not been told about.
    let failed = rig.enqueue("dev-1", "will fail", false);
    step(&rig, m, i, "failure", |r| {
        r.core
            .record_failure(&r.ctx("owner"), &failed, "pane gone")
            .unwrap()
    });
    replay_equals_fresh(&rig, start, "the notices");
}

// ----------------------------------------------------------------------------------- findings, budgets, replacements

#[test]
fn an_unresolved_finding_check_queues_a_message_the_model_learns_of() {
    let mut rig = Rig::new();
    rig.owner(
        "syncRoleDefinitions",
        json!([[
            {"name": "pm", "kind": "PM", "host": "claude", "configHash": "b".repeat(64)},
            {"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)},
            {"name": "architect", "kind": "Developer", "host": "claude", "configHash": "e".repeat(64)},
            {"name": "reviewer", "kind": "Verifier", "host": "claude", "configHash": "f".repeat(64)},
            {"name": "supervisor", "kind": "Supervisor", "host": "claude", "configHash": "c".repeat(64)},
        ]]),
    );
    rig.add_agent("sup-1", "supervisor", "Supervisor");
    let start = fresh(&rig.core);
    let mut model = start.clone();
    let mut invalidated = Vec::new();
    let (m, i) = (&mut model, &mut invalidated);
    let (finding, _) = step(&rig, m, i, "raise", |r| {
        r.core
            .raise_finding(
                &r.ctx("sup-1"),
                &json!({"targetAgentId": "dev-1", "severity": "high", "evidence": "it skipped the tests",
                    "correction": "run them", "doneWhen": "tests ran"}),
            )
            .unwrap()
    });
    let messages = rig.core.messages_for("dev-1").unwrap();
    let delivery = messages.as_array().unwrap().last().unwrap()["messageId"]
        .as_str()
        .unwrap()
        .to_string();
    step(&rig, m, i, "sent", |r| {
        r.core.record_sent(&r.ctx("owner"), &delivery).unwrap()
    });
    step(&rig, m, i, "acked", |r| {
        r.core.ack_message(&r.ctx("dev-1"), &delivery).unwrap()
    });
    let before = count(m, "message");
    let (_, events) = step(&rig, m, i, "unresolved check", |r| {
        r.core
            .check_finding(
                &r.ctx("sup-1"),
                &json!({"findingId": finding["findingId"], "result": "unresolved", "evidence": "still skipped"}),
            )
            .unwrap()
    });
    assert!(
        events.iter().any(|e| e["body"]["key"] == "message"),
        "{events:#?}"
    );
    assert!(count(m, "message") > before, "the intervention waits");
    replay_equals_fresh(&rig, start, "the finding");
}

/// An approved plan of `packages` plain packages `p0`, `p1`, ...
fn wide_plan(rig: &Rig, title: &str, packages: usize) {
    wide_plan_of(rig, title, packages, false);
}

fn wide_plan_of(rig: &Rig, title: &str, packages: usize, design: bool) -> String {
    let list: Vec<Value> = (0..packages)
        .map(|n| {
            json!({"id": format!("p{n}"), "title": format!("P{n}"), "owns": [format!("src/{n}")],
                "acceptance": ["works"], "estimate_hours": 1, "design": design})
        })
        .collect();
    let order: Vec<String> = (0..packages).map(|n| format!("p{n}")).collect();
    let body = json!({"summary": "s", "packages": list, "risks": [], "integration_order": order})
        .to_string();
    let plan = rig
        .core
        .open_plan(&rig.ctx("pm-1"), &json!({"tier": "normal", "title": title}))
        .unwrap();
    let parsed =
        serde_json::to_string(&capstan_kernel::plan_body::parse_plan_body(&body, 40).unwrap())
            .unwrap();
    rig.core
        .submit_plan(
            &rig.ctx("arch-1"),
            &json!({"planId": plan["planId"], "baseSha": BASE, "bodyJson": parsed, "review": false}),
        )
        .unwrap();
    plan["planId"].as_str().unwrap().to_string()
}

#[test]
fn plans_of_many_packages_are_paged_within_the_read_budget() {
    let rig = Rig::new();
    for n in 0..8 {
        wide_plan(&rig, &format!("wide {n}"), 20);
    }
    // A plan of 20 unassigned packages costs a row, a count (2) and a list (21): a budget of 80 holds three of them.
    let mut after: Option<String> = None;
    let mut keys = Vec::new();
    let mut calls = 0;
    loop {
        let (page, reads) = capstan_kernel::areas::snapshot_feed::plans_page_counted(
            &rig.core,
            after.as_deref(),
            100,
            80,
        )
        .unwrap();
        assert!(reads <= 80, "{reads}");
        assert!(page.items.len() <= 3, "{}", page.items.len());
        calls += 1;
        keys.extend(page.items.iter().map(|p| p.key.clone()));
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    assert!(calls >= 3);
    assert_eq!(keys, (1..=8).map(|n| n.to_string()).collect::<Vec<_>>());
    // The default budget holds them all in one page of 100 items at most.
    assert_eq!(all_plans(&rig.core).len(), 8);
    // A single plan above the budget is still examined: a call always makes progress.
    let (page, _) =
        capstan_kernel::areas::snapshot_feed::plans_page_counted(&rig.core, None, 100, 10).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.next.as_deref(), Some("1"));
}

#[test]
fn design_candidates_never_overshoot_the_budget_and_none_is_skipped_by_the_continuation() {
    let mut rig = Rig::new();
    for n in 3..7 {
        rig.add_agent(&format!("dev-{n}"), "developer", "Developer");
    }
    let body = r#"{"summary":"s","packages":[
        {"id":"d1","title":"D1","owns":["design/1"],"acceptance":["ok"],"estimate_hours":1,"design":true},
        {"id":"d2","title":"D2","owns":["design/2"],"acceptance":["ok"],"estimate_hours":1,"design":true},
        {"id":"d3","title":"D3","owns":["design/3"],"acceptance":["ok"],"estimate_hours":1,"design":true},
        {"id":"d4","title":"D4","owns":["design/4"],"acceptance":["ok"],"estimate_hours":1,"design":true},
        {"id":"n1","title":"N1","owns":["src/1"],"acceptance":["ok"],"estimate_hours":1}],
        "risks":[],"integration_order":["d1","d2","d3","d4","n1"]}"#;
    let plan = rig.approved_plan_of(body);
    for (n, package) in ["d1", "d2", "d3", "d4"].iter().enumerate() {
        let agent = format!("dev-{}", n + 3);
        let branch = format!("feat/{package}");
        rig.pane(&agent, &branch);
        rig.assign(&plan, package, &agent);
        rig.report_as(
            &agent,
            &branch,
            &format!("{}", n + 1).repeat(40),
            "designed",
        );
    }
    rig.pane("dev-1", "feat/n1");
    rig.assign(&plan, "n1", "dev-1");
    let wanted: Vec<String> = all_waiting(&rig.core, Some("design"))
        .into_iter()
        .map(|i| i.key)
        .collect();
    assert_eq!(wanted.len(), 4);
    for max in 14..=60usize {
        let mut after: Option<String> = None;
        let mut keys = Vec::new();
        let mut calls = 0;
        loop {
            let (page, reads) =
                waiting_page_counted(&rig.core, after.as_deref(), 100, Some("design"), max)
                    .unwrap();
            assert!(reads <= max, "budget {max}: {reads} reads");
            keys.extend(page.items.iter().map(|i| i.key.clone()));
            calls += 1;
            assert!(calls < 100, "no progress with budget {max}");
            match page.next {
                Some(next) => after = Some(next),
                None => break,
            }
        }
        assert_eq!(keys, wanted, "budget {max}");
    }
}

#[test]
fn a_replaced_developer_moves_the_counts_of_its_plan_and_a_replaced_record_rebinds_them() {
    let mut rig = Rig::new();
    rig.add_agent("dev-3", "developer", "Developer");
    let start = fresh(&rig.core);
    let mut model = start.clone();
    let mut invalidated = Vec::new();
    let (m, i) = (&mut model, &mut invalidated);
    rig.pane("dev-1", "feat/ui");
    rig.pane("dev-2", "feat/docs");
    let plan = rig.approved_plan();
    settle(&rig, m, i, "plan", &rig.since(0));
    step(&rig, m, i, "assign ui", |r| r.assign(&plan, "ui", "dev-1"));
    step(&rig, m, i, "assign docs", |r| {
        r.assign(&plan, "docs", "dev-2")
    });
    step(&rig, m, i, "reports", |r| {
        r.report_as("dev-1", "feat/ui", COMMIT, "designed");
        r.report_as("dev-2", "feat/docs", &"2".repeat(40), "written");
    });
    assert_eq!(m.plans["1"]["packages"]["reported"], 2);
    assert_eq!(count(m, "design"), 1);
    // A new generation of dev-1: its report no longer counts.
    step(&rig, m, i, "replace", |r| {
        r.core
            .replace_agent_generation(&r.ctx("owner"), "dev-1")
            .unwrap();
    });
    assert_eq!(m.plans["1"]["packages"]["reported"], 1);
    assert_eq!(count(m, "design"), 0);
    // dev-2 ends and dev-3 takes over its packages.
    step(&rig, m, i, "end", |r| {
        r.core.end_agent(&r.ctx("owner"), "dev-2", None).unwrap();
    });
    step(&rig, m, i, "replaced", |r| {
        r.core
            .record_agent_replaced(
                &r.ctx("owner"),
                &json!({"predecessorId": "dev-2", "successorId": "dev-3"}),
            )
            .unwrap();
    });
    replay_equals_fresh(&rig, start, "the replacements");
}

#[test]
fn plans_full_of_reported_design_packages_stay_within_the_actual_reads() {
    let mut rig = Rig::new();
    // A report counts for the package its agent was assigned last, so each package has an agent of its own.
    for agent in 0..100 {
        let id = format!("w{agent:03}");
        rig.add_agent(&id, "developer", "Developer");
        rig.pane(&id, &format!("feat/{id}"));
    }
    let mut n = 0u64;
    for p in 0..5 {
        let plan = wide_plan_of(&rig, &format!("designs {p}"), 20, true);
        for package in 0..20 {
            let id = format!("w{:03}", p * 20 + package);
            rig.assign(&plan, &format!("p{package}"), &id);
            n += 1;
            rig.report_as(&id, &format!("feat/{id}"), &format!("{n:040x}"), "designed");
        }
    }
    // Each plan can take 1 + 20 + 2 * 20 reads: a budget of 300 admits three of the five.
    let mut after: Option<String> = None;
    let mut keys = Vec::new();
    let mut calls = 0;
    loop {
        capstan_kernel::areas::snapshot_feed::actual_reads_reset();
        let (page, reserved) = capstan_kernel::areas::snapshot_feed::plans_page_counted(
            &rig.core,
            after.as_deref(),
            100,
            300,
        )
        .unwrap();
        let actual = capstan_kernel::areas::snapshot_feed::actual_reads();
        assert!(reserved <= 300, "{reserved}");
        assert!(
            actual <= reserved,
            "actual {actual} reads, {reserved} reserved"
        );
        assert!(actual <= 300, "{actual}");
        calls += 1;
        keys.extend(page.items.iter().map(|p| p.key.clone()));
        for item in &page.items {
            assert_eq!(item.packages.reported, 20);
        }
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    assert!(calls >= 2);
    assert_eq!(keys, (1..=5).map(|n| n.to_string()).collect::<Vec<_>>());
    // The default budget: 23 of these plans would not exceed 2000 actual reads either.
    capstan_kernel::areas::snapshot_feed::actual_reads_reset();
    let page = plans_page(&rig.core, None, 100).unwrap();
    assert_eq!(page.items.len(), 5);
    assert!(capstan_kernel::areas::snapshot_feed::actual_reads() <= 2000);
}

#[test]
fn approving_a_superseding_plan_retires_the_old_plan_and_its_waiting_design_in_the_model() {
    for via_review in [false, true] {
        let rig = Rig::new();
        let start = fresh(&rig.core);
        let mut model = start.clone();
        let mut invalidated = Vec::new();
        let (m, i) = (&mut model, &mut invalidated);
        rig.pane("dev-1", "feat/ui");
        let (plan, _) = step(&rig, m, i, "plan A", |r| r.approved_plan());
        step(&rig, m, i, "assign", |r| r.assign(&plan, "ui", "dev-1"));
        step(&rig, m, i, "report", |r| {
            r.report_as("dev-1", "feat/ui", COMMIT, "designed")
        });
        assert_eq!(count(m, "design"), 1);
        assert!(m.plans.contains_key("1"));
        let (b, _) = step(&rig, m, i, "open B", |r| {
            r.core
                .open_plan(
                    &r.ctx("pm-1"),
                    &json!({"tier": "normal", "title": "Plan two", "supersedesPlanId": plan}),
                )
                .unwrap()["planId"]
                .as_str()
                .unwrap()
                .to_string()
        });
        let body = serde_json::to_string(
            &capstan_kernel::plan_body::parse_plan_body(
                r#"{"summary":"s","packages":[
                {"id":"only","title":"Only","owns":["src/only"],"acceptance":["works"],"estimate_hours":1}],
                "risks":[],"integration_order":["only"]}"#,
                8,
            )
            .unwrap(),
        )
        .unwrap();
        step(&rig, m, i, "submit B", |r| {
            r.core
                .submit_plan(
                    &r.ctx("arch-1"),
                    &json!({"planId": b, "baseSha": BASE, "bodyJson": body, "review": via_review}),
                )
                .unwrap()
        });
        if via_review {
            assert!(m.plans.contains_key("1"), "A is still approved until B is");
            step(&rig, m, i, "review of B", |r| r.review(&b, "pass", "fine"));
        }
        assert!(!m.plans.contains_key("1"), "A is superseded");
        assert_eq!(
            count(m, "design"),
            0,
            "its design report no longer waits (review: {via_review})"
        );
        replay_equals_fresh(&rig, start, "the supersession");
    }
}

/// Calls `page` (which returns the page and the reads it reserved) until it has no `next`, checking each call's counted
/// reads (a statement and a row each) against the budget.
fn pages_within_budget<T: Clone>(
    max_reads: usize,
    mut page: impl FnMut(Option<&str>) -> (Page<T>, usize),
) -> (usize, Vec<T>) {
    let mut after: Option<String> = None;
    let mut items = Vec::new();
    let mut calls = 0;
    loop {
        capstan_kernel::areas::snapshot_feed::actual_reads_reset();
        let (found, reserved) = page(after.as_deref());
        let actual = capstan_kernel::areas::snapshot_feed::actual_reads();
        assert!(reserved <= max_reads, "{reserved} reserved");
        assert!(actual <= max_reads, "{actual} reads");
        assert!(actual <= reserved, "{actual} reads, {reserved} reserved");
        calls += 1;
        items.extend(found.items.iter().cloned());
        match found.next {
            Some(next) => after = Some(next),
            None => return (calls, items),
        }
    }
}

#[test]
fn plans_of_assigned_and_reviewed_reports_are_paged_within_2000_counted_reads() {
    let mut rig = Rig::new();
    let mut n = 0u64;
    for p in 0..24 {
        let plan = wide_plan_of(&rig, &format!("reviewed {p}"), 20, false);
        for package in 0..20 {
            let id = format!("w{p:02}{package:02}");
            rig.add_agent(&id, "developer", "Developer");
            rig.pane(&id, &format!("feat/{id}"));
            rig.assign(&plan, &format!("p{package}"), &id);
            n += 1;
            let report =
                rig.report_as(&id, &format!("feat/{id}"), &format!("{n:040x}"), "reviewed");
            let report_id = report["record"]["reportId"].as_str().unwrap().to_string();
            rig.review(&report_id, "pass", "fine");
        }
    }
    let core = &rig.core;
    let (calls, items) = pages_within_budget(2000, |after| {
        capstan_kernel::areas::snapshot_feed::plans_page_counted(core, after, 100, 2000).unwrap()
    });
    assert!(calls >= 2, "{calls}");
    assert_eq!(items.len(), 24);
    let keys: Vec<String> = items.iter().map(|p| p.key.clone()).collect();
    assert_eq!(keys, (1..=24).map(|n| n.to_string()).collect::<Vec<_>>());
    for item in &items {
        assert_eq!((item.packages.assigned, item.packages.reported), (20, 20));
    }
}

#[test]
fn over_2500_plain_messages_are_paged_within_2000_counted_reads_and_each_waiting_one_appears_once()
{
    let rig = Rig::new();
    for n in 0..2500 {
        rig.enqueue("dev-1", &format!("plain {n}"), false);
    }
    let wanted: Vec<String> = (0..150)
        .map(|n| rig.enqueue("dev-2", &format!("act {n}"), true))
        .collect();
    let core = &rig.core;
    let (calls, items) = pages_within_budget(2000, |after| {
        waiting_page_counted(core, after, 100, Some("message"), 2000).unwrap()
    });
    assert!(calls >= 3, "{calls}");
    let seen: Vec<String> = items.iter().map(|i| i.request_id.clone()).collect();
    let want: Vec<String> = wanted.iter().map(|id| format!("message:{id}")).collect();
    assert_eq!(seen, want);
}
