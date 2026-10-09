//! The plan command (src/commands/plans.ts): packages, tasks and their sync with Nexora.
//!
//! Where Node awaits (git, the launcher), this module leaves the kernel between its ledger steps; the steps with no await
//! between them are one closure on the kernel thread.

use super::shared::{
    fail, is_safe_agent_id, map_error, map_kernel_error, ok, CommandCall, CommandEnv,
    CommandResponse, ErrorCode, HandlerError,
};
use super::HandlerMap;
use crate::deps::new_context;
use crate::reviews::{release_reviewer_later, request_review, RequestError};
use capstan_herdr::api::is_agent_name;
use capstan_kernel::plan_body::parse_plan_body;
use capstan_kernel::records::MAX_REVIEW_ROUNDS;
use capstan_kernel::KernelError;
use serde_json::{json, Map, Value};

/// Registers the handlers of the routes this module serves (plan).
pub fn register(map: &mut HandlerMap) {
    map.insert("plan", plan);
}

/// What the `catch` of the Node handler is handed.
enum Thrown {
    Kernel(KernelError),
    /// A `GitCheckError`.
    Git,
    Handler(HandlerError),
}

impl From<KernelError> for Thrown {
    fn from(error: KernelError) -> Self {
        Self::Kernel(error)
    }
}

type Outcome = Result<CommandResponse, Thrown>;

/// The `catch` of the Node handler.
fn caught(thrown: Thrown) -> CommandResponse {
    match thrown {
        Thrown::Kernel(error) if error.is_controller_error() => fail(
            ErrorCode::Rejected,
            format!("plan_refused: {}", error.message()),
        ),
        Thrown::Kernel(error) => map_kernel_error(&error),
        Thrown::Git => fail(
            ErrorCode::Error,
            "the controller could not run git just now",
        ),
        Thrown::Handler(error) => map_error(&error),
    }
}

fn field(value: &Value, key: &str) -> Value {
    value.get(key).cloned().unwrap_or(Value::Null)
}

fn plan(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let sub = call.args.first().map(String::as_str);
    let rest: &[String] = call.args.get(1..).unwrap_or(&[]);
    if !matches!(
        sub,
        Some("open" | "submit" | "show" | "assign" | "signoff" | "cancel")
    ) {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "plan needs open, submit, show, assign, signoff or cancel",
        ));
    }
    let Some(config) = env.config().filter(|config| config.architect.enabled) else {
        return Some(fail(
            ErrorCode::NotConfigured,
            "plans need [architect] enabled = true in capstan.toml",
        ));
    };
    let outcome = match sub {
        Some("open") => open(env, call, rest),
        Some("cancel") => cancel(env, call, rest),
        Some("assign") => assign(env, call, rest),
        Some("signoff") => signoff(env, call, rest, &config.architect.role),
        Some("submit") => submit(env, call, rest),
        _ => show(env, call, rest),
    };
    Some(outcome.unwrap_or_else(caught))
}

fn open(env: &CommandEnv<'_>, call: &CommandCall<'_>, rest: &[String]) -> Outcome {
    let Some(requested_by) = env.worker_manager(call.identity) else {
        return Ok(fail(
            ErrorCode::Forbidden,
            "only the PM or the operator may open a plan",
        ));
    };
    let (tier, title, supersedes) = (rest.first(), rest.get(1), rest.get(2));
    let (Some(tier), Some(title)) = (tier, title) else {
        return Ok(open_usage());
    };
    if (tier != "normal" && tier != "high-risk") || rest.len() > 3 {
        return Ok(open_usage());
    }
    if supersedes.is_some_and(|id| !is_safe_agent_id(id)) {
        return Ok(fail(
            ErrorCode::InvalidRequest,
            "the superseded plan id is not valid",
        ));
    }
    env.log(
        "plan_open_requested",
        json!({"requestedBy": requested_by, "tier": tier}),
    );
    let mut input = Map::new();
    input.insert(
        "tier".into(),
        json!(if tier == "normal" {
            "normal"
        } else {
            "high_risk"
        }),
    );
    input.insert("title".into(), json!(title));
    if let Some(supersedes) = supersedes {
        input.insert("supersedesPlanId".into(), json!(supersedes));
    }
    let credential = call.credential.to_string();
    let plan = env.deps.kernel.run(move |core| {
        let context = new_context(core, &credential)?;
        core.open_plan(&context, &Value::Object(input))
    })?;
    Ok(ok(json!({
        "planId": field(&plan, "planId"),
        "tier": field(&plan, "tier"),
        "state": field(&plan, "state"),
    })))
}

