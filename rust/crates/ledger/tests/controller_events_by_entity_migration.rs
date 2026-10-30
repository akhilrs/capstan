//! The controller-events-by-entity migration (the file whose name ends in `_controller_events_by_entity.sql`): a ledger at
//! the previous version migrates after a backup, the index has exactly the columns the feed lookup needs, and nothing else
//! of `controller_events` changes. No version number is written here: it comes from the embedded list.
mod common;

use capstan_ledger::{
    max_embedded_migration, migrate, migrations, open_database, Database, OpenOptions,
};
use common::scratch;
use rusqlite::Connection;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const INDEX: &str = "controller_events_by_entity";

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
        .find(|m| m.name.ends_with("_controller_events_by_entity.sql"))
        .expect("the controller-events-by-entity migration is embedded")
        .version as usize
}

/// The fixture migrated to the version before the migration.
fn ledger_before(file: &Path) {
    std::fs::copy(fixture(), file).unwrap();
    let db = Database::new(Connection::open(file).unwrap());
    migrate(&db, file, &migrations()[..version() - 1]).unwrap();
    db.close().unwrap();
}

/// Name and SQL of every schema object except the new index (and what later migrations add), and every `controller_events` row.
fn snapshot(conn: &Connection) -> (BTreeSet<(String, String)>, Vec<String>) {
    let mut schema = conn
        .prepare("SELECT name, COALESCE(sql, '') FROM sqlite_master WHERE name <> ? AND name <> 'controller_events_by_successor' AND name NOT LIKE '%integration_details%' AND name NOT IN ('plan_signoffs_by_integration', 'verify_runs_by_integration')")
        .unwrap();
    let schema = schema
        .query_map([INDEX], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut rows = conn
        .prepare("SELECT sequence || '|' || event_id || '|' || payload_json FROM controller_events ORDER BY sequence")
        .unwrap();
    let rows = rows
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    (schema, rows)
}

fn columns(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT name FROM pragma_index_info(?) ORDER BY seqno")
        .unwrap();
    let columns = stmt
        .query_map([INDEX], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    columns
}

#[test]
fn a_ledger_at_the_previous_version_migrates_after_a_backup() {
    let (dir, file) = scratch();
    ledger_before(&file);
    let before = Connection::open(&file).unwrap();
    assert!(columns(&before).is_empty(), "no index yet");
    let unchanged = snapshot(&before);
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
    assert_eq!(snapshot(&conn), unchanged);
    assert_eq!(
        columns(&conn),
        ["project_id", "entity_type", "entity_id", "sequence"]
    );
    let on: String = conn
        .query_row(
            "SELECT tbl_name FROM sqlite_master WHERE name = ?",
            [INDEX],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(on, "controller_events");
}

#[test]
fn a_fresh_ledger_has_the_index() {
    let (_dir, file) = scratch();
    open_database(&file, &keep_all()).unwrap().close().unwrap();
    let conn = Connection::open(&file).unwrap();
    assert_eq!(
        columns(&conn),
        ["project_id", "entity_type", "entity_id", "sequence"]
    );
    assert!(
        migrations().iter().any(|m| m.version as usize == version()),
        "the migration is in the embedded list"
    );
}
