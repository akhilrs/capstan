//! The report, request-review and integrate commands (src/commands/reports.ts): a worker's report and the review and
//! integration it leads to.
//!
//! `report` leaves the kernel for git exactly where Node awaits; `integrate` runs the kernel's pipeline
//! (`capstan_kernel::integrate`) in one closure on the kernel thread, where it runs git on the project's repository.

use super::shared::{
    fail, is_safe_agent_id, map_kernel_error, ok, one_line_summary, report_reason_text,
    CommandCall, CommandEnv, CommandResponse, ErrorCode, MAX_CHECKED_COMMITS,
};
use super::HandlerMap;
use crate::deps::{
    new_context, CommitInspection, InspectCommitInput, NewCommitMessages, NewCommitMessagesInput,
};
use crate::reviews::{request_review, RequestError, Requested};
use capstan_kernel::areas::messaging::parse_iso_ms;
use capstan_kernel::integrate::{
    check_commit_message, integrate, settle_integration, GitRepo, IntegrateError, IntegrationDeps,
    IntegrationState,
};
use capstan_kernel::records::MAX_REPORT_SUMMARY_BYTES;
use capstan_kernel::types::MutationContext;
use capstan_kernel::KernelError;
use capstan_ledger::iso_from_millis;
use serde_json::{json, Map, Value};
use std::cell::RefCell;

/// Registers the handlers of the routes this module serves (report, request-review, integrate).
pub fn register(map: &mut HandlerMap) {
    map.insert("report", report);
    map.insert("request-review", request_review_command);
    map.insert("integrate", integrate_command);
}

fn field(value: &Value, key: &str) -> Value {
    value.get(key).cloned().unwrap_or(Value::Null)
}

fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or("").to_string()
}

fn is_hex40(text: &str) -> bool {
    text.len() == 40
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `/^[a-z][a-z0-9-]{0,31}$/` (`NAME_PATTERN`).
fn is_role_name(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && text.len() <= 32
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// `text.slice(0, max)` counted in UTF-16 units.
fn slice_units(text: &str, max: usize) -> String {
    let mut units = 0;
    let mut out = String::new();
    for c in text.chars() {
        units += c.len_utf16();
        if units > max {
            break;
        }
        out.push(c);
    }
    out
}

// ------------------------------------------------------------------------------------------------ report

fn report(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let caller = env.agent_of(call.identity);
    let Some(caller) =
        caller.filter(|c| c.state == "active" && (c.kind == "Developer" || c.kind == "Verifier"))
    else {
        return Some(fail(ErrorCode::Forbidden, "only a worker agent can report"));
    };
    if let Some(workspace) = env
        .deps
        .options
        .capstan
        .as_ref()
        .and_then(|config| config.workspace.as_ref())
    {
        return Some(workspace_report(env, call, caller, workspace));
    }
    if call.args.len() != 2 {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "report needs a commit id and a summary",
        ));
    }
    let sha = call.args[0].to_lowercase();
    if !is_hex40(&sha) {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "the commit must be a full 40-character id (git rev-parse HEAD)",
        ));
    }
    let summary = one_line_summary(&call.args[1], MAX_REPORT_SUMMARY_BYTES);
    if summary.is_empty() {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "the summary must not be empty",
        ));
    }
    Some(report_checked(env, call, caller, &sha, summary))
}

/// Every report command counts, a repeat included: a repeat still costs a read.
fn rate_limited(
    env: &CommandEnv<'_>,
    caller: &capstan_kernel::types::AgentRecord,
) -> Option<CommandResponse> {
    let now = env.deps.kernel.now_ms();
    let allowed = env
        .state
        .report_limiter
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .allow(&format!("{}:{}", caller.agent_id, caller.generation), now);
    (!allowed).then(|| {
        fail(
            ErrorCode::Rejected,
            "rate_limited: at most 10 reports a minute; wait and report once",
        )
    })
}

