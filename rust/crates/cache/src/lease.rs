//! Slot leases and the entry guard behind the verify gate's per-project build directories.
//!
//! A slot is `(entry, project id, n)`. Its directories are `<entry>/<gate subdir>/<project>/slot-<n>`; its lease is the
//! file `<entry>/.slot-<project>-<n>.lease`, beside the slots and outside any directory a build can remove, covering
//! all of the slot's directories as one unit. A lease is an `flock` on that file: it dies with the holding process
//! and is not inherited by children (the descriptor is close-on-exec), so a build that outlives its gate does not
//! keep the slot busy. The entry guard `<entry>/.capstan-gate.lock` orders slot creation against prune and removal.
//!
//! Lock order everywhere: store lock, then entry guard, then slot leases.
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use crate::fsutil::{mkdir_private, nanos};

/// The name of the entry guard file inside an entry directory.
pub const GUARD_FILE: &str = ".capstan-gate.lock";
/// Why prune and clear skip a slot that is leased.
pub const IN_USE_REASON: &str = "in use";
/// How often a waiting acquisition tries again.
const POLL: Duration = Duration::from_millis(50);
/// A lock file replaced this many times in a row is reported instead of chased forever.
const MAX_REPLACED: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseError {
    /// Another holder has the guard or the slot and the wait ran out.
    Busy,
    Io(String),
    /// The abort flag was set while waiting.
    Aborted,
}

impl fmt::Display for LeaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => write!(f, "the gate slot is busy"),
            Self::Io(message) => write!(f, "{message}"),
            Self::Aborted => write!(f, "waiting for the gate slot was aborted"),
        }
    }
}

impl std::error::Error for LeaseError {}

fn io_error(context: impl fmt::Display, error: std::io::Error) -> LeaseError {
    LeaseError::Io(format!("{context}: {error}"))
}

/// Opens `path` afresh, takes the flock without blocking, and checks the locked file is still the one at `path`
/// (a clear may have renamed it into `.trash` meanwhile); otherwise it drops the file and tries again.
/// `Ok(None)`: the file does not exist and `create` is off.
fn lock_path(
    path: &Path,
    create: bool,
    exclusive: bool,
    after_open: &dyn Fn(),
) -> Result<Option<File>, LeaseError> {
    let mut mkdir_error = None;
    for _ in 0..MAX_REPLACED {
        let mut options = OpenOptions::new();
        if create {
            options.create(true).append(true).mode(0o600);
        } else {
            options.read(true);
        }
        let file = match options.open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !create => return Ok(None),
            // The directory itself is gone (an idle whole-entry prune or a clear): recreate it and open again.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some(parent) = path.parent() {
                    // A prune renaming the directory away mid-create fails with `File exists`: open again, and
                    // report the failure only if the retries never get past it.
                    if let Err(e) = mkdir_private(parent) {
                        mkdir_error = Some(e.to_string());
                    }
                }
                continue;
            }
            Err(e) => return Err(io_error(format!("open {}", path.display()), e)),
        };
        after_open();
        let op = if exclusive {
            libc::LOCK_EX
        } else {
            libc::LOCK_SH
        } | libc::LOCK_NB;
        // SAFETY: the descriptor stays open for the lifetime of `file`.
        if unsafe { libc::flock(file.as_raw_fd(), op) } != 0 {
            let error = std::io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::EWOULDBLOCK) => return Err(LeaseError::Busy),
                Some(libc::EINTR) => continue,
                _ => return Err(io_error(format!("lock {}", path.display()), error)),
            }
        }
        let held = file
            .metadata()
            .map_err(|e| io_error(format!("stat {}", path.display()), e))?;
        match fs::symlink_metadata(path) {
            Ok(now) if now.ino() == held.ino() && now.dev() == held.dev() => return Ok(Some(file)),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_error(format!("stat {}", path.display()), e)),
        }
    }
    Err(LeaseError::Io(mkdir_error.unwrap_or_else(|| {
        format!("{} keeps being replaced", path.display())
    })))
}

/// The entry guard: shared by the gate path while it creates and leases a slot, exclusive for prune and clear.
#[derive(Debug)]
pub struct EntryGuard {
    _file: File,
}

