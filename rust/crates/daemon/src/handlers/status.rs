//! The status commands (src/commands/status.ts, plus `ping` of src/daemon.ts): the operator's view of the run.
//!
//! `status` is the largest payload and feeds `cstan-dash`: members are written in the order Node writes them and values
//! are the kernel's JSON as it is. Everything the ledger answers is read in one kernel closure (Node reads it without an
//! await in between); what the driver, the launcher and the Operator service answer is read before it, outside the
//! kernel thread.

use super::shared::{
    describe_full_auto, describe_grant_record, fail, map_kernel_error, ok, CommandCall, CommandEnv,
    CommandResponse, ErrorCode, MAX_STATUS_CLEARS, MAX_STATUS_MESSAGES, MAX_STATUS_PROPOSALS,
    MAX_STATUS_REPORTS, MAX_STATUS_TASK_PACKAGES, MAX_STATUS_TASK_PLANS,
    MAX_STATUS_TASK_REQUIREMENTS,
};
use super::HandlerMap;
use crate::deps::{Deps, DriverSnapshot};
use capstan_config::DEFAULT_PM_STALE_MINUTES;
use capstan_operator::api::ListFilter;
use capstan_wire::js::date_parse_ms;
use serde_json::{json, Map, Value};

/// Registers the handlers of the routes this module serves (status, ping).
pub fn register(map: &mut HandlerMap) {
    map.insert("status", status);
    map.insert("ping", ping);
}

/// `{projectRoot, ledgerPath}` of the project this daemon serves (`options.location` of src/daemon.ts).
pub fn controller_location(deps: &Deps) -> Map<String, Value> {
    let mut location = Map::new();
    location.insert(
        "projectRoot".into(),
        json!(deps.options.workspace_root.to_string_lossy()),
    );
    let ledger = capstan_ledger::resolve_database_path(&deps.options.state_directory)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| {
            deps.options
                .state_directory
                .join("controller.sqlite")
                .to_string_lossy()
                .into_owned()
        });
    location.insert("ledgerPath".into(), json!(ledger));
    location
}

/// `{pid, projectRoot, ledgerPath}` as `ping` and the `controller` member of status carry them.
fn pid_and_location(deps: &Deps) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("pid".into(), json!(std::process::id()));
    map.extend(controller_location(deps));
    map
}

fn ping(env: &CommandEnv<'_>, _call: &CommandCall<'_>) -> Option<CommandResponse> {
    let mut result = Map::new();
    result.insert("pong".into(), json!(true));
    result.extend(pid_and_location(env.deps));
    Some(ok(Value::Object(result)))
}

fn pick(value: &Value, keys: &[&str]) -> Value {
    let mut map = Map::new();
    for key in keys {
        map.insert(
            (*key).to_string(),
            value.get(*key).cloned().unwrap_or(Value::Null),
        );
    }
    Value::Object(map)
}

fn pick_each(values: &Value, keys: &[&str]) -> Value {
    Value::Array(
        values
            .as_array()
            .into_iter()
            .flatten()
            .map(|value| pick(value, keys))
            .collect(),
    )
}

/// `pmMailSummary` of src/pm-mail.ts: how many messages wait for the PM, which is the oldest and whether it waited too long.
pub fn pm_mail_summary(messages: &Value, now_ms: i64, stale_seconds: i64) -> Map<String, Value> {
    let mut pending = 0;
    // (message, queued time; None is a time that does not parse)
    let mut oldest: Option<(&Value, Option<i64>)> = None;
    for message in messages.as_array().into_iter().flatten() {
        if !matches!(
            message["state"].as_str(),
            Some("queued" | "deferred" | "sent" | "unacked")
        ) {
            continue;
        }
        pending += 1;
        let queued = message["queuedAt"].as_str().and_then(date_parse_ms);
        let replace = match &oldest {
            None => true,
            Some((_, Some(current))) => queued.is_some_and(|q| q < *current),
            Some((_, None)) => true,
        };
        if replace {
            oldest = Some((message, queued));
        }
    }
    let mut out = Map::new();
    let Some((message, queued)) = oldest else {
        out.insert("pending".into(), json!(0));
        out.insert("oldestMessageId".into(), Value::Null);
        out.insert("oldestQueuedAt".into(), Value::Null);
        out.insert("oldestAgeSeconds".into(), json!(0));
        out.insert("stale".into(), json!(false));
        return out;
    };
    let age = queued.map_or(0, |queued| ((now_ms - queued).div_euclid(1000)).max(0));
    out.insert("pending".into(), json!(pending));
    out.insert("oldestMessageId".into(), message["messageId"].clone());
    out.insert("oldestQueuedAt".into(), message["queuedAt"].clone());
    out.insert("oldestAgeSeconds".into(), json!(age));
    out.insert("stale".into(), json!(age >= stale_seconds));
    out
}

