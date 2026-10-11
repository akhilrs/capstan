//! Migration 0038 (the cleanup records): a ledger at schema 37 that holds integrations and reports migrates with every
//! row intact, the new tables are STRICT and empty, and their triggers keep the records append-only.
mod common;

use capstan_ledger::{migrate, migrations, open_database, Database, OpenOptions};
use common::scratch;
use rusqlite::{Connection, OpenFlags};
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

/// Every row of every table, as sorted text per table.
fn row_dump(file: &Path) -> std::collections::BTreeMap<String, Vec<String>> {
    let conn = Connection::open_with_flags(file, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut dump = std::collections::BTreeMap::new();
    for table in tables {
        let columns: Vec<String> = conn
            .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let select = columns
            .iter()
            .map(|c| format!("quote(\"{c}\")"))
            .collect::<Vec<_>>()
            .join(" || '|' || ");
        let mut rows: Vec<String> = conn
            .prepare(&format!("SELECT {select} FROM \"{table}\""))
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        rows.sort();
        dump.insert(table, rows);
    }
    dump
}

/// The fixture brought to schema 37, with a confirmed integration that holds a report.
fn ledger_0037(file: &Path) {
    std::fs::copy(fixture(), file).unwrap();
    let db = Database::new(Connection::open(file).unwrap());
    migrate(&db, file, &migrations()[..37]).unwrap();
    db.close().unwrap();
    let conn = Connection::open(file).unwrap();
    let project: String = conn
        .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
        .unwrap();
    let report: String = conn
        .query_row("SELECT report_id FROM agent_reports", [], |r| r.get(0))
        .unwrap();
    conn.execute(
        "INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, created_at)
         VALUES (?, 'int-old', 1, ?, 'capstan/integration/int-old', 'operator', 'running', '2026-01-01T00:00:00Z')",
        rusqlite::params![project, "a".repeat(40)],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO integration_reports(project_id, integration_id, position, report_id) VALUES (?, 'int-old', 1, ?)",
        rusqlite::params![project, report],
    )
    .unwrap();
    conn.execute(
        "UPDATE integrations SET state = 'merged', head_sha = ?, completed_at = '2026-01-01T00:00:01Z' WHERE integration_id = 'int-old'",
        ["c".repeat(40)],
    )
    .unwrap();
    conn.execute(
        "UPDATE integrations SET state = 'confirmed' WHERE integration_id = 'int-old'",
        [],
    )
    .unwrap();
}

#[test]
fn a_ledger_at_schema_37_migrates_to_38_with_every_row_intact() {
    let (dir, file) = scratch();
    ledger_0037(&file);
    let before = row_dump(&file);
    assert_eq!(before["schema_migrations"].len(), 37);
    assert_eq!(before["integrations"].len(), 1);
    open_database(&file, &keep_all()).unwrap().close().unwrap();
    let mut after = row_dump(&file);
    let migrations_after = after.remove("schema_migrations").unwrap();
    assert_eq!(migrations_after.len(), 38);
    assert!(migrations_after
        .iter()
        .any(|r| r.contains("0038_cleanups.sql")));
    assert_eq!(after.remove("cleanups"), Some(vec![]));
    assert_eq!(after.remove("cleanup_branches"), Some(vec![]));
    let mut before = before;
    before.remove("schema_migrations");
    assert_eq!(before, after, "no existing row changed");
    let backup = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.unwrap().file_name().into_string().ok())
        .any(|n| n.starts_with("controller.sqlite.pre-v37-"));
    assert!(backup, "the ledger is backed up before 0038");
}

#[test]
fn the_cleanup_tables_are_strict_and_their_records_only_move_forward() {
    let (_dir, file) = scratch();
    ledger_0037(&file);
    open_database(&file, &keep_all()).unwrap().close().unwrap();
    let conn = Connection::open(&file).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
    for table in ["cleanups", "cleanup_branches"] {
        let strict: i64 = conn
            .query_row("SELECT strict FROM pragma_table_list(?)", [table], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(strict, 1, "{table} is STRICT");
    }
    let project: String = conn
        .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
        .unwrap();
    let insert = |id: &str, sequence: i64, trigger: &str, mode: &str, state: &str| {
        conn.execute(
            "INSERT INTO cleanups(project_id, cleanup_id, sequence, trigger, integration_id, requested_by, mode, state, created_at, completed_at)
             VALUES (?, ?, ?, ?, 'int-old', 'operator', ?, ?, '2026-01-02T00:00:00Z', CASE WHEN ? = 'offered' THEN NULL ELSE '2026-01-02T00:00:01Z' END)",
            rusqlite::params![project, id, sequence, trigger, mode, state, state],
        )
    };
    assert!(insert("c-bad", 1, "sometimes", "ask", "offered").is_err());
    assert!(insert("c-bad", 1, "confirm", "maybe", "offered").is_err());
    assert!(insert("c-bad", 1, "confirm", "ask", "done").is_err());
    insert("c-1", 1, "confirm", "ask", "offered").unwrap();
    assert!(
        insert("c-2", 1, "manual", "ask", "offered").is_err(),
        "sequences are unique"
    );
    let branch = |name: &str, sha: &str, class: &str, outcome: &str| {
        conn.execute(
            "INSERT INTO cleanup_branches(project_id, cleanup_id, branch, sha, class, outcome, decided_at)
             VALUES (?, 'c-1', ?, ?, ?, ?, CASE WHEN ? = 'offered' THEN NULL ELSE '2026-01-02T00:00:02Z' END)",
            rusqlite::params![project, name, sha, class, outcome, outcome],
        )
    };
    assert!(
        branch("b", &"A".repeat(40), "landed", "offered").is_err(),
        "the sha is lowercase hex"
    );
    assert!(
        branch("b", &"a".repeat(39), "landed", "offered").is_err(),
        "the sha has 40 characters"
    );
    assert!(branch("b", &"a".repeat(40), "merged", "offered").is_err());
    assert!(branch("b", &"a".repeat(40), "landed", "gone").is_err());
    branch("b", &"a".repeat(40), "landed", "offered").unwrap();
    let refused = |sql: &str| conn.execute(sql, []).unwrap_err().to_string();
    assert!(refused("UPDATE cleanups SET trigger = 'manual'").contains("immutable"));
    assert!(refused("UPDATE cleanup_branches SET sha = 'b'").contains("immutable"));
    assert!(refused("DELETE FROM cleanups").contains("immutable"));
    assert!(refused("DELETE FROM cleanup_branches").contains("immutable"));
    conn.execute(
        "UPDATE cleanup_branches SET outcome = 'deleted', decided_at = '2026-01-02T00:00:03Z'",
        [],
    )
    .unwrap();
    assert!(
        refused("UPDATE cleanup_branches SET outcome = 'offered', decided_at = NULL")
            .contains("forward")
    );
    assert!(
        refused("UPDATE cleanup_branches SET outcome = 'skipped', reason = 'x'")
            .contains("forward")
    );
    conn.execute(
        "UPDATE cleanups SET state = 'applied', completed_at = '2026-01-02T00:00:04Z'",
        [],
    )
    .unwrap();
    assert!(
        refused("UPDATE cleanups SET state = 'offered', completed_at = NULL").contains("forward")
    );
    assert!(refused("UPDATE cleanups SET state = 'declined'").contains("forward"));
}