fn report_checked(
    env: &CommandEnv<'_>,
    call: &CommandCall<'_>,
    caller: &capstan_kernel::types::AgentRecord,
    sha: &str,
    summary: String,
) -> CommandResponse {
    if let Some(limited) = rate_limited(env, caller) {
        return limited;
    }
    let looked_up = {
        let (agent_id, generation, sha, controller) = (
            caller.agent_id.clone(),
            caller.generation,
            sha.to_string(),
            env.deps.credential().to_string(),
        );
        env.deps.kernel.run(move |core| {
            let known = core.accepted_report_for(&agent_id, generation, &sha)?;
            if !known.is_null() {
                return Ok((known, None, None));
            }
            let panes = core.agent_panes(&controller)?;
            let row = panes
                .as_array()
                .into_iter()
                .flatten()
                .find(|row| row["agentId"] == agent_id.as_str());
            let branch = row
                .and_then(|row| row["branch"].as_str())
                .map(str::to_string);
            let base = row
                .and_then(|row| row["baseSha"].as_str())
                .map(str::to_lowercase);
            Ok((Value::Null, branch, base))
        })
    };
    let (known, branch, base_sha) = match looked_up {
        Ok(found) => found,
        Err(error) => return map_report_error(&error),
    };
    if !known.is_null() {
        // A repeat writes nothing: no row, no mutation record, no notice.
        return ok(json!({
            "reportId": field(&known, "reportId"),
            "state": "accepted",
            "duplicate": true,
            "announced": !field(&known, "notifiedMessageId").is_null(),
        }));
    }
    let inspected = branch.is_some() && base_sha.is_some();
    let inspection = match (&branch, &base_sha) {
        (Some(branch), Some(base)) => {
            match env.deps.git.inspect_commit(&InspectCommitInput {
                branch: branch.clone(),
                base_sha: Some(base.clone()),
                sha: sha.to_string(),
            }) {
                Ok(inspection) => inspection,
                Err(error) => return git_check_failed(env, &error.message),
            }
        }
        _ => CommitInspection {
            commit_exists: false,
            branch_tip: None,
            is_ancestor_of_tip: false,
            is_ancestor_of_base: false,
            committed_at: None,
        },
    };
    if let Some(committed_at) = &inspection.committed_at {
        let committed = parse_iso_ms(committed_at);
        let agent_id = caller.agent_id.clone();
        let unread = env.deps.kernel.run(move |core| {
            let messages = core.messages_for(&agent_id)?;
            Ok(messages
                .as_array()
                .into_iter()
                .flatten()
                .filter(|message| {
                    matches!(
                        message["state"].as_str(),
                        Some("queued" | "deferred" | "sent" | "unacked")
                    ) && parse_iso_ms(message["queuedAt"].as_str().unwrap_or("")) <= committed
                })
                .map(|message| text(message, "messageId"))
                .collect::<Vec<String>>())
        });
        match unread {
            Ok(unread) if !unread.is_empty() => {
                return fail(
                    ErrorCode::Rejected,
                    format!(
                        "unread_messages: {} message(s) to you are not acknowledged ({}); run cstan inbox, act on and ack them, then report again",
                        unread.len(),
                        unread.join(", ")
                    ),
                )
            }
            Ok(_) => {}
            Err(error) => return map_report_error(&error),
        }
    }
    if let (Some(branch), Some(base)) = (&branch, &base_sha) {
        if let Some(refused) = check_new_commits(
            env,
            caller,
            &*env.deps.git,
            branch,
            base,
            sha,
            &inspection,
            "",
        ) {
            return refused;
        }
    }
    // The order of `{generation, branch, baseSha, ...inspection, checkedAt}`: `committedAt` is part of an inspection git made.
    let mut evidence = Map::new();
    evidence.insert("generation".into(), json!(caller.generation));
    evidence.insert("branch".into(), json!(branch.clone().unwrap_or_default()));
    evidence.insert("baseSha".into(), json!(base_sha));
    evidence.insert("commitExists".into(), json!(inspection.commit_exists));
    if inspected {
        evidence.insert("committedAt".into(), json!(inspection.committed_at));
    }
    evidence.insert("branchTip".into(), json!(inspection.branch_tip));
    evidence.insert(
        "isAncestorOfTip".into(),
        json!(inspection.is_ancestor_of_tip),
    );
    evidence.insert(
        "isAncestorOfBase".into(),
        json!(inspection.is_ancestor_of_base),
    );
    evidence.insert(
        "checkedAt".into(),
        json!(iso_from_millis(env.deps.kernel.now_ms())),
    );
    let input = json!({"commitSha": sha, "summary": summary, "evidence": Value::Object(evidence)});
    let credential = call.credential.to_string();
    let recorded = env.deps.kernel.run(move |core| {
        let context = new_context(core, &credential)?;
        core.record_agent_report(&context, &input)
    });
    let result = match recorded {
        Ok(result) => result,
        Err(error) => return map_report_error(&error),
    };
    let record = &result["record"];
    if record["state"] == "rejected" {
        let reason = record["reason"]
            .as_str()
            .and_then(report_reason_text)
            .unwrap_or("undefined");
        return fail(
            ErrorCode::Rejected,
            format!(
                "report_rejected: {reason} (report {} is recorded)",
                text(record, "reportId")
            ),
        );
    }
    env.log(
        "report_accepted",
        json!({
            "reportId": field(record, "reportId"),
            "agentId": field(record, "agentId"),
            "duplicate": field(&result, "duplicate"),
        }),
    );
    // A verified report is gated automatically when `[verify] auto` names reports; a repeat was gated the first time.
    if result["duplicate"] != true {
        if let Some(report_id) = record["reportId"].as_str() {
            super::verify::auto_queue(env.deps, super::verify::Auto::Report, report_id);
        }
    }
    ok(json!({
        "reportId": field(record, "reportId"),
        "state": "accepted",
        "duplicate": field(&result, "duplicate"),
        "announced": !field(record, "notifiedMessageId").is_null(),
    }))
}

