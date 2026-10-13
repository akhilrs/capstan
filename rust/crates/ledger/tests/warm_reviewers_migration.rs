//! The warm-reviewers migration (the file whose name ends in `_warm_reviewers.sql`): a ledger at the previous version migrates
//! with its rows intact, and the table's constraints keep one warm reviewer per plan and one plan per reviewer, and a released
//! row final. No version number is written here: it comes from the file name.
mod common;

use capstan_ledger::{
    max_embedded_migration, migrate, migrations, open_database, Database, OpenOptions,
};
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

fn version() -> usize {
    migrations()
        .iter()
        .find(|m| m.name.ends_with("_warm_reviewers.sql"))
        .expect("the warm-reviewers migration is embedded")
        .version as usize
}

/// The fixture one version before the migration; returns the project id.
fn ledger_before(file: &Path) -> String {
    std::fs::copy(fixture(), file).unwrap();
    let db = Database::new(Connection::open(file).unwrap());
    migrate(&db, file, &migrations()[..version() - 1]).unwrap();
    db.close().unwrap();
    let conn = Connection::open(file).unwrap();
    conn.query_row("SELECT project_id FROM projects", [], |r| r.get(0))
        .unwrap()
}

fn migrated(file: &Path) -> Connection {
    open_database(file, &keep_all()).unwrap().close().unwrap();
    let conn = Connection::open(file).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    conn
}

fn insert(conn: &Connection, project: &str, plan: &str, agent: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "INSERT INTO warm_reviewers(project_id, plan_id, reviewer_role, agent_id, reviews_count, created_at, last_used_at)
         VALUES (?, ?, 'reviewer', ?, 1, ?, ?)",
        rusqlite::params![project, plan, agent, NOW, NOW],
    )
}

#[test]
fn a_ledger_at_the_previous_version_migrates_with_its_rows_intact() {
    let (_dir, file) = scratch();
    ledger_before(&file);
    let before = Connection::open(&file).unwrap();
    let reports: i64 = before
        .query_row("SELECT count(*) FROM agent_reports", [], |r| r.get(0))
        .unwrap();
    drop(before);
    let conn = migrated(&file);
    let versions: i64 = conn
        .query_row("SELECT count(*) FROM schema_migrations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(versions, max_embedded_migration());
    let after: i64 = conn
        .query_row("SELECT count(*) FROM agent_reports", [], |r| r.get(0))
        .unwrap();
    assert_eq!(reports, after);
    let warm: i64 = conn
        .query_row("SELECT count(*) FROM warm_reviewers", [], |r| r.get(0))
        .unwrap();
    assert_eq!(warm, 0);
    let (strict, without_rowid): (i64, i64) = conn
        .query_row(
            "SELECT strict, wr FROM pragma_table_list WHERE name = 'warm_reviewers'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((strict, without_rowid), (1, 1));
}

#[test]
fn one_unreleased_row_per_plan_and_per_agent() {
    let (_dir, file) = scratch();
    let project = ledger_before(&file);
    let conn = migrated(&file);
    insert(&conn, &project, "plan-1", "rev-1").unwrap();
    assert!(insert(&conn, &project, "plan-1", "rev-2").is_err());
    assert!(insert(&conn, &project, "plan-2", "rev-1").is_err());
    insert(&conn, &project, "plan-2", "rev-2").unwrap();
    conn.execute(
        "UPDATE warm_reviewers SET released_at = ?, release_reason = 'signoff' WHERE plan_id = 'plan-1'",
        [NOW],
    )
    .unwrap();
    insert(&conn, &project, "plan-1", "rev-3").unwrap();
    insert(&conn, &project, "plan-3", "rev-1").unwrap();
}

#[test]
fn a_released_row_is_final_and_the_reason_is_checked() {
    let (_dir, file) = scratch();
    let project = ledger_before(&file);
    let conn = migrated(&file);
    insert(&conn, &project, "plan-1", "rev-1").unwrap();
    let error = conn
        .execute(
            "UPDATE warm_reviewers SET release_reason = 'because', released_at = ?",
            [NOW],
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("CHECK"), "{error}");
    let error = conn
        .execute("UPDATE warm_reviewers SET reviews_count = -1", [])
        .unwrap_err()
        .to_string();
    assert!(error.contains("CHECK"), "{error}");
    let error = conn
        .execute("UPDATE warm_reviewers SET plan_id = 'plan-9'", [])
        .unwrap_err()
        .to_string();
    assert!(error.contains("immutable"), "{error}");
    conn.execute(
        "UPDATE warm_reviewers SET released_at = ?, release_reason = 'idle'",
        [NOW],
    )
    .unwrap();
    for sql in [
        "UPDATE warm_reviewers SET reviews_count = 5",
        "UPDATE warm_reviewers SET release_reason = 'cancel'",
        "UPDATE warm_reviewers SET released_at = NULL, release_reason = NULL",
    ] {
        let error = conn.execute(sql, []).unwrap_err().to_string();
        assert!(error.contains("final"), "{sql}: {error}");
    }
    let error = conn
        .execute("DELETE FROM warm_reviewers", [])
        .unwrap_err()
        .to_string();
    assert!(error.contains("immutable"), "{error}");
}
