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
use crate::reviews::{request_review, RequestError};
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

fn report_checked(
    env: &CommandEnv<'_>,
    call: &CommandCall<'_>,
    caller: &capstan_kernel::types::AgentRecord,
    sha: &str,
    summary: String,
) -> CommandResponse {
    // Every report command counts, a repeat included: a repeat still costs a read.
    let now = env.deps.kernel.now_ms();
    let allowed = env
        .state
        .report_limiter
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .allow(&format!("{}:{}", caller.agent_id, caller.generation), now);
    if !allowed {
        return fail(
            ErrorCode::Rejected,
            "rate_limited: at most 10 reports a minute; wait and report once",
        );
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
        if inspection.commit_exists
            && inspection.is_ancestor_of_tip
            && !inspection.is_ancestor_of_base
            && sha != base
        {
            let found = env.deps.git.new_commit_messages(&NewCommitMessagesInput {
                sha: sha.to_string(),
                base_sha: base.clone(),
                own_branch: branch.clone(),
                limit: MAX_CHECKED_COMMITS,
            });
            let commits = match found {
                Ok(NewCommitMessages::Commits(commits)) => commits,
                Ok(NewCommitMessages::TooMany) => {
                    env.log(
                        "report_commit_refused",
                        json!({"agentId": caller.agent_id, "rule": "too_many_commits"}),
                    );
                    return fail(
                        ErrorCode::Rejected,
                        format!(
                            "commit_message: more than {MAX_CHECKED_COMMITS} new commits since the base; squash or rebase your branch down and report the new tip"
                        ),
                    );
                }
                Err(error) => return git_check_failed(env, &error.message),
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
                        let subject =
                            slice_units(commit.message.split('\n').next().unwrap_or(""), 80);
                        let id: String = commit.sha.chars().take(8).collect();
                        format!("commit_message: {id} \"{subject}\" breaks {rule}: {reason}")
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
                return fail(
                    ErrorCode::Rejected,
                    format!(
                        "{}{more}. No report was recorded. Fix: for a commit you have not reported yet, reword it (git commit --amend for the tip, or git rebase -i then reword for an earlier one) and report the new full id; or put the fixed commits on a new branch (git switch -c <new-branch> <base>, git cherry-pick the work, reword) and report from there. Use Conventional Commits subjects (type(scope): description) and no Claude Co-Authored-By, Claude-Session or 'Generated with Claude Code' lines; check git log of the range before reporting.",
                        shown.join("; ")
                    ),
                );
            }
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
    ok(json!({
        "reportId": field(record, "reportId"),
        "state": "accepted",
        "duplicate": field(&result, "duplicate"),
        "announced": !field(record, "notifiedMessageId").is_null(),
    }))
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
            Ok((review, spawn_state)) => ok(json!({
                "reviewId": field(&review, "reviewId"),
                "round": field(&review, "round"),
                "reviewerAgentId": field(&review, "reviewerAgentId"),
                "reviewerRole": field(&review, "reviewerRole"),
                "commit": field(&review, "commitSha"),
                "state": field(&review, "state"),
                "reviewerState": spawn_state,
            })),
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
        env.log(
            "integration_requested",
            json!({"requestedBy": requested_by, "reportIds": call.args}),
        );
    }
    let root = env.deps.options.workspace_root.clone();
    let controller = env.deps.credential().to_string();
    let args = call.args.to_vec();
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
                integrate(&deps, &args, &requested_by)
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
            ok(json!({
                "integrationId": field(record, "integrationId"),
                "state": field(record, "state"),
                "branch": field(record, "branch"),
                "branchRemoved": field(&result, "branchRemoved"),
            }))
        }
        Ok(record) => {
            let merged = record["state"] == "merged";
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