fn open_usage() -> CommandResponse {
    fail(
        ErrorCode::InvalidRequest,
        "plan open needs normal|high-risk, a title and optionally a superseded plan id",
    )
}

fn cancel(env: &CommandEnv<'_>, call: &CommandCall<'_>, rest: &[String]) -> Outcome {
    if call.identity.role != "operator" {
        return Ok(fail(
            ErrorCode::Forbidden,
            "only the operator may cancel a plan",
        ));
    }
    let usage = || {
        fail(
            ErrorCode::InvalidRequest,
            "plan cancel needs a plan id and optionally a package id",
        )
    };
    let Some(plan_id) = rest.first() else {
        return Ok(usage());
    };
    let package_id = rest.get(1);
    if rest.len() > 2
        || !is_safe_agent_id(plan_id)
        || package_id.is_some_and(|id| !is_safe_agent_id(id))
    {
        return Ok(usage());
    }
    // `{planId, packageId}` with `packageId` undefined: the member is not there.
    let mut requested = Map::new();
    requested.insert("planId".into(), json!(plan_id));
    if let Some(package_id) = package_id {
        requested.insert("packageId".into(), json!(package_id));
    }
    env.log("plan_cancel_requested", Value::Object(requested));
    let mut input = Map::new();
    input.insert("planId".into(), json!(plan_id));
    if let Some(package_id) = package_id {
        input.insert("packageId".into(), json!(package_id));
    }
    let credential = call.credential.to_string();
    let cancelled = env.deps.kernel.run(move |core| {
        let context = new_context(core, &credential)?;
        core.cancel_plan(&context, &Value::Object(input))
    })?;
    if !cancelled["reviewerAgentId"].is_null() && env.deps.launcher.is_some() {
        release_reviewer_later(
            env.deps,
            &json!({
                "reviewId": field(&cancelled, "reviewId"),
                "reviewerAgentId": field(&cancelled, "reviewerAgentId"),
            }),
        );
    }
    Ok(ok(json!({
        "planId": field(&cancelled, "planId"),
        "packageId": field(&cancelled, "packageId"),
        "cancelledPackages": field(&cancelled, "cancelledPackages"),
        "notified": field(&cancelled, "notified"),
    })))
}

