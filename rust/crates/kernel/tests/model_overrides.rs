//! Model overrides and agent models: the override rows, their actor rule, `openPlan` with overrides, `registerAgent`
//! with a model, the members on the agent list, the agents item and the `agent.registered` event, and the scope of a
//! review subject.

#[path = "rust_only/fixture.rs"]
mod fixture;

use capstan_kernel::areas::event_feed::{feed_head, feed_page, Position};
use capstan_kernel::areas::snapshot_feed::agents_page;
use capstan_kernel::Core;
use fixture::{context, step, Fixture, OWNER};
use serde_json::{json, Value};

const REFUSED: &str = "only the user, the PM or the operator may set a model override";
const COMMIT: &str = "1111111111111111111111111111111111111111";
const EARLY: &str = "2026-01-01T00:00:00.000Z";

fn db(f: &Fixture) -> rusqlite::Connection {
    rusqlite::Connection::open(f.database()).unwrap()
}

fn open_plan(f: &Fixture) -> String {
    let core = f.core();
    core.open_plan(
        &context(core, &f.pm_credential),
        &json!({"tier": "normal", "title": "T"}),
    )
    .unwrap()["planId"]
        .as_str()
        .unwrap()
        .to_string()
}

fn set(f: &Fixture, credential: &str, input: Value) -> Result<Value, String> {
    let core = f.core();
    core.set_model_override(&context(core, credential), &input)
        .map_err(|e| e.to_string())
}

fn link_requirement(f: &Fixture, id: &str) {
    step(
        f.core(),
        "linkExternal",
        json!([{"refKind": "requirement", "refId": id, "externalId": "PM-47"}]),
    );
}

fn create_actor(f: &Fixture, name: &str, kind: &str) -> String {
    let seat = format!("seat-{name}");
    step(
        f.core(),
        "createSeat",
        json!([{"seatId": seat, "name": seat, "role": kind}]),
    );
    let actor = step(
        f.core(),
        "createActor",
        json!([{"displayName": name, "role": kind, "seatId": seat}]),
    );
    actor["credential"].as_str().unwrap().to_string()
}

#[test]
fn set_change_and_clear() {
    let f = Fixture::new();
    let plan = open_plan(&f);
    let first = set(
        &f,
        OWNER,
        json!({"scopeType": "plan", "scopeId": plan, "roleKind": null, "model": "m-1"}),
    )
    .unwrap();
    assert_eq!(first["previous"], Value::Null);
    assert_eq!(first["roleKind"], Value::Null);
    let second = set(
        &f,
        &f.pm_credential,
        json!({"scopeType": "plan", "scopeId": plan, "roleKind": null, "model": "m-2"}),
    )
    .unwrap();
    assert_eq!(second["previous"], "m-1");
    assert_eq!(second["setBy"], f.pm_actor);
    let core = f.core();
    let listed = core.model_overrides("plan", &plan).unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["model"], "m-2");
    let input = json!({"scopeType": "plan", "scopeId": plan, "roleKind": null});
    let cleared = core
        .clear_model_override(&context(core, OWNER), &input)
        .unwrap();
    assert_eq!(cleared, json!({"cleared": true, "previous": "m-2"}));
    let again = core
        .clear_model_override(&context(core, OWNER), &input)
        .unwrap();
    assert_eq!(again, json!({"cleared": false, "previous": null}));
}

#[test]
fn kind_row_beats_all_kinds_and_requirement_beats_plan() {
    let f = Fixture::new();
    let plan = open_plan(&f);
    link_requirement(&f, "req-1");
    for (scope, id, kind, model) in [
        ("plan", plan.as_str(), Value::Null, "plan-all"),
        ("plan", plan.as_str(), json!("Developer"), "plan-dev"),
        ("requirement", "req-1", Value::Null, "req-all"),
    ] {
        set(
            &f,
            OWNER,
            json!({"scopeType": scope, "scopeId": id, "roleKind": kind, "model": model}),
        )
        .unwrap();
    }
    let core = f.core();
    let plan_only = core
        .model_override_for(&[("plan", plan.as_str())], "Developer")
        .unwrap()
        .unwrap();
    assert_eq!(
        plan_only,
        json!({"model": "plan-dev", "source": "plan", "scopeId": plan})
    );
    let verifier = core
        .model_override_for(&[("plan", plan.as_str())], "Verifier")
        .unwrap()
        .unwrap();
    assert_eq!(verifier["model"], "plan-all");
    let both = core
        .model_override_for(
            &[("requirement", "req-1"), ("plan", plan.as_str())],
            "Developer",
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        both,
        json!({"model": "req-all", "source": "task", "scopeId": "req-1"})
    );
    assert!(core
        .model_override_for(&[("plan", "plan-99")], "Developer")
        .unwrap()
        .is_none());
    let rows = core.model_overrides("plan", &plan).unwrap();
    assert_eq!(rows[0]["roleKind"], Value::Null);
    assert_eq!(rows[1]["roleKind"], "Developer");
}

