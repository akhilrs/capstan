//! `plan show` reads only what it prints. The links it needs come from `plan_links`, which reads the plan's own links by
//! the primary key of `external_links` and never evaluates the wanted state of a link outside the shown plan (that costs a
//! scan of the event log per requirement link), so the work does not grow with the other plans or links of the ledger.

use crate::fixture::Fixture;
use capstan_kernel::areas::links::{
    PACKAGE_LINKS_OF_ONE, PLAN_LINKS_OF_ALL, PLAN_LINK_OF_ONE, REQUIREMENT_REPORT_OF_CHAIN,
};
use capstan_kernel::plan_body::parse_plan_body;
use rusqlite::{types::Null, Connection};
use serde_json::Value;

const EARLY: &str = "2026-01-01T00:00:00.000Z";

fn db(f: &Fixture) -> Connection {
    Connection::open(f.database()).unwrap()
}

/// `count` plans `plan-1..`, each with two packages and a link on the plan and on each package, and `requirements`
/// requirement links.
fn ledger(f: &Fixture, count: i64, requirements: i64) {
    let conn = db(f);
    let project: String = conn
        .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
        .unwrap();
    let link = |kind: &str, id: &str| {
        conn.execute(
            "INSERT INTO external_links(project_id, ref_kind, ref_id, system, external_id, synced_state, linked_by, linked_at, synced_at)
             VALUES (?, ?, ?, 'nexora', ?, 'todo', ?, ?, ?)",
            rusqlite::params![project, kind, id, format!("n-{id}"), f.pm_actor, EARLY, EARLY],
        )
        .unwrap();
    };
    let body = serde_json::to_string(
        &parse_plan_body(
            r#"{"summary":"s","packages":[
              {"id":"api","title":"T","owns":["src/api"],"acceptance":["x"],"estimate_hours":1},
              {"id":"ui","title":"T","owns":["src/ui"],"acceptance":["x"],"estimate_hours":1}],
              "risks":[],"integration_order":["api","ui"]}"#,
            8,
        )
        .unwrap(),
    )
    .unwrap();
    for n in 1..=count {
        let plan = format!("plan-{n}");
        conn.execute(
            "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, current_revision, created_at, updated_at)
             VALUES (?, ?, ?, 'T', 'normal', 'draft', 'x', 0, ?, ?)",
            rusqlite::params![project, plan, n, EARLY, EARLY],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
             VALUES (?, ?, 1, ?, ?, ?, 'dev-1', (SELECT actor_id FROM agents WHERE agent_id = 'dev-1'), ?)",
            rusqlite::params![project, plan, "0".repeat(40), body, "c".repeat(64), EARLY],
        )
        .unwrap();
        conn.execute(
            "UPDATE plans SET current_revision = 1, state = 'approved', approved_revision = 1 WHERE plan_id = ?",
            [&plan],
        )
        .unwrap();
        link("plan", &plan);
        for package in ["api", "ui"] {
            conn.execute(
                "INSERT INTO plan_packages(project_id, plan_id, package_id) VALUES (?, ?, ?)",
                rusqlite::params![project, plan, package],
            )
            .unwrap();
            link("package", &format!("{plan}/{package}"));
        }
    }
    for n in 0..requirements {
        link("requirement", &format!("req-{n}"));
    }
}

fn ref_ids(links: &Value) -> Vec<String> {
    links
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["refId"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn one_plan_reads_its_own_links_and_those_of_its_packages_only() {
    let f = Fixture::new();
    // `plan-1` is a prefix of `plan-10`, which must not leak into its range.
    ledger(&f, 12, 30);
    let one = f
        .core()
        .plan_links(crate::fixture::OWNER, Some("plan-1"))
        .unwrap();
    assert_eq!(ref_ids(&one), ["plan-1/api", "plan-1/ui", "plan-1"]);
}

#[test]
fn a_link_outside_the_shown_plan_is_never_evaluated() {
    let f = Fixture::new();
    ledger(&f, 5, 0);
    // A link whose plan does not exist fails its wanted state, so it fails any call that evaluates it.
    let conn = db(&f);
    conn.execute(
        "INSERT INTO external_links(project_id, ref_kind, ref_id, system, external_id, synced_state, linked_by, linked_at, synced_at)
         SELECT project_id, 'plan', 'ghost', 'nexora', 'n', 'todo', ?, ?, ? FROM projects",
        rusqlite::params![f.pm_actor, EARLY, EARLY],
    )
    .unwrap();
    assert!(f.core().external_links(crate::fixture::OWNER).is_err());
    assert!(f.core().plan_links(crate::fixture::OWNER, None).is_err());
    let one = f
        .core()
        .plan_links(crate::fixture::OWNER, Some("plan-3"))
        .unwrap();
    assert_eq!(ref_ids(&one), ["plan-3/api", "plan-3/ui", "plan-3"]);
}

#[test]
fn the_list_reads_plan_links_and_no_requirement_link() {
    let f = Fixture::new();
    ledger(&f, 8, 40);
    let all = f.core().plan_links(crate::fixture::OWNER, None).unwrap();
    assert_eq!(all.as_array().unwrap().len(), 8);
    assert!(all
        .as_array()
        .unwrap()
        .iter()
        .all(|l| l["refKind"] == "plan"));
}

/// The plan lines of a statement run with every parameter null.
fn plan_of(conn: &Connection, sql: &str) -> Vec<String> {
    let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
    let nulls = vec![Null; stmt.parameter_count()];
    stmt.query_map(rusqlite::params_from_iter(nulls), |r| r.get::<_, String>(3))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn both_link_statements_search_the_primary_key_and_never_scan() {
    let f = Fixture::new();
    let conn = db(&f);
    for sql in [PLAN_LINK_OF_ONE, PACKAGE_LINKS_OF_ONE, PLAN_LINKS_OF_ALL] {
        let plan = plan_of(&conn, sql);
        for line in plan.iter().filter(|l| l.contains("external_links")) {
            assert!(
                line.starts_with("SEARCH external_links USING PRIMARY KEY")
                    && line.contains("project_id=?")
                    && line.contains("ref_kind=?"),
                "{sql}: {line}"
            );
        }
        assert!(
            plan.iter().any(|l| l.contains("external_links")),
            "{plan:?}"
        );
        assert!(!plan.iter().any(|l| l.starts_with("SCAN")), "{plan:?}");
    }
}

#[test]
fn the_replacement_chain_of_a_requirement_link_reads_the_successor_index_and_not_the_event_log() {
    let f = Fixture::new();
    let conn = db(&f);
    let plan = plan_of(&conn, REQUIREMENT_REPORT_OF_CHAIN);
    assert!(
        plan.iter()
            .any(|l| l.contains("SEARCH e USING INDEX controller_events_by_successor")),
        "{plan:?}"
    );
    // No step of the walk reads the event log any other way (a scan, or the primary key by project).
    assert!(
        !plan
            .iter()
            .any(|l| l.contains("controller_events")
                && !l.contains("controller_events_by_successor")),
        "{plan:?}"
    );
}
