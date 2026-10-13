//! Starting a review and finishing it (src/reviews.ts): pick the reviewer role, spawn a fresh reviewer on a worktree at the
//! reported commit, record the review, and release the reviewer afterwards.
//!
//! The launcher and git calls run on the caller's thread and never inside a kernel closure; each ledger step is one
//! `deps.kernel` call, where Node awaits between them.

use crate::deps::Deps;
use capstan_config::RoleConfig;
use capstan_kernel::helpers::{js_trim, normalize_text};
use capstan_kernel::records::MAX_REVIEW_TEXT_BYTES;
use capstan_kernel::KernelError;
use capstan_launcher::api::{LauncherError, SpawnOptions};
use serde_json::{json, Value};
use std::time::Duration;
use unicode_segmentation::UnicodeSegmentation;

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

/// Normalizes what a reviewer wrote: CR, CRLF, NEL and the Unicode line and paragraph separators become LF, control and
/// format characters other than LF become spaces, and the text is cut at a character boundary within the limit, with a
/// marker when something was cut.
pub fn review_text(text: &str) -> String {
    let clean = normalize_text(text);
    if clean.len() <= MAX_REVIEW_TEXT_BYTES {
        return clean;
    }
    let marker = " [text cut]";
    let room = MAX_REVIEW_TEXT_BYTES - marker.len();
    let mut out = String::new();
    let mut bytes = 0;
    for segment in clean.graphemes(true) {
        if bytes + segment.len() > room {
            break;
        }
        out.push_str(segment);
        bytes += segment.len();
    }
    if out.is_empty() {
        // A first character longer than the limit (a letter with a flood of combining marks): keep whole code points.
        bytes = 0;
        for point in clean.chars() {
            if bytes + point.len_utf8() > room {
                break;
            }
            out.push(point);
            bytes += point.len_utf8();
        }
    }
    let joined = format!(
        "{}{marker}",
        out.trim_end_matches(capstan_kernel::helpers::is_js_space)
    );
    js_trim(&joined).to_string()
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
    let launcher = deps.launcher.as_ref().ok_or_else(|| {
        RequestError::Launcher(LauncherError::new(
            "not_configured",
            "reviews need capstan.toml and Herdr",
        ))
    })?;
    // The warm reviewer of the subject's plan, when the configuration keeps one.
    let mut warm_plan: Option<String> = None;
    let mut reuse: Option<String> = None;
    if config.review.warm_reviewer {
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
    let diff_scope = match check["baseSha"].as_str() {
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
    };
    let mut reused_agent: Option<String> = None;
    if let Some(agent_id) = reuse {
        crate::warm::wait_for_verdict_print(&agent_id);
        match launcher.retarget_review(&agent_id, &commit_sha, subject_id) {
            Ok(_) => reused_agent = Some(agent_id),
            Err(error) => {
                crate::warm::retarget_failed(deps, &error, &agent_id);
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

    #[test]
    fn short_text_is_only_normalized() {
        assert_eq!(review_text("  a\r\nb\u{7}c \n"), "a\nb c");
    }

    #[test]
    fn long_text_is_cut_at_a_character_with_a_marker() {
        let text = "é".repeat(MAX_REVIEW_TEXT_BYTES);
        let cut = review_text(&text);
        assert!(cut.len() <= MAX_REVIEW_TEXT_BYTES);
        assert!(cut.ends_with(" [text cut]"));
        assert!(cut.starts_with('é'));
    }

    #[test]
    fn a_first_character_longer_than_the_limit_keeps_code_points() {
        let text = format!("e{}", "\u{301}".repeat(MAX_REVIEW_TEXT_BYTES));
        let cut = review_text(&text);
        assert!(cut.len() <= MAX_REVIEW_TEXT_BYTES);
        assert!(cut.starts_with('e'));
        assert!(cut.ends_with("[text cut]"));
    }
}