#[test]
fn scopes_and_callers_are_checked() {
    let f = Fixture::new();
    let plan = open_plan(&f);
    let bad = |scope: &str, id: &str| {
        set(
            &f,
            OWNER,
            json!({"scopeType": scope, "scopeId": id, "roleKind": null, "model": "m"}),
        )
    };
    assert!(bad("plan", "plan-99")
        .unwrap_err()
        .contains("does not exist"));
    assert!(bad("requirement", "req-9")
        .unwrap_err()
        .contains("not linked"));
    assert!(set(
        &f,
        OWNER,
        json!({"scopeType": "plan", "scopeId": plan, "roleKind": "PM", "model": "m"})
    )
    .is_err());
    assert!(set(
        &f,
        OWNER,
        json!({"scopeType": "plan", "scopeId": plan, "roleKind": "developer", "model": "m"})
    )
    .is_err());
    let project: String = db(&f)
        .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
        .unwrap();
    db(&f)
        .execute(
            "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, current_revision, approved_revision, created_at, updated_at) VALUES (?, 'plan-90', 90, 'T', 'normal', 'superseded', 'x', 1, 1, ?, ?)",
            rusqlite::params![project, EARLY, EARLY],
        )
        .unwrap();
    assert!(bad("plan", "plan-90").unwrap_err().contains("superseded"));
    db(&f)
        .execute(
            "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, current_revision, cancelled_at, created_at, updated_at) VALUES (?, 'plan-91', 91, 'T', 'normal', 'draft', 'x', 0, ?, ?, ?)",
            rusqlite::params![project, EARLY, EARLY, EARLY],
        )
        .unwrap();
    assert!(bad("plan", "plan-91").unwrap_err().contains("cancelled"));
    let verifier = create_actor(&f, "ver", "Verifier");
    let input = json!({"scopeType": "plan", "scopeId": plan, "roleKind": null, "model": "m"});
    // A Developer-kind context (an architect is one) has plan:write and is refused with the text; a Verifier lacks
    // plan:write and is refused by the capability check before.
    assert!(set(&f, &f.developer_credential, input.clone())
        .unwrap_err()
        .contains(REFUSED));
    let core = f.core();
    let cleared = core.clear_model_override(&context(core, &f.developer_credential), &input);
    assert!(cleared.unwrap_err().to_string().contains(REFUSED));
    assert!(set(&f, &verifier, input.clone()).is_err());
    set(&f, &f.pm_credential, input.clone()).unwrap();
    set(&f, OWNER, input).unwrap();
    let long = "x".repeat(101);
    let err = set(
        &f,
        OWNER,
        json!({"scopeType": "plan", "scopeId": plan, "roleKind": null, "model": long}),
    );
    assert!(err.is_err());
}

