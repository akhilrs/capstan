mod common;
use capstan_ledger::{migrate, migrations, open_database, Database, Migration, OpenOptions};
use common::{exec, ledger_at, parity_file, scratch};
use rusqlite::Connection;

fn refusal(case: &str) -> String {
    let fixture: serde_json::Value = serde_json::from_str(&parity_file("refusals.json")).unwrap();
    fixture
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["case"] == case)
        .unwrap_or_else(|| panic!("no fixture for {case}"))["message"]
        .as_str()
        .unwrap()
        .to_string()
}

fn refused(file: &std::path::Path) -> String {
    match open_database(file, &OpenOptions::default()) {
        Ok(_) => panic!("accepted"),
        Err(e) => e.to_string(),
    }
}

#[test]
fn gap() {
    let (_d, f) = scratch();
    ledger_at(&f, 5);
    exec(&f, "DELETE FROM schema_migrations WHERE version = 3");
    assert_eq!(refused(&f), refusal("gap"));
}

#[test]
fn newer_than_the_controller() {
    let (_d, f) = scratch();
    let last = migrations().len();
    ledger_at(&f, last);
    let newer = last + 1;
    exec(&f, &format!("INSERT INTO schema_migrations(version, name, checksum, applied_at) VALUES ({newer}, '{newer:04}_future.sql', 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 'y')"));
    let message = refused(&f);
    assert_eq!(
        message,
        format!("database schema version {newer} is newer than this controller")
    );
    // The Node text, captured by the exporter, has the same shape for its own (newest + 1) version.
    let node = refusal("newer");
    assert!(
        node.starts_with("database schema version ")
            && node.ends_with(" is newer than this controller"),
        "{node}"
    );
}

#[test]
fn renamed_file() {
    let (_d, f) = scratch();
    ledger_at(&f, 3);
    exec(
        &f,
        "UPDATE schema_migrations SET name = '0002_renamed.sql' WHERE version = 2",
    );
    assert_eq!(refused(&f), refusal("renamed"));
}

#[test]
fn changed_checksum() {
    let (_d, f) = scratch();
    ledger_at(&f, 3);
    exec(
        &f,
        "UPDATE schema_migrations SET checksum = 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb' WHERE version = 1",
    );
    assert_eq!(refused(&f), refusal("checksum"));
}

#[test]
fn tables_without_a_ledger() {
    let (_d, f) = scratch();
    exec(&f, "CREATE TABLE stray (id INTEGER)");
    assert_eq!(refused(&f), refusal("tables-without-ledger"));
}

#[test]
fn invalid_utf8() {
    let (_d, f) = scratch();
    let mut broken: Vec<Migration> = migrations().to_vec();
    broken[0].bytes = &[0xff, 0xfe, 0x00];
    let db = Database::new(Connection::open(&f).unwrap());
    let error = migrate(&db, &f, &broken).unwrap_err();
    assert_eq!(error.to_string(), refusal("invalid-utf8"));
}

#[test]
fn a_failed_step_rolls_back_and_leaves_the_ledger_alone() {
    let (_d, f) = scratch();
    ledger_at(&f, 2);
    let mut broken: Vec<Migration> = migrations().to_vec();
    broken[2].bytes = b"CREATE TABLE half (x); THIS IS NOT SQL;";
    let db = Database::new(Connection::open(&f).unwrap());
    assert!(migrate(&db, &f, &broken).is_err());
    let tables: i64 = db
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'half'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let versions: i64 = db
        .connection()
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!((tables, versions), (0, 2));
}
