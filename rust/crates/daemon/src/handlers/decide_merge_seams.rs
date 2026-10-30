//! The test seams of the decide-merge route, and the one place that builds the route's `GitRepo`. Every seam is off
//! unless a test installs it with `install_seams`; the seams of one daemon are never seen by another (they live in
//! `CommandState`, not in a static). Off, they cost one lock check per merge.

use crate::deps::Deps;
use capstan_kernel::integrate::{GitRepo, GitSeams, MergeSeams};
use std::path::Path;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

/// A receiver a merge waits on: a send or a dropped sender releases it.
pub type Hold = Mutex<Receiver<()>>;

/// Switches for tests of the merge, all off by default; nothing but a test installs them.
#[doc(hidden)]
#[derive(Default)]
pub struct DecideMergeSeams {
    /// Applied to the handler's `GitRepo`.
    pub git_seams: Option<GitSeams>,
    /// Applied to the handler's `GitRepo`.
    pub merge_seams: Option<MergeSeams>,
    /// Runs after phase A, before the merge lock is taken.
    pub after_a: Option<Box<dyn Fn() + Send + Sync>>,
    /// Phase B waits here after its re-check, before it writes the marker.
    pub hold_b: Option<Hold>,
    /// Phase B waits here after the marker is written.
    pub hold_after_marker: Option<Hold>,
    /// With `MergeSeams::lock_after_read_tree_killed` the call returns leaving the marker and `index.lock` (the drop
    /// guard is forgotten).
    pub crash_in_b: bool,
    /// The call returns after phase B, before phase C.
    pub crash_after_b: bool,
    /// The `record_merge` mutation fails.
    pub fail_record: bool,
    /// The settle fails after the row is written.
    pub fail_settle: bool,
    pub panic_in_b: bool,
    /// The hand-off thread waits here before its cleanup and its PM message.
    pub block_pm_message: Option<Hold>,
    /// Replaces the 1000-commit lookup of the merge commit.
    pub lookup_depth: Option<usize>,
}

/// Waits on `hold` when it is set, until a send or until the sender is dropped.
pub fn wait_hold(hold: &Option<Hold>) {
    if let Some(receiver) = hold {
        let receiver = receiver.lock().unwrap_or_else(|p| p.into_inner());
        let _ = receiver.recv();
    }
}

/// Installs `seams` on this daemon; a second call replaces the first.
#[doc(hidden)]
pub fn install_seams(deps: &Deps, seams: DecideMergeSeams) {
    *deps
        .commands
        .merge_seams
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = Some(Arc::new(seams));
}

/// The seams installed on this daemon, if any.
#[doc(hidden)]
pub fn seams(deps: &Deps) -> Option<Arc<DecideMergeSeams>> {
    deps.commands
        .merge_seams
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
}

/// The `GitRepo` of the route: plain, or with the installed seams. The only call site of the two seam setters outside
/// the tests directories. The decide-merge handler builds its repository only through it.
#[allow(dead_code)]
pub(crate) fn seamed_repo(deps: &Deps, root: &Path) -> GitRepo {
    let mut repo = GitRepo::new(root);
    if let Some(installed) = seams(deps) {
        if let Some(git) = &installed.git_seams {
            repo = repo.with_seams(git.clone());
        }
        if let Some(merge) = &installed.merge_seams {
            repo = repo.with_merge_seams(merge.clone());
        }
    }
    repo
}