fn plan_count(f: &Fixture) -> i64 {
    db(f)
        .query_row("SELECT COUNT(*) FROM plans", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn open_plan_writes_overrides_with_the_plan() {
    let f = Fixture::new();
    let core = f.core();
    let opened = core
        .open_plan(
            &context(core, &f.pm_credential),
            &json!({"tier": "normal", "title": "T", "modelOverrides": [{"roleKind": "Developer", "model": "dev-m"}, {"roleKind": null, "model": "all-m"}]}),
        )
        .unwrap();
    let plan = opened["planId"].as_str().unwrap();
    let rows = core.model_overrides("plan", plan).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 2);
    // A bad row leaves neither the plan nor any row.
    let before = plan_count(&f);
    for bad in [
        json!([{"roleKind": "Developer", "model": "ok"}, {"roleKind": "Verifier", "model": "x".repeat(101)}]),
        json!([{"roleKind": "PM", "model": "ok"}]),
        json!([{"roleKind": null, "model": "a"}, {"roleKind": null, "model": "b"}]),
    ] {
        let result = core.open_plan(
            &context(core, &f.pm_credential),
            &json!({"tier": "normal", "title": "T", "modelOverrides": bad}),
        );
        assert!(result.is_err());
    }
    assert_eq!(plan_count(&f), before);
    let rows: i64 = db(&f)
        .query_row("SELECT COUNT(*) FROM model_overrides", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 2);
    // A Developer context is refused with the text, writing nothing.
    let refused = core.open_plan(
        &context(core, &f.developer_credential),
        &json!({"tier": "normal", "title": "T", "modelOverrides": [{"roleKind": null, "model": "m"}]}),
    );
    assert!(refused.unwrap_err().to_string().contains(REFUSED));
    assert_eq!(plan_count(&f), before);
}

fn register(f: &Fixture, agent: &str, extra: Value) -> Value {
    let seat = format!("seat-{agent}");
    step(
        f.core(),
        "createSeat",
        json!([{"seatId": seat, "name": seat, "role": "Developer"}]),
    );
    let actor = step(
        f.core(),
        "createActor",
        json!([{"displayName": agent, "role": "Developer", "seatId": seat}]),
    );
    let mut input = json!({"agentId": agent, "roleName": "developer", "seatId": seat, "actorId": actor["actorId"]});
    for (k, v) in extra.as_object().into_iter().flatten() {
        input[k] = v.clone();
    }
    let core = f.core();
    core.register_agent(&context(core, OWNER), &input).unwrap()
}

fn item_of(core: &Core, agent: &str) -> Value {
    let page = agents_page(core, None, 50).unwrap();
    let item = page.items.iter().find(|a| a.agent_id == agent).unwrap();
    serde_json::to_value(item).unwrap()
}

fn registered_event(core: &Core, after: u64, agent: &str) -> Value {
    feed_page(
        core,
        Position {
            seq: after,
            k: u32::MAX - 1,
        },
        500,
        None,
    )
    .unwrap()
    .events
    .into_iter()
    .find(|e| e["type"] == "agent.registered" && e["body"]["agentId"] == agent)
    .expect("the registered event")
}

#[test]
fn register_with_a_model_is_listed_and_in_the_feed() {
    let f = Fixture::new();
    let head = feed_head(f.core()).unwrap().head;
    register(
        &f,
        "dev-m",
        json!({"model": "opus-x", "modelSource": "plan", "modelScope": "plan-1"}),
    );
    let core = f.core();
    assert_eq!(
        core.agent_model("dev-m").unwrap().unwrap(),
        json!({"model": "opus-x", "source": "plan", "scopeId": "plan-1"})
    );
    assert!(core.agent_model("dev-1").unwrap().is_none());
    let listed = core.list_agents().unwrap();
    let mine = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["agentId"] == "dev-m")
        .unwrap();
    assert_eq!(
        (mine["model"].clone(), mine["modelSource"].clone()),
        (json!("opus-x"), json!("plan"))
    );
    let plain = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["agentId"] == "dev-1")
        .unwrap();
    assert!(plain.get("model").is_none() && plain.get("modelSource").is_none());
    let item = item_of(core, "dev-m");
    assert_eq!(
        (item["model"].clone(), item["modelSource"].clone()),
        (json!("opus-x"), json!("plan"))
    );
    assert!(item_of(core, "dev-1").get("model").is_none());
    let event = registered_event(core, head, "dev-m");
    assert_eq!(event["body"]["model"], "opus-x");
    assert_eq!(event["body"]["modelSource"], "plan");
    assert_eq!(event["body"]["effects"][0]["fields"]["model"], "opus-x");
    assert_eq!(event["body"]["effects"][0]["fields"]["modelSource"], "plan");
    assert!(event["body"]["untrusted"]
        .as_array()
        .unwrap()
        .contains(&json!("body.model")));
    // Without a model neither member exists.
    let head = feed_head(core).unwrap().head;
    register(&f, "dev-n", json!({}));
    let event = registered_event(f.core(), head, "dev-n");
    assert!(event["body"].get("model").is_none() && event["body"].get("modelSource").is_none());
    assert!(event["body"]["effects"][0]["fields"].get("model").is_none());
    let bad = f.core().register_agent(
        &context(f.core(), OWNER),
        &json!({"agentId": "dev-z", "roleName": "developer", "seatId": "s", "actorId": "a", "model": "m", "modelSource": "nope"}),
    );
    assert!(bad.is_err());
}

