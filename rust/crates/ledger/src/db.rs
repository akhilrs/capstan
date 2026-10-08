use crate::error::LedgerError;
use rusqlite::backup::Backup;
use rusqlite::types::Value;
use rusqlite::{CachedStatement, Connection, OpenFlags};
use std::cell::Cell;
use std::path::Path;
use std::time::Duration;

const STATEMENT_CACHE_SIZE: usize = 256;
const BUSY_TIMEOUT: Duration = Duration::from_millis(5000);
/// Node's ledger adapter reads integers as JS numbers and throws above this magnitude.
pub const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// Refuses an integer Node's adapter could not read back (|n| > 2^53 - 1). Writers of ledger rows pass every INTEGER
/// they bind through this, so Rust never stores a value `openDatabase` readers would throw on.
pub fn safe_integer(n: i64) -> Result<i64, LedgerError> {
    if (-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&n) {
        Ok(n)
    } else {
        Err(LedgerError::UnsafeValue(format!(
            "integer {n} is outside the range Node can read back (+-2^53-1)"
        )))
    }
}

/// An open ledger connection: the shape of src/controller/sqlite.ts `Database`.
pub struct Database {
    conn: Connection,
    savepoints: Cell<u32>,
}

impl Database {
    /// Wraps an open connection (statement cache and savepoint counter included).
    pub fn new(conn: Connection) -> Self {
        conn.set_prepared_statement_cache_capacity(STATEMENT_CACHE_SIZE);
        Self {
            conn,
            savepoints: Cell::new(0),
        }
    }

    pub(crate) fn open_with(path: &Path, flags: OpenFlags) -> Result<Self, LedgerError> {
        let conn = Connection::open_with_flags(path, flags)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        Ok(Self::new(conn))
    }

    /// The underlying connection, for queries the wrapper does not cover.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// The statement for `sql`, reused from an LRU cache of 256 texts.
    pub fn prepare(&self, sql: &str) -> Result<CachedStatement<'_>, LedgerError> {
        Ok(self.conn.prepare_cached(sql)?)
    }

    pub fn exec(&self, sql: &str) -> Result<(), LedgerError> {
        Ok(self.conn.execute_batch(sql)?)
    }

    /// `PRAGMA <statement>`; the result rows (empty when it returns none).
    pub fn pragma(&self, statement: &str) -> Result<Vec<Vec<Value>>, LedgerError> {
        let mut stmt = self.prepare(&format!("PRAGMA {statement}"))?;
        let columns = stmt.column_count();
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(
                (0..columns)
                    .map(|i| row.get::<_, Value>(i))
                    .collect::<Result<Vec<_>, _>>()?,
            );
        }
        Ok(out)
    }

    /// Runs `f` in BEGIN ... COMMIT, or SAVEPOINT capstan_sp_<n> ... RELEASE when already inside a transaction. An error or a
    /// panic rolls back.
    pub fn transaction<T>(
        &self,
        f: impl FnOnce(&Database) -> Result<T, LedgerError>,
    ) -> Result<T, LedgerError> {
        let nested = !self.conn.is_autocommit();
        let n = self.savepoints.get();
        self.savepoints.set(n + 1);
        let name = format!("capstan_sp_{n}");
        let outcome = (|| {
            self.conn.execute_batch(&if nested {
                format!("SAVEPOINT {name}")
            } else {
                "BEGIN".to_string()
            })?;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self)));
            match result {
                Ok(Ok(value)) => {
                    self.conn.execute_batch(&if nested {
                        format!("RELEASE {name}")
                    } else {
                        "COMMIT".to_string()
                    })?;
                    Ok(value)
                }
                Ok(Err(error)) => {
                    self.rollback(nested, &name);
                    Err(error)
                }
                Err(panic) => {
                    self.rollback(nested, &name);
                    std::panic::resume_unwind(panic)
                }
            }
        })();
        self.savepoints.set(n);
        outcome
    }

    fn rollback(&self, nested: bool, name: &str) {
        if self.conn.is_autocommit() {
            return;
        }
        let _ = self.conn.execute_batch(&if nested {
            format!("ROLLBACK TO {name}; RELEASE {name}")
        } else {
            "ROLLBACK".to_string()
        });
    }

    /// An online backup of this database to `dest` (the SQLite backup API).
    pub fn backup(&self, dest: &Path) -> Result<(), LedgerError> {
        let mut target = Connection::open(dest)?;
        let backup = Backup::new(&self.conn, &mut target)?;
        backup.run_to_completion(100, Duration::from_millis(10), None)?;
        drop(backup);
        target.close().map_err(|(_, e)| e)?;
        Ok(())
    }

    pub fn close(self) -> Result<(), LedgerError> {
        self.conn.close().map_err(|(_, e)| LedgerError::Sqlite(e))
    }
}
