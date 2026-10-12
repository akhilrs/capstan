use std::fs;
use std::path::Path;
use std::time::Instant;

use crate::fsutil::since_epoch_ms;
use crate::runtime::{Runtime, SETUP_ENTRY};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntrySize {
    pub name: String,
    pub bytes: u64,
    pub files: u64,
    pub last_used_ms: u64,
    /// The deadline hit before the whole tree was counted.
    pub partial: bool,
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
                stack.push(child.path());
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
        let (bytes, files, partial) = if Instant::now() >= deadline {
            (0, 0, true)
        } else {
            walk(&dir, deadline)
        };
        out.push(EntrySize {
            name: entry.name.clone(),
            bytes,
            files,
            last_used_ms: last_used(&dir),
            partial,
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
