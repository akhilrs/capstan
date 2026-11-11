//! What the daemon repairs when it starts, after a stop that cut something off: finished reviews whose reviewer is still
//! active (`recoverReviews`, in reviews.rs) and integrations that were merging (`recoverIntegrations` of
//! src/integration.ts).

use crate::deps::{new_context, Deps};
use serde_json::{json, Value};

/// A `running` integration that this process is not merging was cut off: its branch is removed and it is marked failed.
/// Branches of settled integrations that could not be deleted earlier are swept first, and confirmed ones get their
/// covered reports recorded. `stopped` is polled between rows so a stop does not wait for all of them.
pub fn recover_integrations(deps: &Deps, stopped: &dyn Fn() -> bool) {
    let credential = deps.credential().to_string();
    let rows = |read: fn(&capstan_kernel::Core, &str) -> capstan_kernel::KernelResult<Value>| {
        let credential = credential.clone();
        match deps.kernel.run(move |core| read(core, &credential)) {
            Ok(Value::Array(rows)) => rows,
            _ => Vec::new(),
        }
    };
    let settled = rows(|core, credential| core.settled_integrations(credential, None));
    // sweepSettledBranches
    for row in &settled {
        if stopped() {
            return;
        }
        let Some(head_sha) = row["headSha"].as_str() else {
            continue;
        };
        let integration_id = row["integrationId"].as_str().unwrap_or("");
        let branch = row["branch"].as_str().unwrap_or("");
        let git = git_of_integration(deps, integration_id);
        match git.branch_tip(branch) {
            Ok(tip) if tip.as_deref() != Some(head_sha) => {}
            Ok(_) => match git.delete_branch(branch, head_sha) {
                Ok(true) => deps.detail_log(
                    "integration_branch_swept",
                    json!({"integrationId": integration_id}),
                ),
                Ok(false) => {}
                Err(error) => sweep_failed(deps, integration_id, &error.message),
            },
            Err(error) => sweep_failed(deps, integration_id, &error.message),
        }
    }
    for row in &settled {
        if stopped() {
            return;
        }
        if row["state"] == "confirmed" {
            record_coverage(deps, row["integrationId"].as_str().unwrap_or(""));
        }
    }
    for row in rows(|core, credential| core.running_integrations(credential)) {
        if stopped() {
            return;
        }
        let integration_id = row["integrationId"].as_str().unwrap_or("").to_string();
        let branch = row["branch"].as_str().unwrap_or("").to_string();
        let outcome = (|| -> Result<(), String> {
            // A set member's branch lives in its own repo.
            let git = git_of_integration(deps, &integration_id);
            let tip = git
                .branch_tip(&branch)
                .map_err(|e| format!("Error: {}", e.message))?;
            if let Some(tip) = tip {
                if !git
                    .delete_branch(&branch, &tip)
                    .map_err(|e| format!("Error: {}", e.message))?
                {
                    deps.detail_log(
                        "integration_branch_not_removed",
                        json!({"integrationId": integration_id, "branch": branch}),
                    );
                }
            }
            let (credential, id) = (credential.clone(), integration_id.clone());
            deps.kernel
                .run(move |core| {
                    let context = new_context(core, &credential)?;
                    core.finish_integration(
                        &context,
                        &json!({
                            "integrationId": id,
                            "outcome": {
                                "kind": "failed",
                                "reason": "the daemon stopped while the integration was merging",
                            },
                        }),
                    )
                })
                .map_err(|e| e.to_string())?;
            deps.detail_log(
                "integration_recovered",
                json!({"integrationId": integration_id}),
            );
            Ok(())
        })();
        if let Err(error) = outcome {
            deps.detail_log(
                "integration_not_recovered",
                json!({"integrationId": integration_id, "error": error}),
            );
        }
    }
    recover_sets(deps, stopped);
}

/// A set still `running` was cut off. Its members are read after the running rows above were settled: a conflicted
/// member makes the set conflicted with its files; a failed or discarded one makes it failed; every member merged
/// moves it to merged (death after the last merge, before the state move); members that never started leave it failed,
/// 'cut off before repo <r>'. Unstarted members stay detached.
fn recover_sets(deps: &Deps, stopped: &dyn Fn() -> bool) {
    let credential = deps.credential().to_string();
    let ids = deps
        .kernel
        .run(|core| core.set_running())
        .unwrap_or_default();
    for set_id in ids {
        if stopped() {
            return;
        }
        let set = match deps.kernel.run({
            let id = set_id.clone();
            move |core| core.integration_set(&id)
        }) {
            Ok(set) => set,
            Err(_) => continue,
        };
        let (state, reason) = set_outcome(deps, &set);
        let (c, id, st, rs) = (
            credential.clone(),
            set_id.clone(),
            state.clone(),
            reason.clone(),
        );
        let result = deps.kernel.run(move |core| {
            let context = new_context(core, &c)?;
            core.set_integration_state(
                &context,
                &json!({"setId": id, "state": st, "reason": if st == "merged" { Value::Null } else { json!(rs) }}),
            )
        });
        match result {
            Ok(_) => deps.detail_log(
                "integration_set_recovered",
                json!({"setId": set_id, "state": state, "reason": reason}),
            ),
            Err(error) => deps.detail_log(
                "integration_set_not_recovered",
                json!({"setId": set_id, "error": error.to_string()}),
            ),
        }
    }
}

