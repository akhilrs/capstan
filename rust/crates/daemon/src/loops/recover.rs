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
        match deps.git.branch_tip(branch) {
            Ok(tip) if tip.as_deref() != Some(head_sha) => {}
            Ok(_) => match deps.git.delete_branch(branch, head_sha) {
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
            let tip = deps
                .git
                .branch_tip(&branch)
                .map_err(|e| format!("Error: {}", e.message))?;
            if let Some(tip) = tip {
                if !deps
                    .git
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
        let covered = deps
            .git
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