fn assign(env: &CommandEnv<'_>, call: &CommandCall<'_>, rest: &[String]) -> Outcome {
    let Some(requested_by) = env.worker_manager(call.identity) else {
        return Ok(fail(
            ErrorCode::Forbidden,
            "only the PM or the operator may assign a package",
        ));
    };
    let usage = || {
        fail(
            ErrorCode::InvalidRequest,
            "plan assign needs a plan id, a package id and an agent id, and takes --early only with a reason: --early \"<reason>\"",
        )
    };
    let (Some(plan_id), Some(package_id), Some(agent_id)) =
        (rest.first(), rest.get(1), rest.get(2))
    else {
        return Ok(usage());
    };
    let tail: &[String] = &rest[3..];
    let with_early = tail.first().map(String::as_str) == Some("--early");
    let early = if with_early { tail.get(1) } else { None };
    let extra = if with_early {
        &tail[tail.len().min(2)..]
    } else {
        tail
    };
    if !extra.is_empty()
        || (with_early && early.is_none())
        || ![plan_id, package_id, agent_id]
            .iter()
            .all(|id| is_safe_agent_id(id))
    {
        return Ok(usage());
    }
    if !is_agent_name(agent_id) {
        return Ok(fail(
            ErrorCode::RecipientNotDeliverable,
            "this agent id cannot be used as a Herdr agent name",
        ));
    }
    env.log(
        "plan_assign_requested",
        json!({
            "requestedBy": requested_by,
            "planId": plan_id,
            "packageId": package_id,
            "agentId": agent_id,
        }),
    );
    let mut input = Map::new();
    input.insert("planId".into(), json!(plan_id));
    input.insert("packageId".into(), json!(package_id));
    input.insert("agentId".into(), json!(agent_id));
    if let Some(early) = early {
        input.insert("early".into(), json!(early));
    }
    let credential = call.credential.to_string();
    let assigned = env.deps.kernel.run(move |core| {
        let context = new_context(core, &credential)?;
        core.assign_package(&context, &Value::Object(input))
    })?;
    let named = env.rename_branch(agent_id, &format!("{plan_id}/{package_id}"));
    let unmet = assigned["unmet"].as_array().cloned().unwrap_or_default();
    let mut result = Map::new();
    result.insert("planId".into(), json!(plan_id));
    result.insert("packageId".into(), field(&assigned, "packageId"));
    result.insert("agentId".into(), json!(agent_id));
    result.insert("messageId".into(), field(&assigned, "assignmentMessageId"));
    result.insert("early".into(), json!(!unmet.is_empty()));
    result.insert("unmet".into(), Value::Array(unmet));
    result.extend(named);
    Ok(ok(Value::Object(result)))
}

/// Whether the caller is the active designated architect.
fn is_active_architect(env: &CommandEnv<'_>, call: &CommandCall<'_>, role: &str) -> bool {
    env.agent_of(call.identity)
        .is_some_and(|c| c.kind == "Developer" && c.state == "active" && c.role_name == role)
}

fn signoff(env: &CommandEnv<'_>, call: &CommandCall<'_>, rest: &[String], role: &str) -> Outcome {
    if !is_active_architect(env, call, role) {
        return Ok(fail(
            ErrorCode::Forbidden,
            "not_architect: only the designated architect may sign off a plan",
        ));
    }
    let caller_id = env
        .agent_of(call.identity)
        .map(|c| c.agent_id.clone())
        .unwrap_or_default();
    let (Some(plan_id), Some(integration_id), Some(summary)) =
        (rest.first(), rest.get(1), rest.get(2))
    else {
        return Ok(signoff_usage());
    };
    if rest.len() > 3 || !is_safe_agent_id(plan_id) || !is_safe_agent_id(integration_id) {
        return Ok(signoff_usage());
    }
    env.log(
        "plan_signoff_requested",
        json!({
            "requestedBy": caller_id,
            "planId": plan_id,
            "integrationId": integration_id,
        }),
    );
    let input = json!({"planId": plan_id, "integrationId": integration_id, "summary": summary});
    let credential = call.credential.to_string();
    let signed = env.deps.kernel.run(move |core| {
        let context = new_context(core, &credential)?;
        core.record_signoff(&context, &input)
    })?;
    Ok(ok(json!({
        "planId": plan_id,
        "integrationId": field(&signed, "integrationId"),
        "signedAt": field(&signed, "createdAt"),
    })))
}

fn signoff_usage() -> CommandResponse {
    fail(
        ErrorCode::InvalidRequest,
        "plan signoff needs a plan id, an integration id and a summary",
    )
}

