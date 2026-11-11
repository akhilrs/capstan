//! Starting a review and finishing it (src/reviews.ts): pick the reviewer role, spawn a fresh reviewer on a worktree at the
//! reported commit, record the review, and release the reviewer afterwards.
//!
//! The launcher and git calls run on the caller's thread and never inside a kernel closure; each ledger step is one
//! `deps.kernel` call, where Node awaits between them.

use crate::deps::Deps;
use capstan_config::RoleConfig;
use capstan_kernel::helpers::{js_trim, normalize_text};
use capstan_kernel::records::GateSubject;
use capstan_kernel::KernelError;
use capstan_launcher::api::{LauncherError, RepoBase, SpawnOptions};
use serde_json::{json, Value};
use std::time::Duration;

/// How long the reviewer's own `cstan review` call has to finish printing before its pane is closed.
pub const REVIEWER_RELEASE_DELAY_MS: u64 = 750;

/// `ReviewRequestError`: a refusal with a code the command layer shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewRequestError {
    pub code: String,
    pub message: String,
}

impl ReviewRequestError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ReviewRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ReviewRequestError {}

/// Why `request_review` failed.
#[derive(Debug)]
pub enum RequestError {
    Review(ReviewRequestError),
    Kernel(KernelError),
    Launcher(LauncherError),
}

impl From<ReviewRequestError> for RequestError {
    fn from(error: ReviewRequestError) -> Self {
        Self::Review(error)
    }
}
impl From<KernelError> for RequestError {
    fn from(error: KernelError) -> Self {
        Self::Kernel(error)
    }
}
impl From<LauncherError> for RequestError {
    fn from(error: LauncherError) -> Self {
        Self::Launcher(error)
    }
}

/// The role to review with: the one asked for, else `reviewer`, else the only Verifier role.
pub fn choose_reviewer_role(
    config: &RoleConfig,
    requested: Option<&str>,
) -> Result<String, ReviewRequestError> {
    let verifiers: Vec<&str> = config
        .roles
        .iter()
        .filter(|role| role.kind == "Verifier")
        .map(|role| role.name.as_str())
        .collect();
    if let Some(requested) = requested {
        if !verifiers.contains(&requested) {
            return Err(ReviewRequestError::new(
                "unknown_reviewer_role",
                format!("{requested} is not a Verifier role in the configuration"),
            ));
        }
        return Ok(requested.to_string());
    }
    if verifiers.contains(&"reviewer") {
        return Ok("reviewer".to_string());
    }
    if verifiers.len() == 1 {
        return Ok(verifiers[0].to_string());
    }
    Err(ReviewRequestError::new(
        "no_reviewer_role",
        if verifiers.is_empty() {
            "the configuration has no Verifier role to review with"
        } else {
            "several Verifier roles exist and none is named reviewer; name one"
        },
    ))
}

/// Normalizes what a reviewer wrote: CR, CRLF, NEL and the Unicode line and paragraph separators become LF, and control
/// and format characters other than LF become spaces. The text is not cut; the caller refuses one over the limit.
pub fn review_text(text: &str) -> String {
    js_trim(&normalize_text(text)).to_string()
}

fn field(value: &Value, key: &str) -> Value {
    value.get(key).cloned().unwrap_or(Value::Null)
}

/// What `request_review` settled: the review record, the reviewer's state and whether a warm reviewer was reused.
pub struct Requested {
    pub review: Value,
    pub reviewer_state: String,
    pub reused: bool,
}

