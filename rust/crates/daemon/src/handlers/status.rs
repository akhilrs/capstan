//! The status commands (src/commands/status.ts, plus `ping` of src/daemon.ts): the operator's view of the run.
//!
//! `status` is the largest payload and feeds `cstan-dash`: members are written in the order Node writes them and values
//! are the kernel's JSON as it is. Everything the ledger answers is read in one kernel closure (Node reads it without an
//! await in between); what the driver, the launcher and the Operator service answer is read before it, outside the
//! kernel thread.

use super::shared::{
    describe_full_auto, describe_grant_record, describe_window, fail, map_kernel_error, ok,
    CommandCall, CommandEnv, CommandResponse, ErrorCode, MAX_STATUS_CLEARS, MAX_STATUS_MESSAGES,
    MAX_STATUS_PROPOSALS, MAX_STATUS_REPORTS, MAX_STATUS_TASK_PACKAGES, MAX_STATUS_TASK_PLANS,
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
    map.insert("about", about);
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

/// `about`: what `cstan controllers` shows of a controller (Rust-only): the process, build, project and how many agents are
/// active. It only reads.
fn about(env: &CommandEnv<'_>, _call: &CommandCall<'_>) -> Option<CommandResponse> {
    let mut result = pid_and_location(env.deps);
    result.insert("version".into(), json!(crate::VERSION));
    let counted = env.deps.kernel.run(|core| {
        let agents = core.list_agents()?;
        Ok(agents
            .as_array()
            .into_iter()
            .flatten()
            .filter(|agent| agent["state"] == "active")
            .count())
    });
    Some(match counted {
        Ok(count) => {
            result.insert("activeAgents".into(), json!(count));
            ok(Value::Object(result))
        }
        Err(error) => map_kernel_error(&error),
    })
}

/// The old control row is created disabled and `degraded`, and nothing enables it, so its health is not applicable
/// rather than a fault: a disabled row reports `disabled`. An enabled row keeps its stored health.
pub fn disabled_legacy_health(mut legacy: Value) -> Value {
    if legacy["enabled"] == false && legacy["health"] == "degraded" {
        legacy["health"] = json!("disabled");
    }
    legacy
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
pub struct Outside {
    pub driver: DriverSnapshot,
    /// (full auto, grants, pending proposals, open window), when `[operator]` is enabled and the service runs.
    pub operator: Option<(Value, Value, Value, Value)>,
    pub launcher: Option<Value>,
}

pub fn outside(deps: &Deps) -> Outside {
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
            service
                .window()
                .as_ref()
                .map_or(Value::Null, describe_window),
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

/// How a status input stays current in the cache: `Keyed` inputs only change together with the cache key (the project's
/// `state_version`, the caller, the driver's `cache_key`, the configuration), `Ttl` inputs can change without any of
/// them, so a cached answer shows them for at most `[daemon] status_cache_ms`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputClass {
    Keyed,
    Ttl,
}

/// Everything one status answer is made of, read before anything is rendered. Every field is listed in
/// `StatusInputs::classes`, which destructures the struct without `..`, so a new input does not compile until it is
/// classified.
///
/// Writer audit (changes to a status input without a `state_version` bump; each input they feed is `Ttl`):
/// - `Kernel::touch_agent` (`agents.last_activity_at`, every authenticated call) feeds `agents`;
/// - `agent_state_history` inserts, `agent_panes` / `orphan_panes` / `fallback_panes` writes feed `agents`, `panes`,
///   `cleanup_failed` and `orphan_panes`;
/// - `supervision_checks`, `pm_stuck_episodes` and `message_input_clears` rows feed `supervision_activity`,
///   `supervision_reason`, `input_clears` and `messages`;
/// - `prompt_relays` updates feed `prompt_relay`;
/// - ledger retention (`mutation_requests`) and maintenance (vacuum, checkpoint) change no status member, but a pass that
///   deletes rows is TTL-bound too: nothing keyed on `state_version` sees it;
/// - verify runs outside the ledger (process output, the worker's claim) feed `verify`;
/// - ntfy and cache status, the Operator service (grants, proposals, window), the launcher and the clock (`now`) are not
///   in the ledger at all.
pub struct StatusInputs {
    pub is_operator: bool,
    pub is_pm: bool,
    pub config: Option<std::sync::Arc<capstan_config::RoleConfig>>,
    pub agents: Value,
    pub snapshot: Value,
    pub supervision_activity: Value,
    pub location: Map<String, Value>,
    pub now: i64,
    pub messages: Option<Value>,
    pub supervision_reason: Option<Value>,
    pub pm_open_messages: Option<Value>,
    pub driver: Option<DriverSnapshot>,
    pub input_clears: Option<Value>,
    pub operator_service: Option<(Value, Value, Value, Value)>,
    pub panes: Option<Value>,
    pub reviews: Option<Value>,
    pub integrations: Option<Value>,
    pub pipeline_counts: Option<Value>,
    pub active_tasks: Option<Value>,
    pub awaiting_confirm: Option<Value>,
    pub findings: Option<Value>,
    pub reports: Option<Value>,
    pub launcher: Option<Value>,
    pub prompt_relay: Option<Value>,
    pub sync_drift: Option<Value>,
    pub cache: Option<Value>,
    pub verify: Option<Value>,
    pub ntfy: Option<Value>,
}

impl StatusInputs {
    /// The class of every field, by name.
    pub fn classes(&self) -> Vec<(&'static str, InputClass)> {
        use InputClass::{Keyed, Ttl};
        let StatusInputs {
            is_operator: _,
            is_pm: _,
            config: _,
            agents: _,
            snapshot: _,
            supervision_activity: _,
            location: _,
            now: _,
            messages: _,
            supervision_reason: _,
            pm_open_messages: _,
            driver: _,
            input_clears: _,
            operator_service: _,
            panes: _,
            reviews: _,
            integrations: _,
            pipeline_counts: _,
            active_tasks: _,
            awaiting_confirm: _,
            findings: _,
            reports: _,
            launcher: _,
            prompt_relay: _,
            sync_drift: _,
            cache: _,
            verify: _,
            ntfy: _,
        } = self;
        vec![
            // The caller class and the configuration are part of the cache key.
            ("is_operator", Keyed),
            ("is_pm", Keyed),
            ("config", Keyed),
            ("agents", Ttl),
            ("snapshot", Keyed),
            ("supervision_activity", Ttl),
            ("location", Ttl),
            ("now", Ttl),
            ("messages", Ttl),
            ("supervision_reason", Ttl),
            ("pm_open_messages", Ttl),
            // Keyed by `DriverSnapshot::cache_key`.
            ("driver", Keyed),
            ("input_clears", Ttl),
            ("operator_service", Ttl),
            ("panes", Ttl),
            ("reviews", Keyed),
            ("integrations", Keyed),
            ("pipeline_counts", Keyed),
            ("active_tasks", Keyed),
            ("awaiting_confirm", Keyed),
            ("findings", Keyed),
            ("reports", Keyed),
            ("launcher", Ttl),
            ("prompt_relay", Ttl),
            ("sync_drift", Keyed),
            ("cache", Ttl),
            ("verify", Ttl),
            ("ntfy", Ttl),
        ]
    }
}

/// What a cached status answer is stored under: the project's `state_version`, who asks (the operator, or one actor), the
/// driver's `cache_key` and whether a verify service runs. The configuration is fixed for the daemon's life.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StatusKey {
    pub state_version: i64,
    pub caller: String,
    pub driver: u64,
    pub with_verify: bool,
}

/// The most answers kept.
pub const STATUS_CACHE_ENTRIES: usize = 64;

/// Reads every input in the kernel's closure, in the order Node reads them.
fn gather(
    core: &capstan_kernel::Core,
    mut inputs: StatusInputs,
    credential: &str,
) -> capstan_kernel::KernelResult<StatusInputs> {
    inputs.agents = core.list_agents()?;
    inputs.snapshot = core.status_snapshot()?;
    inputs.supervision_activity = core.supervision_activity()?;
    if inputs.is_operator {
        inputs.messages = Some(core.unresolved_messages(credential, MAX_STATUS_MESSAGES as i64)?);
        inputs.supervision_reason = Some(core.supervision_reason(credential)?);
        let pm = inputs
            .agents
            .as_array()
            .into_iter()
            .flatten()
            .find(|agent| agent["kind"] == "PM" && agent["state"] == "active");
        if let Some(pm) = pm {
            let agent_id = pm["agentId"].as_str().unwrap_or("");
            inputs.pm_open_messages = Some(core.open_messages_for(agent_id)?);
            inputs.now = core.kernel().env.now();
        }
        inputs.input_clears = Some(core.input_clears(credential, MAX_STATUS_CLEARS as i64)?);
        inputs.panes = Some(core.agent_panes(credential)?);
        inputs.reviews = Some(core.reviews(credential, Some(MAX_STATUS_REPORTS as i64))?);
        inputs.integrations = Some(core.integrations(credential, Some(MAX_STATUS_REPORTS as i64))?);
        inputs.pipeline_counts = Some(core.pipeline_counts(credential)?);
        inputs.active_tasks = Some(core.active_tasks(
            credential,
            &json!({
                "plans": MAX_STATUS_TASK_PLANS,
                "packages": MAX_STATUS_TASK_PACKAGES,
                "requirements": MAX_STATUS_TASK_REQUIREMENTS,
            }),
        )?);
        inputs.awaiting_confirm =
            Some(core.awaiting_confirm(credential, Some(MAX_STATUS_REPORTS as i64))?);
        inputs.findings = Some(core.findings(credential, Some(MAX_STATUS_REPORTS as i64))?);
        inputs.reports = Some(core.agent_reports(credential, Some(MAX_STATUS_REPORTS as i64))?);
    }
    if inputs
        .config
        .as_deref()
        .is_some_and(|c| c.prompt_relay.enabled)
    {
        inputs.prompt_relay = Some(core.prompt_relay_status()?);
    }
    if inputs.is_pm {
        inputs.sync_drift = Some(core.sync_drift(credential)?);
    }
    if inputs.verify.is_some() {
        inputs.verify = Some(core.verify_status()?);
    }
    Ok(inputs)
}

/// The status answer from its inputs: no reads, no clock.
pub fn render(inputs: StatusInputs) -> Value {
    let StatusInputs {
        is_operator,
        is_pm,
        config,
        agents,
        snapshot,
        supervision_activity,
        location,
        now,
        messages,
        supervision_reason,
        pm_open_messages,
        driver,
        input_clears,
        operator_service,
        panes,
        reviews,
        integrations,
        pipeline_counts,
        active_tasks,
        awaiting_confirm,
        findings,
        reports,
        launcher,
        prompt_relay,
        sync_drift,
        cache,
        verify,
        ntfy,
    } = inputs;
    let mut snapshot = match snapshot {
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
    if let Value::Object(activity) = supervision_activity {
        state.extend(activity);
    }
    result.insert("supervisionState".into(), Value::Object(state));
    result.insert("legacySupervision".into(), disabled_legacy_health(legacy));
    result.insert("controller".into(), Value::Object(location));
    if is_operator {
        let unresolved = messages.unwrap_or(Value::Null);
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
        let reason = supervision_reason.unwrap_or(Value::Null);
        let disabled = result["legacySupervision"]["health"] == "disabled";
        result.insert(
            "legacySupervisionReason".into(),
            if disabled { Value::Null } else { reason },
        );
        result.insert("messagesTruncated".into(), unresolved["truncated"].clone());
        let driver = driver.unwrap_or_default();
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
            let open = pm_open_messages.unwrap_or(Value::Null);
            let mut mail = Map::new();
            mail.insert("agentId".into(), pm["agentId"].clone());
            mail.extend(pm_mail_summary(&open, now, stale_minutes * 60));
            mail.insert(
                "notified".into(),
                json!(driver
                    .pm_stale
                    .as_ref()
                    .and_then(|stale| stale["notified"].as_bool())
                    .unwrap_or(false)),
            );
            result.insert("pmMail".into(), Value::Object(mail));
        }
        result.insert("stalledAgentIds".into(), json!(driver.stalled_agent_ids));
        result.insert(
            "lostAgentIds".into(),
            json!(driver.lost_agent_ids.clone().unwrap_or_default()),
        );
        result.insert(
            "stuck".into(),
            Value::Array(
                driver
                    .stuck
                    .iter()
                    .map(|(id, reason)| json!({"messageId": id, "reason": reason}))
                    .collect(),
            ),
        );
        result.insert("inputClears".into(), input_clears.unwrap_or(Value::Null));
        if let Some((full_auto, grants, _, window)) = &operator_service {
            let mut operator = json!({"fullAuto": full_auto, "grants": grants});
            if !window.is_null() {
                operator["window"] = window.clone();
            }
            result.insert("operator".into(), operator);
        }
        result.insert("panes".into(), panes.unwrap_or(Value::Null));
        result.insert(
            "reviews".into(),
            review_rows(&reviews.unwrap_or(Value::Null)),
        );
        result.insert(
            "integrations".into(),
            integration_rows(&integrations.unwrap_or(Value::Null)),
        );
        result.insert(
            "pipelineCounts".into(),
            pipeline_counts.unwrap_or(Value::Null),
        );
        result.insert("activeTasks".into(), active_tasks.unwrap_or(Value::Null));
        result.insert(
            "awaitingConfirm".into(),
            awaiting_confirm.unwrap_or(Value::Null),
        );
        if let Some((_, _, proposals, _)) = &operator_service {
            result.insert("pendingProposals".into(), proposals.clone());
        }
        result.insert(
            "agentFindings".into(),
            pick_each(
                &findings.unwrap_or(Value::Null),
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
            report_rows(&reports.unwrap_or(Value::Null)),
        );
        if let Some(launcher) = &launcher {
            if let Some(failed) = launcher.get("cleanupFailed") {
                result.insert("cleanupFailed".into(), failed.clone());
            }
            if let Some(orphans) = launcher.get("orphanPanes") {
                result.insert("orphanPanes".into(), orphans.clone());
            }
        }
    }
    if let Some(relay) = prompt_relay {
        result.insert("promptRelay".into(), relay);
    }
    if is_pm {
        result.insert(
            "nexoraDrift".into(),
            pick_each(
                &sync_drift.unwrap_or(Value::Null),
                &["refKind", "refId", "externalId", "syncedState", "wanted"],
            ),
        );
    }
    if let Some(cache) = cache {
        result.insert("cache".into(), cache);
    }
    if let Some(verify) = verify {
        result.insert("verify".into(), verify);
    }
    if let Some(ntfy) = ntfy {
        result.insert("ntfy".into(), ntfy);
    }
    Value::Object(result)
}

fn status(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let deps = env.deps;
    let is_operator = call.identity.role == "operator";
    let is_pm = call
        .identity
        .agent
        .as_ref()
        .is_some_and(|agent| agent.kind == "PM");
    let ttl_ms = deps
        .options
        .capstan
        .as_deref()
        .map_or(0, |config| config.daemon.status_cache_ms);
    // What the driver shows is read first (outside the kernel thread); it is part of the key.
    let outside = is_operator.then(|| outside(deps));
    let key = (ttl_ms > 0).then(|| {
        (
            call.identity.role.clone(),
            call.identity.actor_id.clone(),
            outside.as_ref().map_or(0, |o| o.driver.cache_key()),
        )
    });
    let cache_key = match key {
        Some((role, actor, driver)) => {
            // One kernel job reads both: the project's `state_version` (one statement) and the clock.
            let read = deps
                .kernel
                .run(|core| Ok((core.state_version()?, core.kernel().env.now())));
            let (version, now) = match read {
                Ok(read) => read,
                Err(error) => return Some(map_kernel_error(&error)),
            };
            let key = StatusKey {
                state_version: version,
                caller: if is_operator {
                    "operator".to_string()
                } else {
                    format!("{role}:{actor}")
                },
                driver,
                with_verify: deps.verify.is_some(),
            };
            let hit = {
                let mut guard = env
                    .state
                    .status_cache
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                match guard.as_mut() {
                    Some(cache) if cache.ttl_ms() == ttl_ms => cache.get(&key, now),
                    _ => None,
                }
            };
            if let Some(hit) = hit {
                return Some(ok(hit));
            }
            Some((key, now))
        }
        None => None,
    };
    let config = deps.options.capstan.clone();
    let credential = call.credential.to_string();
    let inputs = StatusInputs {
        is_operator,
        is_pm,
        config,
        agents: Value::Null,
        snapshot: Value::Null,
        supervision_activity: Value::Null,
        location: pid_and_location(deps),
        now: 0,
        messages: None,
        supervision_reason: None,
        pm_open_messages: None,
        driver: outside.as_ref().map(|o| o.driver.clone()),
        input_clears: None,
        operator_service: outside.as_ref().and_then(|o| o.operator.clone()),
        panes: None,
        reviews: None,
        integrations: None,
        pipeline_counts: None,
        active_tasks: None,
        awaiting_confirm: None,
        findings: None,
        reports: None,
        launcher: outside.as_ref().and_then(|o| o.launcher.clone()),
        prompt_relay: None,
        sync_drift: None,
        cache: super::cache::runtime_of_deps(deps)
            .map(|runtime| super::cache::status_member(&runtime)),
        // `Some(Null)` marks that a verify service runs; `gather` fills it.
        verify: deps.verify.is_some().then_some(Value::Null),
        ntfy: env.deps.ntfy().map(|sender| sender.status()),
    };
    let built = deps
        .kernel
        .run(move |core| gather(core, inputs, &credential).map(render));
    Some(match built {
        Ok(result) => {
            if let Some((key, now)) = cache_key {
                let mut guard = env
                    .state
                    .status_cache
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                if guard.as_ref().is_none_or(|cache| cache.ttl_ms() != ttl_ms) {
                    *guard = Some(super::shared::TtlLru::new(STATUS_CACHE_ENTRIES, ttl_ms));
                }
                if let Some(cache) = guard.as_mut() {
                    cache.insert(key, result.clone(), now);
                }
            }
            ok(result)
        }
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
    fn a_disabled_legacy_row_is_not_applicable_and_an_enabled_one_keeps_its_health() {
        let row = |enabled: bool, health: &str| json!({"enabled": enabled, "health": health});
        assert_eq!(
            disabled_legacy_health(row(false, "degraded")),
            row(false, "disabled")
        );
        assert_eq!(
            disabled_legacy_health(row(true, "degraded")),
            row(true, "degraded")
        );
        assert_eq!(
            disabled_legacy_health(row(false, "healthy")),
            row(false, "healthy")
        );
        assert_eq!(disabled_legacy_health(Value::Null), Value::Null);
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
