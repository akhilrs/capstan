use crate::db::Database;
use crate::error::LedgerError;
use crate::migrations::{checksum, migrations, Migration};
use crate::time::{now_iso, now_millis};
use rusqlite::types::Value;
use rusqlite::OpenFlags;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_KEEP_MIGRATION_BACKUPS: usize = 3;

/// Receives a message when an old backup cannot be listed or deleted.
pub type PruneErrorHandler = Box<dyn Fn(&str)>;

/// Options of `open_database`: src/controller/database.ts `OpenDatabaseOptions`.
pub struct OpenOptions {
    /// How many pre-migration backups to keep after a successful migration.
    pub keep_migration_backups: usize,
    /// Called when an old backup cannot be listed or deleted; the default writes to stderr.
    pub on_prune_error: Option<PruneErrorHandler>,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            keep_migration_backups: DEFAULT_KEEP_MIGRATION_BACKUPS,
            on_prune_error: None,
        }
    }
}

/// Opens (creating it) the ledger at `path` and brings it to the newest schema: foreign keys on, WAL, synchronous FULL,
/// busy timeout 5000, then migrate, then prune old backups if anything was migrated.
pub fn open_database(path: &Path, options: &OpenOptions) -> Result<Database, LedgerError> {
    let database = Database::open_with(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let prepared = (|| {
        database.exec("PRAGMA foreign_keys = ON")?;
        database.exec("PRAGMA journal_mode = WAL")?;
        database.exec("PRAGMA synchronous = FULL")?;
        database.exec("PRAGMA busy_timeout = 5000")?;
        let migrated = migrate(&database, path, migrations())?;
        if migrated {
            let mut report = |message: &str| match &options.on_prune_error {
                Some(handler) => handler(message),
                None => eprintln!("{message}"),
            };
            prune_migration_backups(path, options.keep_migration_backups, &mut report);
        }
        Ok(())
    })();
    match prepared {
        Ok(()) => Ok(database),
        Err(error) => {
            let _ = database.close();
            Err(error)
        }
    }
}

/// Opens an existing ledger read-only. A missing file is an error; it is never created.
pub fn open_database_read_only(path: &Path) -> Result<Database, LedgerError> {
    fs::symlink_metadata(path)?;
    Database::open_with(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}

/// `<state_dir>/controller.sqlite`; the directory must be absolute.
pub fn resolve_database_path(state_dir: &Path) -> Result<PathBuf, LedgerError> {
    if !state_dir.is_absolute() {
        return Err(LedgerError::InvalidArgument(
            "state directory must be absolute".to_string(),
        ));
    }
    Ok(state_dir.join("controller.sqlite"))
}

fn text(value: &Value) -> String {
    match value {
        Value::Text(s) => s.clone(),
        other => format!("{other:?}"),
    }
}

/// Applies `all` (ascending, 1..N) to the database, with the refusals of src/controller/database.ts. Returns whether any
/// step was applied. Public only so tests can feed it migrations the build did not embed.
#[doc(hidden)]
pub fn migrate(
    database: &Database,
    database_path: &Path,
    all: &[Migration],
) -> Result<bool, LedgerError> {
    let conn = database.connection();
    let has_ledger: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations')",
        [],
        |r| r.get(0),
    )?;
    let existing_tables: i64 = conn.query_row(
        "SELECT COUNT(*) AS count FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        [],
        |r| r.get(0),
    )?;
    if !has_ledger && existing_tables != 0 {
        return Err(LedgerError::migration(
            "database has tables but no migration ledger",
        ));
    }

    let mut applied: Vec<(i64, String, String)> = Vec::new();
    if has_ledger {
        let mut stmt = database
            .prepare("SELECT version, name, checksum FROM schema_migrations ORDER BY version")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            applied.push((
                row.get(0)?,
                text(&row.get::<_, Value>(1)?),
                text(&row.get::<_, Value>(2)?),
            ));
        }
    }
    for (index, (version, name, sum)) in applied.iter().enumerate() {
        if *version != index as i64 + 1 {
            return Err(LedgerError::migration(format!(
                "migration ledger has a gap before version {version}"
            )));
        }
        let Some(migration) = all.iter().find(|m| m.version == *version) else {
            return Err(LedgerError::migration(format!(
                "database schema version {version} is newer than this controller"
            )));
        };
        if migration.name != name {
            return Err(LedgerError::migration(format!(
                "unknown migration record {version}"
            )));
        }
        if &checksum(migration.bytes) != sum {
            return Err(LedgerError::migration(format!(
                "migration {name} checksum changed"
            )));
        }
    }

    let mut current = applied.last().map_or(0, |a| a.0);
    let mut migrated = false;
    for migration in all {
        if migration.version <= current {
            continue;
        }
        if migration.version != current + 1 {
            return Err(LedgerError::migration(format!(
                "missing migration after version {current}"
            )));
        }
        if current > 0 {
            let mut backup = OsString::from(database_path);
            backup.push(format!(
                ".pre-v{}-{}.sqlite",
                migration.version,
                now_millis()
            ));
            database.backup(Path::new(&backup))?;
        }
        let sum = checksum(migration.bytes);
        let sql = std::str::from_utf8(migration.bytes).map_err(|_| {
            LedgerError::migration(format!("migration {} is not valid UTF-8", migration.name))
        })?;
        database.exec("BEGIN IMMEDIATE")?;
        let step = (|| {
            database.exec(sql)?;
            database
                .prepare(
                    "INSERT INTO schema_migrations(version, name, checksum, applied_at) VALUES (?, ?, ?, ?)",
                )?
                .execute(rusqlite::params![
                    migration.version,
                    migration.name,
                    sum,
                    now_iso()
                ])?;
            database.exec("COMMIT")
        })();
        if let Err(error) = step {
            let _ = database.exec("ROLLBACK");
            return Err(error);
        }
        current = migration.version;
        migrated = true;
    }
    Ok(migrated)
}

