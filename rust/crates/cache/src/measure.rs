use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::fsutil::since_epoch_ms;
use crate::lease;
use crate::runtime::{Runtime, SETUP_ENTRY};

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
        let (bytes, files, partial) = if Instant::now() >= deadline {
            (0, 0, true)
        } else {
            walk_skipping(&dir, deadline, &skip)
        };
        let (bytes, files, partial) = (
            bytes + slot_bytes,
            files + slot_files,
            partial || slot_partial,
        );
        out.push(EntrySize {
            name: entry.name.clone(),
            bytes,
            files,
            last_used_ms: last_used(&dir),
            partial,
            slots,
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