#[test]
fn without_a_model_the_payload_and_the_hash_are_as_before() {
    let f = Fixture::new();
    let payload: String = db(&f)
        .query_row(
            "SELECT payload_json FROM controller_events WHERE entity_type = 'agent' AND entity_id = 'dev-1' ORDER BY sequence LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let parsed: Value = serde_json::from_str(&payload).unwrap();
    let input = &parsed;
    let actor = input["payload"]["actorId"].as_str().unwrap();
    let expected = format!(
        r#"{{"action":"agent.register","details":null,"entityVersion":1,"payload":{{"actorId":"{actor}","agentId":"dev-1","roleName":"developer","seatId":"seat-dev-1"}}}}"#
    );
    assert_eq!(payload, expected);
    let item = item_of(f.core(), "dev-1");
    assert_eq!(
        item["contentHash"],
        "b4f045b6e6f57de6c9bcf11373ce42ab0959a698e99d40293413fd13403ce3f0"
    );
}

fn agent_report_row(f: &Fixture, agent: &str) -> String {
    let db = db(f);
    let (project, actor): (String, String) = db
        .query_row(
            "SELECT project_id, actor_id FROM agents WHERE agent_id = ?",
            [agent],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let sequence: i64 = db
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM agent_reports",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let id = format!("report-{sequence}");
    db.execute(
        "INSERT INTO agent_reports(project_id, report_id, sequence, agent_id, generation, actor_id, commit_sha, branch, summary, state, evidence_json, created_at)
         VALUES (?, ?, ?, ?, 1, ?, ?, 'b', 's', 'accepted', '{}', ?)",
        rusqlite::params![project, id, sequence, agent, actor, COMMIT, EARLY],
    )
    .unwrap();
    id
}

/// An approved plan with one package held by `agent`.
fn approved_plan(f: &Fixture, plan: &str, package: &str, agent: &str) {
    let db = db(f);
    let project: String = db
        .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
        .unwrap();
    let body = format!(
        r#"{{"summary":"s","packages":[{{"id":"{package}","title":"P","owns":["src/{package}"],"acceptance":["works"],"estimate_hours":1,"interfaces":["i"]}}],"risks":[],"integration_order":["{package}"]}}"#
    );
    let stored =
        serde_json::to_string(&capstan_kernel::plan_body::parse_plan_body(&body, 8).unwrap())
            .unwrap();
    db.execute(
        "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, architect_agent_id, current_revision, created_at, updated_at)
         VALUES (?, ?, 1, 'T', 'normal', 'draft', 'x', 'dev-1', 0, ?, ?)",
        rusqlite::params![project, plan, EARLY, EARLY],
    )
    .unwrap();
    db.execute(
        "INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
         VALUES (?, ?, 1, ?, ?, ?, 'dev-1', (SELECT actor_id FROM agents WHERE agent_id = 'dev-1'), ?)",
        rusqlite::params![project, plan, COMMIT, stored, "c".repeat(64), EARLY],
    )
    .unwrap();
    db.execute("UPDATE plans SET current_revision = 1, state = 'approved', approved_revision = 1 WHERE plan_id = ?", [plan]).unwrap();
    db.execute(
        "INSERT INTO plan_packages(project_id, plan_id, package_id, assignee_agent_id, assigned_at) VALUES (?, ?, ?, ?, ?)",
        rusqlite::params![project, plan, package, agent, EARLY],
    )
    .unwrap();
}

fn pane(f: &Fixture, agent: &str, task_ref: &str) {
    let db = db(f);
    let project: String = db
        .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
        .unwrap();
    db.execute(
        "INSERT INTO agent_panes(project_id, agent_id, generation, created_at, updated_at, task_ref) VALUES (?, ?, 1, ?, ?, ?)",
        rusqlite::params![project, agent, EARLY, EARLY, task_ref],
    )
    .unwrap();
}

#[test]
fn scope_of_a_review_subject() {
    let f = Fixture::new();
    let core = f.core();
    // A follow-up report of an agent with a requirement task ref.
    pane(&f, "dev-1", "req-7");
    let followup = agent_report_row(&f, "dev-1");
    assert_eq!(
        core.model_scope_of_subject(&followup).unwrap(),
        Some(("requirement".to_string(), "req-7".to_string()))
    );
    // The same agent on a package task ref of a plan (no package row): the part before the slash.
    db(&f)
        .execute(
            "UPDATE agent_panes SET task_ref = 'plan-4/pkg-a' WHERE agent_id = 'dev-1'",
            [],
        )
        .unwrap();
    assert_eq!(
        core.model_scope_of_subject(&followup).unwrap(),
        Some(("plan".to_string(), "plan-4".to_string()))
    );
    assert!(core.model_scope_of_subject("report-999").unwrap().is_none());
    // A plan report and an integration of it map through the package.
    let project: String = db(&f)
        .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
        .unwrap();
    approved_plan(&f, "plan-1", "a", "dev-1");
    let report = agent_report_row(&f, "dev-1");
    assert_eq!(
        core.model_scope_of_subject(&report).unwrap(),
        Some(("plan".to_string(), "plan-1".to_string()))
    );
    db(&f)
        .execute(
            "INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, created_at)
             VALUES (?, 'int-1', 1, ?, 'br', 'x', 'running', ?)",
            rusqlite::params![project, COMMIT, EARLY],
        )
        .unwrap();
    db(&f)
        .execute(
            "INSERT INTO integration_reports(project_id, integration_id, position, report_id) VALUES (?, 'int-1', 1, ?)",
            rusqlite::params![project, report],
        )
        .unwrap();
    assert_eq!(
        core.model_scope_of_subject("int-1").unwrap(),
        Some(("plan".to_string(), "plan-1".to_string()))
    );
}
