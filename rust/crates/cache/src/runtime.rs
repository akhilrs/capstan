use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use capstan_config::{
    CacheConfig, CacheEntry, CacheOrigin, CacheScope, SetupReuse, DETECT_MARKERS, DETECT_MAX_DEPTH,
    DETECT_SKIP_DIRS,
};

use crate::error::{CacheError, Result};
use crate::fsutil::{mkdir_private, remove_tree, touch};
use crate::lease::{self, EntryGuard, GateLease, GateLeases, LeaseError, Target};
use crate::root::{resolve_root, segment};
use crate::BASE_ENV_ALLOWLIST;

/// The entry that holds setup-reuse snapshots.
pub const SETUP_ENTRY: &str = "setup";
/// At most this many direct subdirectories of the project root are scanned for markers.
const DETECT_MAX_DIRS: usize = 64;
/// A wildcard marker lists a directory; this bounds how many names it reads.
const DETECT_MAX_NAMES: usize = 4096;
const GIB: u64 = 1 << 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Preset,
    Custom,
    Detected,
    Internal,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Preset => "preset",
            Self::Custom => "custom",
            Self::Detected => "detected",
            Self::Internal => "internal",
        }
    }
}

/// Which environment `ensure_scoped` builds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateScope {
    /// What an agent gets: the shared directories (today's `ensure`).
    Agent,
    /// A verify gate: `gate_dirs` override the shared directories with this project's slot.
    Gate { slot: usize },
    /// The shared directories minus every variable some `gate_dirs` names, so the tool builds in its own default.
    Shared,
    /// Nothing: no variables, nothing created.
    Bare,
}

#[derive(Clone, Debug)]
pub struct ResolvedEntry {
    pub name: String,
    pub origin: Origin,
    pub mode: Option<String>,
    pub enabled: bool,
    pub reason: Option<String>,
    /// Absolute cache directories and fixed values; empty for a disabled entry.
    pub env: Vec<(String, String)>,
    /// The directories behind the `env` pairs that name one.
    pub dirs: Vec<PathBuf>,
    /// `(variable, subdirectory)` a verify gate uses instead of the same variable in `env`; never enables an entry.
    pub gate_dirs: Vec<(String, String)>,
    pub prune: Vec<String>,
    pub stop: Vec<String>,
    pub max_bytes: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Runtime {
    pub root: PathBuf,
    pub entries: Vec<ResolvedEntry>,
    pub warnings: Vec<String>,
    pub project_root: PathBuf,
    pub project_id: String,
    pub scope: CacheScope,
    pub max_bytes: u64,
    pub max_age_days: u32,
    pub setup_reuse: Option<SetupReuse>,
}

fn sccache_size(bytes: u64) -> String {
    if bytes % GIB == 0 {
        format!("{}G", bytes / GIB)
    } else {
        format!("{}M", (bytes / (1024 * 1024)).max(1))
    }
}

fn on_path(tool: &str, path_env: &str) -> bool {
    path_env.split(':').filter(|d| !d.is_empty()).any(|dir| {
        fs::metadata(Path::new(dir).join(tool))
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

fn wildcard_match(pattern: &str, name: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == name,
        Some((head, tail)) => {
            name.len() >= head.len() + tail.len() && name.starts_with(head) && name.ends_with(tail)
        }
    }
}

fn dir_has_marker(dir: &Path, patterns: &[&str], listing: &mut Option<Vec<String>>) -> bool {
    patterns.iter().any(|pattern| {
        if !pattern.contains('*') {
            return fs::metadata(dir.join(pattern)).is_ok_and(|m| m.is_file());
        }
        let names = listing.get_or_insert_with(|| {
            fs::read_dir(dir).map_or_else(
                |_| Vec::new(),
                |rd| {
                    rd.filter_map(|e| e.ok())
                        .take(DETECT_MAX_NAMES)
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect()
                },
            )
        });
        names.iter().any(|name| wildcard_match(pattern, name))
    })
}

/// The presets whose marker files are in the project root or one directory below it.
fn detect_presets(project_root: &Path) -> BTreeSet<&'static str> {
    let mut dirs = vec![project_root.to_path_buf()];
    if DETECT_MAX_DEPTH >= 1 {
        let mut subs: Vec<String> = fs::read_dir(project_root)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| !n.starts_with('.') && !DETECT_SKIP_DIRS.contains(&n.as_str()))
                    .collect()
            })
            .unwrap_or_default();
        subs.sort();
        subs.truncate(DETECT_MAX_DIRS);
        dirs.extend(subs.into_iter().map(|n| project_root.join(n)));
    }
    let mut found = BTreeSet::new();
    for dir in &dirs {
        let mut listing = None;
        for (preset, patterns) in DETECT_MARKERS {
            if !found.contains(preset) && dir_has_marker(dir, patterns, &mut listing) {
                found.insert(preset);
            }
        }
    }
    found
}

