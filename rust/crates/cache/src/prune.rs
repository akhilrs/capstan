use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::error::{CacheError, Result};
use crate::fsutil::{nanos, now_ms, pid_alive, remove_tree};
use crate::idle::is_idle;
use crate::lease::{
    self, slot_label, EntryGuard, GateLease, LeaseError, GUARD_FILE, IN_USE_REASON,
};
use crate::manifest::{read_manifest, write_locked, Manifest, OccupantRecord, StoreLock};
use crate::measure::{
    entry_slots, find_units, measure, measure_dir, setup_keys, EntrySize, SetupKey, SlotRef,
    SlotSize, UnitSize,
};
use crate::runtime::{ResolvedEntry, Runtime, SETUP_ENTRY};

const COMMAND_TIMEOUT_MS: u64 = 60_000;
const MEASURE_BUDGET: Duration = Duration::from_secs(60);
const DAY_MS: u64 = 86_400_000;

/// Why prune and clear skip an entry whose guard another process holds for the moment (a gate creating a slot).
pub const GUARD_BUSY_REASON: &str = "busy, retried next prune";
/// Why a prune left whole entries, expired items and setup keys while agents run.
pub const NOT_IDLE_REASON: &str = "agents active";
/// Why a unit stayed: its build tool holds the unit lock.
pub const BUILDING_REASON: &str = "building";
/// Why a unit stayed while agents run: something inside changed within `Runtime::active_unit_min_age`.
pub const RECENT_REASON: &str = "recently used";
/// Why a unit stayed while agents run: a process runs a binary from it or has it as its working directory.
pub const IN_PROCESS_REASON: &str = "a process runs from it";
/// Why an entry over the cap cannot be trimmed unit by unit.
pub const NO_UNITS_REASON: &str = "no evictable units";
/// Why a scratch directory the daemon knows about is listed: it counts against the cap and is never removed.
pub const OCCUPANT_REASON: &str = "held by verify";
/// Why bytes over the cap stay: the tool (sccache) trims its own directory, only after it restarts.
pub const TOOL_TRIMMED_REASON: &str = "trimmed by sccache";

/// Disk the daemon knows is in use outside the cache (verify scratch): counted against the cap, never removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occupant {
    pub label: String,
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub max_bytes: u64,
    pub max_age_ms: u64,
    pub idle: bool,
}

