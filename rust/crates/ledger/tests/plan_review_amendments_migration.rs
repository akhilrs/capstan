//! The plan review amendment migration: a ledger at schema 39 with plans, revisions and finished reviews migrates with every
//! row intact and the new columns empty; afterwards the ledger refuses an amended revision that does not answer a mechanical
//! findings review of the previous revision.
mod common;

use capstan_ledger::{max_embedded_migration, open_database, OpenOptions};
use common::scratch;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/ledger/schema-39-messages.sqlite")
}

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const BODY: &str = r#"{"packages":[{"id":"a"}]}"#;

fn ids(conn: &Connection) -> (String, String, String, String) {
    let one = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, String>(0)).unwrap();
    (
        one("SELECT project_id FROM projects"),
        one("SELECT agent_id FROM agents WHERE kind = 'Developer'"),
        one("SELECT actor_id FROM agents WHERE kind = 'Developer'"),
        one("SELECT actor_id FROM agents WHERE kind = 'PM'"),
    )
}

/// A plan in review at revision 1, with its first review started; the reviewer is the PM seat, which is enough for the ledger.
fn plan_in_review(conn: &Connection, plan: &str, sequence: i64) {
    let (project, agent, actor, _) = ids(conn);
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    conn.execute(
        "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, architect_agent_id, current_revision, created_at, updated_at)
         VALUES (?, ?, ?, 't', 'normal', 'draft', ?, ?, 0, 'now', 'now')",
        rusqlite::params![project, plan, sequence, actor, agent],
    )
    .unwrap();
    add_revision(conn, plan, 1, None).unwrap();
    conn.execute(
        "UPDATE plans SET current_revision = 1, state = 'in_review' WHERE plan_id = ?",
        [plan],
    )
    .unwrap();
}

fn add_revision(
    conn: &Connection,
    plan: &str,
    revision: i64,
    amends: Option<&str>,
) -> rusqlite::Result<usize> {
    let (project, agent, actor, _) = ids(conn);
    let sql = if amends.is_some() {
        "INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at, amendment_json, amends_review_id)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'now', '{\"reason\":\"r\"}', ?)"
    } else {
        "INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'now')"
    };
    let sha = "f".repeat(64);
    match amends {
        Some(review) => conn.execute(
            sql,
            rusqlite::params![project, plan, revision, SHA, BODY, sha, agent, actor, review],
        ),
        None => conn.execute(
            sql,
            rusqlite::params![project, plan, revision, SHA, BODY, sha, agent, actor],
        ),
    }
}

fn start_review(conn: &Connection, plan: &str, review: &str, round: i64, revision: i64) {
    let (project, agent, actor, reviewer) = ids(conn);
    let reviewer_agent: String = conn
        .query_row("SELECT agent_id FROM agents WHERE kind = 'PM'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let sequence: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM reviews",
            [],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO reviews(project_id, review_id, sequence, round, subject_plan_id, subject_plan_revision, commit_sha, base_sha, author_agent_id,
           author_actor_id, requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, state, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'reviewer', ?, ?, 'started', 'now')",
        rusqlite::params![
            project, review, sequence, round, plan, revision, SHA, SHA, agent, actor, actor, reviewer_agent, reviewer
        ],
    )
    .unwrap();
}

/// Finishes a review on the schema before the migration, which has no class.
fn finish_unclassified(conn: &Connection, review: &str, state: &str) {
    conn.execute(
        "UPDATE reviews SET state = ?, verdict_text = 'v', completed_at = 'later' WHERE review_id = ?",
        rusqlite::params![state, review],
    )
    .unwrap();
}

fn finish(
    conn: &Connection,
    review: &str,
    state: &str,
    class: Option<&str>,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE reviews SET state = ?, verdict_text = 'v', completed_at = 'later', findings_class = ? WHERE review_id = ?",
        rusqlite::params![state, class, review],
    )
}

