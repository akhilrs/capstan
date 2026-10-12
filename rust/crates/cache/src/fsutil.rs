//! Filesystem helpers that never follow symlinks.
use std::fs::{self, DirBuilder, File};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{CacheError, Result};

pub(crate) fn now_ms() -> u64 {
    since_epoch_ms(SystemTime::now())
}

pub(crate) fn since_epoch_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub(crate) fn nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}

/// `mkdir -p` with mode 0700 for every directory it creates; an existing directory is left alone.
pub(crate) fn mkdir_private(path: &Path) -> Result<()> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(|e| CacheError::io(format!("create {}", path.display()), e))
}

/// Creates the file when needed and sets its modified time to now.
pub(crate) fn touch(path: &Path) -> std::io::Result<()> {
    let file = File::options().create(true).append(true).open(path)?;
    file.set_modified(SystemTime::now())
}

/// Removes a tree without following symlinks, making read-only directories writable first (the Go module cache).
pub(crate) fn remove_tree(path: &Path) -> std::io::Result<()> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if !meta.is_dir() {
        return fs::remove_file(path);
    }
    let mut mode = meta.permissions();
    mode.set_mode(0o700);
    fs::set_permissions(path, mode)?;
    for child in fs::read_dir(path)? {
        remove_tree(&child?.path())?;
    }
    fs::remove_dir(path)
}

/// True when a process with this id exists.
pub(crate) fn pid_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    let rc = unsafe { libc::kill(pid as i32, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}
