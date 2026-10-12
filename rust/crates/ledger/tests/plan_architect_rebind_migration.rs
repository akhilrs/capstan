//! Migration 0039 (plan architect rebinding): a ledger at schema 38 with plans migrates with every row intact; a plan's
//! architect can move to the active successor of an ended architect of the same role, and to nothing else.
mod common;

use capstan_ledger::{migrate, migrations, open_database, Database, OpenOptions};
use common::scratch;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/ledger/ledger-better-sqlite3.sqlite")
}

fn keep_all() -> OpenOptions {
    OpenOptions {
        keep_migration_backups: 100,
        ..OpenOptions::default()
    }
}

const NOW: &str = "2026-01-01T00:00:00.000Z";

/// The fixture at schema 38 with extra agents and plans: architects `old` (role architect), `new` (same role),
/// `other` (another role) and `idle` (same role, ended), plans `open` (architect old) and `done` (superseded, architect old).
fn ledger_0038(file: &Path) -> String {
    std::fs::copy(fixture(), file).unwrap();
    let db = Database::new(Connection::open(file).unwrap());
    migrate(&db, file, &migrations()[..38]).unwrap();
    db.close().unwrap();
    let conn = Connection::open(file).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    let project: String = conn
        .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
        .unwrap();
    for (agent, role, state) in [
        ("old", "architect", "ended"),
        ("new", "architect", "active"),
        ("new2", "architect", "active"),
        ("other", "reviewer", "active"),
        ("idle", "architect", "ended"),
    ] {
        conn.execute(
            "INSERT INTO agents(project_id, agent_id, role_name, kind, seat_id, actor_id, generation, state, last_activity_at, created_at)
             VALUES (?, ?, ?, 'Developer', ?, ?, 1, 'active', ?, ?)",
            rusqlite::params![project, agent, role, format!("seat-{agent}"), format!("actor-{agent}"), NOW, NOW],
        )
        .unwrap();
        if state == "ended" {
            conn.execute(
                "UPDATE agents SET state = 'ended', ended_at = ? WHERE agent_id = ?",
                rusqlite::params![NOW, agent],
            )
            .unwrap();
        }
    }
    for (n, plan) in ["open", "done"].into_iter().enumerate() {
        conn.execute(
            "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, architect_agent_id, current_revision, approved_revision, created_at, updated_at)
             VALUES (?, ?, ?, 't', 'normal', 'draft', 'pm', 'old', 0, NULL, ?, ?)",
            rusqlite::params![project, plan, 100 + n as i64, NOW, NOW],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
         VALUES (?, 'done', 1, ?, '{}', ?, 'old', 'actor-old', ?)",
        rusqlite::params![project, "a".repeat(40), "f".repeat(64), NOW],
    )
    .unwrap();
    conn.execute(
        "UPDATE plans SET current_revision = 1 WHERE plan_id = 'done'",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE plans SET state = 'approved', approved_revision = 1 WHERE plan_id = 'done'",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE plans SET state = 'superseded' WHERE plan_id = 'done'",
        [],
    )
    .unwrap();
    project
}

fn architect_of(conn: &Connection, plan: &str) -> String {
    conn.query_row(
        "SELECT architect_agent_id FROM plans WHERE plan_id = ?",
        [plan],
        |r| r.get(0),
    )
    .unwrap()
}

fn rebind(conn: &Connection, plan: &str, to: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE plans SET architect_agent_id = ? WHERE plan_id = ?",
        [to, plan],
    )
}

#[test]
fn a_ledger_at_schema_38_migrates_to_39_with_its_plans_intact() {
    let (_dir, file) = scratch();
    ledger_0038(&file);
    let before = Connection::open(&file).unwrap();
    let plans_before: Vec<(String, Option<String>)> = before
        .prepare("SELECT plan_id, architect_agent_id FROM plans ORDER BY plan_id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    drop(before);
    open_database(&file, &keep_all()).unwrap().close().unwrap();
    let conn = Connection::open(&file).unwrap();
    let versions: i64 = conn
        .query_row("SELECT count(*) FROM schema_migrations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(versions, 39);
    let plans_after: Vec<(String, Option<String>)> = conn
        .prepare("SELECT plan_id, architect_agent_id FROM plans ORDER BY plan_id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(plans_before, plans_after);
}

#[test]
fn the_architect_moves_only_to_an_active_agent_of_the_same_role_after_the_old_one_stopped() {
    let (_dir, file) = scratch();
    ledger_0038(&file);
    open_database(&file, &keep_all()).unwrap().close().unwrap();
    let conn = Connection::open(&file).unwrap();
    let refused = |plan: &str, to: &str| {
        let error = rebind(&conn, plan, to).unwrap_err().to_string();
        assert!(error.contains("the plan architect is final"), "{error}");
    };
    refused("open", "other");
    refused("open", "idle");
    refused("open", "nobody");
    refused("done", "new");
    assert_eq!(architect_of(&conn, "open"), "old");
    assert_eq!(architect_of(&conn, "done"), "old");
    rebind(&conn, "open", "new").unwrap();
    assert_eq!(architect_of(&conn, "open"), "new");
}

#[test]
fn an_architect_that_is_still_active_is_not_replaced() {
    let (_dir, file) = scratch();
    ledger_0038(&file);
    open_database(&file, &keep_all()).unwrap().close().unwrap();
    let conn = Connection::open(&file).unwrap();
    // The architect of this plan is the active `new`; moving it to another agent of any role is refused.
    conn.execute(
        "UPDATE plans SET architect_agent_id = 'new' WHERE plan_id = 'open'",
        [],
    )
    .unwrap();
    for to in ["other", "new2"] {
        let error = rebind(&conn, "open", to).unwrap_err().to_string();
        assert!(error.contains("the plan architect is final"), "{error}");
    }
    assert_eq!(architect_of(&conn, "open"), "new");
}
