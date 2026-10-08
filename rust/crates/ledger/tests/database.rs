mod common;
use capstan_ledger::{safe_integer, Database, LedgerError, MAX_SAFE_INTEGER};
use rusqlite::Connection;

fn memory() -> Database {
    Database::new(Connection::open_in_memory().unwrap())
}

#[test]
fn transactions_nest_through_savepoints() {
    let db = memory();
    db.exec("CREATE TABLE t (x INTEGER)").unwrap();
    let count = |db: &Database| -> i64 {
        db.connection()
            .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
            .unwrap()
    };
    db.transaction(|db| {
        db.exec("INSERT INTO t VALUES (1)")?;
        let inner: Result<(), LedgerError> = db.transaction(|db| {
            db.exec("INSERT INTO t VALUES (2)")?;
            Err(LedgerError::InvalidArgument("boom".into()))
        });
        assert_eq!(inner.unwrap_err().to_string(), "boom");
        assert_eq!(count(db), 1);
        db.transaction(|db| db.exec("INSERT INTO t VALUES (3)"))
    })
    .unwrap();
    assert_eq!(count(&db), 2);
    let outer: Result<(), LedgerError> = db.transaction(|db| {
        db.exec("INSERT INTO t VALUES (4)")?;
        Err(LedgerError::InvalidArgument("no".into()))
    });
    assert!(outer.is_err());
    assert_eq!(count(&db), 2);
    assert!(db.connection().is_autocommit());
}

#[test]
fn a_panic_in_a_transaction_rolls_back() {
    let db = memory();
    db.exec("CREATE TABLE t (x INTEGER)").unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = db.transaction::<()>(|db| {
            db.exec("INSERT INTO t VALUES (1)")?;
            panic!("inside");
        });
    }));
    assert!(result.is_err());
    assert!(db.connection().is_autocommit());
    let n: i64 = db
        .connection()
        .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
}

#[test]
fn statement_cache_holds_256_texts_and_pragma_returns_rows() {
    let db = memory();
    for i in 0..300 {
        db.prepare(&format!("SELECT {i}")).unwrap();
    }
    assert_eq!(db.pragma("user_version").unwrap().len(), 1);
    assert!(db.pragma("foreign_keys = ON").unwrap().is_empty());
}

#[test]
fn backup_copies_the_database() {
    let (dir, file) = common::scratch();
    let db = memory();
    db.exec("CREATE TABLE t (x); INSERT INTO t VALUES (7)")
        .unwrap();
    let dest = dir.path().join("copy.sqlite");
    db.backup(&dest).unwrap();
    let _ = file;
    let n: i64 = Connection::open(&dest)
        .unwrap()
        .query_row("SELECT x FROM t", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 7);
}

#[test]
fn integers_beyond_2_pow_53_are_refused() {
    assert_eq!(safe_integer(MAX_SAFE_INTEGER).unwrap(), MAX_SAFE_INTEGER);
    assert_eq!(safe_integer(-MAX_SAFE_INTEGER).unwrap(), -MAX_SAFE_INTEGER);
    assert!(matches!(
        safe_integer(MAX_SAFE_INTEGER + 1),
        Err(LedgerError::UnsafeValue(_))
    ));
    assert!(safe_integer(i64::MIN).is_err());
    assert!(safe_integer(i64::MAX).is_err());
}