impl Policy {
    /// The runtime's own bounds; `idle` is the caller's answer from `is_idle`.
    pub fn for_runtime(runtime: &Runtime, idle: bool) -> Policy {
        Policy {
            max_bytes: runtime.max_bytes,
            max_age_ms: u64::from(runtime.max_age_days) * DAY_MS,
            idle,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CommandOutcome {
    pub ok: bool,
    pub message: String,
}

/// Runs one shell command: (command, cwd, exact environment, timeout in ms).
pub type Runner<'a> = &'a dyn Fn(&str, &Path, &BTreeMap<String, String>, u64) -> CommandOutcome;

#[derive(Clone, Debug)]
pub struct CommandRun {
    pub entry: String,
    pub command: String,
    pub ok: bool,
    pub message: String,
}

#[derive(Clone, Debug, Default)]
pub struct PruneReport {
    pub commands: Vec<CommandRun>,
    /// Entries (or `setup/<key>`) removed.
    pub cleared: Vec<String>,
    /// What an idle prune would remove; the same list a non-idle prune reports instead of clearing.
    pub would_clear: Vec<String>,
    pub total_before: u64,
    pub total_after: u64,
    pub partial: bool,
    pub errors: Vec<String>,
    /// What prune left alone and why: a leased slot (`IN_USE_REASON`) or a busy entry guard (`GUARD_BUSY_REASON`).
    pub skipped: Vec<Skipped>,
}

/// One label prune or clear did not remove, with the reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    pub label: String,
    pub reason: String,
}

/// Test hooks around the rename into `.trash`. `before_rename` runs with the entry guard and the slot leases held
/// (given the entry directory); `after_rename` runs once they are released, before the trash is deleted.
#[doc(hidden)]
pub struct PruneHooks<'a> {
    pub before_rename: &'a dyn Fn(&Path),
    pub after_rename: &'a dyn Fn(),
    /// Runs with the unit lock held, before the active rule is checked again and the unit is emptied (the unit dir).
    pub before_unit_rename: &'a dyn Fn(&Path),
}

type LockOpenHook = Box<dyn Fn(&Path)>;

thread_local! {
    static AFTER_LOCK_OPEN: std::cell::RefCell<Option<LockOpenHook>> =
        const { std::cell::RefCell::new(None) };
}

/// Test hook: runs on this thread after a unit lock file is opened and before it is locked.
#[doc(hidden)]
pub fn set_after_lock_open_hook(hook: Option<LockOpenHook>) {
    AFTER_LOCK_OPEN.with(|cell| *cell.borrow_mut() = hook);
}

fn no_path(_: &Path) {}
fn no_op() {}

impl Default for PruneHooks<'_> {
    fn default() -> Self {
        PruneHooks {
            before_rename: &no_path,
            after_rename: &no_op,
            before_unit_rename: &no_path,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ClearReport {
    pub cleared: Vec<String>,
    pub stop_commands: Vec<CommandRun>,
    /// Victims that could not be removed (`name: reason`); the others were still processed.
    pub errors: Vec<String>,
    /// What clear left alone and why (see `PruneReport::skipped`).
    pub skipped: Vec<Skipped>,
}

/// One directory of an eviction unit: what is emptied (`rel`) and the directory whose lock file guards it (`lock_rel`,
/// the same for a unit, the unit's for its `incremental/` sub-unit); both relative to the entry directory.
#[derive(Clone, Debug, PartialEq, Eq)]
struct UnitMember {
    rel: String,
    lock_rel: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Victim {
    Key(String),
    Entry(String),
    Slot {
        entry: String,
        project: String,
        slot: usize,
    },
    /// One unit, or a pair (artifact and build directory of a split build dir) emptied together.
    Unit {
        entry: String,
        members: Vec<UnitMember>,
    },
}

impl Victim {
    fn labels(&self) -> Vec<String> {
        match self {
            Self::Unit { entry, members } => members
                .iter()
                .map(|m| format!("{entry}/{}", m.rel))
                .collect(),
            other => vec![other.label()],
        }
    }

    fn label(&self) -> String {
        match self {
            Self::Unit { entry, members } => format!(
                "{entry}/{}",
                members.first().map(|m| m.rel.as_str()).unwrap_or_default()
            ),
            Self::Key(key) => format!("{SETUP_ENTRY}/{key}"),
            Self::Entry(name) => name.clone(),
            Self::Slot {
                entry,
                project,
                slot,
            } => slot_label(entry, project, *slot),
        }
    }
}

fn run_stop(
    runtime: &Runtime,
    entry: &ResolvedEntry,
    base_env: &BTreeMap<String, String>,
    runner: Runner<'_>,
) -> Vec<CommandRun> {
    let dir = runtime.entry_dir(&entry.name);
    if !entry.enabled || !dir.is_dir() {
        return Vec::new();
    }
    let env = runtime.command_environment(entry, base_env);
    entry
        .stop
        .iter()
        .map(|command| {
            let outcome = runner(command, &dir, &env, COMMAND_TIMEOUT_MS);
            CommandRun {
                entry: entry.name.clone(),
                command: command.clone(),
                ok: outcome.ok,
                message: outcome.message,
            }
        })
        .collect()
}

static TRASH_SEQ: AtomicU64 = AtomicU64::new(0);

/// Renames `source` into `<root>/.trash`; `None` when it is already gone. The caller holds the store lock.
fn move_to_trash(root: &Path, source: &Path, label: &str) -> Result<Option<PathBuf>> {
    let trash_dir = root.join(".trash");
    crate::fsutil::mkdir_private(&trash_dir)?;
    let target = trash_dir.join(format!(
        "{}-{}-{}",
        label.replace('/', "_"),
        nanos(),
        TRASH_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    match fs::rename(source, &target) {
        Ok(()) => Ok(Some(target)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CacheError::io(
            format!("move {} to trash", source.display()),
            e,
        )),
    }
}

fn delete_trashed(targets: &[PathBuf]) -> Result<()> {
    let mut first = None;
    for target in targets {
        if let Err(e) = remove_tree(target) {
            first.get_or_insert(CacheError::io(format!("delete {}", target.display()), e));
        }
    }
    first.map_or(Ok(()), Err)
}

/// Renames `source` into `.trash` and deletes it there (no entry guard involved: setup snapshots).
fn trash(root: &Path, source: &Path, label: &str, hooks: &PruneHooks<'_>) -> Result<bool> {
    let Some(target) = move_to_trash(root, source, label)? else {
        return Ok(false);
    };
    (hooks.after_rename)();
    delete_trashed(&[target])?;
    Ok(true)
}

/// What the victims applied so far did.
#[derive(Default)]
struct Outcome {
    cleared: Vec<String>,
    skipped: Vec<Skipped>,
    stops: Vec<CommandRun>,
}

impl Outcome {
    fn skip(&mut self, label: String, reason: &str) {
        let item = Skipped {
            label,
            reason: reason.to_string(),
        };
        if !self.skipped.contains(&item) {
            self.skipped.push(item);
        }
    }
}

/// The entry guard (exclusive) and every slot lease of an entry that could be taken.
struct Held {
    _guard: Option<EntryGuard>,
    _leases: Vec<GateLease>,
    /// Slots whose lease another process holds.
    leased: BTreeSet<(String, usize)>,
}

enum Locked {
    Held(Held),
    /// The guard is busy: leave the entry for the next prune.
    Busy,
    Gone,
}

fn lock_entry(dir: &Path, gated: bool) -> Result<Locked> {
    let guard = if gated {
        match EntryGuard::try_exclusive(dir) {
            Ok(guard) => Some(guard),
            Err(LeaseError::Busy) => return Ok(Locked::Busy),
            Err(e) if dir.is_dir() => return Err(CacheError::Io(e.to_string())),
            Err(_) => return Ok(Locked::Gone),
        }
    } else {
        None
    };
    let mut leases = Vec::new();
    let mut leased = BTreeSet::new();
    for (project, slot, path) in lease::slot_leases(dir) {
        match GateLease::try_acquire(dir, &project, slot) {
            Some(lease) => leases.push(lease),
            None if path.exists() => {
                leased.insert((project, slot));
            }
            None => {}
        }
    }
    Ok(Locked::Held(Held {
        _guard: guard,
        _leases: leases,
        leased,
    }))
}

fn is_gated(runtime: &Runtime, name: &str, dir: &Path) -> bool {
    runtime.entry(name).is_some_and(|e| !e.gate_dirs.is_empty())
        || dir.join(GUARD_FILE).exists()
        || !lease::slot_leases(dir).is_empty()
}

fn subdirs_of(runtime: &Runtime, name: &str) -> Vec<String> {
    runtime
        .entry(name)
        .map(|e| e.gate_dirs.iter().map(|(_, s)| s.clone()).collect())
        .unwrap_or_default()
}

/// Moves a slot's directories into `.trash` and removes its lease file. The caller holds the guard and the lease.
fn take_slot(
    runtime: &Runtime,
    dir: &Path,
    entry: &str,
    found: &SlotRef,
    hooks: &PruneHooks<'_>,
    targets: &mut Vec<PathBuf>,
) -> Result<()> {
    let label = slot_label(entry, &found.project, found.slot);
    for (index, slot_dir) in found.dirs.iter().enumerate() {
        if index == 0 {
            (hooks.before_rename)(dir);
        }
        targets.extend(move_to_trash(&runtime.root, slot_dir, &label)?);
    }
    match fs::remove_file(&found.lease_file) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(CacheError::io(
                format!("remove {}", found.lease_file.display()),
                e,
            ))
        }
    }
    Ok(())
}

fn apply_entry(
    runtime: &Runtime,
    name: &str,
    base_env: &BTreeMap<String, String>,
    runner: Runner<'_>,
    hooks: &PruneHooks<'_>,
    out: &mut Outcome,
) -> Result<()> {
    // Stop commands run before the guard is taken, so a gate never waits for them.
    if let Some(entry) = runtime.entry(name) {
        out.stops.extend(run_stop(runtime, entry, base_env, runner));
    }
    let dir = runtime.entry_dir(name);
    if !dir.is_dir() {
        return Ok(());
    }
    let held = match lock_entry(&dir, is_gated(runtime, name, &dir))? {
        Locked::Held(held) => held,
        Locked::Busy => {
            out.skip(name.to_string(), GUARD_BUSY_REASON);
            return Ok(());
        }
        Locked::Gone => return Ok(()),
    };
    let mut targets = Vec::new();
    let mut removed = Vec::new();
    let mut unit_locks = Vec::new();
    if held.leased.is_empty() {
        // A build holding a unit lock is never renamed away under its feet.
        let lock_files = unit_lock_files(runtime, name, &dir);
        match lock_files_nb(&lock_files)? {
            LockFiles::Held(files) => unit_locks = files,
            LockFiles::Busy => {
                out.skip(name.to_string(), BUILDING_REASON);
                return Ok(());
            }
            LockFiles::Gone => return Ok(()),
        }
        (hooks.before_rename)(&dir);
        targets.extend(move_to_trash(&runtime.root, &dir, name)?);
        if targets.is_empty() {
            return Ok(());
        }
        removed.push(name.to_string());
    } else {
        // A leased slot keeps the entry; only the slots nobody holds go.
        for found in entry_slots(&dir, &subdirs_of(runtime, name)) {
            let label = slot_label(name, &found.project, found.slot);
            if held.leased.contains(&(found.project.clone(), found.slot)) {
                out.skip(label, IN_USE_REASON);
                continue;
            }
            take_slot(runtime, &dir, name, &found, hooks, &mut targets)?;
            removed.push(label);
        }
        for (project, slot) in &held.leased {
            out.skip(slot_label(name, project, *slot), IN_USE_REASON);
        }
    }
    drop(unit_locks);
    drop(held);
    (hooks.after_rename)();
    delete_trashed(&targets)?;
    out.cleared.extend(removed);
    Ok(())
}

/// The unit lock files of an entry (every unit directory that exists now), in path order.
fn unit_lock_files(runtime: &Runtime, name: &str, dir: &Path) -> Vec<PathBuf> {
    let Some(entry) = runtime.entry(name) else {
        return Vec::new();
    };
    let lock = entry.unit_lock.as_deref().unwrap_or_default();
    find_units(entry, dir)
        .into_iter()
        .map(|(_, path)| path.join(lock))
        .collect()
}

enum LockFiles {
    /// Every file locked; the locks are released when the files are dropped.
    Held(Vec<File>),
    Busy,
    Gone,
}

/// Opens each file (without creating it) and takes `flock` LOCK_EX|LOCK_NB, in the given order.
fn lock_files_nb(paths: &[PathBuf]) -> Result<LockFiles> {
    let mut held = Vec::new();
    for path in paths {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(LockFiles::Gone),
            Err(e) => return Err(CacheError::io(format!("open {}", path.display()), e)),
        };
        AFTER_LOCK_OPEN.with(|cell| {
            if let Some(hook) = cell.borrow().as_ref() {
                hook(path);
            }
        });
        // SAFETY: the descriptor stays open for the lifetime of `file`.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            return if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
                Ok(LockFiles::Busy)
            } else {
                Err(CacheError::io(format!("lock {}", path.display()), error))
            };
        }
        // The lock file may have been replaced between the open and the lock (a build recreating its target
        // directory): the lock we hold is then on an orphaned inode and protects nothing, so the unit counts as
        // building.
        match same_file(&file, path) {
            Ok(true) => {}
            Ok(false) => return Ok(LockFiles::Busy),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(LockFiles::Gone),
            Err(e) => return Err(CacheError::io(format!("stat {}", path.display()), e)),
        }
        held.push(file);
    }
    Ok(LockFiles::Held(held))
}

/// Whether the open `file` is still the file at `path` (same device and inode).
fn same_file(file: &File, path: &Path) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let held = file.metadata()?;
    let current = fs::metadata(path)?;
    Ok(held.dev() == current.dev() && held.ino() == current.ino())
}

