//! The integration-details migration (the file whose name ends in `_integration_details.sql`): it applies to a fresh
//! ledger and to a ledger at the previous version after a backup, the table is write-once, and nothing that retention
//! or a discard does reaches a row. No version number is written here: it comes from the embedded list.
mod common;

use capstan_ledger::{
    max_embedded_migration, migrate, migrations, open_database, Database, OpenOptions,
};
use common::scratch;
use rusqlite::Connection;
use std::collections::BTreeSet;
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

fn version() -> usize {
    let found: Vec<_> = migrations()
        .iter()
        .filter(|m| m.name.ends_with("_integration_details.sql"))
        .collect();
    assert_eq!(found.len(), 1, "exactly one integration-details migration");
    found[0].version as usize
}

#[test]
fn the_numbering_has_no_gap_and_no_duplicate() {
    let all = migrations();
    assert!(
        all.iter().any(|m| m.name.starts_with("0052_")),
        "0052 is embedded"
    );
    let numbers: BTreeSet<i64> = all.iter().map(|m| m.version).collect();
    assert_eq!(numbers.len(), all.len(), "two files claim one number");
    for (index, m) in all.iter().enumerate() {
        assert_eq!(m.version, index as i64 + 1, "{}", m.name);
    }
}

fn exists(conn: &Connection, kind: &str, name: &str) -> bool {
    conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = ? AND name = ?",
        [kind, name],
        |r| r.get::<_, i64>(0),
    )
    .unwrap()
        == 1
}

fn shape(conn: &Connection) {
    assert!(exists(conn, "table", "integration_details"));
    assert!(exists(conn, "index", "plan_signoffs_by_integration"));
    assert!(exists(conn, "index", "verify_runs_by_integration"));
    let fks: Vec<(String, String)> = conn
        .prepare("SELECT \"table\", \"from\" FROM pragma_foreign_key_list('integration_details') ORDER BY id, seq")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        fks,
        vec![
            ("integrations".to_string(), "project_id".to_string()),
            ("integrations".to_string(), "integration_id".to_string())
        ]
    );
}

#[test]
fn a_fresh_ledger_has_the_table_the_indexes_and_the_foreign_key() {
    let (_dir, file) = scratch();
    open_database(&file, &keep_all()).unwrap().close().unwrap();
    shape(&Connection::open(&file).unwrap());
}

#[test]
fn a_ledger_at_the_previous_version_migrates_after_a_backup() {
    let (dir, file) = scratch();
    std::fs::copy(fixture(), &file).unwrap();
    let db = Database::new(Connection::open(&file).unwrap());
    migrate(&db, &file, &migrations()[..version() - 1]).unwrap();
    db.close().unwrap();
    assert!(!exists(
        &Connection::open(&file).unwrap(),
        "table",
        "integration_details"
    ));

    open_database(&file, &keep_all()).unwrap().close().unwrap();

    let backups = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .contains(&format!(".pre-v{}-", version()))
        })
        .count();
    assert!(backups >= 1, "a backup is made before the migration");
    let conn = Connection::open(&file).unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM schema_migrations", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        max_embedded_migration()
    );
    shape(&conn);
}

fn with_merged_integration() -> (tempfile::TempDir, Connection) {
    let (dir, file) = scratch();
    open_database(&file, &keep_all()).unwrap().close().unwrap();
    let conn = Connection::open(&file).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
    let sha = "a".repeat(40);
    let head = "b".repeat(40);
    conn.execute_batch(&format!(
        "INSERT INTO projects(project_id, name, current_input_revision, state_version, created_at) VALUES ('p', 'p', 1, 1, 't');
         INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, head_sha, created_at, completed_at)
           VALUES ('p', 'i1', 1, '{sha}', 'capstan/integration/i1', 'pm', 'merged', '{head}', 't', 't');
         INSERT INTO integration_details(project_id, integration_id, details_json, computed_at) VALUES ('p', 'i1', '{{\"v\":1}}', 't');"
    ))
    .unwrap();
    (dir, conn)
}

#[test]
fn the_table_is_write_once() {
    let (_dir, conn) = with_merged_integration();
    let update = conn.execute("UPDATE integration_details SET details_json = '{}'", []);
    assert!(update.unwrap_err().to_string().contains("immutable"));
    let delete = conn.execute("DELETE FROM integration_details", []);
    assert!(delete.unwrap_err().to_string().contains("immutable"));
    // INSERT OR IGNORE of the same key is a no-op and does not fire the trigger.
    let changed = conn
        .execute(
            "INSERT OR IGNORE INTO integration_details(project_id, integration_id, details_json, computed_at) VALUES ('p', 'i1', '{\"v\":2}', 'u')",
            [],
        )
        .unwrap();
    assert_eq!(changed, 0);
    let stored: String = conn
        .query_row("SELECT details_json FROM integration_details", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(stored, "{\"v\":1}");
    // The row must name an integration, and stay within 16 KiB of valid JSON.
    assert!(conn
        .execute(
            "INSERT INTO integration_details VALUES ('p', 'nope', '{}', 't')",
            []
        )
        .is_err());
    assert!(conn
        .execute(
            "INSERT INTO integration_details VALUES ('p', 'i1', 'x', 't')",
            []
        )
        .is_err());
}

#[test]
fn a_discard_and_the_retention_prune_delete_nothing() {
    let (_dir, conn) = with_merged_integration();
    conn.execute("UPDATE integrations SET state = 'discarded'", [])
        .unwrap();
    // What retention deletes: old mutation_requests rows, nothing else.
    conn.execute("DELETE FROM mutation_requests", []).unwrap();
    assert!(conn.execute("DELETE FROM integrations", []).is_err());
    let rows: i64 = conn
        .query_row("SELECT count(*) FROM integration_details", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1);
}