/// The commit-message check of a reported commit: the new commits between `base` and `sha` follow the project's rules. Only
/// a commit that exists, is on the branch and is new is looked at. `label` is empty, or `repo <name>: ` in a workspace.
#[allow(clippy::too_many_arguments)]
fn check_new_commits(
    env: &CommandEnv<'_>,
    caller: &capstan_kernel::types::AgentRecord,
    git: &dyn crate::deps::GitPort,
    branch: &str,
    base: &str,
    sha: &str,
    inspection: &CommitInspection,
    label: &str,
) -> Option<CommandResponse> {
    if inspection.commit_exists
        && inspection.is_ancestor_of_tip
        && !inspection.is_ancestor_of_base
        && sha != base
    {
        let found = git.new_commit_messages(&NewCommitMessagesInput {
            sha: sha.to_string(),
            base_sha: base.to_string(),
            own_branch: branch.to_string(),
            limit: MAX_CHECKED_COMMITS,
        });
        let commits = match found {
            Ok(NewCommitMessages::Commits(commits)) => commits,
            Ok(NewCommitMessages::TooMany) => {
                env.log(
                    "report_commit_refused",
                    json!({"agentId": caller.agent_id, "rule": "too_many_commits"}),
                );
                return Some(fail(
                    ErrorCode::Rejected,
                    format!(
                        "commit_message: {label}more than {MAX_CHECKED_COMMITS} new commits since the base; squash or rebase your branch down and report the new tip"
                    ),
                ));
            }
            Err(error) => return Some(git_check_failed(env, &error.message)),
        };
        let mut violations = Vec::new();
        for commit in &commits {
            for (rule, reason) in check_commit_message(&commit.message, commit.parents) {
                violations.push((commit, rule, reason));
            }
        }
        if !violations.is_empty() {
            let shown: Vec<String> = violations
                .iter()
                .take(5)
                .map(|(commit, rule, reason)| {
                    let subject = slice_units(commit.message.split('\n').next().unwrap_or(""), 80);
                    let id: String = commit.sha.chars().take(8).collect();
                    format!("commit_message: {label}{id} \"{subject}\" breaks {rule}: {reason}")
                })
                .collect();
            let more = if violations.len() > 5 {
                format!(" (and {} more)", violations.len() - 5)
            } else {
                String::new()
            };
            let mut seen: Vec<&str> = Vec::new();
            for (_, rule, _) in &violations {
                if !seen.contains(rule) {
                    seen.push(rule);
                    env.log(
                        "report_commit_refused",
                        json!({"agentId": caller.agent_id, "rule": rule}),
                    );
                }
            }
            return Some(fail(
                ErrorCode::Rejected,
                format!(
                    "{}{more}. No report was recorded. Fix: for a commit you have not reported yet, reword it (git commit --amend for the tip, or git rebase -i then reword for an earlier one) and report the new full id; or put the fixed commits on a new branch (git switch -c <new-branch> <base>, git cherry-pick the work, reword) and report from there. Use Conventional Commits subjects (type(scope): description) and no Claude Co-Authored-By, Claude-Session or 'Generated with Claude Code' lines; check git log of the range before reporting.",
                    shown.join("; ")
                ),
            ));
        }
    }
    None
}

// ------------------------------------------------------------------------------------------------ report in a workspace

const ONE_FORM: &str = "use one form: <repo>=<sha>... or --heads";

/// `<name>=<40-hex>`: the repo and the lowercased commit; the name is not checked here.
fn pair_of(text: &str) -> Option<(&str, String)> {
    let (name, sha) = text.split_once('=')?;
    let sha = sha.to_lowercase();
    (!name.is_empty() && is_hex40(&sha)).then_some((name, sha))
}

enum Form {
    Heads,
    Pairs(Vec<(String, String)>),
}

/// The commit specs of a workspace report: `--heads`, or one or more `<repo>=<sha>` of distinct repos.
fn parse_form(specs: &[String]) -> Result<Form, CommandResponse> {
    let refuse = |text: &str| Err(fail(ErrorCode::InvalidRequest, text));
    if specs.len() == 1 && specs[0] == "--heads" {
        return Ok(Form::Heads);
    }
    if specs.iter().all(|spec| spec.contains('=')) {
        let mut pairs: Vec<(String, String)> = Vec::new();
        for spec in specs {
            let Some((name, sha)) = pair_of(spec) else {
                let name = spec.split('=').next().unwrap_or("");
                return refuse(&format!(
                    "repo {name}: the commit must be a full 40-character id (git rev-parse HEAD)"
                ));
            };
            if pairs.iter().any(|(seen, _)| seen == name) {
                return refuse(ONE_FORM);
            }
            pairs.push((name.to_string(), sha));
        }
        return Ok(Form::Pairs(pairs));
    }
    if specs.len() == 1 && is_hex40(&specs[0].to_lowercase()) {
        return refuse("name each repo: <repo>=<sha>");
    }
    refuse(ONE_FORM)
}

/// A live worktree of the reporting agent.
struct Held {
    path: std::path::PathBuf,
    branch: String,
    base: String,
}

/// A commit a workspace report names.
struct Named {
    repo: String,
    sha: String,
    branch: String,
    base: String,
    /// Taken from the earlier report, not named now.
    carried: bool,
}