/// The paths in a process command line: absolute arguments, and relative ones with a separator when the process
/// working directory is known, each resolved through symlinks (an unresolvable one stays as written).
#[cfg(target_os = "linux")]
fn cmdline_paths(raw: &[u8], cwd: Option<&Path>) -> Vec<PathBuf> {
    raw.split(|b| *b == 0)
        .filter_map(|arg| {
            let text = String::from_utf8_lossy(arg).into_owned();
            let path = PathBuf::from(&text);
            let full = if path.is_absolute() {
                path
            } else if text.contains('/') {
                cwd?.join(path)
            } else {
                return None;
            };
            Some(fs::canonicalize(&full).unwrap_or(full))
        })
        .collect()
}

/// What processes run from: their executable and working directory, read once per prune.
struct ProcScan {
    paths: Vec<PathBuf>,
    /// A link of one of our own processes could not be read: every unit counts as in use.
    unknown: bool,
}

impl ProcScan {
    #[cfg(target_os = "linux")]
    fn take() -> ProcScan {
        use std::os::unix::fs::MetadataExt;
        let mut scan = ProcScan {
            paths: Vec::new(),
            unknown: false,
        };
        let Ok(read) = fs::read_dir("/proc") else {
            return scan;
        };
        // SAFETY: geteuid has no preconditions.
        let me = unsafe { libc::geteuid() };
        for process in read.flatten() {
            let name = process.file_name().to_string_lossy().into_owned();
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            for link in ["exe", "cwd"] {
                match fs::read_link(process.path().join(link)) {
                    Ok(target) => {
                        let text = target.to_string_lossy();
                        let text = text.strip_suffix(" (deleted)").unwrap_or(&text);
                        scan.paths.push(PathBuf::from(text));
                    }
                    Err(e) => {
                        let gone = matches!(e.raw_os_error(), Some(libc::ENOENT | libc::ESRCH));
                        // Another user's process cannot reach the private store; ours might.
                        let ours = fs::metadata(process.path()).is_ok_and(|m| m.uid() == me);
                        if !gone && ours {
                            // A process of ours whose links are hidden (a privilege-dropped daemon, an ssh session):
                            // its command line still tells whether it was started from a path in the store. Only
                            // a process that hides that too makes every unit count as in use.
                            match fs::read(process.path().join("cmdline")) {
                                Ok(raw) if !raw.is_empty() => {
                                    let cwd = fs::read_link(process.path().join("cwd")).ok();
                                    scan.paths.extend(cmdline_paths(&raw, cwd.as_deref()));
                                }
                                Ok(_) => {}
                                Err(_) => scan.unknown = true,
                            }
                        }
                    }
                }
            }
        }
        scan
    }

    #[cfg(not(target_os = "linux"))]
    fn take() -> ProcScan {
        ProcScan {
            paths: Vec::new(),
            unknown: false,
        }
    }

    fn runs_from(&self, dir: &Path) -> bool {
        let dir = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        self.unknown || self.paths.iter().any(|p| p.starts_with(&dir))
    }
}