fn submit(env: &CommandEnv<'_>, call: &CommandCall<'_>, rest: &[String]) -> Outcome {
    let Some(config) = env.config() else {
        return Ok(fail(
            ErrorCode::NotConfigured,
            "plans need [architect] enabled = true in capstan.toml",
        ));
    };
    if !is_active_architect(env, call, &config.architect.role) {
        return Ok(fail(
            ErrorCode::Forbidden,
            "not_architect: only the designated architect may submit a plan",
        ));
    }
    let caller_id = env
        .agent_of(call.identity)
        .map(|c| c.agent_id.clone())
        .unwrap_or_default();
    if rest.len() != 2 || !is_safe_agent_id(&rest[0]) {
        return Ok(fail(
            ErrorCode::InvalidRequest,
            "plan submit needs a plan id and the plan JSON",
        ));
    }
    let (plan_id, body_text) = (rest[0].clone(), rest[1].clone());
    let credential = call.credential.to_string();
    let existing = {
        let (credential, plan_id) = (credential.clone(), plan_id.clone());
        env.deps
            .kernel
            .run(move |core| core.plan_record(&credential, &plan_id))?
    };
    if existing.is_null() {
        return Ok(fail(
            ErrorCode::Rejected,
            format!("unknown_plan: no plan {plan_id}"),
        ));
    }
    let record = &existing["plan"];
    let state = record["state"].as_str().unwrap_or("");
    if state != "draft" {
        return Ok(fail(
            ErrorCode::Rejected,
            format!("plan_not_open: plan {plan_id} is {state}, not a draft"),
        ));
    }
    let plan_review = config.architect.plan_review.as_str();
    let needs_review =
        (record["tier"] == "high_risk" && plan_review != "never") || plan_review == "always";
    if needs_review {
        let rounds = {
            let (credential, plan_id) = (credential.clone(), plan_id.clone());
            env.deps
                .kernel
                .run(move |core| core.plan_review_rounds(&credential, &plan_id))?
        };
        if rounds.as_i64().unwrap_or(0) >= MAX_REVIEW_ROUNDS {
            return Ok(fail(
                ErrorCode::Rejected,
                format!(
                    "review_limit: plan {plan_id} used {MAX_REVIEW_ROUNDS} review rounds; the PM has been told"
                ),
            ));
        }
        if env.deps.launcher.is_none() {
            return Ok(fail(
                ErrorCode::NotConfigured,
                "a plan review needs Herdr and a git repository",
            ));
        }
    }
    let parsed = match parse_plan_body(&body_text, config.architect.max_packages.max(0) as usize) {
        Ok(parsed) => parsed,
        Err(error) => {
            return Ok(fail(
                ErrorCode::Rejected,
                format!("invalid_plan: {}", error.reason),
            ))
        }
    };
    let base_sha = env.deps.git.head_commit().map_err(|_| Thrown::Git)?;
    env.log(
        "plan_submit_requested",
        json!({"requestedBy": caller_id, "planId": plan_id}),
    );
    let body_json = serde_json::to_string(&parsed).map_err(KernelError::from)?;
    let stored = {
        let (credential, plan_id) = (credential.clone(), plan_id.clone());
        env.deps.kernel.run(move |core| {
            let context = new_context(core, &credential)?;
            core.submit_plan(
                &context,
                &json!({
                    "planId": plan_id,
                    "bodyJson": body_json,
                    "baseSha": base_sha,
                    "review": needs_review,
                }),
            )
        })?
    };
    if !needs_review {
        return Ok(ok(json!({
            "planId": field(&stored, "planId"),
            "revision": field(&stored, "currentRevision"),
            "state": field(&stored, "state"),
        })));
    }
    let requested_role = config.architect.reviewer_role.as_deref();
    match request_review(env.deps, &plan_id, requested_role, &credential) {
        Ok((review, spawn_state)) => Ok(ok(json!({
            "planId": field(&stored, "planId"),
            "revision": field(&stored, "currentRevision"),
            "state": "in_review",
            "reviewId": field(&review, "reviewId"),
            "reviewerAgentId": field(&review, "reviewerAgentId"),
            "reviewerState": spawn_state,
        }))),
        Err(error) => {
            let reason = match &error {
                RequestError::Review(refused) => refused.message.clone(),
                RequestError::Kernel(kernel) if kernel.is_controller_error() => kernel.message(),
                _ => "the reviewer could not be started".to_string(),
            };
            let abandoned = {
                let (credential, plan_id) = (credential.clone(), plan_id.clone());
                env.deps.kernel.run(move |core| {
                    let context = new_context(core, &credential)?;
                    core.abandon_plan_review(
                        &context,
                        &json!({"planId": plan_id, "reason": reason}),
                    )
                })
            };
            if let Err(cleanup) = abandoned {
                env.log(
                    "plan_review_abandon_failed",
                    json!({"planId": plan_id, "error": HandlerError::from(cleanup).to_string()}),
                );
            }
            match error {
                RequestError::Review(refused) => Ok(fail(
                    ErrorCode::Rejected,
                    format!("{}: {}", refused.code, refused.message),
                )),
                RequestError::Kernel(kernel) if kernel.is_controller_error() => Ok(fail(
                    ErrorCode::Rejected,
                    format!("review_refused: {}", kernel.message()),
                )),
                RequestError::Kernel(kernel) => Err(Thrown::Kernel(kernel)),
                RequestError::Launcher(launcher) => Err(Thrown::Handler(launcher.into())),
            }
        }
    }
}