fn workspace_report(
    env: &CommandEnv<'_>,
    call: &CommandCall<'_>,
    caller: &capstan_kernel::types::AgentRecord,
    workspace: &capstan_config::Workspace,
) -> CommandResponse {
    if call.args.len() < 2 {
        return fail(
            ErrorCode::InvalidRequest,
            "report needs commits and a summary",
        );
    }
    let (specs, last) = call.args.split_at(call.args.len() - 1);
    let last = &last[0];
    if pair_of(last).is_some() || is_hex40(&last.to_lowercase()) {
        return fail(ErrorCode::InvalidRequest, ONE_FORM);
    }
    let form = match parse_form(specs) {
        Ok(form) => form,
        Err(refused) => return refused,
    };
    if let Form::Pairs(pairs) = &form {
        if let Some((name, _)) = pairs
            .iter()
            .find(|(name, _)| workspace.repo(name).is_none())
        {
            return fail(
                ErrorCode::InvalidRequest,
                format!("repo {name} is not a repo of this project"),
            );
        }
    }
    let summary = one_line_summary(last, MAX_REPORT_SUMMARY_BYTES);
    if summary.is_empty() {
        return fail(ErrorCode::InvalidRequest, "the summary must not be empty");
    }
    if let Some(limited) = rate_limited(env, caller) {
        return limited;
    }
    // The agent's live worktrees and the commits of its newest earlier accepted report.
    let looked_up = {
        let (agent_id, controller) = (caller.agent_id.clone(), env.deps.credential().to_string());
        env.deps.kernel.run(move |core| {
            let rows = core.list_agent_worktrees(&agent_id)?;
            let reports = core.agent_reports(&controller, Some(1000))?;
            let earlier = reports
                .as_array()
                .into_iter()
                .flatten()
                .find(|r| r["agentId"] == agent_id.as_str() && r["state"] == "accepted")
                .and_then(|r| r["reportId"].as_str().map(str::to_string));
            let earlier = match earlier {
                Some(id) => {
                    let commits = core.report_commits(&id)?;
                    Some((id, commits))
                }
                None => None,
            };
            Ok((rows, earlier))
        })
    };
    let (rows, earlier) = match looked_up {
        Ok(found) => found,
        Err(error) => return map_report_error(&error),
    };
    let mut held: std::collections::BTreeMap<String, Held> = std::collections::BTreeMap::new();
    for row in rows.as_array().into_iter().flatten() {
        held.insert(
            text(row, "repo"),
            Held {
                path: std::path::PathBuf::from(text(row, "worktreePath")),
                branch: text(row, "branch"),
                base: text(row, "baseSha").to_lowercase(),
            },
        );
    }
    let earlier_id = earlier.as_ref().map(|(id, _)| id.clone());
    let earliest: std::collections::HashMap<String, String> = earlier
        .iter()
        .flat_map(|(_, commits)| commits.as_array().into_iter().flatten())
        .map(|c| (text(c, "repo"), text(c, "commitSha")))
        .collect();
    let since = |text: &str| match &earlier_id {
        Some(id) => format!("{text} {id}"),
        None => text.to_string(),
    };
    let mut named: Vec<Named> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    // The repos the specs name, in either form, are decided; the rest follow the coverage rule.
    let mut decided: Vec<String> = Vec::new();
    if let Form::Pairs(pairs) = &form {
        for (repo, sha) in pairs {
            decided.push(repo.clone());
            let Some(wt) = held.get(repo) else {
                return fail(
                    ErrorCode::Rejected,
                    format!("repo {repo} is not in your change set"),
                );
            };
            if *sha == wt.base {
                continue; // naming the base drops the repo
            }
            named.push(Named {
                repo: repo.clone(),
                sha: sha.clone(),
                branch: wt.branch.clone(),
                base: wt.base.clone(),
                carried: false,
            });
        }
    }
    for (repo, wt) in &held {
        if decided.contains(repo) {
            continue;
        }
        let heads = matches!(form, Form::Heads);
        let covered = earliest.get(repo);
        if !heads && covered.is_none() {
            continue; // a repo nothing covers and nothing names is left out
        }
        let git = env.deps.repo_git(repo);
        let state = match git.worktree_state(&wt.path) {
            Ok(state) => state,
            Err(error) => return repo_check_failed(env, repo, &error.message),
        };
        if heads {
            if let Some(kind) = state.in_progress {
                return fail(
                    ErrorCode::Rejected,
                    format!(
                        "repo {repo} has a {kind} in progress; finish or abort it and report again"
                    ),
                );
            }
            if state.branch.is_none() {
                return fail(
                    ErrorCode::Rejected,
                    format!(
                        "repo {repo} has a detached HEAD; check out {} and report again",
                        wt.branch
                    ),
                );
            }
            if state.branch.as_deref() != Some(wt.branch.as_str()) {
                return fail(
                    ErrorCode::Rejected,
                    format!(
                        "repo {repo} is on branch {}, not {}; check out {} and report again",
                        state.branch.as_deref().unwrap_or(""),
                        wt.branch,
                        wt.branch
                    ),
                );
            }
            if state.dirty {
                warnings.push(format!(
                    "repo {repo} has uncommitted changes; they are not in this report"
                ));
            }
        }
        let head = state.head;
        let back_at_base = || {
            fail(
                ErrorCode::Rejected,
                format!(
                    "repo {repo} is back at its base; name it {repo}={} to drop it, or commit",
                    wt.base
                ),
            )
        };
        match covered {
            Some(commit) if *commit == head => named.push(Named {
                repo: repo.clone(),
                sha: head,
                branch: wt.branch.clone(),
                base: wt.base.clone(),
                carried: true,
            }),
            Some(_) if head == wt.base => return back_at_base(),
            Some(_) if !heads => {
                return fail(
                    ErrorCode::Rejected,
                    format!("repo {repo} has commits after {}; name it", since("report")),
                )
            }
            Some(_) => named.push(Named {
                repo: repo.clone(),
                sha: head,
                branch: wt.branch.clone(),
                base: wt.base.clone(),
                carried: false,
            }),
            None if heads && head != wt.base => named.push(Named {
                repo: repo.clone(),
                sha: head,
                branch: wt.branch.clone(),
                base: wt.base.clone(),
                carried: false,
            }),
            None => {}
        }
    }
    named.sort_by(|a, b| a.repo.cmp(&b.repo));
    let carried: Vec<String> = named
        .iter()
        .filter(|n| n.carried)
        .map(|n| n.repo.clone())
        .collect();
    if named.iter().all(|n| n.carried) {
        let text = if carried.is_empty() {
            "no repo has new commits".to_string()
        } else {
            format!("no repo has new commits since {}", since("report"))
        };
        return fail(ErrorCode::Rejected, text);
    }
    let first = &named[0];
    // A repeat of the very same commits writes nothing.
    {
        let (agent_id, generation, sha) = (
            caller.agent_id.clone(),
            caller.generation,
            first.sha.clone(),
        );
        let known = env.deps.kernel.run(move |core| {
            let known = core.accepted_report_for(&agent_id, generation, &sha)?;
            let commits = match known["reportId"].as_str() {
                Some(id) => core.report_commits(id)?,
                None => Value::Null,
            };
            Ok((known, commits))
        });
        let (known, commits) = match known {
            Ok(found) => found,
            Err(error) => return map_report_error(&error),
        };
        let same = !known.is_null()
            && commits.as_array().is_some_and(|list| {
                list.len() == named.len()
                    && list.iter().zip(&named).all(|(row, n)| {
                        row["repo"] == n.repo.as_str() && row["commitSha"] == n.sha.as_str()
                    })
            });
        if same {
            return ok(json!({
                "reportId": field(&known, "reportId"),
                "state": "accepted",
                "duplicate": true,
                "announced": !field(&known, "notifiedMessageId").is_null(),
            }));
        }
    }
    // Each commit is checked in its own repo, as a single-repo report is.
    let mut evidence: Option<Map<String, Value>> = None;
    let mut latest_commit_ms = 0_i64;
    for (index, n) in named.iter().enumerate() {
        if n.carried && index != 0 {
            continue;
        }
        let label = format!("repo {}: ", n.repo);
        let git = env.deps.repo_git(&n.repo);
        let inspection = match git.inspect_commit(&InspectCommitInput {
            branch: n.branch.clone(),
            base_sha: Some(n.base.clone()),
            sha: n.sha.clone(),
        }) {
            Ok(inspection) => inspection,
            Err(error) => return repo_check_failed(env, &n.repo, &error.message),
        };
        if let Some(committed_at) = &inspection.committed_at {
            latest_commit_ms = latest_commit_ms.max(parse_iso_ms(committed_at));
        }
        if index > 0 && !n.carried {
            let reason = if !inspection.commit_exists {
                Some("commit_missing")
            } else if n.sha == n.base || inspection.is_ancestor_of_base {
                Some("not_new_on_branch")
            } else if !inspection.is_ancestor_of_tip {
                Some("not_on_branch")
            } else {
                None
            };
            if let Some(reason) = reason {
                return fail(
                    ErrorCode::Rejected,
                    format!(
                        "report_rejected: {label}{} (nothing was recorded)",
                        report_reason_text(reason).unwrap_or("undefined")
                    ),
                );
            }
        }
        if !n.carried {
            if let Some(refused) = check_new_commits(
                env,
                caller,
                &*git,
                &n.branch,
                &n.base,
                &n.sha,
                &inspection,
                &label,
            ) {
                return refused;
            }
        }
        if index == 0 {
            let mut e = Map::new();
            e.insert("generation".into(), json!(caller.generation));
            e.insert("branch".into(), json!(n.branch));
            e.insert("baseSha".into(), json!(n.base));
            e.insert("commitExists".into(), json!(inspection.commit_exists));
            e.insert("committedAt".into(), json!(inspection.committed_at));
            e.insert("branchTip".into(), json!(inspection.branch_tip));
            e.insert(
                "isAncestorOfTip".into(),
                json!(inspection.is_ancestor_of_tip),
            );
            e.insert(
                "isAncestorOfBase".into(),
                json!(inspection.is_ancestor_of_base),
            );
            e.insert(
                "checkedAt".into(),
                json!(iso_from_millis(env.deps.kernel.now_ms())),
            );
            evidence = Some(e);
        }
    }
    if latest_commit_ms > 0 {
        let agent_id = caller.agent_id.clone();
        let unread = env.deps.kernel.run(move |core| {
            let messages = core.messages_for(&agent_id)?;
            Ok(messages
                .as_array()
                .into_iter()
                .flatten()
                .filter(|message| {
                    matches!(
                        message["state"].as_str(),
                        Some("queued" | "deferred" | "sent" | "unacked")
                    ) && parse_iso_ms(message["queuedAt"].as_str().unwrap_or(""))
                        <= latest_commit_ms
                })
                .map(|message| text(message, "messageId"))
                .collect::<Vec<String>>())
        });
        match unread {
            Ok(unread) if !unread.is_empty() => {
                return fail(
                    ErrorCode::Rejected,
                    format!(
                        "unread_messages: {} message(s) to you are not acknowledged ({}); run cstan inbox, act on and ack them, then report again",
                        unread.len(),
                        unread.join(", ")
                    ),
                )
            }
            Ok(_) => {}
            Err(error) => return map_report_error(&error),
        }
    }
    let commits: Vec<Value> = named
        .iter()
        .map(|n| json!({"repo": n.repo, "commitSha": n.sha, "branch": n.branch, "baseSha": n.base}))
        .collect();
    let input = json!({
        "commitSha": first.sha,
        "summary": summary,
        "evidence": Value::Object(evidence.unwrap_or_default()),
        "commits": commits,
    });
    let credential = call.credential.to_string();
    let recorded = env.deps.kernel.run(move |core| {
        let context = new_context(core, &credential)?;
        core.record_agent_report(&context, &input)
    });
    let result = match recorded {
        Ok(result) => result,
        Err(error) => return map_report_error(&error),
    };
    let record = &result["record"];
    if record["state"] == "rejected" {
        let reason = record["reason"]
            .as_str()
            .and_then(report_reason_text)
            .unwrap_or("undefined");
        return fail(
            ErrorCode::Rejected,
            format!(
                "report_rejected: repo {}: {reason} (report {} is recorded)",
                first.repo,
                text(record, "reportId")
            ),
        );
    }
    env.log(
        "report_accepted",
        json!({
            "reportId": field(record, "reportId"),
            "agentId": field(record, "agentId"),
            "duplicate": field(&result, "duplicate"),
        }),
    );
    if result["duplicate"] != true {
        if let Some(report_id) = record["reportId"].as_str() {
            super::verify::auto_queue(env.deps, super::verify::Auto::Report, report_id);
        }
    }
    ok(json!({
        "reportId": field(record, "reportId"),
        "state": "accepted",
        "duplicate": field(&result, "duplicate"),
        "announced": !field(record, "notifiedMessageId").is_null(),
        "repos": named.iter().map(|n| json!({"repo": n.repo, "commitSha": n.sha})).collect::<Vec<_>>(),
        "carried": carried,
        "warnings": warnings,
    }))
}