impl EntryGuard {
    pub fn try_shared(entry_dir: &Path) -> Result<EntryGuard, LeaseError> {
        Self::acquire_hooked(entry_dir, false, &|| {})
    }

    pub fn try_exclusive(entry_dir: &Path) -> Result<EntryGuard, LeaseError> {
        Self::acquire_hooked(entry_dir, true, &|| {})
    }

    /// `after_open` runs between opening the guard file and locking it; for tests of the rename race.
    #[doc(hidden)]
    pub fn acquire_hooked(
        entry_dir: &Path,
        exclusive: bool,
        after_open: &dyn Fn(),
    ) -> Result<EntryGuard, LeaseError> {
        match lock_path(&entry_dir.join(GUARD_FILE), true, exclusive, after_open)? {
            Some(file) => Ok(EntryGuard { _file: file }),
            None => Err(LeaseError::Io("guard file vanished".into())),
        }
    }
}

/// An exclusive hold on one slot of one entry. Dropping it releases the slot.
#[derive(Debug)]
pub struct GateLease {
    file: File,
}

impl AsRawFd for GateLease {
    fn as_raw_fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }
}

/// `<entry>/.slot-<project>-<n>.lease`.
pub fn lease_path(entry_dir: &Path, project: &str, slot: usize) -> PathBuf {
    entry_dir.join(format!(".slot-{project}-{slot}.lease"))
}

/// The slot's directories, one per gate subdirectory: `<entry>/<subdir>/<project>/slot-<n>`.
pub fn slot_dirs(entry_dir: &Path, subdirs: &[String], project: &str, slot: usize) -> Vec<PathBuf> {
    subdirs
        .iter()
        .map(|subdir| {
            entry_dir
                .join(subdir)
                .join(project)
                .join(format!("slot-{slot}"))
        })
        .collect()
}

impl GateLease {
    /// Takes the slot when its lease file exists and nobody holds it. Opens the file afresh on every call and never
    /// creates anything; a missing file, a busy slot and an error all give `None`.
    pub fn try_acquire(entry_dir: &Path, project: &str, slot: usize) -> Option<GateLease> {
        let path = lease_path(entry_dir, project, slot);
        match lock_path(&path, false, true, &|| {}) {
            Ok(Some(file)) => Some(GateLease { file }),
            _ => None,
        }
    }

    /// Takes the slot, creating the lease file when needed. `Busy` when another description holds it.
    pub fn acquire(entry_dir: &Path, project: &str, slot: usize) -> Result<GateLease, LeaseError> {
        Self::acquire_hooked(entry_dir, project, slot, &|| {})
    }

    /// `acquire` with `after_open` between opening the file and locking it; for tests of the rename race.
    #[doc(hidden)]
    pub fn acquire_hooked(
        entry_dir: &Path,
        project: &str,
        slot: usize,
        after_open: &dyn Fn(),
    ) -> Result<GateLease, LeaseError> {
        let path = lease_path(entry_dir, project, slot);
        match lock_path(&path, true, true, after_open)? {
            Some(file) => Ok(GateLease { file }),
            None => Err(LeaseError::Io("lease file vanished".into())),
        }
    }

    fn touch(&self) {
        let _ = self.file.set_modified(SystemTime::now());
    }
}

/// True while another description holds the slot. Never creates anything; a lease it cannot examine counts as held.
pub fn in_use(entry_dir: &Path, project: &str, slot: usize) -> bool {
    let path = lease_path(entry_dir, project, slot);
    !matches!(
        lock_path(&path, false, true, &|| {}),
        Ok(None) | Ok(Some(_))
    )
}

/// The leases in an entry directory, of every project: `(project, slot, lease file)`. In `.slot-<project>-<n>.lease`
/// the slot is the segment after the last hyphen (a project id may contain hyphens); a name that does not parse is
/// ignored.
pub fn slot_leases(entry_dir: &Path) -> Vec<(String, usize, PathBuf)> {
    let Ok(read) = fs::read_dir(entry_dir) else {
        return Vec::new();
    };
    let mut found: Vec<(String, usize, PathBuf)> = read
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let middle = name.strip_prefix(".slot-")?.strip_suffix(".lease")?;
            let (project, slot) = middle.rsplit_once('-')?;
            if project.is_empty() || slot.is_empty() || !slot.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            Some((project.to_string(), slot.parse().ok()?, e.path()))
        })
        .collect();
    found.sort();
    found
}

