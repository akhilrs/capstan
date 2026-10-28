//! Which project roots a stream follows.
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use crate::controllers::Proc;

/// `path` made absolute against `cwd` and canonical; a path that cannot be canonicalised (it is gone) is lexically
/// normalised instead.
pub(super) fn normalise(cwd: &Path, path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    std::fs::canonicalize(&absolute).unwrap_or_else(|_| lexical(&absolute))
}

fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The roots to look at: the roots of the controller processes, the roots asked for and the roots of the cursor,
/// canonical, without duplicates, sorted.
///
/// Test seam: in a debug build `CSTAN_EVENTS_NO_PROCESS_SCAN=1` leaves the process list out.
pub fn discover_roots(
    procs: &[Proc],
    cwd: &Path,
    explicit: &[PathBuf],
    cursor_roots: &[String],
) -> Vec<PathBuf> {
    let mut roots = BTreeSet::new();
    if scan_processes() {
        for proc in procs {
            if let Some(root) = &proc.project {
                roots.insert(normalise(cwd, root));
            }
        }
    }
    for root in explicit {
        roots.insert(normalise(cwd, root));
    }
    for root in cursor_roots {
        roots.insert(normalise(cwd, Path::new(root)));
    }
    roots.into_iter().collect()
}

#[cfg(debug_assertions)]
fn scan_processes() -> bool {
    std::env::var("CSTAN_EVENTS_NO_PROCESS_SCAN").as_deref() != Ok("1")
}

#[cfg(not(debug_assertions))]
fn scan_processes() -> bool {
    true
}