fn env_names(entry: &CacheEntry) -> impl Iterator<Item = &str> {
    entry
        .dirs
        .iter()
        .chain(entry.set.iter())
        .map(|(n, _)| n.as_str())
}

impl Runtime {
    /// Resolves the configured and detected entries against the agent PATH. An entry whose required tool is absent is
    /// disabled with one warning; nothing is created on disk beyond the root.
    pub fn from_config(
        config: &CacheConfig,
        project_root: &Path,
        project_id: &str,
        env: &BTreeMap<String, String>,
        path_env: &str,
    ) -> Runtime {
        let root = resolve_root(config, project_id, env);
        let mut warnings = Vec::new();
        let mut entries = Vec::new();
        let mut taken: Vec<(String, String)> = Vec::new();
        for entry in &config.entries {
            for name in env_names(entry) {
                taken.push((name.to_string(), entry.name.clone()));
            }
            entries.push(resolve_entry(
                entry,
                &root,
                config.max_size_bytes,
                path_env,
                &mut warnings,
            ));
        }
        if config.detect && !config.detectable.is_empty() {
            let found = detect_presets(project_root);
            for entry in config
                .detectable
                .iter()
                .filter(|e| found.contains(e.name.as_str()))
            {
                let clash = env_names(entry).find_map(|name| {
                    taken
                        .iter()
                        .find(|(n, _)| n == name)
                        .map(|(n, other)| (n.clone(), other.clone()))
                });
                if let Some((name, other)) = clash {
                    warnings.push(format!(
                        "cache entry {} (detected) is skipped: it sets {name}, which cache entry {other} already sets",
                        entry.name
                    ));
                    continue;
                }
                for name in env_names(entry) {
                    taken.push((name.to_string(), entry.name.clone()));
                }
                let mut resolved = entry.clone();
                resolved.origin = CacheOrigin::Detected;
                entries.push(resolve_entry(
                    &resolved,
                    &root,
                    config.max_size_bytes,
                    path_env,
                    &mut warnings,
                ));
            }
        }
        if config.setup_reuse.is_some() {
            entries.push(ResolvedEntry {
                name: SETUP_ENTRY.into(),
                origin: Origin::Internal,
                mode: None,
                enabled: true,
                reason: None,
                env: Vec::new(),
                dirs: Vec::new(),
                gate_dirs: Vec::new(),
                prune: Vec::new(),
                stop: Vec::new(),
                max_bytes: None,
            });
        }
        Runtime {
            root,
            entries,
            warnings,
            project_root: project_root.to_path_buf(),
            project_id: project_id.to_string(),
            scope: config.scope,
            max_bytes: config.max_size_bytes,
            max_age_days: config.max_age_days,
            setup_reuse: config.setup_reuse.clone(),
        }
    }

