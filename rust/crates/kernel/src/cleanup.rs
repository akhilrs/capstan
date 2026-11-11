//! The guarded branch cleanup: which local branches are done with, offering them, and deleting them without losing work.
//!
//! A branch is `landed` when its tip is the commit of an accepted report that a confirmed integration holds (as a member
//! or by coverage), `landed_by_content` when no report says so but every path it changed since the merge base has the
//! same content in HEAD, and `unlanded` otherwise. Every deletion is re-checked at the moment it happens: the tip must
//! still be the recorded commit, the class must still hold, and no guard may apply. The deletion is a compare-and-delete
//! at the recorded commit and is ledgered before the next branch is touched, so `git branch <name> <sha>` restores it.

use crate::areas::cleanups::{self, PaneRef};
use crate::areas::{integrations, pauses, workspace};
use crate::errors::KernelError;
use crate::integrate::{GitRepo, IntegrateError, IntegrationState, Worktree};
use crate::kernel::Kernel;
use crate::types::MutationContext;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// What the project's cleanup configuration says (a mirror of `capstan_config::Cleanup`, so this module does not wait for it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CleanupSettings {
    pub branches: CleanupBranches,
    pub worktrees: CleanupWorktrees,
    pub include_unlanded: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanupBranches {
    Auto,
    Ask,
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanupWorktrees {
    Release,
    Keep,
}

/// What the cleanup works with; built by the daemon on the kernel thread like `IntegrationDeps`.
pub struct CleanupDeps<'a> {
    pub kernel: &'a Kernel,
    /// The project root.
    pub git: &'a GitRepo,
    pub integrations: &'a IntegrationState,
    pub credential: &'a str,
    pub context: &'a dyn Fn(&str) -> MutationContext,
    pub log: &'a dyn Fn(&str, Value),
    pub settings: CleanupSettings,
    /// How long one offer or apply may take; entries left when it is spent are skipped with reason `budget`.
    pub budget: Duration,
    pub now: &'a dyn Fn() -> Instant,
    /// Called with the path of a worktree about to be deleted from disk, before the final re-check of that path. A no-op
    /// in production; a test uses it to swap the path and see that the re-check holds.
    pub before_delete: &'a dyn Fn(&Path),
}

/// The repo of a workspace cleanup and where its leftover worktrees live; `None` scope means the single-repo cleanup.
#[derive(Clone, Copy, Debug)]
pub struct RepoScope<'a> {
    pub repo: &'a str,
    /// The folder of the repo (also the place named in the by-hand command); `deps.git` points at it unless `gone`.
    pub repo_root: &'a Path,
    /// A worktree is removed from disk only when it lies under this folder.
    pub worktree_dir: &'a Path,
    /// The repo left the configuration: no git is run for it, and only its recorded worktrees are offered.
    pub gone: bool,
}

impl CleanupDeps<'_> {
    fn ctx(&self) -> MutationContext {
        (self.context)(self.credential)
    }
}

// ------------------------------------------------------------------------------------------------------ errors

/// What the cleanup can fail with.
#[derive(Debug)]
pub enum CleanupError {
    /// `cleanup_refused: <code>: ...` with the code `paused`, `integration_running`, `unknown_cleanup`, `not_open` or `not_auto`.
    Refused {
        code: &'static str,
        message: String,
    },
    Git(IntegrateError),
    Kernel(KernelError),
}

pub type CleanupResult<T> = Result<T, CleanupError>;

impl CleanupError {
    pub fn message(&self) -> String {
        match self {
            Self::Refused { message, .. } => message.clone(),
            Self::Git(e) => e.message(),
            Self::Kernel(e) => e.message(),
        }
    }

    fn refused(code: &'static str, detail: impl std::fmt::Display) -> Self {
        Self::Refused {
            code,
            message: format!("cleanup_refused: {code}: {detail}"),
        }
    }
}

impl From<KernelError> for CleanupError {
    fn from(error: KernelError) -> Self {
        Self::Kernel(error)
    }
}

impl From<IntegrateError> for CleanupError {
    fn from(error: IntegrateError) -> Self {
        Self::Git(error)
    }
}

// ------------------------------------------------------------------------------------------------- classification

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    Unlanded,
    LandedByContent,
    Landed,
}