/// What the operator view takes from outside the ledger.
struct Outside {
    driver: DriverSnapshot,
    /// (full auto, grants, pending proposals), when `[operator]` is enabled and the service runs.
    operator: Option<(Value, Value, Value)>,
    launcher: Option<Value>,
}

fn outside(deps: &Deps) -> Outside {
    let driver = deps.driver.snapshot();
    let operator_on = deps
        .options
        .capstan
        .as_deref()
        .is_some_and(|config| config.operator.enabled);
    let operator = match (&deps.operator, operator_on) {
        (Some(service), true) => Some((
            describe_full_auto(service.full_auto_status()),
            Value::Array(service.grants().iter().map(describe_grant_record).collect()),
            Value::Array(
                service
                    .list(&ListFilter {
                        states: Some(vec!["proposed".to_string()]),
                        proposer_agent_id: None,
                        limit: Some(MAX_STATUS_PROPOSALS),
                    })
                    .iter()
                    .map(|proposal| {
                        json!({
                            "proposalId": proposal["proposalId"],
                            "kind": proposal["kind"],
                            "command": proposal["command"],
                            "proposer": proposal["proposerAgentId"],
                            "reason": proposal["reason"],
                            "createdAt": proposal["createdAt"],
                        })
                    })
                    .collect(),
            ),
        )),
        _ => None,
    };
    Outside {
        driver,
        operator,
        launcher: deps
            .launcher
            .as_ref()
            .map(|launcher| launcher.status().to_value()),
    }
}