    pub fn entry_dir(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    pub fn entry(&self, name: &str) -> Option<&ResolvedEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// Creates every enabled entry's directories and marks each used. Idempotent and safe to call concurrently.
    pub fn ensure(&self) -> Result<Vec<(String, String)>> {
        self.ensure_scoped(GateScope::Agent)
    }

    /// The entries that have gate directories, in name order.
    fn gate_entries(&self) -> Vec<&ResolvedEntry> {
        let mut list: Vec<&ResolvedEntry> = self
            .entries
            .iter()
            .filter(|e| e.enabled && !e.gate_dirs.is_empty())
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    fn project_segment(&self) -> String {
        segment(&self.project_id)
    }

    /// The environment for `scope`, with the directories it names created and every used entry marked used.
    pub fn ensure_scoped(&self, scope: GateScope) -> Result<Vec<(String, String)>> {
        if scope == GateScope::Bare {
            return Ok(Vec::new());
        }
        mkdir_private(&self.root)?;
        let project = self.project_segment();
        let mut env = Vec::new();
        for entry in self.entries.iter().filter(|e| e.enabled) {
            let dir = self.entry_dir(&entry.name);
            let named = |name: &str| entry.gate_dirs.iter().any(|(g, _)| g == name);
            // A concurrent clear may rename the entry away between the two steps; try again.
            let mut attempt = 0;
            loop {
                mkdir_private(&dir)?;
                for (index, extra) in entry.dirs.iter().enumerate() {
                    // The variables a gate or the shared scope replaces keep their directory untouched.
                    let overridden = entry.env.get(index).is_some_and(|(name, _)| named(name));
                    if !(overridden && scope != GateScope::Agent) {
                        mkdir_private(extra)?;
                    }
                }
                if let GateScope::Gate { slot } = scope {
                    let subdirs: Vec<String> =
                        entry.gate_dirs.iter().map(|(_, s)| s.clone()).collect();
                    for slot_dir in lease::slot_dirs(&dir, &subdirs, &project, slot) {
                        mkdir_private(&slot_dir)?;
                    }
                }
                match touch(&dir.join(".last-used")) {
                    Ok(()) => break,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound && attempt < 5 => {
                        attempt += 1
                    }
                    Err(e) => return Err(CacheError::io(format!("touch {}", dir.display()), e)),
                }
            }
            match scope {
                GateScope::Agent => env.extend(entry.env.iter().cloned()),
                GateScope::Shared => {
                    env.extend(entry.env.iter().filter(|(n, _)| !named(n)).cloned())
                }
                GateScope::Gate { slot } => {
                    let mut seen = Vec::new();
                    for (name, value) in &entry.env {
                        match entry.gate_dirs.iter().find(|(g, _)| g == name) {
                            Some((_, subdir)) => {
                                seen.push(name.as_str());
                                env.push((name.clone(), slot_path(&dir, subdir, &project, slot)));
                            }
                            None => env.push((name.clone(), value.clone())),
                        }
                    }
                    for (name, subdir) in &entry.gate_dirs {
                        if !seen.contains(&name.as_str()) {
                            env.push((name.clone(), slot_path(&dir, subdir, &project, slot)));
                        }
                    }
                }
                GateScope::Bare => {}
            }
        }
        Ok(env)
    }

    /// Leases this project's slot of every entry that has gate directories, in entry-name order, creating the slot
    /// directories. All or nothing: on an error the leases already taken are released. With no such entry the set
    /// is empty and nothing is created.
    pub fn lease_gate(
        &self,
        slot: usize,
        wait: Duration,
        abort: &AtomicBool,
    ) -> std::result::Result<GateLeases, LeaseError> {
        GateLeases::acquire(self.gate_targets(), slot, wait, abort)
    }

    fn gate_targets(&self) -> Vec<Target> {
        let project = self.project_segment();
        self.gate_entries()
            .into_iter()
            .map(|entry| Target {
                entry_dir: self.entry_dir(&entry.name),
                project: project.clone(),
                subdirs: entry.gate_dirs.iter().map(|(_, s)| s.clone()).collect(),
            })
            .collect()
    }

    /// This project's slots: `(entry, slot, existing slot directories)`, from the lease files and the slot
    /// directories alike. Other projects' slots are never listed.
    pub fn gate_slots(&self) -> Vec<(String, usize, Vec<PathBuf>)> {
        let project = self.project_segment();
        let mut out = Vec::new();
        for entry in self.gate_entries() {
            let dir = self.entry_dir(&entry.name);
            let subdirs: Vec<String> = entry.gate_dirs.iter().map(|(_, s)| s.clone()).collect();
            let mut slots: BTreeSet<usize> = lease::slot_leases(&dir)
                .into_iter()
                .filter(|(p, _, _)| *p == project)
                .map(|(_, n, _)| n)
                .collect();
            for subdir in &subdirs {
                let Ok(read) = fs::read_dir(dir.join(subdir).join(&project)) else {
                    continue;
                };
                for child in read.filter_map(|e| e.ok()) {
                    let name = child.file_name().to_string_lossy().into_owned();
                    if let Some(n) = name
                        .strip_prefix("slot-")
                        .and_then(|n| n.parse::<usize>().ok())
                    {
                        slots.insert(n);
                    }
                }
            }
            for n in slots {
                let dirs = lease::slot_dirs(&dir, &subdirs, &project, n)
                    .into_iter()
                    .filter(|d| d.exists())
                    .collect();
                out.push((entry.name.clone(), n, dirs));
            }
        }
        out
    }

    /// Removes this project's slot of `entry`: its directories and its lease file, only while holding the lease.
    /// `false` when the slot is busy (leased, or a prune holds the guard) or does not exist.
    pub fn remove_slot(&self, entry: &str, slot: usize) -> Result<bool> {
        let Some(resolved) = self.gate_entries().into_iter().find(|e| e.name == entry) else {
            return Ok(false);
        };
        let dir = self.entry_dir(&resolved.name);
        let project = self.project_segment();
        let subdirs: Vec<String> = resolved.gate_dirs.iter().map(|(_, s)| s.clone()).collect();
        let dirs = lease::slot_dirs(&dir, &subdirs, &project, slot);
        let lease_file = lease::lease_path(&dir, &project, slot);
        if !dirs.iter().any(|d| d.exists()) && !lease_file.exists() {
            return Ok(false);
        }
        let guard = match EntryGuard::try_shared(&dir) {
            Ok(guard) => guard,
            Err(LeaseError::Busy) => return Ok(false),
            Err(e) => return Err(CacheError::Io(e.to_string())),
        };
        // Creating arbitrates with a gate leasing the same slot under the same shared guard.
        let held = match GateLease::acquire(&dir, &project, slot) {
            Ok(lease) => lease,
            Err(LeaseError::Busy) => return Ok(false),
            Err(e) => return Err(CacheError::Io(e.to_string())),
        };
        for d in &dirs {
            remove_tree(d).map_err(|e| CacheError::io(format!("remove {}", d.display()), e))?;
        }
        match fs::remove_file(&lease_file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(CacheError::io(
                    format!("remove {}", lease_file.display()),
                    e,
                ))
            }
        }
        drop(held);
        drop(guard);
        Ok(true)
    }

    /// The environment prune and stop commands run with: the base allowlist plus the entry's variables.
    pub fn command_environment(
        &self,
        entry: &ResolvedEntry,
        base_env: &BTreeMap<String, String>,
    ) -> BTreeMap<String, String> {
        let mut map: BTreeMap<String, String> = BASE_ENV_ALLOWLIST
            .iter()
            .filter_map(|name| base_env.get(*name).map(|v| (name.to_string(), v.clone())))
            .collect();
        map.extend(entry.env.iter().cloned());
        map
    }
}

fn slot_path(entry_dir: &Path, subdir: &str, project: &str, slot: usize) -> String {
    entry_dir
        .join(subdir)
        .join(project)
        .join(format!("slot-{slot}"))
        .to_string_lossy()
        .into_owned()
}

fn resolve_entry(
    entry: &CacheEntry,
    root: &Path,
    total: u64,
    path_env: &str,
    warnings: &mut Vec<String>,
) -> ResolvedEntry {
    let origin = match entry.origin {
        CacheOrigin::Preset => Origin::Preset,
        CacheOrigin::Custom => Origin::Custom,
        CacheOrigin::Detected => Origin::Detected,
    };
    let base = root.join(&entry.name);
    let mut resolved = ResolvedEntry {
        name: entry.name.clone(),
        origin,
        mode: entry.mode.clone(),
        enabled: true,
        reason: None,
        env: Vec::new(),
        dirs: Vec::new(),
        gate_dirs: entry.gate_dirs.clone(),
        prune: entry.prune.clone(),
        stop: entry.stop.clone(),
        max_bytes: entry.max_size_bytes,
    };
    if entry.dirs.is_empty() && entry.set.is_empty() {
        resolved.enabled = false;
        resolved.reason = Some(match &entry.mode {
            Some(mode) => format!("mode {mode}"),
            None => "nothing to set".into(),
        });
        resolved.prune.clear();
        resolved.stop.clear();
        resolved.gate_dirs.clear();
        return resolved;
    }
    if let Some(tool) = entry.requires.as_deref().filter(|t| !on_path(t, path_env)) {
        let fix = match entry.mode.as_deref() {
            Some(_) => format!(
                "install {tool}, or choose another [cache.{}] mode",
                entry.name
            ),
            None if origin == Origin::Custom => {
                format!("install {tool}, or remove [cache.entries.{}]", entry.name)
            }
            None => format!(
                "install {tool}, or remove {} from [cache] presets",
                entry.name
            ),
        };
        let reason = format!("{tool} is not on the agent PATH");
        warnings.push(format!(
            "cache entry {} is off: {reason}; {fix}",
            entry.name
        ));
        resolved.enabled = false;
        resolved.reason = Some(reason);
        resolved.prune.clear();
        resolved.stop.clear();
        resolved.gate_dirs.clear();
        return resolved;
    }
    for (name, subdir) in &entry.dirs {
        let dir = base.join(subdir);
        resolved
            .env
            .push((name.clone(), dir.to_string_lossy().into_owned()));
        resolved.dirs.push(dir);
    }
    let sccache_half = entry.mode.as_deref() == Some("sccache") && entry.max_size_bytes.is_none();
    for (name, value) in &entry.set {
        let value = if sccache_half && name == "SCCACHE_CACHE_SIZE" {
            sccache_size(total / 2)
        } else {
            value.clone()
        };
        resolved.env.push((name.clone(), value));
    }
    resolved
}