/// A `GitCheckError` of one repo while checking a workspace report.
fn repo_check_failed(env: &CommandEnv<'_>, repo: &str, message: &str) -> CommandResponse {
    env.log(
        "report_check_failed",
        json!({"repo": repo, "error": message}),
    );
    fail(
        ErrorCode::Error,
        format!("the controller could not check repo {repo} just now; report again"),
    )
}

/// A `GitCheckError` while checking a report.
fn git_check_failed(env: &CommandEnv<'_>, message: &str) -> CommandResponse {
    env.log("report_check_failed", json!({"error": message}));
    fail(
        ErrorCode::Error,
        "the controller could not check the commit just now; report again",
    )
}

fn map_report_error(error: &KernelError) -> CommandResponse {
    if error.is_controller_error() && error.message().starts_with("report limit") {
        return fail(
            ErrorCode::Rejected,
            format!("report_limit: {}", error.message()),
        );
    }
    map_kernel_error(error)
}

// ------------------------------------------------------------------------------------------------ request-review

fn request_review_command(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let caller = env.agent_of(call.identity);
    let Some(caller) =
        caller.filter(|c| c.state == "active" && (c.kind == "PM" || env.is_architect(Some(c))))
    else {
        return Some(fail(
            ErrorCode::Forbidden,
            "only the PM or the architect can request a review",
        ));
    };
    if call.args.is_empty() || call.args.len() > 2 {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "request-review needs a report or integration id and optionally a reviewer role",
        ));
    }
    let report_id = call.args[0].clone();
    if !is_safe_agent_id(&report_id) {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "the report or integration id is not valid",
        ));
    }
    let role = call.args.get(1).cloned();
    if role.as_deref().is_some_and(|role| !is_role_name(role)) {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "the reviewer role name is not valid",
        ));
    }
    if let Err(error) = env
        .deps
        .kernel
        .run(|core| core.assert_run_not_paused("request-review"))
    {
        return Some(map_kernel_error(&error));
    }
    if env.deps.launcher.is_none() || env.config().is_none() {
        return Some(fail(
            ErrorCode::NotConfigured,
            "reviews need capstan.toml, Herdr and a git repository",
        ));
    }
    env.log(
        "review_requested",
        json!({"requestedBy": caller.agent_id, "reportId": report_id}),
    );
    Some(
        match request_review(env.deps, &report_id, role.as_deref(), call.credential) {
            Ok(Requested {
                review,
                reviewer_state,
                reused,
            }) => ok(json!({
                "reviewId": field(&review, "reviewId"),
                "round": field(&review, "round"),
                "reviewerAgentId": field(&review, "reviewerAgentId"),
                "reviewerRole": field(&review, "reviewerRole"),
                "commit": field(&review, "commitSha"),
                "state": field(&review, "state"),
                "reviewerState": reviewer_state,
                "reviewerReused": reused,
            })),
            Err(RequestError::Kernel(error)) if error.code() == Some("verify_required") => {
                fail(ErrorCode::Rejected, error.message())
            }
            Err(RequestError::Review(refused)) => fail(
                ErrorCode::Rejected,
                format!("{}: {}", refused.code, refused.message),
            ),
            Err(RequestError::Kernel(error)) if error.is_controller_error() => fail(
                ErrorCode::Rejected,
                format!("review_refused: {}", error.message()),
            ),
            Err(RequestError::Kernel(error)) => map_kernel_error(&error),
            Err(RequestError::Launcher(error)) => {
                super::shared::map_error(&super::shared::HandlerError::from(error))
            }
        },
    )
}