/// The active rule of a prune that runs while agents work.
struct Active {
    min_age_ms: u64,
    scan: OnceCell<ProcScan>,
}

impl Active {
    fn new(runtime: &Runtime) -> Active {
        Active {
            min_age_ms: runtime.active_unit_min_age.as_millis() as u64,
            scan: OnceCell::new(),
        }
    }

    /// Why `dir` may not be evicted now, if it may not.
    fn blocks(&self, dir: &Path) -> Option<&'static str> {
        let size = measure_dir(dir, &[], Instant::now() + MEASURE_BUDGET);
        if size.partial || now_ms().saturating_sub(size.newest_mtime_ms) < self.min_age_ms {
            return Some(RECENT_REASON);
        }
        self.scan
            .get_or_init(ProcScan::take)
            .runs_from(dir)
            .then_some(IN_PROCESS_REASON)
    }
}

/// Empties one unit (or a pair) under its lock(s): the lock file and the directory keep their inodes, so a build
/// blocked on the lock proceeds into the same, now empty, directory.
fn apply_unit(
    runtime: &Runtime,
    entry_name: &str,
    members: &[UnitMember],
    active: Option<&Active>,
    hooks: &PruneHooks<'_>,
    out: &mut Outcome,
) -> Result<()> {
    let dir = runtime.entry_dir(entry_name);
    let Some(lock_name) = runtime
        .entry(entry_name)
        .and_then(|e| e.unit_lock.as_deref())
    else {
        return Ok(());
    };
    let labels: Vec<String> = members
        .iter()
        .map(|m| format!("{entry_name}/{}", m.rel))
        .collect();
    if !dir.is_dir() {
        return Ok(());
    }
    let held = match lock_entry(&dir, true)? {
        Locked::Held(held) => held,
        Locked::Busy => {
            out.skip(entry_name.to_string(), GUARD_BUSY_REASON);
            return Ok(());
        }
        Locked::Gone => return Ok(()),
    };
    // Fixed order (artifact directory before build directory), so two prunes never wait on each other.
    let mut order: Vec<&UnitMember> = members.iter().collect();
    order.sort_by(|a, b| b.lock_rel.cmp(&a.lock_rel));
    let lock_paths: Vec<PathBuf> = order
        .iter()
        .map(|m| dir.join(&m.lock_rel).join(lock_name))
        .collect();
    let locks = match lock_files_nb(&lock_paths)? {
        LockFiles::Held(files) => files,
        LockFiles::Busy => {
            for label in labels {
                out.skip(label, BUILDING_REASON);
            }
            return Ok(());
        }
        LockFiles::Gone => return Ok(()),
    };
    for member in &order {
        (hooks.before_unit_rename)(&dir.join(&member.lock_rel));
    }
    if let Some(active) = active {
        for member in &order {
            if let Some(reason) = active.blocks(&dir.join(&member.rel)) {
                for label in &labels {
                    out.skip(label.clone(), reason);
                }
                return Ok(());
            }
        }
    }
    let mut targets = Vec::new();
    let mut failure = None;
    'members: for member in &order {
        let label = format!("{entry_name}/{}", member.rel);
        let evicted = dir.join(&member.rel);
        let children: Vec<PathBuf> = if member.rel == member.lock_rel {
            match fs::read_dir(&evicted) {
                Ok(read) => read
                    .flatten()
                    .filter(|c| c.file_name() != lock_name)
                    .map(|c| c.path())
                    .collect(),
                Err(_) => Vec::new(),
            }
        } else {
            vec![evicted]
        };
        for child in children {
            match move_to_trash(&runtime.root, &child, &label) {
                Ok(target) => targets.extend(target),
                Err(e) => {
                    failure = Some(e);
                    break 'members;
                }
            }
        }
    }
    drop(locks);
    drop(held);
    (hooks.after_rename)();
    let deleted = delete_trashed(&targets);
    if let Some(e) = failure {
        return Err(e);
    }
    deleted?;
    out.cleared.extend(labels);
    Ok(())
}

fn apply_slot(
    runtime: &Runtime,
    entry: &str,
    project: &str,
    slot: usize,
    hooks: &PruneHooks<'_>,
    out: &mut Outcome,
) -> Result<()> {
    let dir = runtime.entry_dir(entry);
    let label = slot_label(entry, project, slot);
    if !dir.is_dir() {
        return Ok(());
    }
    let held = match lock_entry(&dir, true)? {
        Locked::Held(held) => held,
        Locked::Busy => {
            out.skip(entry.to_string(), GUARD_BUSY_REASON);
            return Ok(());
        }
        Locked::Gone => return Ok(()),
    };
    if held.leased.contains(&(project.to_string(), slot)) {
        out.skip(label, IN_USE_REASON);
        return Ok(());
    }
    let found = entry_slots(&dir, &subdirs_of(runtime, entry))
        .into_iter()
        .find(|f| f.project == project && f.slot == slot);
    let Some(found) = found else {
        return Ok(());
    };
    let mut targets = Vec::new();
    take_slot(runtime, &dir, entry, &found, hooks, &mut targets)?;
    drop(held);
    (hooks.after_rename)();
    delete_trashed(&targets)?;
    out.cleared.push(label);
    Ok(())
}

/// Applies one victim. The store lock is held by the caller.
fn apply(
    runtime: &Runtime,
    victim: &Victim,
    base_env: &BTreeMap<String, String>,
    runner: Runner<'_>,
    hooks: &PruneHooks<'_>,
    active: Option<&Active>,
    out: &mut Outcome,
) -> Result<()> {
    match victim {
        Victim::Key(key) => {
            if trash(
                &runtime.root,
                &runtime.entry_dir(SETUP_ENTRY).join(key),
                &victim.label(),
                hooks,
            )? {
                out.cleared.push(victim.label());
            }
            Ok(())
        }
        Victim::Entry(name) => apply_entry(runtime, name, base_env, runner, hooks, out),
        Victim::Slot {
            entry,
            project,
            slot,
        } => apply_slot(runtime, entry, project, *slot, hooks, out),
        Victim::Unit { entry, members } => apply_unit(runtime, entry, members, active, hooks, out),
    }
}

fn older_first<T>(items: &mut [(T, u64, u64)]) {
    // Least recently used first; ties: larger first.
    items.sort_by(|a, b| a.1.cmp(&b.1).then(b.2.cmp(&a.2)));
}

type SlotKey = (String, String, usize);
type UnitKey = (String, String);

