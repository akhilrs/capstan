use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::fsutil::since_epoch_ms;
use crate::lease;
use crate::runtime::{ResolvedEntry, Runtime, SETUP_ENTRY};

/// The directory inside a unit that is evicted on its own (cargo's incremental cache), under the unit's lock.
pub(crate) const INCREMENTAL_DIR: &str = "incremental";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntrySize {
    pub name: String,
    pub bytes: u64,
    pub files: u64,
    pub last_used_ms: u64,
    /// The deadline hit before the whole tree was counted.
    pub partial: bool,
    /// The gate slots inside the entry, of every project; their bytes are part of `bytes`. Not stored in the manifest.
    pub slots: Vec<SlotSize>,
    /// The eviction units (`ResolvedEntry::unit_lock`) with their incremental sub-units. Not stored in the manifest.
    pub units: Vec<UnitSize>,
    /// Bytes outside every slot and unit (sccache, docs, tool files): prune never evicts them alone.
    pub fixed_bytes: u64,
}

/// One eviction unit: a directory holding the entry's unit lock, or its `incremental/` sub-unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitSize {
    /// The directory, relative to the entry directory (`cargo-target/debug`).
    pub rel: String,
    /// For a sub-unit, the unit it lies in (and whose lock guards it).
    pub parent: Option<String>,
    /// A unit's bytes include its sub-unit's.
    pub bytes: u64,
    pub partial: bool,
    /// The newest modification time of anything inside, in ms; 0 when empty.
    pub newest_mtime_ms: u64,
}

/// What `measure_dir` found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DirSize {
    pub bytes: u64,
    pub files: u64,
    /// The deadline hit before the whole tree was counted.
    pub partial: bool,
    pub newest_mtime_ms: u64,
}

/// Bytes (apparent size), files and newest modification time of the tree under `path` (the directory itself counts for
/// the time), without following symlinks, skipping the direct children named in `exclude`.
pub fn measure_dir(path: &Path, exclude: &[&str], deadline: Instant) -> DirSize {
    let mut out = DirSize::default();
    if Instant::now() >= deadline {
        out.partial = true;
        return out;
    }
    let stamp = |meta: &fs::Metadata| meta.modified().map_or(0, since_epoch_ms);
    if let Ok(meta) = fs::symlink_metadata(path) {
        out.newest_mtime_ms = stamp(&meta);
    }
    let mut stack = vec![(path.to_path_buf(), true)];
    let mut visited = 0u32;
    while let Some((dir, top)) = stack.pop() {
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        for child in read.flatten() {
            visited += 1;
            if visited % 256 == 0 && Instant::now() >= deadline {
                out.partial = true;
                return out;
            }
            if top && exclude.iter().any(|n| child.file_name() == *n) {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(child.path()) else {
                continue;
            };
            out.newest_mtime_ms = out.newest_mtime_ms.max(stamp(&meta));
            if meta.is_dir() {
                stack.push((child.path(), false));
            } else {
                out.files += 1;
                out.bytes += meta.len();
            }
        }
    }
    out
}

fn has_lock(dir: &Path, lock: &str) -> bool {
    fs::symlink_metadata(dir.join(lock)).is_ok_and(|m| m.is_file())
}

fn subdirs_of_dir(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = read
        .flatten()
        .filter(|c| c.file_type().is_ok_and(|t| t.is_dir()))
        .map(|c| c.path())
        .collect();
    out.sort();
    out
}

/// The eviction units of an entry: directories one or two levels below each of its cache directories that hold the
/// entry's unit lock (a unit is not searched further). `(path relative to the entry directory, path)`; empty without
/// a `unit_lock`.
pub(crate) fn find_units(entry: &ResolvedEntry, entry_dir: &Path) -> Vec<(String, PathBuf)> {
    let Some(lock) = entry.unit_lock.as_deref() else {
        return Vec::new();
    };
    let mut found: Vec<(String, PathBuf)> = Vec::new();
    for root in &entry.dirs {
        for child in subdirs_of_dir(root) {
            let units = if has_lock(&child, lock) {
                vec![child]
            } else {
                subdirs_of_dir(&child)
                    .into_iter()
                    .filter(|d| has_lock(d, lock))
                    .collect()
            };
            for unit in units {
                if let Ok(rel) = unit.strip_prefix(entry_dir) {
                    let rel = rel.to_string_lossy().into_owned();
                    if !found.iter().any(|(r, _)| *r == rel) {
                        found.push((rel, unit));
                    }
                }
            }
        }
    }
    found.sort();
    found
}

/// One gate slot of an entry: all of its directories as one unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotSize {
    pub project: String,
    pub slot: usize,
    pub bytes: u64,
    pub partial: bool,
    /// The lease file's modification time (the directory's when there is no lease file); 0 when neither exists.
    pub last_used_ms: u64,
}

