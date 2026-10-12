use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::error::{CacheError, Result};
use crate::fsutil::{nanos, now_ms, pid_alive, remove_tree};
use crate::idle::is_idle;
use crate::manifest::{read_manifest, write_locked, Manifest, StoreLock};
use crate::measure::{measure, setup_keys, EntrySize, SetupKey};
use crate::runtime::{ResolvedEntry, Runtime, SETUP_ENTRY};

const COMMAND_TIMEOUT_MS: u64 = 60_000;
const MEASURE_BUDGET: Duration = Duration::from_secs(60);
const DAY_MS: u64 = 86_400_000;

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
}

#[derive(Clone, Debug, Default)]
pub struct ClearReport {
    pub cleared: Vec<String>,
    pub stop_commands: Vec<CommandRun>,
    /// Victims that could not be removed (`name: reason`); the others were still processed.
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Victim {
    Key(String),
    Entry(String),
}

impl Victim {
    fn label(&self) -> String {
        match self {
            Self::Key(key) => format!("{SETUP_ENTRY}/{key}"),
            Self::Entry(name) => name.clone(),
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

/// Renames `source` into `<root>/.trash` and deletes it there. The caller holds the store lock.
fn trash(root: &Path, source: &Path, label: &str, after_rename: &dyn Fn()) -> Result<bool> {
    let trash_dir = root.join(".trash");
    crate::fsutil::mkdir_private(&trash_dir)?;
    let target = trash_dir.join(format!("{}-{}", label.replace('/', "_"), nanos()));
    match fs::rename(source, &target) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => {
            return Err(CacheError::io(
                format!("move {} to trash", source.display()),
                e,
            ))
        }
    }
    after_rename();
    remove_tree(&target).map_err(|e| CacheError::io(format!("delete {}", target.display()), e))?;
    Ok(true)
}

fn apply(
    runtime: &Runtime,
    victim: &Victim,
    base_env: &BTreeMap<String, String>,
    runner: Runner<'_>,
    after_rename: &dyn Fn(),
    stops: &mut Vec<CommandRun>,
) -> Result<bool> {
    match victim {
        Victim::Key(key) => trash(
            &runtime.root,
            &runtime.entry_dir(SETUP_ENTRY).join(key),
            &victim.label(),
            after_rename,
        ),
        Victim::Entry(name) => {
            if let Some(entry) = runtime.entry(name) {
                stops.extend(run_stop(runtime, entry, base_env, runner));
            }
            trash(&runtime.root, &runtime.entry_dir(name), name, after_rename)
        }
    }
}

fn older_first<T>(items: &mut [(T, u64, u64)]) {
    // Least recently used first; ties: larger first.
    items.sort_by(|a, b| a.1.cmp(&b.1).then(b.2.cmp(&a.2)));
}

struct Chosen {
    victims: Vec<Victim>,
    total: u64,
}

impl Chosen {
    fn take(&mut self, victim: Victim, bytes: u64) {
        self.total = self.total.saturating_sub(bytes);
        self.victims.push(victim);
    }
}

/// Chooses what to remove, in the documented order, without touching anything.
fn plan(
    runtime: &Runtime,
    sizes: &[EntrySize],
    keys: &[SetupKey],
    policy: &Policy,
    now: u64,
) -> Vec<Victim> {
    let expired =
        |last_used: u64| last_used > 0 && now.saturating_sub(last_used) > policy.max_age_ms;
    let mut chosen = Chosen {
        victims: Vec::new(),
        total: sizes.iter().map(|s| s.bytes).sum(),
    };
    for key in keys.iter().filter(|k| expired(k.last_used_ms)) {
        chosen.take(Victim::Key(key.key.clone()), key.bytes);
    }
    for size in sizes
        .iter()
        .filter(|s| s.name != SETUP_ENTRY && expired(s.last_used_ms))
    {
        chosen.take(Victim::Entry(size.name.clone()), size.bytes);
    }
    // An entry with its own bound keeps it, except one whose tool trims itself (sccache).
    for size in sizes.iter().filter(|s| s.name != SETUP_ENTRY) {
        let bound = runtime.entry(&size.name).and_then(|e| {
            (e.mode.as_deref() != Some("sccache"))
                .then_some(e.max_bytes)
                .flatten()
        });
        let already = chosen.victims.contains(&Victim::Entry(size.name.clone()));
        if !already && bound.is_some_and(|b| size.bytes > b) {
            chosen.take(Victim::Entry(size.name.clone()), size.bytes);
        }
    }
    if chosen.total > policy.max_bytes {
        let mut rest: Vec<(String, u64, u64)> = keys
            .iter()
            .filter(|k| !chosen.victims.contains(&Victim::Key(k.key.clone())))
            .map(|k| (k.key.clone(), k.last_used_ms, k.bytes))
            .collect();
        older_first(&mut rest);
        for (key, _, bytes) in rest {
            if chosen.total <= policy.max_bytes {
                break;
            }
            chosen.take(Victim::Key(key), bytes);
        }
    }
    if chosen.total > policy.max_bytes {
        let mut rest: Vec<(String, u64, u64)> = sizes
            .iter()
            .filter(|s| {
                s.name != SETUP_ENTRY && !chosen.victims.contains(&Victim::Entry(s.name.clone()))
            })
            .map(|s| (s.name.clone(), s.last_used_ms, s.bytes))
            .collect();
        older_first(&mut rest);
        for (name, _, bytes) in rest {
            if chosen.total <= policy.max_bytes {
                break;
            }
            chosen.take(Victim::Entry(name), bytes);
        }
    }
    chosen.victims
}

/// Runs every enabled entry's tool prune commands, then (only when idle) removes expired and least-recently-used
/// data until the store is under the cap. When not idle it reports what it would remove. `Busy` when another prune
/// or clear holds the store lock.
pub fn prune(
    runtime: &Runtime,
    policy: Policy,
    base_env: &BTreeMap<String, String>,
    runner: Runner<'_>,
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
    let victims = plan(runtime, &sizes, &keys, &policy, now_ms());
    report.would_clear = victims.iter().map(Victim::label).collect();
    let mut stops = Vec::new();
    if policy.idle {
        for victim in &victims {
            match apply(runtime, victim, base_env, runner, &|| {}, &mut stops) {
                Ok(_) => report.cleared.push(victim.label()),
                Err(e) => report.errors.push(format!("{}: {e}", victim.label())),
            }
        }
        report.would_clear.clear();
    }
    report.commands.extend(stops);
    let after = measure(runtime, Instant::now() + MEASURE_BUDGET);
    report.total_after = after.iter().map(|s| s.bytes).sum();
    let mut manifest = Manifest::new(runtime, after, now_ms());
    manifest.last_prune_at_ms = Some(now_ms());
    write_locked(&runtime.root, &manifest, &|| {})?;
    drop(lock);
    Ok(report)
}

/// Clears the named entries (`None` means all, `setup/<key>` names one snapshot). Refuses with `NotIdle` while a
/// linked worktree exists unless `force`. Stop commands run before each rename.
pub fn clear(
    runtime: &Runtime,
    selection: Option<&[String]>,
    force: bool,
    base_env: &BTreeMap<String, String>,
    runner: Runner<'_>,
) -> Result<ClearReport> {
    clear_hooked(runtime, selection, force, base_env, runner, &|| {})
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
    for victim in &victims {
        match apply(
            runtime,
            victim,
            base_env,
            runner,
            after_rename,
            &mut report.stop_commands,
        ) {
            Ok(true) => report.cleared.push(victim.label()),
            Ok(false) => {}
            Err(e) => report.errors.push(format!("{}: {e}", victim.label())),
        }
    }
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