/// What the plan removes, and whether only an idle prune may.
#[derive(Clone, Debug)]
struct Planned {
    victim: Victim,
    idle_only: bool,
}

/// A unit (or a pair of units of a split build dir) as one candidate.
#[derive(Clone, Debug)]
struct Group {
    entry: String,
    members: Vec<UnitMember>,
    /// Newest modification inside; `u64::MAX` when a member was only partly measured.
    newest: u64,
    sub: bool,
}

struct Plan {
    victims: Vec<Planned>,
    skipped: Vec<Skipped>,
}

struct Planner {
    victims: Vec<Planned>,
    skipped: Vec<Skipped>,
    /// Running total of what remains of the store, in bytes.
    total: u64,
    occupants: u64,
    cap: u64,
    /// A prune with agents running: a unit used lately stays.
    active: bool,
    min_age_ms: u64,
    now: u64,
    /// What remains of each entry: slot and unit victims come off it.
    entry_bytes: BTreeMap<String, u64>,
    unit_left: BTreeMap<UnitKey, u64>,
    leased: BTreeSet<SlotKey>,
    taken_slots: BTreeSet<SlotKey>,
    taken_entries: BTreeSet<String>,
    taken_keys: BTreeSet<String>,
    taken_units: BTreeSet<UnitKey>,
}

impl Planner {
    fn skip(&mut self, label: String, reason: &str) {
        let item = Skipped {
            label,
            reason: reason.to_string(),
        };
        if !self.skipped.contains(&item) {
            self.skipped.push(item);
        }
    }

    fn over(&self) -> bool {
        self.total + self.occupants > self.cap
    }

    /// An unleased slot goes; a leased one is listed and stays.
    fn take_slot(&mut self, entry: &str, slot: &SlotSize, idle_only: bool) {
        let key = (entry.to_string(), slot.project.clone(), slot.slot);
        if self.leased.contains(&key) {
            self.skip(slot_label(entry, &slot.project, slot.slot), IN_USE_REASON);
            return;
        }
        let victim = Victim::Slot {
            entry: entry.to_string(),
            project: slot.project.clone(),
            slot: slot.slot,
        };
        if !self.taken_slots.insert(key) {
            return;
        }
        self.total = self.total.saturating_sub(slot.bytes);
        if let Some(bytes) = self.entry_bytes.get_mut(entry) {
            *bytes = bytes.saturating_sub(slot.bytes);
        }
        self.victims.push(Planned { victim, idle_only });
    }

    /// A unit goes; while agents run one used lately stays and is listed.
    fn take_group(&mut self, group: &Group, idle_only: bool) {
        let keys: Vec<UnitKey> = group
            .members
            .iter()
            .map(|m| (group.entry.clone(), m.rel.clone()))
            .collect();
        let victim = Victim::Unit {
            entry: group.entry.clone(),
            members: group.members.clone(),
        };
        let parent_taken = group.sub
            && group.members.iter().any(|m| {
                self.taken_units
                    .contains(&(group.entry.clone(), m.lock_rel.clone()))
            });
        if parent_taken || keys.iter().any(|k| self.taken_units.contains(k)) {
            return;
        }
        let bytes: u64 = keys
            .iter()
            .map(|k| self.unit_left.get(k).copied().unwrap_or(0))
            .sum();
        if bytes == 0 {
            return;
        }
        if self.active && !idle_only && too_recent(self.now, group.newest, self.min_age_ms) {
            for label in victim.labels() {
                self.skip(label, RECENT_REASON);
            }
            return;
        }
        self.taken_units.extend(keys);
        self.total = self.total.saturating_sub(bytes);
        if let Some(rest) = self.entry_bytes.get_mut(&group.entry) {
            *rest = rest.saturating_sub(bytes);
        }
        if group.sub {
            for m in &group.members {
                if let Some(left) = self
                    .unit_left
                    .get_mut(&(group.entry.clone(), m.lock_rel.clone()))
                {
                    *left = left.saturating_sub(bytes);
                }
            }
        }
        self.victims.push(Planned { victim, idle_only });
    }

    fn take_key(&mut self, key: &SetupKey) {
        if !self.taken_keys.insert(key.key.clone()) {
            return;
        }
        self.total = self.total.saturating_sub(key.bytes);
        self.victims.push(Planned {
            victim: Victim::Key(key.key.clone()),
            idle_only: true,
        });
    }

    /// A whole entry goes unless one of its slots is leased; then only its unleased slots do.
    fn take_entry(&mut self, size: &EntrySize) {
        let blocked = size.slots.iter().any(|s| {
            self.leased
                .contains(&(size.name.clone(), s.project.clone(), s.slot))
        });
        if blocked {
            for slot in &size.slots {
                self.take_slot(&size.name, slot, true);
            }
            return;
        }
        if !self.taken_entries.insert(size.name.clone()) {
            return;
        }
        let rest = self.entry_bytes.get(&size.name).copied().unwrap_or(0);
        self.total = self.total.saturating_sub(rest);
        self.victims.push(Planned {
            victim: Victim::Entry(size.name.clone()),
            idle_only: true,
        });
    }
}

fn too_recent(now: u64, then: u64, min_age_ms: u64) -> bool {
    // `true` when `then` is too recent to evict.
    then == u64::MAX || now.saturating_sub(then) < min_age_ms
}

/// The units of an entry as candidates: whole units (a pair of a split build dir as one) and sub-units.
fn groups_of(runtime: &Runtime, size: &EntrySize) -> (Vec<Group>, Vec<Group>) {
    let newest = |u: &UnitSize| {
        if u.partial {
            u64::MAX
        } else {
            u.newest_mtime_ms
        }
    };
    let split = runtime
        .entry(&size.name)
        .is_some_and(|e| e.mode.as_deref() == Some("shared-build-dir"));
    let mut whole: Vec<Group> = Vec::new();
    let mut pairs: BTreeMap<String, Vec<&UnitSize>> = BTreeMap::new();
    let mut subs = Vec::new();
    for unit in &size.units {
        if let Some(parent) = &unit.parent {
            subs.push(Group {
                entry: size.name.clone(),
                members: vec![UnitMember {
                    rel: unit.rel.clone(),
                    lock_rel: parent.clone(),
                }],
                newest: newest(unit),
                sub: true,
            });
        } else if split && unit.rel.contains('/') {
            let key = unit.rel.split_once('/').map(|(_, r)| r).unwrap_or_default();
            pairs.entry(key.to_string()).or_default().push(unit);
        } else {
            pairs.entry(unit.rel.clone()).or_default().push(unit);
        }
    }
    for (_, members) in pairs {
        whole.push(Group {
            entry: size.name.clone(),
            newest: members.iter().map(|u| newest(u)).max().unwrap_or(0),
            members: members
                .iter()
                .map(|u| UnitMember {
                    rel: u.rel.clone(),
                    lock_rel: u.rel.clone(),
                })
                .collect(),
            sub: false,
        });
    }
    (whole, subs)
}