impl Class {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Landed => "landed",
            Self::LandedByContent => "landed_by_content",
            Self::Unlanded => "unlanded",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "landed" => Some(Self::Landed),
            "landed_by_content" => Some(Self::LandedByContent),
            "unlanded" => Some(Self::Unlanded),
            _ => None,
        }
    }
}

/// A branch that may be cleaned up.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub branch: String,
    pub sha: String,
    pub class: Class,
    pub report_id: Option<String>,
    pub integration_id: Option<String>,
    /// The clean worktree of an ended agent that has the branch checked out; removed before the branch.
    pub worktree_path: Option<String>,
    pub reason: String,
}

/// A branch that is not for cleanup, and why: `main_or_head`, `checked_out`, `active_agent`,
/// `integration_branch_unsettled`, `dirty_worktree` or `kept_worktree`.
#[derive(Clone, Debug, PartialEq)]
pub struct Protected {
    pub branch: String,
    pub sha: String,
    pub reason: &'static str,
}

/// `landed` holds the candidates of class `landed` and `landed_by_content`, `unlanded` the rest.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Classification {
    pub landed: Vec<Candidate>,
    pub unlanded: Vec<Candidate>,
    pub protected: Vec<Protected>,
}

enum Verdict {
    Candidate(Candidate),
    Protected(&'static str),
}

/// What the repository and the ledger say right now.
struct Facts {
    branches: BTreeMap<String, String>,
    names: BTreeSet<String>,
    worktrees: Vec<Worktree>,
    panes: Vec<PaneRef>,
    unsettled: BTreeSet<String>,
}

/// The recorded panes plus the live worktrees of workspace workers, as panes (both say where an agent's branch lives).
fn pane_and_worktree_refs(kernel: &Kernel) -> CleanupResult<Vec<PaneRef>> {
    let mut panes = cleanups::pane_refs(kernel)?;
    panes.extend(
        cleanups::worktree_refs(kernel)?
            .into_iter()
            .map(|w| PaneRef {
                agent_id: w.agent_id,
                active: w.active,
                branch: Some(w.branch),
                worktree_path: Some(w.worktree_path),
            }),
    );
    Ok(panes)
}

fn same_path(a: &Path, b: &Path) -> bool {
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canonical(a) == canonical(b)
}

impl Facts {
    fn gather(deps: &CleanupDeps) -> CleanupResult<Self> {
        let branches = deps.git.local_branches()?;
        let mut names: BTreeSet<String> = ["main", "master"].map(String::from).into();
        names.extend(deps.git.root_branch()?);
        names.extend(deps.git.origin_head_branch()?);
        Ok(Self {
            branches: branches.into_iter().collect(),
            names,
            worktrees: deps.git.worktrees()?,
            panes: pane_and_worktree_refs(deps.kernel)?,
            unsettled: cleanups::unsettled_integration_branches(deps.kernel)?,
        })
    }

    fn pane_at(&self, path: &Path) -> Option<&PaneRef> {
        self.panes.iter().find(|p| {
            p.worktree_path
                .as_deref()
                .is_some_and(|w| same_path(Path::new(w), path))
        })
    }