/// Starts a review of the subject at its commit. With `[review] warm_reviewer` the plan's warm reviewer is moved to the
/// commit and reused; otherwise (or when it cannot take the subject) a fresh reviewer is spawned on a worktree at the
/// commit. A failure after a spawn releases the fresh reviewer; a reused one stays warm.
pub fn request_review(
    deps: &Deps,
    subject_id: &str,
    requested_role: Option<&str>,
    pm_credential: &str,
) -> Result<Requested, RequestError> {
    let config = deps.options.capstan.as_deref().ok_or_else(|| {
        RequestError::Review(ReviewRequestError::new(
            "no_reviewer_role",
            "the configuration has no Verifier role to review with",
        ))
    })?;
    let role = choose_reviewer_role(config, requested_role)?;
    let workspace = crate::handlers::verify::is_workspace(deps);
    if workspace {
        workspace_review_gate(deps, subject_id)?;
    }
    let check = {
        let (subject, role) = (subject_id.to_string(), role.clone());
        deps.kernel
            .run(move |core| core.check_review_request(&subject, &role))?
    };
    // The gate is a ledger read: a run that is queued or running is refused at once, never waited for.
    crate::handlers::verify::check_gate(
        deps,
        crate::handlers::verify::Gate::Review,
        &[subject_id.to_string()],
    )?;
    let commit_sha = check["commitSha"].as_str().unwrap_or("").to_string();
    // A workspace report names a commit in each repo: each must exist in its own repo, and the reviewer starts there.
    let repo_commits = if workspace {
        report_commit_rows(deps, subject_id)?
    } else {
        Vec::new()
    };
    if repo_commits.is_empty() {
        let exists = deps
            .git
            .commit_exists(&commit_sha)
            .map_err(|e| ReviewRequestError::new("commit_missing", e.message))?;
        if !exists {
            return Err(ReviewRequestError::new(
                "commit_missing",
                "the commit to review no longer exists in the repository",
            )
            .into());
        }
    }
    for (repo, commit, _) in &repo_commits {
        let exists = deps
            .repo_git(repo)
            .commit_exists(commit)
            .map_err(|e| ReviewRequestError::new("commit_missing", e.message))?;
        if !exists {
            return Err(ReviewRequestError::new(
                "commit_missing",
                format!("the commit to review no longer exists in repo {repo}"),
            )
            .into());
        }
    }
    let launcher = deps.launcher.as_ref().ok_or_else(|| {
        RequestError::Launcher(LauncherError::new(
            "not_configured",
            "reviews need capstan.toml and Herdr",
        ))
    })?;
    // The warm reviewer of the subject's plan, when the configuration keeps one.
    let mut warm_plan: Option<String> = None;
    let mut reuse: Option<String> = None;
    // A warm reviewer is moved to one commit of one repo; a workspace report needs a worktree per repo, so it gets a
    // fresh reviewer.
    if config.review.warm_reviewer && repo_commits.is_empty() {
        let plan = deps
            .kernel
            .run({
                let subject = subject_id.to_string();
                move |core| core.review_plan_of(&subject)
            })
            .unwrap_or(Value::Null);
        let is_report = !plan["packageId"].is_null();
        if let Some(plan_id) = plan["planId"].as_str() {
            if is_report || config.review.integration {
                warm_plan = Some(plan_id.to_string());
                let answer = deps.kernel.run({
                    let (subject, role) = (subject_id.to_string(), role.clone());
                    let max = config.review.max_reviews;
                    move |core| core.warm_reviewer_for(&subject, &role, max)
                })?;
                match (answer["reason"].as_str(), answer["agentId"].as_str()) {
                    (Some("ok"), Some(agent_id)) => reuse = Some(agent_id.to_string()),
                    (Some("inactive"), _) => {
                        // The plan's warm reviewer is gone: free its row so a fresh reviewer can take its place.
                        let stale = crate::warm::rows(deps)
                            .into_iter()
                            .find(|row| row["planId"].as_str() == Some(plan_id));
                        if let Some(agent_id) =
                            stale.as_ref().and_then(|row| row["agentId"].as_str())
                        {
                            crate::warm::release_row(deps, agent_id, "lost", false);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    // The brief carries what the commit changes, when git can say.
    let diff_scope = if !repo_commits.is_empty() {
        workspace_diff_scope(deps, subject_id, &repo_commits)
    } else {
        match check["baseSha"].as_str() {
            Some(base) => match deps.git.diff_stat(base, &commit_sha) {
                Ok(scope) => Some(scope),
                Err(error) => {
                    deps.detail_log(
                        "review_diff_stat_failed",
                        json!({"subjectId": subject_id, "error": error.message}),
                    );
                    None
                }
            },
            None => None,
        }
    };
    let mut reused_agent: Option<String> = None;
    // Held from the retarget until the review has begun, so the idle loop cannot release the reviewer in between.
    let mut _claim: Option<crate::warm::Claim> = None;
    if let Some(agent_id) = reuse {
        if let Some(held) = crate::warm::claim(deps, &agent_id) {
            // The reviewer may have been released between the check above and the claim.
            let still = deps.kernel.run({
                let (subject, role) = (subject_id.to_string(), role.clone());
                let max = config.review.max_reviews;
                move |core| core.warm_reviewer_for(&subject, &role, max)
            })?;
            if still["agentId"].as_str() == Some(agent_id.as_str()) {
                if let Some(row) = crate::warm::rows(deps)
                    .iter()
                    .find(|row| row["agentId"].as_str() == Some(agent_id.as_str()))
                {
                    crate::warm::wait_for_verdict_print(deps, row);
                }
                _claim = Some(held);
                match launcher.retarget_review(&agent_id, &commit_sha, subject_id) {
                    Ok(_) => reused_agent = Some(agent_id),
                    Err(error) => {
                        if error.code == capstan_launcher::api::MODEL_CHANGED {
                            deps.detail_log(
                                "review_reviewer_model_changed",
                                json!({"agentId": agent_id, "code": error.code, "message": error.message}),
                            );
                        } else {
                            crate::warm::retarget_failed(deps, &error, &agent_id);
                        }
                        crate::warm::release_row(deps, &agent_id, "retarget_failed", false);
                        if let Err(release_error) = launcher.release(&agent_id) {
                            deps.detail_log(
                                "review_reviewer_not_released",
                                json!({"agentId": agent_id, "error": format!("LauncherError: {}", release_error.message)}),
                            );
                        }
                    }
                }
            }
        }
    }
    let (reviewer_agent_id, reviewer_state) = match &reused_agent {
        Some(agent_id) => (agent_id.clone(), "reused".to_string()),
        None => {
            let spawned = crate::warm::spawn_with_eviction(
                deps,
                launcher,
                &role,
                &SpawnOptions {
                    base_sha: Some(commit_sha),
                    review_target: Some(subject_id.to_string()),
                    repo_bases: (!repo_commits.is_empty()).then(|| {
                        repo_commits
                            .iter()
                            .map(|(repo, commit, _)| RepoBase {
                                repo: repo.clone(),
                                commit: commit.clone(),
                            })
                            .collect()
                    }),
                    ..SpawnOptions::default()
                },
            )?;
            (spawned.agent_id, spawned.state)
        }
    };
    // The brief of a project with a gate carries the recorded gate result for the commands in force.
    let mut input = json!({"subjectId": subject_id, "reviewerRole": role, "reviewerAgentId": reviewer_agent_id});
    if let Some(verify) = &deps.verify {
        input["verifyCommandsSha"] = json!(verify.commands_sha());
    }
    if !repo_commits.is_empty() {
        let service = deps.verify.as_ref();
        let shas: serde_json::Map<String, Value> = repo_commits
            .iter()
            .filter_map(|(repo, _, _)| {
                service.map(|v| (repo.clone(), json!(v.commands_sha_for(repo))))
            })
            .collect();
        if !shas.is_empty() {
            input["verifyCommandsShas"] = Value::Object(shas);
        }
    }
    if let Some(plan_id) = &warm_plan {
        input["warmPlanId"] = json!(plan_id);
    }
    if let Some(scope) = diff_scope {
        input["diffScope"] = scope;
    }
    let begun = deps.kernel.run({
        let credential = pm_credential.to_string();
        move |core| {
            let context = crate::deps::new_context(core, &credential)?;
            core.begin_review(&context, &input)
        }
    });
    match begun {
        Ok(review) => Ok(Requested {
            review,
            reviewer_state,
            reused: reused_agent.is_some(),
        }),
        Err(error) => {
            // A reused reviewer stays warm (it is idle at the new commit); a fresh one is released.
            if reused_agent.is_none() {
                if let Err(release_error) = launcher.release(&reviewer_agent_id) {
                    deps.detail_log(
                        "review_reviewer_not_released",
                        json!({"agentId": reviewer_agent_id, "error": format!("LauncherError: {}", release_error.message)}),
                    );
                }
            }
            Err(error.into())
        }
    }
}

/// `(repo, commit, base)` of each repo a workspace report names a commit for; empty for any other subject.
fn report_commit_rows(
    deps: &Deps,
    subject_id: &str,
) -> Result<Vec<(String, String, String)>, RequestError> {
    let subject = subject_id.to_string();
    let rows = deps.kernel.run(move |core| core.report_commits(&subject))?;
    Ok(rows
        .as_array()
        .into_iter()
        .flatten()
        .map(|row| {
            (
                row["repo"].as_str().unwrap_or("").to_string(),
                row["commitSha"].as_str().unwrap_or("").to_string(),
                row["baseSha"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect())
}

/// The review gate of a workspace: a set or a member of one is refused with the set text. When `[verify]` is configured
/// the gate is forced whatever `require_review` says (as integrate is): a report is reviewed only when every repo it
/// names a commit for has passed verify (a report without per-repo commits is refused). The ledger check is coarse (a
/// passed run at the commit under any commands); `workspace_gate`, with each repo's current commands, is the authority.
/// Without `[verify]` a workspace review is not verify-gated, as in a single repo.
fn workspace_review_gate(deps: &Deps, subject_id: &str) -> Result<(), RequestError> {
    let subject = subject_id.to_string();
    let verify = deps.verify.is_some();
    let refused = deps.kernel.run(move |core| {
        if let Some(why) = crate::handlers::verify::set_refusal(core, &subject)? {
            return Ok(Some(why));
        }
        if !verify {
            return Ok(None);
        }
        Ok(core.review_gate(&GateSubject::Report(subject)).err())
    })?;
    let Some(service) = &deps.verify else {
        return match refused {
            None => Ok(()),
            Some(why) => Err(RequestError::Kernel(KernelError::MutationConflict(why))),
        };
    };
    match refused {
        None => {
            crate::handlers::verify::workspace_gate(deps, service, &[subject_id.to_string()])?;
            Ok(())
        }
        // The ledger refusal names the missing run; the gate adds what a queued or running run is waiting for.
        Some(why) if why.starts_with("verify_required:") => {
            crate::handlers::verify::workspace_gate(deps, service, &[subject_id.to_string()])?;
            Err(ReviewRequestError::new("verify_required", why).into())
        }
        Some(why) => match why.strip_prefix("review_refused: ") {
            Some(text) => Err(ReviewRequestError::new("review_refused", text).into()),
            None => Err(RequestError::Kernel(KernelError::MutationConflict(why))),
        },
    }
}

/// The diff scope of a workspace report: the changed paths of `base..commit` in each repo, written `<repo>:<path>`,
/// with the totals summed. `None` when no repo's diff could be read.
fn workspace_diff_scope(
    deps: &Deps,
    subject_id: &str,
    repo_commits: &[(String, String, String)],
) -> Option<Value> {
    let mut files = Vec::new();
    let (mut total, mut insertions, mut deletions) = (0_i64, 0_i64, 0_i64);
    let mut any = false;
    for (repo, commit, base) in repo_commits {
        match deps.repo_git(repo).diff_stat(base, commit) {
            Ok(scope) => {
                any = true;
                for file in scope["files"].as_array().into_iter().flatten() {
                    let mut file = file.clone();
                    file["path"] = json!(format!("{repo}:{}", file["path"].as_str().unwrap_or("")));
                    files.push(file);
                }
                total += scope["filesTotal"].as_i64().unwrap_or(0);
                insertions += scope["insertions"].as_i64().unwrap_or(0);
                deletions += scope["deletions"].as_i64().unwrap_or(0);
            }
            Err(error) => deps.detail_log(
                "review_diff_stat_failed",
                json!({"subjectId": subject_id, "repo": repo, "error": error.message}),
            ),
        }
    }
    any.then(|| {
        json!({"files": files, "filesTotal": total, "insertions": insertions, "deletions": deletions})
    })
}

/// Releases the reviewer of a finished review shortly after the reply, so the pane is not closed under the call that is
/// still printing. Runs on its own thread; the daemon does not wait for it.
pub fn release_reviewer_later(deps: &Deps, review: &Value) {
    let Some(launcher) = deps.launcher.clone() else {
        return;
    };
    let deps = deps.clone();
    let review_id = field(review, "reviewId");
    let reviewer = review["reviewerAgentId"].as_str().unwrap_or("").to_string();
    let _ = std::thread::Builder::new()
        .name("review-release".into())
        .spawn(move || {
            std::thread::sleep(Duration::from_millis(REVIEWER_RELEASE_DELAY_MS));
            match launcher.release(&reviewer) {
                Ok(_) => deps.detail_log("review_reviewer_released", json!({"reviewId": review_id})),
                Err(error) => deps.detail_log(
                    "review_reviewer_not_released",
                    json!({"reviewId": review_id, "error": format!("LauncherError: {}", error.message)}),
                ),
            }
        });
}

/// At daemon start: finished reviews whose reviewer is still active (a crash after the verdict) get their reviewer released, except a held
/// warm reviewer, which stays; a warm row whose agent is no longer active is marked lost.
pub fn recover_reviews(deps: &Deps, credential: &str) {
    let Some(launcher) = deps.launcher.as_ref() else {
        return;
    };
    crate::warm::recover_lost(deps);
    let rows = deps.kernel.run({
        let credential = credential.to_string();
        move |core| core.reviews_to_release(&credential)
    });
    let Ok(Value::Array(reviews)) = rows else {
        return;
    };
    for review in reviews {
        let reviewer = review["reviewerAgentId"].as_str().unwrap_or("");
        match launcher.release(reviewer) {
            Ok(_) => deps.detail_log(
                "review_reviewer_released",
                json!({"reviewId": field(&review, "reviewId")}),
            ),
            Err(error) => deps.detail_log(
                "review_reviewer_not_released",
                json!({"reviewId": field(&review, "reviewId"), "error": format!("LauncherError: {}", error.message)}),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use capstan_kernel::records::MAX_REVIEW_TEXT_BYTES;

    #[test]
    fn short_text_is_only_normalized() {
        assert_eq!(review_text("  a\r\nb\u{7}c \n"), "a\nb c");
    }

    #[test]
    fn long_text_is_kept_whole() {
        let text = "é".repeat(MAX_REVIEW_TEXT_BYTES);
        let kept = review_text(&text);
        assert_eq!(kept, text);
        assert!(!kept.contains("cut"));
    }

    #[test]
    fn a_text_over_the_limit_is_not_shortened() {
        let text = format!("e{}", "\u{301}".repeat(MAX_REVIEW_TEXT_BYTES));
        let kept = review_text(&text);
        assert_eq!(kept, text);
        assert!(kept.len() > MAX_REVIEW_TEXT_BYTES);
    }
}