/// `linkFields`: what the Nexora links add to a plan or package.
fn link_fields(links: &[Value], kind: &str, id: &str) -> Map<String, Value> {
    let mut fields = Map::new();
    if let Some(link) = links
        .iter()
        .find(|l| l["refKind"] == kind && l["refId"] == id)
    {
        for (key, source) in [
            ("externalId", "externalId"),
            ("syncedState", "syncedState"),
            ("wanted", "wanted"),
            ("drift", "drift"),
        ] {
            fields.insert(key.into(), field(link, source));
        }
    }
    fields
}

fn show(env: &CommandEnv<'_>, call: &CommandCall<'_>, rest: &[String]) -> Outcome {
    if rest.len() > 1 || rest.first().is_some_and(|id| !is_safe_agent_id(id)) {
        return Ok(fail(
            ErrorCode::InvalidRequest,
            "plan show takes an optional plan id",
        ));
    }
    let credential = call.credential.to_string();
    let links: Vec<Value> = {
        let credential = credential.clone();
        env.deps
            .kernel
            .run(move |core| core.external_links(&credential))?
            .as_array()
            .cloned()
            .unwrap_or_default()
    };
    let Some(plan_id) = rest.first() else {
        let plans = env
            .deps
            .kernel
            .run(move |core| core.list_plans(&credential))?;
        let shown: Vec<Value> = plans
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| {
                let mut map = Map::new();
                for key in ["planId", "tier", "state", "title"] {
                    map.insert(key.into(), field(p, key));
                }
                map.extend(link_fields(
                    &links,
                    "plan",
                    p["planId"].as_str().unwrap_or(""),
                ));
                Value::Object(map)
            })
            .collect();
        return Ok(ok(json!({ "plans": shown })));
    };
    let detail = {
        let id = plan_id.clone();
        env.deps
            .kernel
            .run(move |core| core.plan_record(&credential, &id))?
    };
    if detail.is_null() {
        return Ok(fail(
            ErrorCode::Rejected,
            format!("unknown_plan: no plan {plan_id}"),
        ));
    }
    let revision = if detail["revision"].is_null() {
        Value::Null
    } else {
        let revision = &detail["revision"];
        let body: Value = serde_json::from_str(revision["bodyJson"].as_str().unwrap_or("null"))
            .map_err(KernelError::from)?;
        json!({
            "revision": field(revision, "revision"),
            "baseSha": field(revision, "baseSha"),
            "authorAgentId": field(revision, "authorAgentId"),
            "createdAt": field(revision, "createdAt"),
            "body": body,
        })
    };
    let mut result = Map::new();
    result.insert("plan".into(), field(&detail, "plan"));
    result.insert("revision".into(), revision);
    if links
        .iter()
        .any(|l| l["refKind"] == "plan" && l["refId"] == plan_id.as_str())
    {
        result.insert(
            "nexora".into(),
            Value::Object(link_fields(&links, "plan", plan_id)),
        );
    }
    let packages: Vec<Value> = detail["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|package| {
            let mut map = package.as_object().cloned().unwrap_or_default();
            let id = format!("{plan_id}/{}", package["packageId"].as_str().unwrap_or(""));
            map.extend(link_fields(&links, "package", &id));
            Value::Object(map)
        })
        .collect();
    result.insert("packages".into(), Value::Array(packages));
    result.insert("signoffs".into(), field(&detail, "signoffs"));
    Ok(ok(Value::Object(result)))
}
