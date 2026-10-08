//! The capstan ledger: the SQLite storage layer shared with the Node implementation (src/controller/database.ts,
//! sqlite.ts and ownership.ts), with the same migration rules, file formats and lock protocol.
//!
//! Integers: Node's ledger adapter reads INTEGER columns as JS numbers and throws above +-(2^53-1), and returns BLOBs
//! as `Uint8Array`. The ledger has no BLOB columns today. Rust writers bind integers through [`safe_integer`] so they
//! never store a value Node cannot read back; BLOBs written by Rust read back in Node as byte arrays.

mod db;
mod error;
mod lock;
mod migrations;
mod open;
mod time;

pub use db::{safe_integer, Database, MAX_SAFE_INTEGER};
pub use error::LedgerError;
pub use lock::ProjectLock;
pub use migrations::{checksum, max_embedded_migration, migrations, Migration};
pub use open::{
    migrate, open_database, open_database_read_only, prune_migration_backups,
    resolve_database_path, OpenOptions, PruneErrorHandler, DEFAULT_KEEP_MIGRATION_BACKUPS,
};
pub use time::{iso_from_millis, now_iso, now_millis};

/// The version of the SQLite library bundled into this build.
pub fn sqlite_version() -> &'static str {
    rusqlite::version()
}

#[cfg(test)]
mod tests {
    use super::sqlite_version;

    fn parse(version: &str) -> (u32, u32, u32) {
        let mut parts = version.split('.').map(|p| p.parse::<u32>().unwrap());
        (
            parts.next().unwrap(),
            parts.next().unwrap(),
            parts.next().unwrap(),
        )
    }

    #[test]
    fn bundled_sqlite_is_at_least_node_24_6() {
        // Node 24.6's node:sqlite embeds SQLite 3.50.4.
        assert!(
            parse(sqlite_version()) >= (3, 50, 4),
            "{}",
            sqlite_version()
        );
    }
}