/// `<entry>/<project>/slot-<n>`, how prune reports a slot.
pub fn slot_label(entry: &str, project: &str, slot: usize) -> String {
    format!("{entry}/{project}/slot-{slot}")
}

/// False when a second open file description can take a lock the first one holds (NFS, synced folders): leases
/// would not exclude anyone there.
pub fn self_test(root: &Path) -> bool {
    let path = root.join(format!(
        ".lease-selftest-{}-{}",
        std::process::id(),
        nanos()
    ));
    let verdict = (|| -> std::io::Result<bool> {
        let first = OpenOptions::new()
            .create_new(true)
            .append(true)
            .mode(0o600)
            .open(&path)?;
        let second = OpenOptions::new().read(true).open(&path)?;
        // SAFETY: both descriptors stay open for the whole block.
        let held = unsafe { libc::flock(first.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
        if !held {
            return Ok(false);
        }
        let rc = unsafe { libc::flock(second.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        let refused =
            rc != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EWOULDBLOCK);
        Ok(refused)
    })();
    let _ = fs::remove_file(&path);
    verdict.unwrap_or(false)
}

/// One entry's part of a gate lease set.
#[derive(Clone, Debug)]
pub(crate) struct Target {
    pub entry_dir: PathBuf,
    pub project: String,
    pub subdirs: Vec<String>,
}

/// The leases of one gate: this project's slot of every entry that has gate directories.
#[derive(Debug)]
pub struct GateLeases {
    slot: usize,
    targets: Vec<Target>,
    leases: Vec<GateLease>,
}

fn lease_one(
    target: &Target,
    slot: usize,
    wait: Duration,
    abort: &AtomicBool,
) -> Result<GateLease, LeaseError> {
    let deadline = Instant::now() + wait;
    loop {
        if abort.load(Ordering::SeqCst) {
            return Err(LeaseError::Aborted);
        }
        match attempt(target, slot) {
            Ok(lease) => return Ok(lease),
            Err(LeaseError::Busy) => {}
            Err(other) => return Err(other),
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(LeaseError::Busy);
        }
        std::thread::sleep(POLL.min(deadline - now));
    }
}

/// Guard shared, slot directories, slot lease exclusive, guard released.
fn attempt(target: &Target, slot: usize) -> Result<GateLease, LeaseError> {
    // The guard file's open creates the entry directory when it is missing (see `lock_path`).
    let _guard = EntryGuard::try_shared(&target.entry_dir)?;
    for dir in slot_dirs(&target.entry_dir, &target.subdirs, &target.project, slot) {
        mkdir_private(&dir).map_err(|e| LeaseError::Io(e.to_string()))?;
    }
    let lease = GateLease::acquire(&target.entry_dir, &target.project, slot)?;
    lease.touch();
    Ok(lease)
}

fn lease_all(
    targets: &[Target],
    slot: usize,
    wait: Duration,
    abort: &AtomicBool,
) -> Result<Vec<GateLease>, LeaseError> {
    let mut leases = Vec::new();
    for target in targets {
        // On an error the leases already taken drop with `leases`.
        leases.push(lease_one(target, slot, wait, abort)?);
    }
    Ok(leases)
}

impl GateLeases {
    pub(crate) fn acquire(
        targets: Vec<Target>,
        slot: usize,
        wait: Duration,
        abort: &AtomicBool,
    ) -> Result<GateLeases, LeaseError> {
        let leases = lease_all(&targets, slot, wait, abort)?;
        Ok(GateLeases {
            slot,
            targets,
            leases,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.leases.is_empty()
    }

    pub fn slot(&self) -> usize {
        self.slot
    }

    /// Drops every lease and leases the same slot again in place (recreating directories a build removed).
    /// All or nothing: on an error no lease is held.
    pub fn reacquire(&mut self, wait: Duration, abort: &AtomicBool) -> Result<(), LeaseError> {
        self.leases.clear();
        self.leases = lease_all(&self.targets, self.slot, wait, abort)?;
        Ok(())
    }
}
