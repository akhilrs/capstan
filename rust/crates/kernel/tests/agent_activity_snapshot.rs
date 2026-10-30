//! The activity of an agent on the agents snapshot item: the latest `agent_state_history` row, its tie-break, a generation
//! replace, the content hash, and the query plan of the statements that read it.

mod common;
#[path = "rust_only/fixture.rs"]
mod fixture;

use capstan_kernel::areas::snapshot_feed::{
    actual_reads, actual_reads_reset, agent_content_hash, agent_sql_texts, agents_page,
    agents_page_counted, snapshot_item, AgentItem, MAX_READS,
};
use fixture::{context, Fixture, OWNER};
use serde_json::{json, Value};

fn observe(f: &Fixture, state: &str) {
    let core = f.core();
    core.record_agent_observation(&context(core, OWNER), "dev-1", &json!(state))
        .unwrap();
}

fn page_agent(f: &Fixture) -> AgentItem {
    let page = agents_page(f.core(), None, 10).unwrap();
    page.items
        .into_iter()
        .find(|a| a.agent_id == "dev-1")
        .unwrap()
}

fn item_json(f: &Fixture) -> Value {
    snapshot_item(f.core(), "agents", "dev-1")
        .unwrap()
        .expect("the agent item")
}

fn fields(item: &Value) -> (Value, Value, Value) {
    (
        item["activity"].clone(),
        item["activitySince"].clone(),
        item["activitySeq"].clone(),
    )
}

fn history(f: &Fixture) -> Vec<(i64, String, String)> {
    let db = rusqlite::Connection::open(f.database()).unwrap();
    let mut stmt = db
        .prepare("SELECT sequence, herdr_state, observed_at FROM agent_state_history WHERE agent_id = 'dev-1' ORDER BY sequence")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn an_agent_never_observed_has_no_activity() {
    let f = Fixture::new();
    let agent = page_agent(&f);
    assert_eq!(
        (agent.activity, agent.activity_since, agent.activity_seq),
        (None, None, None)
    );
    let item = item_json(&f);
    assert_eq!(fields(&item), (Value::Null, Value::Null, Value::Null));
    assert!(item.as_object().unwrap().contains_key("activitySeq"));
}

#[test]
fn the_latest_observation_is_the_activity_in_the_page_and_the_item() {
    let f = Fixture::new();
    observe(&f, "working");
    observe(&f, "idle");
    let rows = history(&f);
    let (seq, state, at) = rows.last().unwrap().clone();
    assert_eq!(state, "idle");
    let agent = page_agent(&f);
    assert_eq!(agent.activity.as_deref(), Some("idle"));
    assert_eq!(agent.activity_since.as_deref(), Some(at.as_str()));
    assert_eq!(agent.activity_seq, Some(seq));
    let item = item_json(&f);
    assert_eq!(fields(&item), (json!("idle"), json!(at), json!(seq)));
}

#[test]
fn the_greater_sequence_wins_when_observed_at_is_equal() {
    let mut f = Fixture::new();
    f.close();
    {
        let db = rusqlite::Connection::open(f.database()).unwrap();
        let project: String = db
            .query_row("SELECT project_id FROM agents LIMIT 1", [], |r| r.get(0))
            .unwrap();
        for (seq, state) in [(1, "idle"), (2, "blocked"), (3, "working")] {
            db.execute(
                "INSERT INTO agent_state_history(project_id, agent_id, sequence, herdr_state, observed_at) VALUES (?, 'dev-1', ?, ?, '2026-01-01T00:00:00.000Z')",
                rusqlite::params![project, seq, state],
            )
            .unwrap();
        }
    }
    f.reopen();
    let agent = page_agent(&f);
    assert_eq!(agent.activity.as_deref(), Some("working"));
    assert_eq!(agent.activity_seq, Some(3));
    assert_eq!(item_json(&f)["activity"], "working");
    assert_eq!(item_json(&f)["activitySeq"], 3);
}

#[test]
fn a_generation_replace_shows_unknown_with_a_greater_sequence() {
    let f = Fixture::new();
    observe(&f, "working");
    let before = page_agent(&f).activity_seq.unwrap();
    let core = f.core();
    core.replace_agent_generation(&context(core, OWNER), "dev-1")
        .unwrap();
    let agent = page_agent(&f);
    assert_eq!(agent.activity.as_deref(), Some("unknown"));
    assert!(agent.activity_seq.unwrap() > before);
    let item = item_json(&f);
    assert_eq!(item["activity"], "unknown");
    assert!(item["activitySeq"].as_i64().unwrap() > before);
}

#[test]
fn an_observation_does_not_change_the_content_hash() {
    let f = Fixture::new();
    let before = page_agent(&f);
    observe(&f, "working");
    observe(&f, "idle");
    let after = page_agent(&f);
    assert_eq!(before.content_hash, after.content_hash);
    assert_eq!(
        after.content_hash,
        agent_content_hash("dev-1", after.generation, &after.state, after.paused, None)
    );
    assert_eq!(item_json(&f)["contentHash"], json!(after.content_hash));
}

fn fill_history(f: &mut Fixture, rows: i64) {
    f.close();
    let mut db = rusqlite::Connection::open(f.database()).unwrap();
    let project: String = db
        .query_row("SELECT project_id FROM agents LIMIT 1", [], |r| r.get(0))
        .unwrap();
    let tx = db.transaction().unwrap();
    for seq in 1..=rows {
        tx.execute(
            "INSERT INTO agent_state_history(project_id, agent_id, sequence, herdr_state, observed_at) VALUES (?, 'dev-1', ?, 'working', '2026-01-01T00:00:00.000Z')",
            rusqlite::params![project, seq],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    drop(db);
    f.reopen();
}

#[test]
fn the_history_is_read_by_primary_key_and_costs_no_extra_reads() {
    let mut f = Fixture::new();
    actual_reads_reset();
    let one = agents_page_counted(f.core(), None, 10, MAX_READS).unwrap();
    let reads_one = actual_reads();
    fill_history(&mut f, 5000);
    actual_reads_reset();
    let many = agents_page_counted(f.core(), None, 10, MAX_READS).unwrap();
    assert_eq!(actual_reads(), reads_one);
    assert_eq!(one.1, many.1);
    assert_eq!(many.0.items.len(), one.0.items.len());

    let db = rusqlite::Connection::open(f.database()).unwrap();
    let texts = agent_sql_texts(f.core());
    for (sql, binds) in [(&texts[0], 3), (&texts[1], 2)] {
        let params: Vec<String> = vec!["p".to_string(); binds];
        let mut stmt = db.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let plan: Vec<String> = stmt
            .query_map(rusqlite::params_from_iter(params), |r| {
                r.get::<_, String>(3)
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let plan = plan.join("\n");
        assert!(plan.contains("SEARCH h"), "{plan}");
        assert!(plan.contains("USING PRIMARY KEY"), "{plan}");
        assert!(!plan.contains("SCAN h"), "{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "{plan}");
    }
}