fn status(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let deps = env.deps;
    let is_operator = call.identity.role == "operator";
    let is_pm = call
        .identity
        .agent
        .as_ref()
        .is_some_and(|agent| agent.kind == "PM");
    let outside = is_operator.then(|| outside(deps));
    let config = deps.options.capstan.clone();
    let credential = call.credential.to_string();
    let location = pid_and_location(deps);
    let built = deps.kernel.run(move |core| {
        let agents = core.list_agents()?;
        let mut snapshot = match core.status_snapshot()? {
            Value::Object(map) => map,
            other => {
                let mut map = Map::new();
                map.insert("snapshot".into(), other);
                map
            }
        };
        let legacy = snapshot.shift_remove("supervision").unwrap_or(Value::Null);
        let mut result = snapshot;
        result.insert("agents".into(), agents.clone());
        // The live loop's state; `legacySupervision` is the old control row, which nothing enables any more.
        let mut state = Map::new();
        state.insert(
            "enabled".into(),
            json!(config.as_deref().is_some_and(|c| c.supervision.enabled)),
        );
        state.insert(
            "checkSeconds".into(),
            config
                .as_deref()
                .map_or(Value::Null, |c| json!(c.supervision.check_seconds)),
        );
        if let Value::Object(activity) = core.supervision_activity()? {
            state.extend(activity);
        }
        result.insert("supervisionState".into(), Value::Object(state));
        result.insert("legacySupervision".into(), legacy);
        result.insert("controller".into(), Value::Object(location));
        if let Some(outside) = &outside {
            let unresolved = core.unresolved_messages(&credential, MAX_STATUS_MESSAGES as i64)?;
            result.insert(
                "messages".into(),
                pick_each(
                    &unresolved["messages"],
                    &[
                        "messageId",
                        "recipientAgentId",
                        "state",
                        "sequence",
                        "queuedAt",
                        "deferredReason",
                        "stateReason",
                        "lastNotifiedAt",
                    ],
                ),
            );
            result.insert(
                "legacySupervisionReason".into(),
                core.supervision_reason(&credential)?,
            );
            result.insert("messagesTruncated".into(), unresolved["truncated"].clone());
            let pm = agents
                .as_array()
                .into_iter()
                .flatten()
                .find(|agent| agent["kind"] == "PM" && agent["state"] == "active");
            result.insert("pmMail".into(), Value::Null);
            if let Some(pm) = pm {
                let stale_minutes = config.as_deref().map_or(DEFAULT_PM_STALE_MINUTES, |c| {
                    c.notifications.pm_stale_minutes
                });
                let agent_id = pm["agentId"].as_str().unwrap_or("");
                let open = core.open_messages_for(agent_id)?;
                let now = core.kernel().env.now();
                let mut mail = Map::new();
                mail.insert("agentId".into(), pm["agentId"].clone());
                mail.extend(pm_mail_summary(&open, now, stale_minutes * 60));
                mail.insert(
                    "notified".into(),
                    json!(outside
                        .driver
                        .pm_stale
                        .as_ref()
                        .and_then(|stale| stale["notified"].as_bool())
                        .unwrap_or(false)),
                );
                result.insert("pmMail".into(), Value::Object(mail));
            }
            result.insert(
                "stalledAgentIds".into(),
                json!(outside.driver.stalled_agent_ids),
            );
            result.insert(
                "lostAgentIds".into(),
                json!(outside.driver.lost_agent_ids.clone().unwrap_or_default()),
            );
            result.insert(
                "stuck".into(),
                Value::Array(
                    outside
                        .driver
                        .stuck
                        .iter()
                        .map(|(id, reason)| json!({"messageId": id, "reason": reason}))
                        .collect(),
                ),
            );
            result.insert(
                "inputClears".into(),
                core.input_clears(&credential, MAX_STATUS_CLEARS as i64)?,
            );
            if let Some((full_auto, grants, _)) = &outside.operator {
                result.insert(
                    "operator".into(),
                    json!({"fullAuto": full_auto, "grants": grants}),
                );
            }
            result.insert("panes".into(), core.agent_panes(&credential)?);
            result.insert(
                "reviews".into(),
                review_rows(&core.reviews(&credential, Some(MAX_STATUS_REPORTS as i64))?),
            );
            result.insert(
                "integrations".into(),
                integration_rows(&core.integrations(&credential, Some(MAX_STATUS_REPORTS as i64))?),
            );
            result.insert("pipelineCounts".into(), core.pipeline_counts(&credential)?);
            result.insert(
                "activeTasks".into(),
                core.active_tasks(
                    &credential,
                    &json!({
                        "plans": MAX_STATUS_TASK_PLANS,
                        "packages": MAX_STATUS_TASK_PACKAGES,
                        "requirements": MAX_STATUS_TASK_REQUIREMENTS,
                    }),
                )?,
            );
            result.insert(
                "awaitingConfirm".into(),
                core.awaiting_confirm(&credential, Some(MAX_STATUS_REPORTS as i64))?,
            );
            if let Some((_, _, proposals)) = &outside.operator {
                result.insert("pendingProposals".into(), proposals.clone());
            }
            result.insert(
                "agentFindings".into(),
                pick_each(
                    &core.findings(&credential, Some(MAX_STATUS_REPORTS as i64))?,
                    &[
                        "findingId",
                        "targetAgentId",
                        "raisedByAgentId",
                        "severity",
                        "state",
                        "interventions",
                        "stateReason",
                        "createdAt",
                    ],
                ),
            );
            result.insert(
                "reports".into(),
                report_rows(&core.agent_reports(&credential, Some(MAX_STATUS_REPORTS as i64))?),
            );
            if let Some(launcher) = &outside.launcher {
                if let Some(failed) = launcher.get("cleanupFailed") {
                    result.insert("cleanupFailed".into(), failed.clone());
                }
                if let Some(orphans) = launcher.get("orphanPanes") {
                    result.insert("orphanPanes".into(), orphans.clone());
                }
            }
        }
        if config.as_deref().is_some_and(|c| c.prompt_relay.enabled) {
            result.insert("promptRelay".into(), core.prompt_relay_status()?);
        }
        if is_pm {
            result.insert(
                "nexoraDrift".into(),
                pick_each(
                    &core.sync_drift(&credential)?,
                    &["refKind", "refId", "externalId", "syncedState", "wanted"],
                ),
            );
        }
        Ok(Value::Object(result))
    });
    Some(match built {
        Ok(result) => ok(result),
        Err(error) => map_kernel_error(&error),
    })
}