// ------------------------------------------------------------------------------------------------ integrate

thread_local! {
    /// What the integration pipeline of this process remembers; the kernel thread runs every integration.
    static INTEGRATIONS: IntegrationState = IntegrationState::new();
}

/// Runs `f` with the integration state of the kernel thread (the cleanup needs to know whether one is running).
pub(super) fn with_integration_state<R>(f: impl FnOnce(&IntegrationState) -> R) -> R {
    INTEGRATIONS.with(f)
}

/// What the pipeline answered, and the log entries it wrote on the kernel thread (written once it is back).
struct Integrated {
    result: Result<Value, IntegrateError>,
    logs: Vec<(String, Value)>,
}

fn integrate_command(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let Some(requested_by) = env.review_integrator(call.identity) else {
        return Some(fail(
            ErrorCode::Forbidden,
            "only the PM, the operator or the architect may integrate reports",
        ));
    };
    if call.args.first().map(String::as_str) == Some("confirm")
        && env.worker_manager(call.identity).is_none()
    {
        return Some(fail(
            ErrorCode::Forbidden,
            "only the PM or the operator may confirm an integration",
        ));
    }
    if call.args.is_empty() {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "integrate needs report ids, or confirm|discard and an integration id",
        ));
    }
    if let Err(error) = env
        .deps
        .kernel
        .run(|core| core.assert_run_not_paused("integrate"))
    {
        return Some(map_kernel_error(&error));
    }
    let first = call.args[0].as_str();
    let settling = first == "confirm" || first == "discard";
    if settling {
        if call.args.len() != 2 || !is_safe_agent_id(&call.args[1]) {
            return Some(fail(
                ErrorCode::InvalidRequest,
                format!("integrate {first} needs one integration id"),
            ));
        }
        env.log(
            "integration_settle_requested",
            json!({"requestedBy": requested_by, "integrationId": call.args[1], "outcome": first}),
        );
    } else {
        if call.args.iter().any(|id| !is_safe_agent_id(id)) {
            return Some(fail(ErrorCode::InvalidRequest, "a report id is not valid"));
        }
        if let Err(error) =
            super::verify::check_gate(env.deps, super::verify::Gate::Integrate, call.args)
        {
            return Some(if error.code() == Some("verify_required") {
                fail(ErrorCode::Rejected, error.message())
            } else {
                map_kernel_error(&error)
            });
        }
        env.log(
            "integration_requested",
            json!({"requestedBy": requested_by, "reportIds": call.args}),
        );
    }
    if !settling && super::verify::is_workspace(env.deps) {
        return Some(workspace_integrate(env, call.args, &requested_by));
    }
    let root = env.deps.options.workspace_root.clone();
    let controller = env.deps.credential().to_string();
    let args = call.args.to_vec();
    let integrator = requested_by.clone();
    let impact = env
        .config()
        .map(|config| config.impact.clone())
        .unwrap_or_default();
    let ran = env.deps.kernel.call(move |core| {
        let repo = GitRepo::new(root);
        let logs: RefCell<Vec<(String, Value)>> = RefCell::new(Vec::new());
        let context = |credential: &str| -> MutationContext {
            crate::deps::new_context(core, credential).unwrap_or_else(|_| MutationContext {
                credential: credential.to_string(),
                request_id: String::new(),
                idempotency_key: String::new(),
                expected_version: 0,
                input_revision: 0,
            })
        };
        let log =
            |event: &str, details: Value| logs.borrow_mut().push((event.to_string(), details));
        let result = INTEGRATIONS.with(|state| {
            let deps = IntegrationDeps {
                kernel: core.kernel(),
                git: &repo,
                state,
                credential: &controller,
                context: &context,
                log: &log,
                impact: &impact,
            };
            if settling {
                settle_integration(
                    &deps,
                    &args[1],
                    if args[0] == "confirm" {
                        "confirmed"
                    } else {
                        "discarded"
                    },
                )
            } else {
                integrate(&deps, &args, &integrator)
            }
        });
        Integrated {
            result,
            logs: logs.into_inner(),
        }
    });
    let integrated = match ran {
        Ok(integrated) => integrated,
        Err(_) => {
            return Some(map_kernel_error(&KernelError::controller(
                "controller is closed",
            )))
        }
    };
    for (event, details) in integrated.logs {
        env.log(&event, details);
    }
    Some(match integrated.result {
        Ok(result) if settling => {
            let record = &result["record"];
            let mut answer = json!({
                "integrationId": field(record, "integrationId"),
                "state": field(record, "state"),
                "branch": field(record, "branch"),
                "branchRemoved": field(&result, "branchRemoved"),
            });
            // The settle above is committed; the cleanup is a second, separate step that can only add to the answer.
            if first == "confirm" && record["state"] == "confirmed" {
                if let Some(cleanup) =
                    super::cleanup::after_confirm(env, &call.args[1], &requested_by)
                {
                    answer["cleanup"] = cleanup;
                }
            }
            ok(answer)
        }
        Ok(record) => {
            let merged = record["state"] == "merged";
            if merged {
                if let Some(id) = record["integrationId"].as_str() {
                    super::verify::auto_queue(env.deps, super::verify::Auto::Integration, id);
                }
            }
            let conflicted = record["state"] == "conflicted";
            let reports: Vec<Value> = record["reports"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|r| field(r, "reportId"))
                .collect();
            ok(json!({
                "integrationId": field(&record, "integrationId"),
                "state": field(&record, "state"),
                "base": field(&record, "baseSha"),
                "branch": if merged { field(&record, "branch") } else { Value::Null },
                "head": field(&record, "headSha"),
                "reports": reports,
                "conflict": if conflicted {
                    json!({
                        "reportId": field(&record, "conflictReportId"),
                        "files": field(&record, "conflictFiles"),
                    })
                } else {
                    Value::Null
                },
                "failure": field(&record, "failureReason"),
            }))
        }
        Err(IntegrateError::Integration { code, message }) => {
            fail(ErrorCode::Rejected, format!("{code}: {message}"))
        }
        Err(IntegrateError::Git(_)) => fail(
            ErrorCode::Error,
            "the controller could not run git just now",
        ),
        Err(IntegrateError::Kernel(error)) if error.is_controller_error() => fail(
            ErrorCode::Rejected,
            format!("integration_refused: {}", error.message()),
        ),
        Err(IntegrateError::Kernel(error)) => map_kernel_error(&error),
    })
}