/// The state and reason a cut-off set moves to, from the state of each member's integration.
fn set_outcome(deps: &Deps, set: &Value) -> (String, String) {
    let mut merged = 0;
    let mut cut_off: Option<String> = None;
    let members = set["members"].as_array().cloned().unwrap_or_default();
    for member in &members {
        let repo = member["repo"].as_str().unwrap_or("").to_string();
        let Some(id) = member["integrationId"].as_str().map(str::to_string) else {
            cut_off.get_or_insert(repo);
            continue;
        };
        let record = deps
            .kernel
            .run(move |core| core.integration(&id))
            .unwrap_or(Value::Null);
        match record["state"].as_str().unwrap_or("") {
            "merged" | "confirmed" => merged += 1,
            "conflicted" => {
                let files: Vec<&str> = record["conflictFiles"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                return (
                    "conflicted".into(),
                    format!("repo {repo} conflicts in {}", files.join(", ")),
                );
            }
            "running" => {
                cut_off.get_or_insert(repo);
            }
            _ => {
                let why = record["failureReason"]
                    .as_str()
                    .unwrap_or("it did not merge");
                return ("failed".into(), format!("repo {repo} failed: {why}"));
            }
        }
    }
    match cut_off {
        Some(repo) => ("failed".into(), format!("cut off before repo {repo}")),
        None if merged == members.len() && merged > 0 => ("merged".into(), String::new()),
        None => ("failed".into(), "cut off before any repo merged".into()),
    }
}

/// The git of the repo an integration lives in: its set member's repo, the root for any other integration.
fn git_of_integration(
    deps: &Deps,
    integration_id: &str,
) -> std::sync::Arc<dyn crate::deps::GitPort> {
    let id = integration_id.to_string();
    let repo = deps
        .kernel
        .run(move |core| core.repo_of_integration(&id))
        .ok()
        .flatten()
        .unwrap_or_default();
    deps.repo_git(&repo)
}

fn sweep_failed(deps: &Deps, integration_id: &str, message: &str) {
    deps.detail_log(
        "integration_branch_sweep_failed",
        json!({"integrationId": integration_id, "error": format!("Error: {message}")}),
    );
}

/// Stores the reports a confirmed integration covers without having merged them. A failure is logged and retried at the
/// next daemon start; it never blocks the confirm.
fn record_coverage(deps: &Deps, integration_id: &str) {
    let outcome = (|| -> Result<(), String> {
        let credential = deps.credential().to_string();
        let (c, id) = (credential.clone(), integration_id.to_string());
        let candidates = deps
            .kernel
            .run(move |core| core.coverage_candidates(&c, &id))
            .map_err(|e| e.to_string())?;
        let reports = candidates["reports"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if candidates.is_null() || reports.is_empty() {
            return Ok(());
        }
        let covered = git_of_integration(deps, integration_id)
            .covered_reports(
                candidates["headSha"].as_str().unwrap_or(""),
                &Value::Array(reports),
                &json!({"memberCommits": candidates["memberCommits"]}),
            )
            .map_err(|e| format!("Error: {}", e.message))?;
        let list = covered.as_array().cloned().unwrap_or_default();
        if list.is_empty() {
            return Ok(());
        }
        let id = integration_id.to_string();
        let record = covered.clone();
        deps.kernel
            .run(move |core| core.record_covered_reports(&credential, &id, &record))
            .map_err(|e| e.to_string())?;
        let ids: Vec<Value> = list.iter().map(|c| c["reportId"].clone()).collect();
        deps.detail_log(
            "integration_coverage_recorded",
            json!({"integrationId": integration_id, "covered": ids}),
        );
        Ok(())
    })();
    if let Err(error) = outcome {
        deps.detail_log(
            "integration_coverage_failed",
            json!({"integrationId": integration_id, "error": error}),
        );
    }
}