#[test]
fn migration_0041_keeps_plans_and_reviews_and_leaves_the_new_columns_empty() {
    let (_dir, file) = scratch();
    std::fs::copy(fixture(), &file).unwrap();
    {
        let conn = Connection::open(&file).unwrap();
        assert!(conn.prepare("SELECT max_review_rounds FROM plans").is_err());
        plan_in_review(&conn, "plan-1", 1);
        start_review(&conn, "plan-1", "r1", 1, 1);
        finish_unclassified(&conn, "r1", "findings");
        conn.execute(
            "UPDATE plans SET state = 'draft' WHERE plan_id = 'plan-1'",
            [],
        )
        .unwrap();
        add_revision(&conn, "plan-1", 2, None).unwrap();
        conn.execute(
            "UPDATE plans SET current_revision = 2, state = 'in_review' WHERE plan_id = 'plan-1'",
            [],
        )
        .unwrap();
        start_review(&conn, "plan-1", "r2", 2, 2);
        finish_unclassified(&conn, "r2", "passed");
    }
    let before: Vec<String> = {
        let conn = Connection::open(&file).unwrap();
        let mut statement = conn
            .prepare(
                "SELECT review_id || state || verdict_text || round FROM reviews ORDER BY sequence",
            )
            .unwrap();
        statement
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };

    open_database(
        &file,
        &OpenOptions {
            keep_migration_backups: 100,
            ..OpenOptions::default()
        },
    )
    .unwrap()
    .close()
    .unwrap();

    let conn = Connection::open(&file).unwrap();
    let latest: i64 = conn
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(latest, max_embedded_migration());
    let after: Vec<String> = {
        let mut statement = conn
            .prepare(
                "SELECT review_id || state || verdict_text || round FROM reviews ORDER BY sequence",
            )
            .unwrap();
        statement
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(after, before);
    let nulls: i64 = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM plans WHERE max_review_rounds IS NOT NULL)
                  + (SELECT COUNT(*) FROM plan_revisions WHERE amendment_json IS NOT NULL OR amends_review_id IS NOT NULL)
                  + (SELECT COUNT(*) FROM reviews WHERE findings_class IS NOT NULL)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(nulls, 0);
    let (plans, revisions): (i64, i64) = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM plans), (SELECT COUNT(*) FROM plan_revisions)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((plans, revisions), (1, 2));
}

#[test]
fn the_ledger_accepts_an_amendment_of_a_mechanical_review_and_refuses_the_rest() {
    let (_dir, file) = scratch();
    std::fs::copy(fixture(), &file).unwrap();
    open_database(&file, &OpenOptions::default())
        .unwrap()
        .close()
        .unwrap();
    let conn = Connection::open(&file).unwrap();

    // The limit of a plan is between 1 and 20 and does not change.
    plan_in_review(&conn, "plan-1", 1);
    assert!(conn
        .execute(
            "UPDATE plans SET max_review_rounds = 3 WHERE plan_id = 'plan-1'",
            []
        )
        .is_err());

    start_review(&conn, "plan-1", "r1", 1, 1);
    // A class belongs to a plan review that finished with findings.
    assert!(finish(&conn, "r1", "passed", Some("mechanical")).is_err());
    assert!(finish(&conn, "r1", "findings", Some("loud")).is_err());
    finish(&conn, "r1", "findings", Some("design")).unwrap();
    assert!(
        conn.execute(
            "UPDATE reviews SET findings_class = 'mechanical' WHERE review_id = 'r1'",
            []
        )
        .is_err(),
        "final"
    );
    conn.execute(
        "UPDATE plans SET state = 'draft' WHERE plan_id = 'plan-1'",
        [],
    )
    .unwrap();

    // Design findings are not amended; neither is a review that does not exist; the pair is set together.
    assert!(add_revision(&conn, "plan-1", 2, Some("r1")).is_err());
    assert!(add_revision(&conn, "plan-1", 2, Some("nope")).is_err());
    assert!(conn
        .execute(
            "INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at, amends_review_id)
             SELECT project_id, plan_id, 2, base_sha, body_json, body_sha, author_agent_id, author_actor_id, 'now', 'r1' FROM plan_revisions WHERE revision = 1",
            [],
        )
        .is_err());

    // A second plan whose review ended in mechanical findings is amended once.
    plan_in_review(&conn, "plan-2", 2);
    start_review(&conn, "plan-2", "r2", 1, 1);
    finish(&conn, "r2", "findings", Some("mechanical")).unwrap();
    conn.execute(
        "UPDATE plans SET state = 'draft' WHERE plan_id = 'plan-2'",
        [],
    )
    .unwrap();
    assert!(
        add_revision(&conn, "plan-1", 2, Some("r2")).is_err(),
        "another plan's review"
    );
    add_revision(&conn, "plan-2", 2, Some("r2")).unwrap();
    conn.execute(
        "UPDATE plans SET current_revision = 2 WHERE plan_id = 'plan-2'",
        [],
    )
    .unwrap();
    assert!(
        add_revision(&conn, "plan-2", 3, Some("r2")).is_err(),
        "the review was amended"
    );
}