/// Chooses what to remove, in the documented order, without touching anything. Leased slots (probed now, checked
/// again when applied) are never chosen; the ones that stood in the way are listed.
fn plan(
    runtime: &Runtime,
    sizes: &[EntrySize],
    keys: &[SetupKey],
    policy: &Policy,
    now: u64,
    occupant_bytes: u64,
) -> Plan {
    let expired =
        |last_used: u64| last_used > 0 && now.saturating_sub(last_used) > policy.max_age_ms;
    let mut leased = BTreeSet::new();
    for size in sizes {
        let dir = runtime.entry_dir(&size.name);
        for slot in &size.slots {
            if lease::in_use(&dir, &slot.project, slot.slot) {
                leased.insert((size.name.clone(), slot.project.clone(), slot.slot));
            }
        }
    }
    let mut p = Planner {
        victims: Vec::new(),
        skipped: Vec::new(),
        total: sizes.iter().map(|s| s.bytes).sum(),
        occupants: occupant_bytes,
        cap: policy.max_bytes,
        active: !policy.idle && runtime.evict_while_active,
        min_age_ms: runtime.active_unit_min_age.as_millis() as u64,
        now,
        entry_bytes: sizes.iter().map(|s| (s.name.clone(), s.bytes)).collect(),
        unit_left: sizes
            .iter()
            .flat_map(|s| {
                s.units
                    .iter()
                    .map(move |u| ((s.name.clone(), u.rel.clone()), u.bytes))
            })
            .collect(),
        leased,
        taken_slots: BTreeSet::new(),
        taken_entries: BTreeSet::new(),
        taken_keys: BTreeSet::new(),
        taken_units: BTreeSet::new(),
    };
    let groups: Vec<(Vec<Group>, Vec<Group>)> = sizes
        .iter()
        .filter(|s| s.name != SETUP_ENTRY)
        .map(|s| groups_of(runtime, s))
        .collect();
    for size in sizes.iter().filter(|s| s.name != SETUP_ENTRY) {
        for slot in size.slots.iter().filter(|s| expired(s.last_used_ms)) {
            p.take_slot(&size.name, slot, true);
        }
    }
    for (whole, _) in &groups {
        for group in whole
            .iter()
            .filter(|g| g.newest != u64::MAX && expired(g.newest))
        {
            p.take_group(group, true);
        }
    }
    for key in keys.iter().filter(|k| expired(k.last_used_ms)) {
        p.take_key(key);
    }
    for size in sizes
        .iter()
        .filter(|s| s.name != SETUP_ENTRY && expired(s.last_used_ms))
    {
        p.take_entry(size);
    }
    // An entry with its own bound keeps it, except one whose tool trims itself (sccache).
    for size in sizes.iter().filter(|s| s.name != SETUP_ENTRY) {
        let bound = runtime.entry(&size.name).and_then(|e| {
            (e.mode.as_deref() != Some("sccache"))
                .then_some(e.max_bytes)
                .flatten()
        });
        let current = p.entry_bytes.get(&size.name).copied().unwrap_or(0);
        if !p.taken_entries.contains(&size.name) && bound.is_some_and(|b| current > b) {
            p.take_entry(size);
        }
    }
    if p.over() {
        // Idle slots before anything else: a cold slot costs one build, a cold entry the whole tool cache.
        let mut rest: Vec<((String, &SlotSize), u64, u64)> = sizes
            .iter()
            .filter(|s| s.name != SETUP_ENTRY && !p.taken_entries.contains(&s.name))
            .flat_map(|s| {
                s.slots
                    .iter()
                    .map(move |slot| ((s.name.clone(), slot), slot.last_used_ms, slot.bytes))
            })
            .collect();
        older_first(&mut rest);
        for ((entry, slot), _, _) in rest {
            if !p.over() {
                break;
            }
            p.take_slot(&entry, slot, false);
        }
    }
    // Then incremental sub-units, then whole units: each oldest first, a partly measured one last.
    for sub in [true, false] {
        if !p.over() {
            break;
        }
        let mut rest: Vec<(&Group, u64, u64)> = groups
            .iter()
            .flat_map(|(whole, subs)| if sub { subs } else { whole })
            .filter(|g| !p.taken_entries.contains(&g.entry))
            .map(|g| {
                let bytes = g
                    .members
                    .iter()
                    .map(|m| {
                        p.unit_left
                            .get(&(g.entry.clone(), m.rel.clone()))
                            .copied()
                            .unwrap_or(0)
                    })
                    .sum();
                (g, g.newest, bytes)
            })
            .collect();
        older_first(&mut rest);
        for (group, _, _) in rest {
            if !p.over() {
                break;
            }
            p.take_group(group, false);
        }
    }
    if p.over() {
        let mut rest: Vec<(&SetupKey, u64, u64)> = keys
            .iter()
            .filter(|k| !p.taken_keys.contains(&k.key))
            .map(|k| (k, k.last_used_ms, k.bytes))
            .collect();
        older_first(&mut rest);
        for (key, _, _) in rest {
            if !p.over() {
                break;
            }
            p.take_key(key);
        }
    }
    if p.over() {
        let mut rest: Vec<(&EntrySize, u64, u64)> = sizes
            .iter()
            .filter(|s| s.name != SETUP_ENTRY && !p.taken_entries.contains(&s.name))
            .map(|s| {
                (
                    s,
                    s.last_used_ms,
                    p.entry_bytes.get(&s.name).copied().unwrap_or(0),
                )
            })
            // An entry with nothing left in it (its units went) frees nothing: its directory and locks stay.
            .filter(|(s, _, bytes)| *bytes > 0 || s.bytes == 0)
            .collect();
        older_first(&mut rest);
        for (size, _, _) in rest {
            if !p.over() {
                break;
            }
            p.take_entry(size);
        }
    }
    Plan {
        victims: p.victims,
        skipped: p.skipped,
    }
}

