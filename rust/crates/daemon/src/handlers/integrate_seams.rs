//! The test seams of the workspace integrate route (`cstan integrate <report-id>...` over several repos). Every seam
//! is off unless a test installs it with `install_integrate_seams`; the seams of one daemon are never seen by another
//! (they live in `CommandState`, not in a static). Separate from `DecideMergeSeams`, which belongs to the
//! decide-merge route's phases.

use crate::deps::Deps;
use capstan_kernel::integrate::{GitRepo, GitSeams, MemberStop};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// What a test makes the set loop do before a member.
#[doc(hidden)]
pub enum SetFault {
    Continue,
    /// The member stops with this reason.
    Error(String),
    /// The loop panics.
    Panic,
    /// The member stops as a shutdown cancel does.
    Cancel,
}

/// Switches for tests of the set loop, all off by default; nothing but a test installs them.
#[doc(hidden)]
#[derive(Default)]
pub struct IntegrateSeams {
    /// Called with the repo before each member's merge.
    #[allow(clippy::type_complexity)]
    pub fault: Option<Box<dyn Fn(&str) -> SetFault + Send + Sync>>,
    /// Applied to each member's `GitRepo`.
    pub git_seams: Option<GitSeams>,
}

impl IntegrateSeams {
    /// The stop (or panic) the installed fault asks for before the member of `repo`.
    pub(crate) fn before_member(&self, repo: &str) -> Result<(), MemberStop> {
        match self.fault.as_ref().map(|fault| fault(repo)) {
            None | Some(SetFault::Continue) => Ok(()),
            Some(SetFault::Error(reason)) => Err(MemberStop::Error(reason)),
            Some(SetFault::Cancel) => Err(MemberStop::Cancel),
            Some(SetFault::Panic) => panic!("injected panic in the set loop"),
        }
    }
}

/// Installs `seams` on this daemon; a second call replaces the first.
#[doc(hidden)]
pub fn install_integrate_seams(deps: &Deps, seams: IntegrateSeams) {
    *deps
        .commands
        .integrate_seams
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = Some(Arc::new(seams));
}

/// The seams installed on this daemon, if any.
pub(crate) fn integrate_seams(
    slot: &Mutex<Option<Arc<IntegrateSeams>>>,
) -> Option<Arc<IntegrateSeams>> {
    slot.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// The `GitRepo` of one member: plain, or with the installed git seams.
pub(crate) fn member_repo(seams: &Option<Arc<IntegrateSeams>>, root: &Path) -> GitRepo {
    let repo = GitRepo::new(root);
    match seams.as_ref().and_then(|s| s.git_seams.clone()) {
        Some(git) => repo.with_seams(git),
        None => repo,
    }
}
