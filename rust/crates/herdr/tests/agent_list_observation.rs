//! `agent_observations` asks Herdr for one `agent list` and falls back to `agent get` for what the list cannot answer.

use capstan_herdr::adapter::{Adapter, AdapterOptions};
use capstan_herdr::api::{
    AdapterError, AdoptPaneInput, HerdrError, HerdrOutput, HerdrRunner, HerdrState, PaneRegistry,
    PaneRole, RunOptions,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// A Herdr that knows agents `name -> (pane, status)` and answers `agent list` from a body the test sets.
struct World {
    agents: Mutex<HashMap<String, (String, String)>>,
    list: Mutex<Option<String>>,
    calls: Mutex<Vec<Vec<String>>>,
}

fn ok(value: Value) -> Result<HerdrOutput, HerdrError> {
    Ok(HerdrOutput {
        code: 0,
        stdout: value.to_string(),
        stderr: String::new(),
    })
}

fn entry(name: &str, pane: &str, status: &str) -> Value {
    json!({"name": name, "pane_id": pane, "agent_status": status, "agent": "claude"})
}

impl World {
    fn new() -> Arc<World> {
        Arc::new(World {
            agents: Mutex::default(),
            list: Mutex::new(None),
            calls: Mutex::default(),
        })
    }
    fn set(&self, name: &str, pane: &str, status: &str) {
        self.agents
            .lock()
            .unwrap()
            .insert(name.into(), (pane.into(), status.into()));
    }
    fn count(&self, a: &str, b: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c[0] == a && c[1] == b)
            .count()
    }
}

impl HerdrRunner for World {
    fn run(&self, args: &[String], _o: &RunOptions) -> Result<HerdrOutput, HerdrError> {
        self.calls.lock().unwrap().push(args.to_vec());
        let key = |i: usize| args.get(i).map(String::as_str).unwrap_or("");
        match (key(0), key(1)) {
            ("pane", "get") => ok(json!({"result": {"pane": {"pane_id": key(2)}}})),
            ("agent", "get") => match self.agents.lock().unwrap().get(key(2)) {
                Some((pane, status)) => {
                    ok(json!({"result": {"agent": entry(key(2), pane, status)}}))
                }
                None => Ok(HerdrOutput {
                    code: 1,
                    stdout: json!({"error": {"code": "agent_not_found", "message": "no agent"}})
                        .to_string(),
                    stderr: String::new(),
                }),
            },
            ("agent", "list") => match self.list.lock().unwrap().clone() {
                Some(body) => Ok(HerdrOutput {
                    code: 0,
                    stdout: body,
                    stderr: String::new(),
                }),
                None => Ok(HerdrOutput {
                    code: 1,
                    stdout: json!({"error": {"code": "boom", "message": "list failed"}})
                        .to_string(),
                    stderr: String::new(),
                }),
            },
            _ => ok(json!({"result": {}})),
        }
    }
}

fn list_of(entries: Vec<Value>) -> String {
    json!({"result": {"agents": entries}}).to_string()
}

struct Rig {
    world: Arc<World>,
    adapter: Adapter,
    logs: Arc<Mutex<Vec<String>>>,
    clock: Arc<AtomicU64>,
    step: Arc<AtomicU64>,
}

/// Agents `w1..wN`, each on pane `w1:p<i>`, registered and "working" in Herdr.
fn rig(count: usize) -> Rig {
    let world = World::new();
    let logs = Arc::new(Mutex::new(Vec::new()));
    let clock = Arc::new(AtomicU64::new(0));
    let step = Arc::new(AtomicU64::new(0));
    let mut options = AdapterOptions::new(world.clone());
    let sink = logs.clone();
    options.log = Some(Arc::new(move |event, _| {
        sink.lock().unwrap().push(event.to_string())
    }));
    let (c, s) = (clock.clone(), step.clone());
    options.monotonic_ms = Some(Arc::new(move || {
        c.fetch_add(s.load(Ordering::SeqCst), Ordering::SeqCst)
    }));
    let adapter = Adapter::new(options);
    for i in 1..=count {
        world.set(&format!("a{i}"), &format!("w1:p{i}"), "working");
    }
    for i in 1..=count {
        adapter
            .adopt_pane_of(&AdoptPaneInput {
                pane_id: &format!("w1:p{i}"),
                role: PaneRole::Worker,
                agent: &format!("a{i}"),
                workspace_id: None,
                worktree_path: None,
            })
            .unwrap();
    }
    world.calls.lock().unwrap().clear();
    Rig {
        world,
        adapter,
        logs,
        clock,
        step,
    }
}

fn ids(count: usize) -> Vec<String> {
    (1..=count).map(|i| format!("a{i}")).collect()
}

fn full_list(count: usize) -> String {
    list_of(
        (1..=count)
            .map(|i| entry(&format!("a{i}"), &format!("w1:p{i}"), "working"))
            .collect(),
    )
}

#[test]
fn one_list_answers_every_agent_without_a_get() {
    let r = rig(3);
    *r.world.list.lock().unwrap() = Some(full_list(3));
    let out = r.adapter.agent_observations(&ids(3));
    assert_eq!(out.len(), 3);
    assert!(out
        .values()
        .all(|v| *v.as_ref().unwrap() == HerdrState::Working));
    assert_eq!(
        (
            r.world.count("agent", "list"),
            r.world.count("agent", "get")
        ),
        (1, 0)
    );
}

