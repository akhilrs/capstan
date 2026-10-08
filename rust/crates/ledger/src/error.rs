use std::fmt;
use std::io;

/// Everything the ledger can refuse or fail at. `Migration` and `Ownership` carry exactly the message texts of the Node
/// `DatabaseMigrationError` and `ControllerOwnershipError`.
#[derive(Debug)]
pub enum LedgerError {
    /// The migration runner refused the database (Node: `DatabaseMigrationError`).
    Migration(String),
    /// The project lock cannot be taken or is no longer valid (Node: `ControllerOwnershipError`).
    Ownership(String),
    /// Another process holds the project lock (Node: `ProjectLockHeldError`).
    ProjectLockHeld,
    /// A caller passed a value the ledger refuses (Node: `TypeError`).
    InvalidArgument(String),
    /// A value that Node's ledger adapter cannot read back (an integer beyond 2^53).
    UnsafeValue(String),
    Io(io::Error),
    Sqlite(rusqlite::Error),
}

impl LedgerError {
    pub(crate) fn migration(message: impl Into<String>) -> Self {
        Self::Migration(message.into())
    }

    pub(crate) fn ownership(message: impl Into<String>) -> Self {
        Self::Ownership(message.into())
    }
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Migration(m)
            | Self::Ownership(m)
            | Self::InvalidArgument(m)
            | Self::UnsafeValue(m) => f.write_str(m),
            Self::ProjectLockHeld => {
                f.write_str("another cooperating controller owns this project")
            }
            Self::Io(e) => e.fmt(f),
            Self::Sqlite(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for LedgerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Sqlite(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for LedgerError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<rusqlite::Error> for LedgerError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}