/// Deletes all but the newest `keep` `controller.sqlite.pre-v<N>-<timestamp>.sqlite` files in the database's directory
/// (newest by schema version, then timestamp). Nothing else is touched, and a failure goes to `on_error`, never
/// returned. Returns the deleted file names.
pub fn prune_migration_backups(
    database_path: &Path,
    keep: usize,
    on_error: &mut dyn FnMut(&str),
) -> Vec<String> {
    let directory = match database_path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let mut deleted = Vec::new();
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            on_error(&format!(
                "could not prune migration backups in {}: {error}",
                directory.display()
            ));
            return deleted;
        }
    };
    let mut backups: Vec<(String, u128, u128)> = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                on_error(&format!(
                    "could not prune migration backups in {}: {error}",
                    directory.display()
                ));
                return deleted;
            }
        };
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if let Some((version, at)) = parse_backup_name(&name) {
            backups.push((name, version, at));
        }
    }
    backups.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)));
    for (name, _, _) in backups.into_iter().skip(keep) {
        let file = directory.join(&name);
        let removed = fs::symlink_metadata(&file).and_then(|meta| {
            if meta.is_file() {
                fs::remove_file(&file).map(|()| true)
            } else {
                Ok(false)
            }
        });
        match removed {
            Ok(true) => deleted.push(name),
            Ok(false) => {}
            Err(error) => on_error(&format!(
                "could not delete old migration backup {}: {error}",
                file.display()
            )),
        }
    }
    deleted
}

/// Matches `^controller\.sqlite\.pre-v(\d+)-(\d+)\.sqlite$`.
fn parse_backup_name(name: &str) -> Option<(u128, u128)> {
    let rest = name
        .strip_prefix("controller.sqlite.pre-v")?
        .strip_suffix(".sqlite")?;
    let (version, at) = rest.split_once('-')?;
    let number = |s: &str| {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            None
        } else {
            Some(s.parse::<u128>().unwrap_or(u128::MAX))
        }
    };
    Some((number(version)?, number(at)?))
}
