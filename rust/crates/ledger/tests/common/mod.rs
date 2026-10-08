#![allow(dead_code)]
use capstan_ledger::{migrate, migrations, Database};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

pub fn parity_file(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("parity")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A ledger at schema version `version`, built by this crate's runner (WAL, then the first `version` migrations).
pub fn ledger_at(path: &Path, version: usize) {
    let db = Database::new(Connection::open(path).unwrap());
    db.exec("PRAGMA journal_mode = WAL").unwrap();
    migrate(&db, path, &migrations()[..version]).unwrap();
    db.close().unwrap();
}

pub fn scratch() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("controller.sqlite");
    (dir, file)
}

pub fn exec(path: &Path, sql: &str) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(sql).unwrap();
}
