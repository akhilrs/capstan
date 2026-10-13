//! The design-approvals migration (the file whose name ends in `_design_approvals.sql`): a ledger at the previous version
//! migrates with its rows intact, and the table's rows are immutable. No version number is written here: it comes from the file name.
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
        .find(|m| m.name.ends_with("_design_approvals.sql"))
        .expect("the design-approvals migration is embedded")
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

fn insert(
    conn: &Connection,
    project: &str,
    decision: &str,
    link: Option<&str>,
) -> rusqlite::Result<usize> {
    conn.execute(
        "INSERT INTO design_approvals(project_id, decision_id, sequence, plan_id, package_id, decision, report_id, link, asked_user, decided_by, decided_at)
         VALUES (?, lower(hex(randomblob(8))), (SELECT COALESCE(MAX(sequence), 0) + 1 FROM design_approvals), 'plan-1', 'ui', ?, 'report-1', ?, 'looks good', 'pm', ?)",
        rusqlite::params![project, decision, link, NOW],
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
    let rows: i64 = conn
        .query_row("SELECT count(*) FROM design_approvals", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0);
}

#[test]
fn decisions_are_checked_and_rows_are_immutable() {
    let (_dir, file) = scratch();
    let project = ledger_before(&file);
    let conn = migrated(&file);
    let error = insert(&conn, &project, "maybe", Some("l"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("CHECK"), "{error}");
    let error = insert(&conn, &project, "approved", None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("CHECK"), "{error}");
    insert(&conn, &project, "approved", Some("https://design/1")).unwrap();
    insert(&conn, &project, "changes", None).unwrap();
    for sql in [
        "UPDATE design_approvals SET decision = 'changes'",
        "UPDATE design_approvals SET asked_user = 'other'",
        "DELETE FROM design_approvals",
    ] {
        let error = conn.execute(sql, []).unwrap_err().to_string();
        assert!(error.contains("immutable"), "{sql}: {error}");
    }
}
