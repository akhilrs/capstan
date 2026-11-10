//! The package-landings migration (the file whose name ends in `_package_landings.sql`): a ledger at the previous version
//! migrates with its rows intact, the table starts empty, a row cannot be updated and the sha and note are checked. No version number is written here: it comes from the file name.
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
        .find(|m| m.name.ends_with("_package_landings.sql"))
        .expect("the package-landings migration is embedded")
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

const SHA: &str = "abcdef0123456789abcdef0123456789abcdef01";

#[test]
fn a_ledger_at_the_previous_version_migrates_with_its_rows_intact() {
    let (_dir, file) = scratch();
    ledger_before(&file);
    let before = Connection::open(&file).unwrap();
    let reports: i64 = before
        .query_row("SELECT count(*) FROM agent_reports", [], |r| r.get(0))
        .unwrap();
    let packages: i64 = before
        .query_row("SELECT count(*) FROM plan_packages", [], |r| r.get(0))
        .unwrap();
    drop(before);
    let conn = migrated(&file);
    let versions: i64 = conn
        .query_row("SELECT count(*) FROM schema_migrations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(versions, max_embedded_migration());
    let count = |table: &str| -> i64 {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(count("agent_reports"), reports);
    assert_eq!(count("plan_packages"), packages);
    assert_eq!(count("package_landings"), 0);
    let (strict, without_rowid): (i64, i64) = conn
        .query_row(
            "SELECT strict, wr FROM pragma_table_list WHERE name = 'package_landings'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((strict, without_rowid), (1, 1));
}

#[test]
fn a_row_is_checked_and_never_updated() {
    let (_dir, file) = scratch();
    let project = ledger_before(&file);
    let conn = migrated(&file);
    let insert = |sha: &str, note: &str| {
        conn.execute(
            "INSERT INTO package_landings(project_id, plan_id, package_id, commit_sha, recorded_by, recorded_at, note)
             VALUES (?, 'p', 'a', ?, 'x', ?, ?)",
            rusqlite::params![project, sha, NOW, note],
        )
    };
    assert!(insert("abc", "n").is_err());
    assert!(insert(SHA, "").is_err());
    assert!(insert(SHA, &"n".repeat(501)).is_err());
    insert(SHA, "n").unwrap();
    assert!(insert(SHA, "again").is_err());
    let update = conn.execute("UPDATE package_landings SET note = 'other'", []);
    assert!(update.is_err());
    assert_eq!(conn.execute("DELETE FROM package_landings", []).unwrap(), 1);
}