/// `integrate <report-id>...` in a workspace: a set of the repos the reports touch, one member at a time in merge
/// order (see `capstan_kernel::integrate::integrate_set`).
fn workspace_integrate(
    env: &CommandEnv<'_>,
    report_ids: &[String],
    requested_by: &str,
) -> CommandResponse {
    use super::integrate_seams::{integrate_seams, member_repo};
    use capstan_kernel::integrate::{integrate_set, SetRun};
    let Some(workspace) = env
        .deps
        .options
        .capstan
        .as_ref()
        .and_then(|config| config.workspace.as_ref())
    else {
        return fail(
            ErrorCode::Rejected,
            "integration_refused: the project has no [repos]",
        );
    };
    let roots: Vec<(String, std::path::PathBuf)> = workspace
        .repos
        .iter()
        .map(|repo| (repo.name.clone(), env.deps.repo_root(&repo.name)))
        .collect();
    let mains: Vec<(String, String)> = workspace
        .repos
        .iter()
        .map(|repo| (repo.name.clone(), workspace.main_branch_of(&repo.name)))
        .collect();
    let merge_order = workspace.merge_order.clone();
    let seams = integrate_seams(&env.state.integrate_seams);
    let controller = env.deps.credential().to_string();
    let ids = report_ids.to_vec();
    let integrator = requested_by.to_string();
    let impact = env
        .config()
        .map(|config| config.impact.clone())
        .unwrap_or_default();
    let ran = env.deps.kernel.call(move |core| {
        let root_repo = GitRepo::new(std::path::PathBuf::new());
        let logs: RefCell<Vec<(String, Value)>> = RefCell::new(Vec::new());
        let context = |credential: &str| -> MutationContext {
            crate::deps::new_context(core, credential).unwrap_or_else(|_| MutationContext {
                credential: credential.to_string(),
                request_id: String::new(),
                idempotency_key: String::new(),
                expected_version: 0,
                input_revision: 0,
            })
        };
        let log =
            |event: &str, details: Value| logs.borrow_mut().push((event.to_string(), details));
        let git_for = |repo: &str| {
            let root = roots
                .iter()
                .find(|(name, _)| name == repo)
                .map(|(_, root)| root.clone())
                .unwrap_or_default();
            member_repo(&seams, &root)
        };
        let main_of = |repo: &str| {
            mains
                .iter()
                .find(|(name, _)| name == repo)
                .map(|(_, main)| main.clone())
                .unwrap_or_default()
        };
        let before = |repo: &str| match &seams {
            Some(seams) => seams.before_member(repo),
            None => Ok(()),
        };
        let result = INTEGRATIONS.with(|state| {
            let deps = IntegrationDeps {
                kernel: core.kernel(),
                git: &root_repo,
                state,
                credential: &controller,
                context: &context,
                log: &log,
                impact: &impact,
            };
            integrate_set(
                &deps,
                &SetRun {
                    report_ids: &ids,
                    requested_by: &integrator,
                    merge_order: &merge_order,
                    main_branch_of: &main_of,
                    git_for: &git_for,
                    before_member: &before,
                },
            )
        });
        Integrated {
            result,
            logs: logs.into_inner(),
        }
    });
    let integrated = match ran {
        Ok(integrated) => integrated,
        Err(_) => return map_kernel_error(&KernelError::controller("controller is closed")),
    };
    for (event, details) in integrated.logs {
        env.log(&event, details);
    }
    match integrated.result {
        Ok(answer) => ok(answer),
        Err(IntegrateError::Integration { code, message }) => {
            fail(ErrorCode::Rejected, format!("{code}: {message}"))
        }
        Err(IntegrateError::Git(_)) => fail(
            ErrorCode::Error,
            "the controller could not run git just now",
        ),
        Err(IntegrateError::Kernel(error)) if error.is_controller_error() => fail(
            ErrorCode::Rejected,
            format!("integration_refused: {}", error.message()),
        ),
        Err(IntegrateError::Kernel(error)) => map_kernel_error(&error),
    }
}
