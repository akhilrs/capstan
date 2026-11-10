use capstan_ledger::Database;
use rusqlite::Connection;

fn memory() -> Database {
    Database::new(Connection::open_in_memory().unwrap())
}

#[test]
fn statements_run_counts_prepare_exec_and_pragma() {
    let db = memory();
    assert_eq!(db.statements_run(), 0);
    db.exec("CREATE TABLE t (x INTEGER)").unwrap();
    assert_eq!(db.statements_run(), 1);
    db.pragma("user_version").unwrap();
    assert_eq!(db.statements_run(), 2);
    let before = db.statements_run();
    for _ in 0..5 {
        db.prepare("SELECT COUNT(*) FROM t").unwrap();
    }
    assert_eq!(db.statements_run() - before, 5);
    // Transaction control and raw connection use are not counted.
    let before = db.statements_run();
    db.transaction(|_| Ok(())).unwrap();
    db.connection().execute_batch("SELECT 1").unwrap();
    assert_eq!(db.statements_run(), before);
}

mod common;
use capstan_ledger::{
    maintain, open_database, prune_old_migration_backups, OpenOptions, PLANNER_STATS_MIGRATION,
};
use common::{exec, ledger_at, scratch};
use std::fs;
use std::path::Path;

fn pragma_int(db: &Database, name: &str) -> i64 {
    match &db.pragma(name).unwrap()[0][0] {
        rusqlite::types::Value::Integer(n) => *n,
        other => panic!("{other:?}"),
    }
}

#[test]
fn defaults_and_zero_values_of_the_cache_and_map_pragmas() {
    let (_dir, file) = scratch();
    let db = open_database(&file, &OpenOptions::default()).unwrap();
    assert_eq!(pragma_int(&db, "cache_size"), -65536);
    // The bundled library may clamp the size; the read-back of a fresh connection is the truth.
    let probe_dir = tempfile::tempdir().unwrap();
    let fresh = Connection::open(probe_dir.path().join("probe.sqlite")).unwrap();
    fresh.execute_batch("PRAGMA mmap_size = 268435456").unwrap();
    let expected: i64 = fresh
        .query_row("PRAGMA mmap_size", [], |r| r.get(0))
        .unwrap();
    assert_eq!(pragma_int(&db, "mmap_size"), expected);
    db.close().unwrap();

    let (_dir, file) = scratch();
    let options = OpenOptions {
        cache_mb: 0,
        mmap_mb: 0,
        ..OpenOptions::default()
    };
    let db = open_database(&file, &options).unwrap();
    assert_eq!(pragma_int(&db, "cache_size"), -2000);
    assert_eq!(pragma_int(&db, "mmap_size"), 0);
}

fn stat_rows(file: &Path) -> i64 {
    Connection::open(file)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM sqlite_stat1", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn maintenance_refills_empty_planner_statistics_and_no_maintenance_leaves_them() {
    let (_dir, file) = scratch();
    open_database(&file, &OpenOptions::default())
        .unwrap()
        .close()
        .unwrap();
    exec(
        &file,
        "CREATE TABLE tune_t (a INTEGER, b INTEGER); CREATE INDEX tune_t_a ON tune_t(a);
         WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 200)
           INSERT INTO tune_t SELECT i, i % 7 FROM n;
         ANALYZE; DELETE FROM sqlite_stat1;",
    );
    assert_eq!(stat_rows(&file), 0);
    open_database(&file, &OpenOptions::default())
        .unwrap()
        .close()
        .unwrap();
    assert_eq!(stat_rows(&file), 0);
    let options = OpenOptions {
        maintenance: true,
        ..OpenOptions::default()
    };
    open_database(&file, &options).unwrap().close().unwrap();
    assert!(stat_rows(&file) > 0);
}

#[test]
fn a_migrated_ledger_with_rows_is_not_analyzed_again() {
    let (dir, file) = scratch();
    ledger_at(&file, PLANNER_STATS_MIGRATION as usize - 1);
    exec(
        &file,
        "CREATE TABLE tune_t (a INTEGER); CREATE INDEX tune_t_a ON tune_t(a); INSERT INTO tune_t VALUES (1), (2), (3);",
    );
    let db = open_database(&file, &OpenOptions::default()).unwrap();
    let mut errors = Vec::new();
    let report = maintain(&db, &file, 3, 14, capstan_ledger::now_millis(), &mut |m| {
        errors.push(m.to_string())
    })
    .unwrap();
    assert!(!report.analyzed, "{errors:?}");
    assert!(stat_rows(&file) > 0);
    drop(dir);
}

fn backup_files(dir: &Path, names: &[&str]) {
    for name in names {
        fs::write(dir.join(name), b"x").unwrap();
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut found: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    found.sort();
    found
}

const NOW: i64 = 100 * 86_400_000;
const DAY: i64 = 86_400_000;

fn backup_name(version: u32, age_days: i64) -> String {
    format!(
        "controller.sqlite.pre-v{version}-{}.sqlite",
        NOW - age_days * DAY
    )
}

#[test]
fn aged_prune_removes_old_backups_and_keeps_the_newest() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("controller.sqlite");
    let all: Vec<String> = [(5, 30), (4, 20), (3, 10), (2, 2), (1, 1)]
        .iter()
        .map(|(v, d)| backup_name(*v, *d))
        .collect();
    let refs: Vec<&str> = all.iter().map(String::as_str).collect();
    backup_files(dir.path(), &refs);
    backup_files(dir.path(), &["controller.sqlite", "notes.txt"]);
    // Newest by version is pre-v5 (30 days old): the age rule must not take it; keep 3 drops v2, v1.
    let mut removed = prune_old_migration_backups(&db, 3, 14, NOW, &mut |m| panic!("{m}"));
    removed.sort();
    let mut expected = vec![backup_name(1, 1), backup_name(2, 2), backup_name(4, 20)];
    expected.sort();
    assert_eq!(removed, expected);
    assert!(names(dir.path()).contains(&backup_name(5, 30)));
    assert!(names(dir.path()).contains(&backup_name(3, 10)));
    assert!(names(dir.path()).contains(&"notes.txt".to_string()));
}

#[test]
fn aged_prune_with_every_backup_old_keeps_the_newest_and_zero_age_counts_only() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("controller.sqlite");
    let all = [backup_name(3, 60), backup_name(2, 70), backup_name(1, 80)];
    let refs: Vec<&str> = all.iter().map(String::as_str).collect();
    backup_files(dir.path(), &refs);
    assert!(prune_old_migration_backups(&db, 3, 0, NOW, &mut |m| panic!("{m}")).is_empty());
    let removed = prune_old_migration_backups(&db, 3, 14, NOW, &mut |m| panic!("{m}"));
    assert_eq!(removed.len(), 2);
    assert_eq!(
        names(dir.path()),
        vec![backup_name(3, 60)],
        "only the newest is left"
    );
}

#[test]
fn a_backup_made_by_this_opens_migration_is_kept() {
    let (dir, file) = scratch();
    let newest = capstan_ledger::max_embedded_migration();
    ledger_at(&file, newest as usize - 2);
    let options = OpenOptions {
        maintenance: true,
        backup_max_age_days: 1,
        ..OpenOptions::default()
    };
    open_database(&file, &options).unwrap().close().unwrap();
    let marker = format!(".pre-v{newest}-");
    let kept = names(dir.path())
        .into_iter()
        .filter(|n| n.contains(&marker))
        .count();
    assert_eq!(kept, 1);
}