/// How far over the cap the store is with its occupants, and what keeps prune from bringing it under.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OverCap {
    pub bytes: u64,
    /// Entries whose bytes outside every slot and unit already exceed the cap on their own: prune cannot
    /// enforce the cap there. `reason` is `TOOL_TRIMMED_REASON` for an sccache entry, else `NO_UNITS_REASON`.
    pub not_enforced: Vec<Skipped>,
}

/// The cap overage of `sizes` plus `occupant_bytes`, from the measurement alone.
pub fn over_cap(runtime: &Runtime, sizes: &[EntrySize], occupant_bytes: u64) -> OverCap {
    let total: u64 = sizes.iter().map(|s| s.bytes).sum::<u64>() + occupant_bytes;
    let bytes = total.saturating_sub(runtime.max_bytes);
    if bytes == 0 {
        return OverCap::default();
    }
    let fixed: Vec<(&EntrySize, u64)> = sizes
        .iter()
        .filter(|s| s.name != SETUP_ENTRY && s.fixed_bytes > 0)
        .map(|s| (s, s.fixed_bytes))
        .collect();
    let floor: u64 = fixed.iter().map(|(_, b)| b).sum();
    let mut not_enforced = Vec::new();
    if floor > runtime.max_bytes {
        for (size, _) in fixed {
            let sccache = runtime
                .entry(&size.name)
                .is_some_and(|e| e.mode.as_deref() == Some("sccache"));
            not_enforced.push(Skipped {
                label: size.name.clone(),
                reason: if sccache {
                    TOOL_TRIMMED_REASON
                } else {
                    NO_UNITS_REASON
                }
                .to_string(),
            });
        }
    }
    OverCap {
        bytes,
        not_enforced,
    }
}

/// Runs every enabled entry's tool prune commands, then (only when idle) removes expired and least-recently-used
/// data until the store is under the cap. Gate slots whose lease is held are never removed. When not idle it
/// reports what it would remove. `Busy` when another prune or clear holds the store lock.
pub fn prune(
    runtime: &Runtime,
    policy: Policy,
    base_env: &BTreeMap<String, String>,
    runner: Runner<'_>,
) -> Result<PruneReport> {
    prune_with_hooks(runtime, policy, base_env, runner, &PruneHooks::default())
}

/// `prune` with test hooks around the rename into `.trash`.
#[doc(hidden)]
pub fn prune_with_hooks(
    runtime: &Runtime,
    policy: Policy,
    base_env: &BTreeMap<String, String>,
    runner: Runner<'_>,
    hooks: &PruneHooks<'_>,
) -> Result<PruneReport> {
    prune_with_occupants(runtime, policy, base_env, runner, hooks, None)
}

/// `prune_with_hooks` that also counts `occupants` (`Some`: the daemon knows them; `None`: unknown, as in a CLI
/// process) against the cap: the store is pruned until `total + occupants <= cap`. Occupants are never removed.
/// `Some(list)` records them (and the skipped list) in the manifest; `None` leaves what the manifest has.
#[doc(hidden)]
pub fn prune_with_occupants(
    runtime: &Runtime,
    policy: Policy,
    base_env: &BTreeMap<String, String>,
    runner: Runner<'_>,
    hooks: &PruneHooks<'_>,
    occupants: Option<&[Occupant]>,
) -> Result<PruneReport> {
    let lock = StoreLock::try_acquire(&runtime.root)?;
    let mut report = PruneReport::default();
    for entry in runtime
        .entries
        .iter()
        .filter(|e| e.enabled && !e.prune.is_empty())
    {
        let dir = runtime.entry_dir(&entry.name);
        if !dir.is_dir() {
            continue;
        }
        let env = runtime.command_environment(entry, base_env);
        for command in &entry.prune {
            let outcome = runner(command, &dir, &env, COMMAND_TIMEOUT_MS);
            report.commands.push(CommandRun {
                entry: entry.name.clone(),
                command: command.clone(),
                ok: outcome.ok,
                message: outcome.message,
            });
        }
    }
    let deadline = Instant::now() + MEASURE_BUDGET;
    let sizes = measure(runtime, deadline);
    let (keys, keys_partial) = setup_keys(runtime, deadline);
    report.partial = keys_partial || sizes.iter().any(|s| s.partial);
    report.total_before = sizes.iter().map(|s| s.bytes).sum();
    let occupant_bytes: u64 = occupants.map_or(0, |list| list.iter().map(|o| o.bytes).sum());
    let planned = plan(runtime, &sizes, &keys, &policy, now_ms(), occupant_bytes);
    report.skipped = planned.skipped;
    let mut out = Outcome::default();
    let mut remaining: Vec<&Planned> = Vec::new();
    if policy.idle {
        for planned in &planned.victims {
            let applied = apply(
                runtime,
                &planned.victim,
                base_env,
                runner,
                hooks,
                None,
                &mut out,
            );
            if let Err(e) = applied {
                report
                    .errors
                    .push(format!("{}: {e}", planned.victim.label()));
            }
        }
    } else if runtime.evict_while_active {
        let active = Active::new(runtime);
        for planned in &planned.victims {
            if planned.idle_only {
                remaining.push(planned);
                continue;
            }
            let applied = apply(
                runtime,
                &planned.victim,
                base_env,
                runner,
                hooks,
                Some(&active),
                &mut out,
            );
            if let Err(e) = applied {
                report
                    .errors
                    .push(format!("{}: {e}", planned.victim.label()));
            }
        }
    } else {
        remaining.extend(planned.victims.iter());
    }
    if !policy.idle {
        report.would_clear = remaining.iter().flat_map(|p| p.victim.labels()).collect();
        for label in &report.would_clear {
            let item = Skipped {
                label: label.clone(),
                reason: NOT_IDLE_REASON.to_string(),
            };
            if !report.skipped.contains(&item) {
                report.skipped.push(item);
            }
        }
    }
    report.cleared = std::mem::take(&mut out.cleared);
    for item in std::mem::take(&mut out.skipped) {
        if !report.skipped.contains(&item) {
            report.skipped.push(item);
        }
    }
    report.commands.extend(out.stops);
    let after = measure(runtime, Instant::now() + MEASURE_BUDGET);
    report.total_after = after.iter().map(|s| s.bytes).sum();
    let mut manifest = Manifest::new(runtime, after, now_ms());
    manifest.last_prune_at_ms = Some(now_ms());
    let previous = read_manifest(&runtime.root).ok().flatten();
    match occupants {
        Some(list) => {
            let at = now_ms();
            let unmet = report.total_after + occupant_bytes > policy.max_bytes;
            if unmet {
                for occupant in list {
                    let item = Skipped {
                        label: occupant.label.clone(),
                        reason: OCCUPANT_REASON.to_string(),
                    };
                    if !report.skipped.contains(&item) {
                        report.skipped.push(item);
                    }
                }
            }
            manifest.last_occupants = list
                .iter()
                .map(|o| OccupantRecord {
                    label: o.label.clone(),
                    bytes: o.bytes,
                    at_ms: at,
                })
                .collect();
            manifest.last_skipped = report.skipped.clone();
        }
        None => {
            // Occupants are unknown here and keep their last record; what this prune skipped is known.
            if let Some(previous) = previous {
                manifest.last_occupants = previous.last_occupants;
            }
            manifest.last_skipped = report.skipped.clone();
        }
    }
    write_locked(&runtime.root, &manifest, &|| {})?;
    drop(lock);
    Ok(report)
}

