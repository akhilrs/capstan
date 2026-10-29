//! The remote-decisions migration (the file whose name ends in `_remote_decisions.sql`): a fresh ledger and a ledger at the
//! previous version both get the table, the ledger is backed up first, and the controller_events triggers are untouched. No
//! version number is written here: it comes from the file name.
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
        .find(|m| m.name.ends_with("_remote_decisions.sql"))
        .expect("the remote-decisions migration is embedded")
        .version as usize
}

fn triggers(conn: &Connection) -> Vec<(String, String)> {
    let mut statement = conn
        .prepare("SELECT name, sql FROM sqlite_master WHERE type = 'trigger' ORDER BY name")
        .unwrap();
    let rows = statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    rows
}

fn insert(conn: &Connection, project: &str, nonce: &str, outcome: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "INSERT INTO remote_decisions(project_id, nonce, decision_id, request_id, content_hash, verb, args_json, client, outcome, result_json, decided_at)
         VALUES (?, ?, ?, 'op:proposal-1', ?, 'deny', '{}', 'remote', ?, '{}', ?)",
        rusqlite::params![project, nonce, format!("decision-{nonce}"), "a".repeat(64), outcome, NOW],
    )
}

#[test]
fn a_ledger_at_the_previous_version_migrates_after_a_backup_and_keeps_its_triggers() {
    let (dir, file) = scratch();
    std::fs::copy(fixture(), &file).unwrap();
    let db = Database::new(Connection::open(&file).unwrap());
    migrate(&db, &file, &migrations()[..version() - 1]).unwrap();
    db.close().unwrap();
    let before = Connection::open(&file).unwrap();
    let project: String = before
        .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
        .unwrap();
    let reports: i64 = before
        .query_row("SELECT count(*) FROM agent_reports", [], |r| r.get(0))
        .unwrap();
    let old_triggers = triggers(&before);
    drop(before);

    open_database(&file, &keep_all()).unwrap().close().unwrap();

    let conn = Connection::open(&file).unwrap();
    let versions: i64 = conn
        .query_row("SELECT count(*) FROM schema_migrations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(versions, max_embedded_migration());
    let after: i64 = conn
        .query_row("SELECT count(*) FROM agent_reports", [], |r| r.get(0))
        .unwrap();
    assert_eq!(reports, after);
    let rows: i64 = conn
        .query_row("SELECT count(*) FROM remote_decisions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0);
    assert_eq!(old_triggers, triggers(&conn), "no trigger changed");
    assert!(conn.execute_batch("PRAGMA foreign_keys = OFF").is_ok());
    insert(&conn, &project, "nonce-0001", "forwarded").unwrap();
    let prefix = format!("controller.sqlite.pre-v{}-", version() - 1);
    let backup = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.unwrap().file_name().into_string().ok())
        .any(|n| n.starts_with(&prefix));
    assert!(backup, "the ledger is backed up before the migration");
}

#[test]
fn a_fresh_ledger_has_the_table_with_its_keys_and_checks() {
    let (_dir, file) = scratch();
    open_database(&file, &keep_all()).unwrap().close().unwrap();
    let conn = Connection::open(&file).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    insert(&conn, "p", "nonce-0001", "applied").unwrap();
    let error = insert(&conn, "p", "nonce-0001", "applied")
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("UNIQUE") || error.contains("PRIMARY"),
        "{error}"
    );
    insert(&conn, "q", "nonce-0001", "forwarded").unwrap();
    let error = insert(&conn, "p", "nonce-0002", "maybe")
        .unwrap_err()
        .to_string();
    assert!(error.contains("CHECK"), "{error}");
    let index: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = 'remote_decisions_by_request'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(index, 1);
}