/// A slot found on disk: its existing directories and its lease file path.
#[derive(Clone, Debug)]
pub(crate) struct SlotRef {
    pub project: String,
    pub slot: usize,
    pub dirs: Vec<PathBuf>,
    pub lease_file: PathBuf,
}

/// Every slot of an entry, of all projects, from the lease files and the slot directories alike.
pub(crate) fn entry_slots(entry_dir: &Path, subdirs: &[String]) -> Vec<SlotRef> {
    let mut found: BTreeSet<(String, usize)> = lease::slot_leases(entry_dir)
        .into_iter()
        .map(|(project, slot, _)| (project, slot))
        .collect();
    for subdir in subdirs {
        let Ok(projects) = fs::read_dir(entry_dir.join(subdir)) else {
            continue;
        };
        for project in projects.flatten() {
            let name = project.file_name().to_string_lossy().into_owned();
            let Ok(slots) = fs::read_dir(project.path()) else {
                continue;
            };
            for child in slots.flatten() {
                let slot = child.file_name().to_string_lossy().into_owned();
                if let Some(n) = slot
                    .strip_prefix("slot-")
                    .and_then(|n| n.parse::<usize>().ok())
                {
                    found.insert((name.clone(), n));
                }
            }
        }
    }
    found
        .into_iter()
        .map(|(project, slot)| SlotRef {
            dirs: lease::slot_dirs(entry_dir, subdirs, &project, slot)
                .into_iter()
                .filter(|d| fs::symlink_metadata(d).is_ok())
                .collect(),
            lease_file: lease::lease_path(entry_dir, &project, slot),
            project,
            slot,
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupKey {
    pub key: String,
    pub bytes: u64,
    pub files: u64,
    pub last_used_ms: u64,
}

/// Bytes (apparent size) and file count of a tree, without following symlinks; the last flag is true when stopped early.
fn walk(path: &Path, deadline: Instant) -> (u64, u64, bool) {
    walk_skipping(path, deadline, &[])
}

/// `walk` without descending into the directories in `skip`.
fn walk_skipping(path: &Path, deadline: Instant, skip: &[PathBuf]) -> (u64, u64, bool) {
    let mut stack = vec![path.to_path_buf()];
    let (mut bytes, mut files, mut visited) = (0u64, 0u64, 0u32);
    while let Some(dir) = stack.pop() {
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        for child in read.flatten() {
            visited += 1;
            if visited % 256 == 0 && Instant::now() >= deadline {
                return (bytes, files, true);
            }
            let Ok(meta) = fs::symlink_metadata(child.path()) else {
                continue;
            };
            if meta.is_dir() {
                if !skip.contains(&child.path()) {
                    stack.push(child.path());
                }
            } else {
                files += 1;
                bytes += meta.len();
            }
        }
    }
    (bytes, files, false)
}

pub(crate) fn last_used(dir: &Path) -> u64 {
    let stamp = |p: &Path| fs::symlink_metadata(p).and_then(|m| m.modified()).ok();
    let time = stamp(&dir.join(".last-used")).or_else(|| stamp(dir));
    time.map_or(0, since_epoch_ms)
}

/// The size of every entry whose directory exists, within `deadline`.
pub fn measure(runtime: &Runtime, deadline: Instant) -> Vec<EntrySize> {
    let mut out = Vec::new();
    for entry in &runtime.entries {
        let dir = runtime.entry_dir(&entry.name);
        let Ok(meta) = fs::symlink_metadata(&dir) else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let subdirs: Vec<String> = entry.gate_dirs.iter().map(|(_, s)| s.clone()).collect();
        let mut slots = Vec::new();
        let mut units = Vec::new();
        let mut skip = Vec::new();
        let (mut slot_bytes, mut slot_files, mut slot_partial) = (0u64, 0u64, false);
        if !subdirs.is_empty() {
            for found in entry_slots(&dir, &subdirs) {
                let (mut bytes, mut partial) = (0u64, false);
                for slot_dir in &found.dirs {
                    let (b, f, cut) = if Instant::now() >= deadline {
                        (0, 0, true)
                    } else {
                        walk(slot_dir, deadline)
                    };
                    bytes += b;
                    slot_files += f;
                    partial |= cut;
                }
                skip.extend(found.dirs.iter().cloned());
                slot_bytes += bytes;
                slot_partial |= partial;
                let stamp = |p: &Path| fs::symlink_metadata(p).and_then(|m| m.modified()).ok();
                let used =
                    stamp(&found.lease_file).or_else(|| found.dirs.first().and_then(|d| stamp(d)));
                slots.push(SlotSize {
                    project: found.project,
                    slot: found.slot,
                    bytes,
                    partial,
                    last_used_ms: used.map_or(0, since_epoch_ms),
                });
            }
        }
        let (mut unit_bytes, mut unit_files, mut unit_partial) = (0u64, 0u64, false);
        for (rel, path) in find_units(entry, &dir) {
            let own = measure_dir(&path, &[INCREMENTAL_DIR], deadline);
            let sub = measure_dir(&path.join(INCREMENTAL_DIR), &[], deadline);
            let sub_exists =
                fs::symlink_metadata(path.join(INCREMENTAL_DIR)).is_ok_and(|m| m.is_dir());
            let partial = own.partial || (sub_exists && sub.partial);
            let total = own.bytes + sub.bytes;
            let newest = own
                .newest_mtime_ms
                .max(if sub_exists { sub.newest_mtime_ms } else { 0 });
            unit_bytes += total;
            unit_files += own.files + sub.files;
            unit_partial |= partial;
            skip.push(path);
            if sub_exists {
                units.push(UnitSize {
                    rel: format!("{rel}/{INCREMENTAL_DIR}"),
                    parent: Some(rel.clone()),
                    bytes: sub.bytes,
                    partial: sub.partial,
                    newest_mtime_ms: sub.newest_mtime_ms,
                });
            }
            units.push(UnitSize {
                rel,
                parent: None,
                bytes: total,
                partial,
                newest_mtime_ms: newest,
            });
        }
        let (fixed, fixed_files, fixed_partial) = if Instant::now() >= deadline {
            (0, 0, true)
        } else {
            walk_skipping(&dir, deadline, &skip)
        };
        let (bytes, files, partial) = (
            fixed + slot_bytes + unit_bytes,
            fixed_files + slot_files + unit_files,
            fixed_partial || slot_partial || unit_partial,
        );
        out.push(EntrySize {
            name: entry.name.clone(),
            bytes,
            files,
            last_used_ms: last_used(&dir),
            partial,
            slots,
            units,
            fixed_bytes: fixed,
        });
    }
    out
}

/// The snapshots inside the setup entry, one per key.
pub fn setup_keys(runtime: &Runtime, deadline: Instant) -> (Vec<SetupKey>, bool) {
    let dir = runtime.entry_dir(SETUP_ENTRY);
    let mut keys = Vec::new();
    let mut partial = false;
    let Ok(read) = fs::read_dir(&dir) else {
        return (keys, false);
    };
    for child in read.flatten() {
        let name = child.file_name().to_string_lossy().into_owned();
        let path = child.path();
        let is_dir = fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir());
        if !is_dir || name.contains(".tmp-") || name.starts_with('.') {
            continue;
        }
        let (bytes, files, cut) = walk(&path, deadline);
        partial |= cut;
        keys.push(SetupKey {
            key: name,
            bytes,
            files,
            last_used_ms: last_used(&path),
        });
    }
    keys.sort_by(|a, b| a.key.cmp(&b.key));
    (keys, partial)
}