/// Clears the named entries (`None` means all, `setup/<key>` names one snapshot). Refuses with `NotIdle` while a
/// linked worktree exists unless `force`. Stop commands run before each rename. A leased gate slot is never
/// removed, `force` or not; the rest of its entry's slots go and the entry stays.
pub fn clear(
    runtime: &Runtime,
    selection: Option<&[String]>,
    force: bool,
    base_env: &BTreeMap<String, String>,
    runner: Runner<'_>,
) -> Result<ClearReport> {
    clear_with_hooks(
        runtime,
        selection,
        force,
        base_env,
        runner,
        &PruneHooks::default(),
    )
}

/// `clear` with a hook between the rename into `.trash` and the delete; for kill-safety tests.
#[doc(hidden)]
pub fn clear_hooked(
    runtime: &Runtime,
    selection: Option<&[String]>,
    force: bool,
    base_env: &BTreeMap<String, String>,
    runner: Runner<'_>,
    after_rename: &dyn Fn(),
) -> Result<ClearReport> {
    clear_with_hooks(
        runtime,
        selection,
        force,
        base_env,
        runner,
        &PruneHooks {
            before_rename: &no_path,
            after_rename,
            before_unit_rename: &no_path,
        },
    )
}

/// `clear` with test hooks around the rename into `.trash`.
#[doc(hidden)]
pub fn clear_with_hooks(
    runtime: &Runtime,
    selection: Option<&[String]>,
    force: bool,
    base_env: &BTreeMap<String, String>,
    runner: Runner<'_>,
    hooks: &PruneHooks<'_>,
) -> Result<ClearReport> {
    let victims: Vec<Victim> = match selection {
        None => runtime
            .entries
            .iter()
            .map(|e| Victim::Entry(e.name.clone()))
            .collect(),
        Some(names) => names
            .iter()
            .map(|name| match name.strip_prefix("setup/") {
                Some(key) if !key.is_empty() && !key.contains('/') && !key.starts_with('.') => {
                    Ok(Victim::Key(key.to_string()))
                }
                Some(_) => Err(CacheError::Invalid(format!(
                    "{name} is not a setup snapshot name"
                ))),
                None if runtime.entry(name).is_some() => Ok(Victim::Entry(name.clone())),
                None => Err(CacheError::Invalid(format!("no cache entry named {name}"))),
            })
            .collect::<Result<_>>()?,
    };
    if !force && !is_idle(&runtime.project_root)? {
        return Err(CacheError::NotIdle);
    }
    let lock = StoreLock::try_acquire(&runtime.root)?;
    let mut report = ClearReport::default();
    let mut out = Outcome::default();
    for victim in &victims {
        if let Err(e) = apply(runtime, victim, base_env, runner, hooks, None, &mut out) {
            report.errors.push(format!("{}: {e}", victim.label()));
        }
    }
    report.cleared = out.cleared;
    report.skipped = out.skipped;
    report.stop_commands = out.stops;
    // Always record the victims already removed, even when another one failed.
    let manifest_update = read_manifest(&runtime.root).and_then(|manifest| {
        let Some(mut manifest) = manifest else {
            return Ok(());
        };
        manifest.entries.retain(|e| {
            !report
                .cleared
                .iter()
                .any(|c| c == &e.name || (e.name == SETUP_ENTRY && c.starts_with("setup/")))
        });
        write_locked(&runtime.root, &manifest, &|| {})
    });
    if let Err(e) = manifest_update {
        report.errors.push(format!("manifest: {e}"));
    }
    drop(lock);
    Ok(report)
}

fn tmp_owner_gone(name: &str) -> bool {
    let Some((_, tail)) = name.split_once(".tmp-") else {
        return false;
    };
    let pid: Option<u32> = tail.split('-').next().and_then(|p| p.parse().ok());
    pid.is_none_or(|pid| !pid_alive(pid))
}

fn sweep_tmp(dir: &Path, removed: &mut Vec<String>) {
    let Ok(read) = fs::read_dir(dir) else { return };
    for child in read.flatten() {
        let name = child.file_name().to_string_lossy().into_owned();
        if name.contains(".tmp-") && tmp_owner_gone(&name) && remove_tree(&child.path()).is_ok() {
            removed.push(name);
        }
    }
}

/// Removes what a killed prune, clear or manifest write left behind: everything in `.trash` and the temporary
/// files and directories whose writer is gone. `Busy` while a prune or clear is running.
pub fn recover(root: &Path) -> Result<Vec<String>> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let _lock = StoreLock::try_acquire(root)?;
    let mut removed = Vec::new();
    if let Ok(read) = fs::read_dir(root.join(".trash")) {
        for child in read.flatten() {
            if remove_tree(&child.path()).is_ok() {
                removed.push(format!(".trash/{}", child.file_name().to_string_lossy()));
            }
        }
    }
    sweep_tmp(root, &mut removed);
    sweep_tmp(&root.join(SETUP_ENTRY), &mut removed);
    Ok(removed)
}

#[cfg(all(test, target_os = "linux"))]
mod cmdline_tests {
    use super::*;

    #[test]
    fn command_line_paths_are_resolved_through_symlinks_and_the_process_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let real = fs::canonicalize(dir.path()).unwrap().join("unit");
        fs::create_dir_all(real.join("bin")).unwrap();
        let link = real.parent().unwrap().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let raw = format!("tool\0{}/bin\0bin/../bin\0plain\0", link.display());
        let paths = cmdline_paths(raw.as_bytes(), Some(&real));
        assert_eq!(paths, [real.join("bin"), real.join("bin")]);
        assert_eq!(cmdline_paths(raw.as_bytes(), None), [real.join("bin")]);
    }
}
