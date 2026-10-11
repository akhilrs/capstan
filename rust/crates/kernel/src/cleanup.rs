//! The guarded branch cleanup: which local branches are done with, offering them, and deleting them without losing work.
//!
//! A branch is `landed` when its tip is the commit of an accepted report that a confirmed integration holds (as a member
//! or by coverage), `landed_by_content` when no report says so but every path it changed since the merge base has the
//! same content in HEAD, and `unlanded` otherwise. Every deletion is re-checked at the moment it happens: the tip must
//! still be the recorded commit, the class must still hold, and no guard may apply. The deletion is a compare-and-delete
//! at the recorded commit and is ledgered before the next branch is touched, so `git branch <name> <sha>` restores it.

use crate::areas::cleanups::{self, PaneRef};
use crate::areas::{integrations, pauses};
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
            panes: cleanups::pane_refs(deps.kernel)?,
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
    if deps.integrations.busy() || running.as_array().is_some_and(|rows| !rows.is_empty()) {
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
    assert_idle(deps)?;
    let classification = classify_with(deps, false)?;
    let mut entries = classification.landed;
    if include_unlanded {
        entries.extend(classification.unlanded);
    }
    entries.sort_by(|a, b| a.branch.cmp(&b.branch));
    let mut offer = CleanupOffer {
        cleanup_id: None,
        entries,
        protected: classification.protected,
        notice: None,
    };
    if offer.entries.is_empty() {
        return Ok(offer);
    }
    let cleanup_id = deps.kernel.env.uuid();
    offer.cleanup_id = Some(cleanup_id.clone());
    let notice = offer_notice_text(&offer);
    cleanups::record_offer(
        deps.kernel,
        &deps.ctx(),
        &json!({
            "cleanupId": cleanup_id,
            "trigger": trigger,
            "integrationId": integration_id,
            "requestedBy": requested_by,
            "mode": "ask",
            "entries": offer.entries.iter().map(entry_json).collect::<Vec<_>>(),
            "notice": if trigger == "confirm" { Some(notice.clone()) } else { None },
        }),
    )?;
    (deps.log)(
        "cleanup_offered",
        json!({"cleanupId": cleanup_id, "branches": offer.entries.len()}),
    );
    offer.notice = Some(notice);
    Ok(offer)
}

/// Deletes the entries of an open cleanup that are still safe to delete. `selection` names the branches to delete;
/// without it the recorded `landed` entries go and no others. `auto` is the view of an automatic run.
fn run_entries(
    deps: &CleanupDeps,
    cleanup_id: &str,
    selection: Option<&[String]>,
    auto: bool,
) -> CleanupResult<Applied> {
    let started = (deps.now)();
    let deadline = started + deps.budget;
    let record_value = cleanups::cleanup(deps.kernel, cleanup_id)?;
    let mut applied = Applied {
        cleanup_id: Some(cleanup_id.to_string()),
        ..Applied::default()
    };
    let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);
    let mut entries: Vec<(String, String, Class)> = Vec::new();
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
        entries.push((branch, sha, class));
    }
    if let Some(names) = selection {
        for name in names {
            if !entries.iter().any(|(b, _, _)| b == name) {
                applied.skipped.push(Skipped {
                    branch: name.clone(),
                    reason: "not_offered".into(),
                });
            }
        }
    }
    for (branch, sha, recorded) in entries {
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
        let facts = Facts::gather(deps)?;
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
    assert_idle(deps)?;
    open_cleanup(deps, cleanup_id)?;
    let applied = run_entries(deps, cleanup_id, selection.as_deref(), false)?;
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
    let applied = run_entries(deps, &cleanup_id, None, true)?;
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