    /// What may be done with `branch`, which points at `tip`. `auto` is the view of an automatic run.
    fn judge(
        &self,
        deps: &CleanupDeps,
        branch: &str,
        tip: &str,
        deadline: Instant,
        auto: bool,
    ) -> CleanupResult<Verdict> {
        if self.names.contains(branch) {
            return Ok(Verdict::Protected("main_or_head"));
        }
        let worktree = self
            .worktrees
            .iter()
            .find(|w| w.branch.as_deref() == Some(branch));
        let held_by_active = self.panes.iter().any(|p| {
            p.active
                && (p.branch.as_deref() == Some(branch)
                    || worktree.is_some_and(|w| {
                        p.worktree_path
                            .as_deref()
                            .is_some_and(|path| same_path(Path::new(path), &w.path))
                    }))
        });
        if held_by_active {
            return Ok(Verdict::Protected("active_agent"));
        }
        if self.unsettled.contains(branch) {
            return Ok(Verdict::Protected("integration_branch_unsettled"));
        }
        let mut worktree_path = None;
        if let Some(worktree) = worktree {
            if worktree.is_main {
                return Ok(Verdict::Protected("main_or_head"));
            }
            if self.pane_at(&worktree.path).is_none() {
                return Ok(Verdict::Protected("checked_out"));
            }
            if deps.git.worktree_dirty(&worktree.path).unwrap_or(true) {
                return Ok(Verdict::Protected("dirty_worktree"));
            }
            if auto && deps.settings.worktrees == CleanupWorktrees::Keep {
                return Ok(Verdict::Protected("kept_worktree"));
            }
            worktree_path = Some(worktree.path.to_string_lossy().into_owned());
        }
        let (class, report_id, integration_id, reason) = if let Some((report_id, integration_id)) =
            cleanups::confirmed_link_for_commit(deps.kernel, tip)?
        {
            let reason =
                format!("report {report_id} is in the confirmed integration {integration_id}");
            (Class::Landed, Some(report_id), Some(integration_id), reason)
        } else if (deps.now)() < deadline && deps.git.content_in_head(tip).unwrap_or(false) {
            (
                Class::LandedByContent,
                None,
                None,
                "no report links it, but HEAD holds every change it made".to_string(),
            )
        } else {
            (
                Class::Unlanded,
                None,
                None,
                "no confirmed integration holds it".to_string(),
            )
        };
        Ok(Verdict::Candidate(Candidate {
            branch: branch.to_string(),
            sha: tip.to_string(),
            class,
            report_id,
            integration_id,
            worktree_path,
            reason,
        }))
    }
}

fn classify_with(deps: &CleanupDeps, auto: bool) -> CleanupResult<Classification> {
    let deadline = (deps.now)() + deps.budget;
    let facts = Facts::gather(deps)?;
    let mut out = Classification::default();
    for (branch, tip) in &facts.branches {
        match facts.judge(deps, branch, tip, deadline, auto)? {
            Verdict::Protected(reason) => out.protected.push(Protected {
                branch: branch.clone(),
                sha: tip.clone(),
                reason,
            }),
            Verdict::Candidate(c) if c.class == Class::Unlanded => out.unlanded.push(c),
            Verdict::Candidate(c) => out.landed.push(c),
        }
    }
    Ok(out)
}

/// Sorts every local branch into landed, unlanded and protected. Reads only; refs/remotes are never looked at.
pub fn classify(deps: &CleanupDeps) -> CleanupResult<Classification> {
    classify_with(deps, false)
}

// ------------------------------------------------------------------------------------------------------- results

#[derive(Clone, Debug, PartialEq)]
pub struct CleanupOffer {
    /// `None` when there was nothing to offer and nothing was recorded.
    pub cleanup_id: Option<String>,
    pub entries: Vec<Candidate>,
    pub protected: Vec<Protected>,
    /// The ACTION NEEDED text (also queued to the PM for a `confirm` trigger).
    pub notice: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Deleted {
    pub branch: String,
    pub sha: String,
    pub worktree_path: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Skipped {
    pub branch: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Applied {
    pub cleanup_id: Option<String>,
    pub deleted: Vec<Deleted>,
    pub skipped: Vec<Skipped>,
    /// `git branch <name> <sha>` for every deleted branch.
    pub restore: Vec<String>,
}

fn shell_word(text: &str) -> String {
    if !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._/-+@".contains(&b))
    {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

fn restore_command(branch: &str, sha: &str) -> String {
    format!("git branch {} {sha}", shell_word(branch))
}

const NOTICE_LINES: usize = 25;

/// The ACTION NEEDED message of an offer: the branches with their commits, the cleanup id and the commands.
pub fn offer_notice_text(offer: &CleanupOffer) -> String {
    let id = offer.cleanup_id.as_deref().unwrap_or("-");
    let mut lines = vec![format!(
        "Cleanup {id}: {} branch(es) can be deleted. Nothing is deleted until you apply it.",
        offer.entries.len()
    )];
    for entry in offer.entries.iter().take(NOTICE_LINES) {
        lines.push(format!(
            "- {} at {} ({}){}",
            entry.branch,
            entry.sha,
            entry.class.as_str(),
            entry
                .worktree_path
                .as_ref()
                .map_or(String::new(), |p| format!(
                    ", worktree {p} is removed first"
                ))
        ));
    }
    if offer.entries.len() > NOTICE_LINES {
        lines.push(format!("- and {} more", offer.entries.len() - NOTICE_LINES));
    }
    lines.push(format!(
        "Apply (landed branches only): cstan cleanup apply {id}. Name branches to include the others: cstan cleanup apply {id} --branch <name>."
    ));
    lines.push(format!("Decline: cstan cleanup decline {id}."));
    lines.join("\n")
}

/// The notice of an automatic run: what it deleted, what it left, and how to bring a branch back.
pub fn auto_notice_text(applied: &Applied) -> String {
    let id = applied.cleanup_id.as_deref().unwrap_or("-");
    let mut lines = vec![format!(
        "Cleanup {id} deleted {} merged branch(es) and skipped {}.",
        applied.deleted.len(),
        applied.skipped.len()
    )];
    for deleted in applied.deleted.iter().take(NOTICE_LINES) {
        lines.push(format!("- deleted {} at {}", deleted.branch, deleted.sha));
    }
    for skipped in applied.skipped.iter().take(NOTICE_LINES) {
        lines.push(format!("- skipped {}: {}", skipped.branch, skipped.reason));
    }
    if !applied.restore.is_empty() {
        lines.push("To bring a branch back:".to_string());
        lines.extend(
            applied
                .restore
                .iter()
                .take(NOTICE_LINES)
                .map(|c| format!("  {c}")),
        );
    }
    lines.join("\n")
}

// ------------------------------------------------------------------------------------------------------ the steps

/// Refuses while the run is paused or an integration is running (or merging in this process).
fn assert_idle(deps: &CleanupDeps) -> CleanupResult<()> {
    match pauses::assert_run_not_paused(deps.kernel, "branch cleanup") {
        Ok(_) => {}
        Err(KernelError::RunPaused(message)) => {
            return Err(CleanupError::refused("paused", message))
        }
        Err(other) => return Err(other.into()),
    }
    let running = integrations::running_integrations(deps.kernel, deps.credential)?;
    if deps.integrations.busy()
        || running.as_array().is_some_and(|rows| !rows.is_empty())
        || !workspace::set_running(deps.kernel)?.is_empty()
    {
        return Err(CleanupError::refused(
            "integration_running",
            "an integration is running",
        ));
    }
    Ok(())
}

fn entry_json(c: &Candidate) -> Value {
    json!({
        "branch": c.branch, "sha": c.sha, "class": c.class.as_str(), "reportId": c.report_id,
        "integrationId": c.integration_id, "worktreePath": c.worktree_path,
    })
}

fn record(
    deps: &CleanupDeps,
    cleanup_id: &str,
    branch: &str,
    outcome: &str,
    reason: Option<&str>,
) -> CleanupResult<()> {
    cleanups::record_branch_outcome(
        deps.kernel,
        &deps.ctx(),
        &json!({"cleanupId": cleanup_id, "branch": branch, "outcome": outcome, "reason": reason}),
    )?;
    Ok(())
}

/// Offers the branches that are done with: records an `ask` cleanup, supersedes an older open one and (for a `confirm`
/// trigger) queues the ACTION NEEDED notice. Landed and landed-by-content branches are offered; unlanded ones only with
/// `include_unlanded`. Nothing is deleted.
pub fn offer(
    deps: &CleanupDeps,
    trigger: &str,
    integration_id: Option<&str>,
    requested_by: &str,
    include_unlanded: bool,
) -> CleanupResult<CleanupOffer> {
    offer_in(
        deps,
        trigger,
        integration_id,
        requested_by,
        include_unlanded,
        None,
    )
}

/// The worktrees of ended agents in `scope`'s repo that git does not list (the repo moved, or left the configuration),
/// as `unlanded` candidates at their recorded path. A worktree that is already gone from disk is not offered.
fn orphan_candidates(
    deps: &CleanupDeps,
    scope: &RepoScope,
    have: &[Candidate],
) -> CleanupResult<Vec<Candidate>> {
    let listed: Vec<Worktree> = if scope.gone {
        Vec::new()
    } else {
        deps.git.worktrees()?
    };
    let refs = cleanups::worktree_refs(deps.kernel)?;
    let mut out: Vec<Candidate> = Vec::new();
    for row in refs.iter().filter(|w| w.repo == scope.repo) {
        let path = Path::new(&row.worktree_path);
        let taken = have
            .iter()
            .chain(out.iter())
            .any(|c| c.branch == row.branch);
        let held = refs
            .iter()
            .any(|o| o.active && same_path(Path::new(&o.worktree_path), path));
        if taken
            || held
            || listed.iter().any(|w| same_path(&w.path, path))
            || std::fs::symlink_metadata(path).is_err()
        {
            continue;
        }
        out.push(Candidate {
            branch: row.branch.clone(),
            sha: row.base_sha.clone(),
            class: Class::Unlanded,
            report_id: None,
            integration_id: None,
            worktree_path: Some(row.worktree_path.clone()),
            reason: if scope.gone {
                "the repo left the configuration; only its worktree is removed, the branch stays".into()
            } else {
                "git does not list this worktree any more; only its folder is removed, the branch stays".into()
            },
        });
    }
    Ok(out)
}

/// The repos named by live worktrees of the ledger that are not in `configured` (they left the configuration).
pub fn removed_repos(deps: &CleanupDeps, configured: &[String]) -> CleanupResult<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for row in cleanups::worktree_refs(deps.kernel)? {
        if !row.repo.is_empty() && !configured.contains(&row.repo) && !out.contains(&row.repo) {
            out.push(row.repo);
        }
    }
    out.sort();
    Ok(out)
}

/// `offer` for one repo of a workspace (`scope`): the repo's own branches plus its leftover worktrees (always offered,
/// as `unlanded`, so only a named apply removes them). A repo that left the configuration offers worktrees only.
pub fn offer_in(
    deps: &CleanupDeps,
    trigger: &str,
    integration_id: Option<&str>,
    requested_by: &str,
    include_unlanded: bool,
    scope: Option<&RepoScope>,
) -> CleanupResult<CleanupOffer> {
    assert_idle(deps)?;
    let (mut entries, protected) = if scope.is_some_and(|s| s.gone) {
        (Vec::new(), Vec::new())
    } else {
        let classification = classify_with(deps, false)?;
        let mut entries = classification.landed;
        if include_unlanded {
            entries.extend(classification.unlanded);
        }
        (entries, classification.protected)
    };
    if let Some(scope) = scope {
        let extra = orphan_candidates(deps, scope, &entries)?;
        entries.extend(extra);
    }
    entries.sort_by(|a, b| a.branch.cmp(&b.branch));
    let mut offer = CleanupOffer {
        cleanup_id: None,
        entries,
        protected,
        notice: None,
    };
    if offer.entries.is_empty() {
        return Ok(offer);
    }
    let cleanup_id = deps.kernel.env.uuid();
    offer.cleanup_id = Some(cleanup_id.clone());
    let notice = offer_notice_text(&offer);
    let repo = match scope {
        Some(scope) => Some(scope.repo.to_string()),
        None => match integration_id {
            Some(id) => workspace::repo_of_integration(deps.kernel, id)?,
            None => None,
        },
    };
    cleanups::record_offer(
        deps.kernel,
        &deps.ctx(),
        &json!({
            "cleanupId": cleanup_id,
            "trigger": trigger,
            "integrationId": integration_id,
            "requestedBy": requested_by,
            "repo": repo,
            "mode": "ask",
            "entries": offer.entries.iter().map(entry_json).collect::<Vec<_>>(),
            "notice": if trigger == "confirm" { Some(notice.clone()) } else { None },
        }),
    )?;
    match scope {
        Some(scope) => workspace::record_cleanup_repo(
            deps.kernel,
            &deps.ctx(),
            &json!({"cleanupId": cleanup_id, "repo": scope.repo}),
        )
        .map(|_| ())?,
        None => record_member_repo(deps, &cleanup_id, integration_id)?,
    }
    (deps.log)(
        "cleanup_offered",
        json!({"cleanupId": cleanup_id, "branches": offer.entries.len()}),
    );
    offer.notice = Some(notice);
    Ok(offer)
}

/// A cleanup that follows the confirm of a set member is about that member's repo: the repo is recorded with it.
fn record_member_repo(
    deps: &CleanupDeps,
    cleanup_id: &str,
    integration_id: Option<&str>,
) -> CleanupResult<()> {
    let Some(integration_id) = integration_id else {
        return Ok(());
    };
    if let Some(repo) = workspace::repo_of_integration(deps.kernel, integration_id)? {
        workspace::record_cleanup_repo(
            deps.kernel,
            &deps.ctx(),
            &json!({"cleanupId": cleanup_id, "repo": repo}),
        )?;
    }
    Ok(())
}

/// Why `path` may not be deleted from disk, or `None` when it may: a real directory (not a link) whose canonical path
/// lies under the canonical `worktree_dir`, with a `.git` file (read as a file) that names a gitdir inside a
/// `.git/worktrees` folder whose own `gitdir` file points back at this path.
fn unsafe_to_delete(path: &Path, worktree_dir: &Path) -> Option<&'static str> {
    let is_dir = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_dir());
    if !is_dir {
        return Some("not_a_directory");
    }
    let (Ok(canonical), Ok(base)) = (
        std::fs::canonicalize(path),
        std::fs::canonicalize(worktree_dir),
    ) else {
        return Some("path_unreadable");
    };
    if canonical == base || !canonical.starts_with(&base) {
        return Some("outside_worktree_dir");
    }
    let dot_git = canonical.join(".git");
    if !std::fs::symlink_metadata(&dot_git).is_ok_and(|m| m.file_type().is_file()) {
        return Some("no_git_file");
    }
    let named = std::fs::read_to_string(&dot_git).ok().and_then(|text| {
        text.lines()
            .find_map(|l| l.strip_prefix("gitdir:").map(|g| g.trim().to_string()))
    });
    let Some(named) = named else {
        return Some("no_git_file");
    };
    let named = {
        let p = PathBuf::from(named);
        if p.is_absolute() {
            p
        } else {
            canonical.join(p)
        }
    };
    let Ok(admin) = std::fs::canonicalize(&named) else {
        return Some("gitdir_missing");
    };
    let in_worktrees = admin.parent().is_some_and(|w| {
        w.file_name().is_some_and(|n| n == "worktrees")
            && w.parent()
                .and_then(Path::file_name)
                .is_some_and(|n| n == ".git")
    });
    if !in_worktrees {
        return Some("gitdir_not_in_worktrees");
    }
    let back = std::fs::read_to_string(admin.join("gitdir"))
        .ok()
        .and_then(|text| std::fs::canonicalize(text.trim()).ok());
    if back.as_deref() != Some(dot_git.as_path()) {
        return Some("back_link_mismatch");
    }
    None
}

/// Removes the folder of a leftover worktree of an ended agent (no git: the repo may have left the configuration). The
/// path must equal a recorded worktree of this repo and branch; the safety re-check runs right before the delete. The
/// error is the skip reason.
fn remove_orphan(
    deps: &CleanupDeps,
    scope: &RepoScope,
    branch: &str,
    path: &str,
) -> Result<(), String> {
    let by_hand = || {
        format!(
            "remove it by hand: git -C {} worktree remove {}",
            shell_word(&scope.repo_root.to_string_lossy()),
            shell_word(path)
        )
    };
    let target = Path::new(path);
    let refs = cleanups::worktree_refs(deps.kernel).map_err(|e| e.message())?;
    let same = |w: &&cleanups::WorktreeRef| same_path(Path::new(&w.worktree_path), target);
    if refs.iter().filter(same).any(|w| w.active) {
        return Err("now_active".into());
    }
    if !refs
        .iter()
        .filter(same)
        .any(|w| w.repo == scope.repo && w.branch == branch)
    {
        return Err("not_recorded".into());
    }
    (deps.before_delete)(target);
    if let Some(reason) = unsafe_to_delete(target, scope.worktree_dir) {
        (deps.log)(
            "cleanup_worktree_kept",
            json!({"path": path, "reason": reason}),
        );
        return Err(by_hand());
    }
    std::fs::remove_dir_all(target).map_err(|_| "delete_failed".to_string())
}

/// Deletes the entries of an open cleanup that are still safe to delete. `selection` names the branches to delete;
/// without it the recorded `landed` entries go and no others. `auto` is the view of an automatic run.
fn run_entries(
    deps: &CleanupDeps,
    cleanup_id: &str,
    selection: Option<&[String]>,
    auto: bool,
    scope: Option<&RepoScope>,
) -> CleanupResult<Applied> {
    let started = (deps.now)();
    let deadline = started + deps.budget;
    let record_value = cleanups::cleanup(deps.kernel, cleanup_id)?;
    let mut applied = Applied {
        cleanup_id: Some(cleanup_id.to_string()),
        ..Applied::default()
    };
    let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);
    let mut entries: Vec<(String, String, Class, Option<String>)> = Vec::new();
    for entry in record_value["entries"].as_array().into_iter().flatten() {
        if entry["outcome"] != "offered" {
            continue;
        }
        let (Some(branch), Some(sha), Some(class)) = (
            text(entry, "branch"),
            text(entry, "sha"),
            text(entry, "class").and_then(|c| Class::parse(&c)),
        ) else {
            continue;
        };
        entries.push((branch, sha, class, text(entry, "worktreePath")));
    }
    if let Some(names) = selection {
        for name in names {
            if !entries.iter().any(|(b, ..)| b == name) {
                applied.skipped.push(Skipped {
                    branch: name.clone(),
                    reason: "not_offered".into(),
                });
            }
        }
    }
    for (branch, sha, recorded, recorded_path) in entries {
        let named = selection.is_some_and(|names| names.contains(&branch));
        let wanted = match selection {
            Some(_) => named,
            None => recorded == Class::Landed,
        };
        if !wanted {
            continue;
        }
        let skip = |reason: &str, applied: &mut Applied| -> CleanupResult<()> {
            record(deps, cleanup_id, &branch, "skipped", Some(reason))?;
            applied.skipped.push(Skipped {
                branch: branch.clone(),
                reason: reason.into(),
            });
            Ok(())
        };
        if (deps.now)() >= deadline {
            skip("budget", &mut applied)?;
            continue;
        }
        let gone = scope.is_some_and(|s| s.gone);
        let facts = if gone {
            None
        } else {
            Some(Facts::gather(deps)?)
        };
        if let (Some(scope), Some(path)) = (scope, recorded_path.as_deref()) {
            let listed = facts.as_ref().is_some_and(|f| {
                f.worktrees
                    .iter()
                    .any(|w| same_path(&w.path, Path::new(path)))
            });
            if !listed {
                match remove_orphan(deps, scope, &branch, path) {
                    Ok(()) => {
                        record(deps, cleanup_id, &branch, "deleted", None)?;
                        applied.deleted.push(Deleted {
                            branch: branch.clone(),
                            sha: sha.clone(),
                            worktree_path: Some(path.to_string()),
                        });
                    }
                    Err(reason) => skip(&reason, &mut applied)?,
                }
                continue;
            }
        }
        let Some(facts) = facts else {
            skip("not_a_worktree", &mut applied)?;
            continue;
        };
        if facts.branches.get(&branch) != Some(&sha) {
            skip("tip_moved", &mut applied)?;
            continue;
        }
        let candidate = match facts.judge(deps, &branch, &sha, deadline, auto)? {
            Verdict::Protected("active_agent") => {
                skip("now_active", &mut applied)?;
                continue;
            }
            Verdict::Protected(_) => {
                skip("now_protected", &mut applied)?;
                continue;
            }
            Verdict::Candidate(candidate) => candidate,
        };
        // The class the person agreed to must still hold: a named branch may be as safe as recorded, an unnamed one must be landed.
        let class_holds = if named {
            candidate.class >= recorded
        } else {
            candidate.class == Class::Landed
        };
        if !class_holds {
            skip("class_changed", &mut applied)?;
            continue;
        }
        if let Some(path) = &candidate.worktree_path {
            match deps.git.remove_worktree(&PathBuf::from(path)) {
                Ok(true) => {}
                Ok(false) | Err(_) => {
                    skip("worktree_not_removed", &mut applied)?;
                    continue;
                }
            }
        }
        match deps.git.delete_branch_at(&branch, &sha) {
            Ok(true) => {
                if let Err(error) = record(deps, cleanup_id, &branch, "deleted", None) {
                    (deps.log)(
                        "cleanup_record_failed",
                        json!({"cleanupId": cleanup_id, "branch": branch, "sha": sha, "restore": restore_command(&branch, &sha)}),
                    );
                    return Err(error);
                }
                applied.restore.push(restore_command(&branch, &sha));
                applied.deleted.push(Deleted {
                    branch: branch.clone(),
                    sha: sha.clone(),
                    worktree_path: candidate.worktree_path.clone(),
                });
            }
            Ok(false) | Err(_) => skip("delete_failed", &mut applied)?,
        }
    }
    Ok(applied)
}

fn open_cleanup(deps: &CleanupDeps, cleanup_id: &str) -> CleanupResult<Value> {
    let record = cleanups::cleanup(deps.kernel, cleanup_id)?;
    if record.is_null() {
        return Err(CleanupError::refused(
            "unknown_cleanup",
            format!("cleanup {cleanup_id} does not exist"),
        ));
    }
    let state = record["state"].as_str().unwrap_or_default();
    if state != "offered" {
        return Err(CleanupError::refused(
            "not_open",
            format!("cleanup {cleanup_id} is {state}"),
        ));
    }
    Ok(record)
}

/// Applies an open offer. Every entry is classified again first; see the module description. Without a `selection` the
/// offered `landed` branches are deleted; with one, exactly the named branches (any class) are considered.
pub fn apply(
    deps: &CleanupDeps,
    cleanup_id: &str,
    selection: Option<Vec<String>>,
) -> CleanupResult<Applied> {
    apply_in(deps, cleanup_id, selection, None)
}

/// `apply` for a cleanup of one repo of a workspace (`scope`): leftover worktrees are removed from disk after the
/// safety re-check, and nothing runs git in a repo that left the configuration.
pub fn apply_in(
    deps: &CleanupDeps,
    cleanup_id: &str,
    selection: Option<Vec<String>>,
    scope: Option<&RepoScope>,
) -> CleanupResult<Applied> {
    assert_idle(deps)?;
    open_cleanup(deps, cleanup_id)?;
    let applied = run_entries(deps, cleanup_id, selection.as_deref(), false, scope)?;
    cleanups::complete_cleanup(
        deps.kernel,
        &deps.ctx(),
        &json!({"cleanupId": cleanup_id, "state": "applied"}),
    )?;
    (deps.log)(
        "cleanup_applied",
        json!({"cleanupId": cleanup_id, "deleted": applied.deleted.len(), "skipped": applied.skipped.len()}),
    );
    Ok(applied)
}

/// The automatic cleanup after an integration is confirmed: records an `auto` cleanup and deletes the `landed`
/// branches only. With worktrees kept, a branch in a worktree is left alone.
pub fn apply_auto(deps: &CleanupDeps, integration_id: &str) -> CleanupResult<Applied> {
    if deps.settings.branches != CleanupBranches::Auto {
        return Err(CleanupError::refused(
            "not_auto",
            "the project does not clean branches automatically",
        ));
    }
    assert_idle(deps)?;
    let classification = classify_with(deps, true)?;
    let mut entries: Vec<Candidate> = classification
        .landed
        .into_iter()
        .filter(|c| c.class == Class::Landed)
        .collect();
    if entries.is_empty() {
        return Ok(Applied::default());
    }
    entries.sort_by(|a, b| a.branch.cmp(&b.branch));
    let cleanup_id = deps.kernel.env.uuid();
    cleanups::record_offer(
        deps.kernel,
        &deps.ctx(),
        &json!({
            "cleanupId": cleanup_id,
            "trigger": "confirm",
            "integrationId": integration_id,
            "requestedBy": "controller",
            "mode": "auto",
            "entries": entries.iter().map(entry_json).collect::<Vec<_>>(),
        }),
    )?;
    record_member_repo(deps, &cleanup_id, Some(integration_id))?;
    let applied = run_entries(deps, &cleanup_id, None, true, None)?;
    let notice = auto_notice_text(&applied);
    cleanups::complete_cleanup(
        deps.kernel,
        &deps.ctx(),
        &json!({"cleanupId": cleanup_id, "state": "applied", "notice": notice}),
    )?;
    (deps.log)(
        "cleanup_applied",
        json!({"cleanupId": cleanup_id, "deleted": applied.deleted.len(), "skipped": applied.skipped.len()}),
    );
    Ok(applied)
}

/// Declines an open offer; nothing is deleted.
pub fn decline(deps: &CleanupDeps, cleanup_id: &str) -> CleanupResult<()> {
    open_cleanup(deps, cleanup_id)?;
    cleanups::complete_cleanup(
        deps.kernel,
        &deps.ctx(),
        &json!({"cleanupId": cleanup_id, "state": "declined"}),
    )?;
    Ok(())
}
