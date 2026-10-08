use crate::error::LedgerError;
use crate::migrations::hex;
use crate::time::now_iso;
use rusqlite::{Connection, ErrorCode, OpenFlags};
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The project lock: an exclusive SQLite lock on controller.lock, the protocol of src/controller/ownership.ts. The
/// connection stays open for the life of the lock. POSIX drops every fcntl lock a process holds on a file when it
/// closes any fd on it, so this file is never opened through std::fs while the lock may be held; identity checks use
/// lstat by path.
pub struct ProjectLock {
    db: Option<Connection>,
    path: PathBuf,
    device: u64,
    inode: u64,
}

/// Creates the lock file private to the user, or reports that it already exists (O_CREAT|O_EXCL|O_NOFOLLOW, 0600).
fn create_if_missing(path: &Path) -> io::Result<()> {
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(path)
    {
        Ok(file) => {
            // The umask may have narrowed the mode; SQLite needs to write the file.
            file.set_permissions(fs::Permissions::from_mode(0o600))
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

fn checked_lstat(path: &Path) -> Result<fs::Metadata, LedgerError> {
    let stat = fs::symlink_metadata(path)?;
    if stat.file_type().is_symlink() || !stat.is_file() {
        return Err(LedgerError::ownership(
            "controller lock path must identify a regular file",
        ));
    }
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    if stat.mode() & 0o077 != 0 || stat.uid() != uid {
        return Err(LedgerError::ownership(
            "controller lock file must be private to the current user",
        ));
    }
    Ok(stat)
}

fn random_token() -> io::Result<String> {
    // /dev/urandom exists on Linux and macOS; it is not the lock file, so reading it cannot drop the lock.
    let mut bytes = [0u8; 16];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(hex(&bytes))
}

fn lock_error(error: LedgerError) -> LedgerError {
    if let LedgerError::Sqlite(rusqlite::Error::SqliteFailure(failure, _)) = &error {
        match failure.code {
            ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => {
                return LedgerError::ProjectLockHeld
            }
            ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt => return LedgerError::ownership(
                "controller lock file is not a lock database: remove it with the daemon stopped",
            ),
            _ => {}
        }
    }
    error
}

impl ProjectLock {
    pub fn acquire(lock_path: &Path) -> Result<ProjectLock, LedgerError> {
        if !lock_path.is_absolute() {
            return Err(LedgerError::ownership("lock path must be absolute"));
        }
        let directory = lock_path.parent().unwrap_or(Path::new("/"));
        let directory_stat = fs::symlink_metadata(directory)?;
        if !directory_stat.is_dir() || directory_stat.file_type().is_symlink() {
            return Err(LedgerError::ownership(
                "controller state directory must be a real directory",
            ));
        }
        create_if_missing(lock_path)?;
        let before = checked_lstat(lock_path)?;

        let attempt = || -> Result<Connection, LedgerError> {
            let conn = Connection::open_with_flags(
                lock_path,
                OpenFlags::SQLITE_OPEN_READ_WRITE
                    | OpenFlags::SQLITE_OPEN_CREATE
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            conn.busy_timeout(Duration::ZERO)?;
            conn.execute_batch("PRAGMA journal_mode=OFF")?;
            conn.execute_batch("PRAGMA locking_mode=EXCLUSIVE")?;
            conn.execute_batch("BEGIN EXCLUSIVE")?;
            let after = fs::symlink_metadata(lock_path)?;
            if after.file_type().is_symlink()
                || after.dev() != before.dev()
                || after.ino() != before.ino()
            {
                return Err(LedgerError::ownership(
                    "controller lock path changed while it was being locked",
                ));
            }
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS owner (pid INTEGER NOT NULL, started_at TEXT NOT NULL, token TEXT NOT NULL)",
            )?;
            conn.execute_batch("DELETE FROM owner")?;
            conn.execute(
                "INSERT INTO owner (pid, started_at, token) VALUES (?, ?, ?)",
                rusqlite::params![std::process::id(), now_iso(), random_token()?],
            )?;
            conn.execute_batch("COMMIT")?;
            Ok(conn)
        };
        match attempt() {
            Ok(conn) => Ok(ProjectLock {
                db: Some(conn),
                path: lock_path.to_path_buf(),
                device: before.dev(),
                inode: before.ino(),
            }),
            Err(error) => Err(lock_error(error)),
        }
    }

    /// Fails unless the lock is still held and the path still names the locked inode.
    pub fn assert_held(&self) -> Result<(), LedgerError> {
        if self.db.is_none() {
            return Err(LedgerError::ownership(
                "controller ownership has been released",
            ));
        }
        let stat = match fs::symlink_metadata(&self.path) {
            Ok(stat) => stat,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(LedgerError::ownership("controller lock path was removed"))
            }
            Err(error) => return Err(error.into()),
        };
        if stat.file_type().is_symlink() || stat.dev() != self.device || stat.ino() != self.inode {
            return Err(LedgerError::ownership(
                "controller lock path no longer identifies the owned inode",
            ));
        }
        Ok(())
    }

    /// Releases the lock. Dropping does the same.
    pub fn close(&mut self) {
        self.db = None;
    }
}