fn review_rows(reviews: &Value) -> Value {
    Value::Array(
        reviews
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| {
                json!({
                    "reviewId": r["reviewId"],
                    "reportId": r["reportId"],
                    "integrationId": r["integrationId"],
                    "round": r["round"],
                    "state": r["state"],
                    "authorAgentId": r["authorAgentId"],
                    "reviewerAgentId": r["reviewerAgentId"],
                    "announced": !r["notifiedMessageId"].is_null(),
                    "createdAt": r["createdAt"],
                })
            })
            .collect(),
    )
}

fn integration_rows(integrations: &Value) -> Value {
    Value::Array(
        integrations
            .as_array()
            .into_iter()
            .flatten()
            .map(|i| {
                let merged = i["state"] == "merged";
                json!({
                    "integrationId": i["integrationId"],
                    "state": i["state"],
                    "branch": if merged { i["branch"].clone() } else { Value::Null },
                    "reports": i["reports"].as_array().into_iter().flatten()
                        .map(|r| r["reportId"].clone()).collect::<Vec<_>>(),
                    "conflictReportId": i["conflictReportId"],
                    "createdAt": i["createdAt"],
                })
            })
            .collect(),
    )
}

fn report_rows(reports: &Value) -> Value {
    Value::Array(
        reports
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| {
                json!({
                    "reportId": r["reportId"],
                    "agentId": r["agentId"],
                    "generation": r["generation"],
                    "commitSha": r["commitSha"],
                    "state": r["state"],
                    "reason": r["reason"],
                    "announced": !r["notifiedMessageId"].is_null(),
                    "createdAt": r["createdAt"],
                })
            })
            .collect(),
    )
}

#[allow(dead_code)]
fn unused(_: ErrorCode) -> CommandResponse {
    fail(ErrorCode::Error, "unused")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(id: &str, state: &str, queued: &str) -> Value {
        json!({"messageId": id, "state": state, "queuedAt": queued})
    }

    #[test]
    fn no_pending_mail_is_a_quiet_summary() {
        let summary = pm_mail_summary(
            &json!([message("a", "acked", "2026-01-01T00:00:00.000Z")]),
            0,
            600,
        );
        assert_eq!(
            Value::Object(summary),
            json!({"pending": 0, "oldestMessageId": null, "oldestQueuedAt": null, "oldestAgeSeconds": 0, "stale": false})
        );
    }

    #[test]
    fn the_oldest_pending_message_sets_the_age_and_staleness() {
        let messages = json!([
            message("late", "queued", "2026-01-01T00:10:00.000Z"),
            message("old", "unacked", "2026-01-01T00:00:00.000Z"),
            message("done", "acked", "2025-01-01T00:00:00.000Z"),
        ]);
        let now = date_parse_ms("2026-01-01T00:15:30.500Z").unwrap();
        let summary = pm_mail_summary(&messages, now, 900);
        assert_eq!(summary["pending"], 2);
        assert_eq!(summary["oldestMessageId"], "old");
        assert_eq!(summary["oldestAgeSeconds"], 930);
        assert_eq!(summary["stale"], true);
        let fresh = pm_mail_summary(&messages, now, 1000);
        assert_eq!(fresh["stale"], false);
    }

    #[test]
    fn a_time_that_does_not_parse_counts_as_pending_with_age_zero() {
        let summary = pm_mail_summary(&json!([message("x", "queued", "not a time")]), 5_000_000, 0);
        assert_eq!(summary["pending"], 1);
        assert_eq!(summary["oldestAgeSeconds"], 0);
        // Age 0 is stale when the limit is 0, as in Node (`ageSeconds >= staleSeconds`).
        assert_eq!(summary["stale"], true);
    }
}
