use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::error::{CacheError, Result};
use crate::fsutil::{nanos, now_ms, pid_alive, remove_tree};
use crate::idle::is_idle;
use crate::lease::{
    self, slot_label, EntryGuard, GateLease, LeaseError, GUARD_FILE, IN_USE_REASON,
};
use crate::manifest::{read_manifest, write_locked, Manifest, StoreLock};
use crate::measure::{entry_slots, measure, setup_keys, EntrySize, SetupKey, SlotRef, SlotSize};
use crate::runtime::{ResolvedEntry, Runtime, SETUP_ENTRY};

const COMMAND_TIMEOUT_MS: u64 = 60_000;
const MEASURE_BUDGET: Duration = Duration::from_secs(60);
const DAY_MS: u64 = 86_400_000;

/// Why prune and clear skip an entry whose guard another process holds for the moment (a gate creating a slot).
pub const GUARD_BUSY_REASON: &str = "busy, retried next prune";

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
}

fn no_path(_: &Path) {}
fn no_op() {}

impl Default for PruneHooks<'_> {
    fn default() -> Self {
        PruneHooks {
            before_rename: &no_path,
            after_rename: &no_op,
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

#[derive(Clone, Debug, PartialEq, Eq)]
enum Victim {
    Key(String),
    Entry(String),
    Slot {
        entry: String,
        project: String,
        slot: usize,
    },
}

impl Victim {
    fn label(&self) -> String {
        match self {
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
    if held.leased.is_empty() {
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
    drop(held);
    (hooks.after_rename)();
    delete_trashed(&targets)?;
    out.cleared.extend(removed);
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
    }
}

fn older_first<T>(items: &mut [(T, u64, u64)]) {
    // Least recently used first; ties: larger first.
    items.sort_by(|a, b| a.1.cmp(&b.1).then(b.2.cmp(&a.2)));
}

type SlotKey = (String, String, usize);

struct Planner {
    victims: Vec<Victim>,
    skipped: Vec<Skipped>,
    /// Running total of what remains, in bytes.
    total: u64,
    /// What remains of each entry: slot victims come off it.
    entry_bytes: BTreeMap<String, u64>,
    leased: BTreeSet<SlotKey>,
    taken_slots: BTreeSet<SlotKey>,
    taken_entries: BTreeSet<String>,
}

impl Planner {
    fn skip_leased(&mut self, entry: &str, project: &str, slot: usize) {
        let item = Skipped {
            label: slot_label(entry, project, slot),
            reason: IN_USE_REASON.to_string(),
        };
        if !self.skipped.contains(&item) {
            self.skipped.push(item);
        }
    }

    /// An unleased slot goes; a leased one is listed and stays.
    fn take_slot(&mut self, entry: &str, slot: &SlotSize) {
        let key = (entry.to_string(), slot.project.clone(), slot.slot);
        if self.leased.contains(&key) {
            self.skip_leased(entry, &slot.project, slot.slot);
            return;
        }
        if !self.taken_slots.insert(key) {
            return;
        }
        self.total = self.total.saturating_sub(slot.bytes);
        if let Some(bytes) = self.entry_bytes.get_mut(entry) {
            *bytes = bytes.saturating_sub(slot.bytes);
        }
        self.victims.push(Victim::Slot {
            entry: entry.to_string(),
            project: slot.project.clone(),
            slot: slot.slot,
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
                self.take_slot(&size.name, slot);
            }
            return;
        }
        if !self.taken_entries.insert(size.name.clone()) {
            return;
        }
        let rest = self.entry_bytes.get(&size.name).copied().unwrap_or(0);
        self.total = self.total.saturating_sub(rest);
        self.victims.push(Victim::Entry(size.name.clone()));
    }
}

/// Chooses what to remove, in the documented order, without touching anything. Leased slots (probed now, checked
/// again when applied) are never chosen; the ones that stood in the way are listed.
fn plan(
    runtime: &Runtime,
    sizes: &[EntrySize],
    keys: &[SetupKey],
    policy: &Policy,
    now: u64,
) -> (Vec<Victim>, Vec<Skipped>) {
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
        entry_bytes: sizes.iter().map(|s| (s.name.clone(), s.bytes)).collect(),
        leased,
        taken_slots: BTreeSet::new(),
        taken_entries: BTreeSet::new(),
    };
    for size in sizes.iter().filter(|s| s.name != SETUP_ENTRY) {
        for slot in size.slots.iter().filter(|s| expired(s.last_used_ms)) {
            p.take_slot(&size.name, slot);
        }
    }
    for key in keys.iter().filter(|k| expired(k.last_used_ms)) {
        p.total = p.total.saturating_sub(key.bytes);
        p.victims.push(Victim::Key(key.key.clone()));
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
    if p.total > policy.max_bytes {
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
            if p.total <= policy.max_bytes {
                break;
            }
            p.take_slot(&entry, slot);
        }
    }
    if p.total > policy.max_bytes {
        let mut rest: Vec<(String, u64, u64)> = keys
            .iter()
            .filter(|k| !p.victims.contains(&Victim::Key(k.key.clone())))
            .map(|k| (k.key.clone(), k.last_used_ms, k.bytes))
            .collect();
        older_first(&mut rest);
        for (key, _, bytes) in rest {
            if p.total <= policy.max_bytes {
                break;
            }
            p.total = p.total.saturating_sub(bytes);
            p.victims.push(Victim::Key(key));
        }
    }
    if p.total > policy.max_bytes {
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
            .collect();
        older_first(&mut rest);
        for (size, _, _) in rest {
            if p.total <= policy.max_bytes {
                break;
            }
            p.take_entry(size);
        }
    }
    (p.victims, p.skipped)
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
    let (victims, skipped) = plan(runtime, &sizes, &keys, &policy, now_ms());
    report.would_clear = victims.iter().map(Victim::label).collect();
    report.skipped = skipped;
    let mut out = Outcome::default();
    if policy.idle {
        for victim in &victims {
            if let Err(e) = apply(runtime, victim, base_env, runner, hooks, &mut out) {
                report.errors.push(format!("{}: {e}", victim.label()));
            }
        }
        report.would_clear.clear();
        report.cleared = std::mem::take(&mut out.cleared);
        for item in std::mem::take(&mut out.skipped) {
            if !report.skipped.contains(&item) {
                report.skipped.push(item);
            }
        }
    }
    report.commands.extend(out.stops);
    let after = measure(runtime, Instant::now() + MEASURE_BUDGET);
    report.total_after = after.iter().map(|s| s.bytes).sum();
    let mut manifest = Manifest::new(runtime, after, now_ms());
    manifest.last_prune_at_ms = Some(now_ms());
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
        if let Err(e) = apply(runtime, victim, base_env, runner, hooks, &mut out) {
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