#[test]
fn a_missing_agent_is_not_found_and_a_moved_one_is_a_pane_mismatch() {
    let r = rig(3);
    *r.world.list.lock().unwrap() = Some(list_of(vec![
        entry("a1", "w1:p1", "idle"),
        entry("a2", "w9:p9", "idle"),
    ]));
    let out = r.adapter.agent_observations(&ids(3));
    assert_eq!(out["a1"].as_ref().unwrap(), &HerdrState::Idle);
    assert!(matches!(out["a2"], Err(AdapterError::AgentPaneMismatch(_))));
    assert_eq!(
        out["a3"].as_ref().unwrap_err().herdr_code(),
        Some("agent_not_found")
    );
    assert_eq!(r.world.count("agent", "get"), 0);
}

fn outcome_text(r: &Result<HerdrState, AdapterError>) -> String {
    match r {
        Ok(s) => format!("ok {}", s.as_str()),
        Err(e) => format!("{} {:?}", e.name(), e.herdr_code()),
    }
}

#[test]
fn a_duplicate_key_and_a_failed_list_give_the_outcomes_of_agent_get() {
    let r = rig(3);
    r.world.set("a2", "w1:p2", "idle");
    let expected: HashMap<String, String> = ids(3)
        .iter()
        .map(|id| (id.clone(), outcome_text(&r.adapter.agent_observation(id))))
        .collect();
    r.world.calls.lock().unwrap().clear();

    // a2 listed twice: only a2 is asked with get.
    *r.world.list.lock().unwrap() = Some(list_of(vec![
        entry("a1", "w1:p1", "working"),
        entry("a2", "w1:p2", "idle"),
        entry("a2", "w1:p2", "idle"),
        entry("a3", "w1:p3", "working"),
    ]));
    let out = r.adapter.agent_observations(&ids(3));
    for id in ids(3) {
        assert_eq!(outcome_text(&out[&id]), expected[&id], "{id}");
    }
    assert_eq!(
        (
            r.world.count("agent", "list"),
            r.world.count("agent", "get")
        ),
        (1, 1)
    );

    // the list command fails: every agent is asked with get.
    *r.world.list.lock().unwrap() = None;
    r.world.calls.lock().unwrap().clear();
    let out = r.adapter.agent_observations(&ids(3));
    for id in ids(3) {
        assert_eq!(outcome_text(&out[&id]), expected[&id], "{id}");
    }
    assert_eq!(r.world.count("agent", "get"), 3);
}

#[test]
fn a_malformed_entry_sends_only_its_agent_to_get() {
    let r = rig(3);
    *r.world.list.lock().unwrap() = Some(list_of(vec![
        entry("a1", "w1:p1", "working"),
        json!({"name": "a2", "pane_id": 7}),
        entry("a3", "w1:p3", "working"),
    ]));
    let out = r.adapter.agent_observations(&ids(3));
    assert!(out
        .values()
        .all(|v| *v.as_ref().unwrap() == HerdrState::Working));
    assert_eq!(
        (
            r.world.count("agent", "list"),
            r.world.count("agent", "get")
        ),
        (1, 1)
    );
    let gets: Vec<_> = r
        .world
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c[1] == "get")
        .cloned()
        .collect();
    assert_eq!(gets[0][2], "a2");
}

#[test]
fn a_paged_or_unparseable_list_falls_back_for_all() {
    let r = rig(2);
    *r.world.list.lock().unwrap() = Some(
        json!({"result": {"agents": [entry("a1", "w1:p1", "working")], "next_cursor": "x"}})
            .to_string(),
    );
    r.adapter.agent_observations(&ids(2));
    assert_eq!(r.world.count("agent", "get"), 2);
    *r.world.list.lock().unwrap() = Some("not json".into());
    r.adapter.agent_observations(&ids(2));
    assert_eq!(r.world.count("agent", "get"), 4);
}

#[test]
fn each_fallback_kind_logs_once_over_three_ticks() {
    let r = rig(2);
    for _ in 0..3 {
        *r.world.list.lock().unwrap() = None;
        r.adapter.agent_observations(&ids(2));
    }
    for _ in 0..3 {
        *r.world.list.lock().unwrap() = Some(list_of(vec![
            entry("a1", "w1:p1", "working"),
            entry("a1", "w1:p1", "working"),
            entry("a2", "w1:p2", "working"),
        ]));
        r.adapter.agent_observations(&ids(2));
    }
    assert_eq!(
        r.logs.lock().unwrap().len(),
        2,
        "{:?}",
        r.logs.lock().unwrap()
    );
}

#[test]
fn a_list_slower_than_a_second_logs_once_over_three_ticks() {
    let r = rig(2);
    *r.world.list.lock().unwrap() = Some(full_list(2));
    r.step.store(1500, Ordering::SeqCst);
    for _ in 0..3 {
        r.adapter.agent_observations(&ids(2));
    }
    assert_eq!(*r.logs.lock().unwrap(), vec!["herdr_list_slow".to_string()]);
    assert!(r.clock.load(Ordering::SeqCst) > 0);
}
