//! The controller-events-by-successor migration (the file whose name ends in `_controller_events_by_successor.sql`): a
//! ledger at the previous version migrates after a backup, the index holds only the replacement events keyed by their
//! successor, and nothing else changes. No version number is written here: it comes from the embedded list.
mod common;

use capstan_ledger::{
    max_embedded_migration, migrate, migrations, open_database, Database, OpenOptions,
};
use common::scratch;
use rusqlite::Connection;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const INDEX: &str = "controller_events_by_successor";

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
    migrations()
        .iter()
        .find(|m| m.name.ends_with("_controller_events_by_successor.sql"))
        .expect("the controller-events-by-successor migration is embedded")
        .version as usize
}

fn has_index(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE name = ?",
        [INDEX],
        |r| r.get::<_, i64>(0),
    )
    .unwrap()
        == 1
}

/// Name and SQL of every schema object except the new index.
fn schema(conn: &Connection) -> BTreeSet<(String, String)> {
    let mut stmt = conn
        .prepare("SELECT name, COALESCE(sql, '') FROM sqlite_master WHERE name <> ? AND name NOT LIKE '%integration_details%' AND name NOT LIKE '%user_messages%' AND name NOT IN ('plan_signoffs_by_integration', 'verify_runs_by_integration')")
        .unwrap();
    let rows = stmt
        .query_map([INDEX], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    rows
}

#[test]
fn a_ledger_at_the_previous_version_migrates_after_a_backup() {
    let (dir, file) = scratch();
    std::fs::copy(fixture(), &file).unwrap();
    let db = Database::new(Connection::open(&file).unwrap());
    migrate(&db, &file, &migrations()[..version() - 1]).unwrap();
    db.close().unwrap();
    let before = Connection::open(&file).unwrap();
    assert!(!has_index(&before), "no index yet");
    let unchanged = schema(&before);
    drop(before);

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
    assert!(has_index(&conn));
    assert_eq!(schema(&conn), unchanged);
}

#[test]
fn the_index_holds_the_replacement_events_by_successor() {
    let (_dir, file) = scratch();
    open_database(&file, &keep_all()).unwrap().close().unwrap();
    let conn = Connection::open(&file).unwrap();
    let sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name = ?",
            [INDEX],
            |r| r.get(0),
        )
        .unwrap();
    assert!(sql.contains("$.details.successorId"), "{sql}");
    assert!(sql.contains("to_state = 'replaced'"), "{sql}");
}
