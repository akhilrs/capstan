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
use crate::reviews::{release_reviewer_later, request_review, RequestError, Requested};
use capstan_herdr::api::is_agent_name;
use capstan_kernel::helpers::amended_plan_body_json;
use capstan_kernel::plan_body::parse_plan_body;
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
        Some(
            "open"
                | "submit"
                | "amend"
                | "show"
                | "model"
                | "assign"
                | "reassign"
                | "signoff"
                | "cancel"
        )
    ) {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "plan needs open, submit, show, assign, reassign, signoff or cancel",
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
        Some("model") => plan_model(env, call, rest),
        Some("assign") => assign(env, call, rest),
        Some("reassign") => reassign(env, call, rest, &config.architect.role),
        Some("signoff") => signoff(env, call, rest, &config.architect.role),
        Some("submit") => submit(env, call, rest),
        Some("amend") => amend(env, call, rest),
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
    let (rest, model_flags) = match take_model_flags(rest) {
        Ok(taken) => taken,
        Err(refusal) => return Ok(refusal),
    };
    let rest = rest.as_slice();
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
    // Every value is checked before anything is written; the plan and its rows are one mutation.
    let (overrides, warnings) = match checked_overrides(env, &model_flags) {
        Ok(checked) => checked,
        Err(refusal) => return Ok(refusal),
    };
    env.log(
        "plan_open_requested",
        json!({"requestedBy": requested_by, "tier": tier}),
    );
    let mut input = Map::new();
    if !overrides.is_empty() {
        input.insert("modelOverrides".into(), Value::Array(overrides.clone()));
    }
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
    if let Some(limit) = env
        .config()
        .map(|config| config.architect.max_review_rounds)
    {
        input.insert("maxReviewRounds".into(), json!(limit));
    }
    let credential = call.credential.to_string();
    let plan = env.deps.kernel.run(move |core| {
        let context = new_context(core, &credential)?;
        core.open_plan(&context, &Value::Object(input))
    })?;
    let mut result = Map::new();
    result.insert("planId".into(), field(&plan, "planId"));
    result.insert("tier".into(), field(&plan, "tier"));
    result.insert("state".into(), field(&plan, "state"));
    if !overrides.is_empty() {
        result.insert("modelOverrides".into(), Value::Array(overrides));
    }
    if !warnings.is_empty() {
        result.insert("warnings".into(), json!(warnings));
    }
    Ok(ok(Value::Object(result)))
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
    release_warm_later(env, &cancelled);
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
    let require_design_approval = env
        .config()
        .is_none_or(|config| config.design.require_approval);
    if let Some(early) = early {
        input.insert("early".into(), json!(early));
    }
    let credential = call.credential.to_string();
    let design_plan_id = plan_id.clone();
    let assigned = env.deps.kernel.run(move |core| {
        let context = new_context(core, &credential)?;
        // The gate matters only for a plan with a design package; a plan without one is assigned with the input it always had.
        if require_design_approval {
            let designs = core.design_approvals(&design_plan_id, None)?;
            if designs["packages"]
                .as_array()
                .is_some_and(|p| !p.is_empty())
            {
                input.insert("requireDesignApproval".into(), json!(true));
            }
        }
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

fn reassign(env: &CommandEnv<'_>, call: &CommandCall<'_>, rest: &[String], role: &str) -> Outcome {
    let Some(requested_by) = env.worker_manager(call.identity) else {
        return Ok(fail(
            ErrorCode::Forbidden,
            "not_manager: only the PM or the operator may reassign a plan",
        ));
    };
    let usage = || {
        fail(
            ErrorCode::InvalidRequest,
            "plan reassign needs a plan id and an architect agent id, and takes --reason only with a text: plan reassign <plan-id> <architect-agent-id> [--reason \"<text>\"]",
        )
    };
    let (Some(plan_id), Some(agent_id)) = (rest.first(), rest.get(1)) else {
        return Ok(usage());
    };
    let tail: &[String] = &rest[2..];
    let reason = match tail {
        [] => None,
        [flag, text] if flag == "--reason" => Some(text),
        _ => return Ok(usage()),
    };
    if ![plan_id, agent_id].iter().all(|id| is_safe_agent_id(id)) {
        return Ok(usage());
    }
    env.log(
        "plan_reassign_requested",
        json!({
            "requestedBy": requested_by,
            "planId": plan_id,
            "agentId": agent_id,
        }),
    );
    let mut input = Map::new();
    input.insert("planId".into(), json!(plan_id));
    input.insert("agentId".into(), json!(agent_id));
    input.insert("architectRole".into(), json!(role));
    if let Some(reason) = reason {
        input.insert("reason".into(), json!(reason));
    }
    let credential = call.credential.to_string();
    let done = env.deps.kernel.run(move |core| {
        let context = new_context(core, &credential)?;
        core.reassign_plan(&context, &Value::Object(input))
    })?;
    let mut result = Map::new();
    for key in [
        "planId",
        "fromAgentId",
        "toAgentId",
        "state",
        "currentRevision",
        "reassignedAt",
        "handoverMessageId",
        "notifiedDevelopers",
    ] {
        result.insert(key.into(), field(&done, key));
    }
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
    release_warm_later(env, &signed);
    Ok(ok(json!({
        "planId": plan_id,
        "integrationId": field(&signed, "integrationId"),
        "signedAt": field(&signed, "createdAt"),
    })))
}

/// Ends the warm reviewers a signoff or a cancel released in the ledger.
fn release_warm_later(env: &CommandEnv<'_>, answer: &Value) {
    for agent_id in answer["releasedReviewerAgentIds"]
        .as_array()
        .into_iter()
        .flatten()
    {
        release_reviewer_later(env.deps, &json!({"reviewerAgentId": agent_id}));
    }
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
        if let Some(refusal) = review_limit_refusal(env, &credential, &plan_id)? {
            return Ok(refusal);
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
    start_review(env, &config.architect, &plan_id, &credential, &stored)
}

/// The refusal of a plan that has used its review rounds, or `None` while it has some left.
fn review_limit_refusal(
    env: &CommandEnv<'_>,
    credential: &str,
    plan_id: &str,
) -> Result<Option<CommandResponse>, Thrown> {
    let status = {
        let (credential, plan_id) = (credential.to_string(), plan_id.to_string());
        env.deps
            .kernel
            .run(move |core| core.plan_review_status(&credential, &plan_id))?
    };
    if status["exhausted"].as_bool() != Some(true) {
        return Ok(None);
    }
    Ok(Some(fail(
        ErrorCode::Rejected,
        format!(
            "review_limit: plan {plan_id} used {} of {} design review rounds ({} reviews in all); the PM has been told",
            status["designRounds"], status["limit"], status["totalRounds"]
        ),
    )))
}

/// Starts the review of a plan that went to `in_review` (submit, or amend of a plan that needs review) and answers the
/// command; a reviewer that cannot start sends the plan back to draft.
fn start_review(
    env: &CommandEnv<'_>,
    architect: &capstan_config::Architect,
    plan_id: &str,
    credential: &str,
    stored: &Value,
) -> Outcome {
    let (plan_id, credential) = (plan_id.to_string(), credential.to_string());
    let requested_role = architect.reviewer_role.as_deref();
    match request_review(env.deps, &plan_id, requested_role, &credential) {
        Ok(Requested {
            review,
            reviewer_state,
            ..
        }) => Ok(ok(json!({
            "planId": field(stored, "planId"),
            "revision": field(stored, "currentRevision"),
            "state": "in_review",
            "reviewId": field(&review, "reviewId"),
            "reviewerAgentId": field(&review, "reviewerAgentId"),
            "reviewerState": reviewer_state,
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

/// `plan amend <plan-id> <json>`: the architect adds owned paths and dependency edges the latest review asked for, when its
/// findings were all mechanical, without another review round.
fn amend(env: &CommandEnv<'_>, call: &CommandCall<'_>, rest: &[String]) -> Outcome {
    let Some(config) = env.config() else {
        return Ok(fail(
            ErrorCode::NotConfigured,
            "plans need [architect] enabled = true in capstan.toml",
        ));
    };
    if !is_active_architect(env, call, &config.architect.role) {
        return Ok(fail(
            ErrorCode::Forbidden,
            "not_architect: only the designated architect may amend a plan",
        ));
    }
    if !config.architect.plan_amend {
        return Ok(fail(
            ErrorCode::Rejected,
            "amend_disabled: plan amend is switched off by plan_amend = false in [architect]; submit a new revision with cstan plan submit",
        ));
    }
    if rest.len() != 2 || !is_safe_agent_id(&rest[0]) {
        return Ok(fail(
            ErrorCode::InvalidRequest,
            "plan amend needs a plan id and the amendment JSON",
        ));
    }
    let (plan_id, amendment_text) = (rest[0].clone(), rest[1].clone());
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
            format!("amend_refused: plan {plan_id} is {state}, not a draft"),
        ));
    }
    let plan_review = config.architect.plan_review.as_str();
    let needs_review =
        (record["tier"] == "high_risk" && plan_review != "never") || plan_review == "always";
    if needs_review {
        if env.deps.launcher.is_none() {
            return Ok(fail(
                ErrorCode::NotConfigured,
                "a plan review needs Herdr and a git repository",
            ));
        }
        if let Some(refusal) = review_limit_refusal(env, &credential, &plan_id)? {
            return Ok(refusal);
        }
    }
    let Some(previous) = existing["revision"]["bodyJson"].as_str() else {
        return Ok(fail(
            ErrorCode::Rejected,
            format!("amend_refused: plan {plan_id} has no revision yet; submit it with cstan plan submit"),
        ));
    };
    let body_json = match amended_plan_body_json(previous, &amendment_text) {
        Ok(body_json) => body_json,
        Err(refusal) => return Ok(fail(ErrorCode::Rejected, refusal)),
    };
    let base_sha = env.deps.git.head_commit().map_err(|_| Thrown::Git)?;
    let caller_id = env
        .agent_of(call.identity)
        .map(|c| c.agent_id.clone())
        .unwrap_or_default();
    env.log(
        "plan_amend_requested",
        json!({"requestedBy": caller_id, "planId": plan_id}),
    );
    let stored = {
        let (credential, plan_id) = (credential.clone(), plan_id.clone());
        let run = env.deps.kernel.run(move |core| {
            let context = new_context(core, &credential)?;
            core.amend_plan(
                &context,
                &json!({
                    "planId": plan_id,
                    "amendmentJson": amendment_text,
                    "bodyJson": body_json,
                    "baseSha": base_sha,
                    "review": needs_review,
                }),
            )
        });
        match run {
            Ok(stored) => stored,
            Err(error) if error.is_controller_error() => {
                let message = error.message();
                let coded = ["amend_refused:", "amend_not_allowed:", "invalid_plan:"]
                    .iter()
                    .any(|code| message.starts_with(code));
                return Ok(fail(
                    ErrorCode::Rejected,
                    if coded {
                        message
                    } else {
                        format!("amend_refused: {message}")
                    },
                ));
            }
            Err(error) => return Err(Thrown::Kernel(error)),
        }
    };
    if !needs_review {
        return Ok(ok(json!({
            "planId": field(&stored, "planId"),
            "revision": field(&stored, "currentRevision"),
            "state": field(&stored, "state"),
        })));
    }
    start_review(env, &config.architect, &plan_id, &credential, &stored)
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

/// The latest verify run of a report or integration as the short record the plan rows carry; null when none ran.
fn latest_gate(
    core: &capstan_kernel::Core,
    subject_id: &str,
) -> capstan_kernel::KernelResult<Value> {
    let runs = core.verify_runs(Some(subject_id), 1)?;
    let Some(run) = runs["runs"].as_array().and_then(|runs| runs.first()) else {
        return Ok(Value::Null);
    };
    let mut gate = Map::new();
    for key in [
        "runId",
        "state",
        "commitSha",
        "durationMs",
        "failedStep",
        "exitCode",
        "finishedAt",
    ] {
        gate.insert(key.into(), field(run, key));
    }
    Ok(Value::Object(gate))
}

/// `packageId` -> the latest gate of the package's latest accepted report (the one its progress is read from), and
/// `integrationId` -> the latest gate of each signed-off integration, for a project with a `[verify]` section.
fn gates_of(
    core: &capstan_kernel::Core,
    controller: &str,
    detail: &Value,
) -> capstan_kernel::KernelResult<Map<String, Value>> {
    let reports = core.agent_reports(controller, Some(1000))?;
    let mut gates = Map::new();
    for package in detail["packages"].as_array().into_iter().flatten() {
        let (Some(id), Some(assignee), Some(assigned_at)) = (
            package["packageId"].as_str(),
            package["assigneeAgentId"].as_str(),
            package["assignedAt"].as_str(),
        ) else {
            gates.insert(
                package["packageId"].as_str().unwrap_or("").to_string(),
                Value::Null,
            );
            continue;
        };
        let report = reports.as_array().into_iter().flatten().find(|r| {
            r["agentId"] == assignee
                && r["state"] == "accepted"
                && r["createdAt"].as_str().is_some_and(|at| at >= assigned_at)
        });
        let gate = match report.and_then(|r| r["reportId"].as_str()) {
            Some(report_id) => latest_gate(core, report_id)?,
            None => Value::Null,
        };
        gates.insert(id.to_string(), gate);
    }
    for signoff in detail["signoffs"].as_array().into_iter().flatten() {
        if let Some(id) = signoff["integrationId"].as_str() {
            gates.insert(format!("integration:{id}"), latest_gate(core, id)?);
        }
    }
    Ok(gates)
}

/// One line of `plan show --brief` per plan: the plan fields and a count of its packages by progress.
fn brief_plans(
    core: &capstan_kernel::Core,
    credential: &str,
    links: &[Value],
) -> capstan_kernel::KernelResult<Vec<Value>> {
    let mut out = Vec::new();
    for p in core
        .list_plans(credential)?
        .as_array()
        .into_iter()
        .flatten()
    {
        let plan_id = p["planId"].as_str().unwrap_or("");
        let mut map = Map::new();
        for key in ["planId", "tier", "state", "title"] {
            map.insert(key.into(), field(p, key));
        }
        let detail = core.plan_record(credential, plan_id)?;
        let mut counts = Map::new();
        for package in detail["packages"].as_array().into_iter().flatten() {
            let progress = package_progress_label(package);
            let seen = counts.get(&progress).and_then(Value::as_i64).unwrap_or(0);
            counts.insert(progress, json!(seen + 1));
        }
        map.insert("packages".into(), Value::Object(counts));
        if let Some(last) = last_reassignment(&detail["plan"]) {
            map.insert("lastReassignment".into(), last);
        }
        map.extend(link_fields(links, "plan", plan_id));
        out.push(Value::Object(map));
    }
    Ok(out)
}

/// The newest reassignment a plan record carries.
fn last_reassignment(plan: &Value) -> Option<Value> {
    plan["reassignments"].as_array()?.last().cloned()
}

fn package_progress_label(package: &Value) -> String {
    if package["cancelledAt"].is_null() {
        package["progress"]
            .as_str()
            .unwrap_or("unknown")
            .to_string()
    } else {
        "cancelled".to_string()
    }
}

/// `plan show <id> --brief`: the plan, then one row per package with what the stored body says of it.
fn brief_plan(
    detail: &Value,
    plan_id: &str,
    links: &[Value],
    gates: Option<&Map<String, Value>>,
) -> Value {
    let body: Value = detail["revision"]["bodyJson"]
        .as_str()
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or(Value::Null);
    let rows: Vec<Value> = detail["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|package| {
            let id = package["packageId"].as_str().unwrap_or("");
            let written = body["packages"]
                .as_array()
                .and_then(|all| all.iter().find(|b| b["id"] == id))
                .cloned()
                .unwrap_or(Value::Null);
            let mut map = Map::new();
            map.insert("packageId".into(), json!(id));
            map.insert("type".into(), field(&written, "type"));
            map.insert("estimateHours".into(), field(&written, "estimate_hours"));
            map.insert(
                "dependsOn".into(),
                written
                    .get("depends_on")
                    .filter(|v| v.is_array())
                    .cloned()
                    .unwrap_or_else(|| json!([])),
            );
            map.insert("assignee".into(), field(package, "assigneeAgentId"));
            map.insert("progress".into(), json!(package_progress_label(package)));
            if let Some(gates) = gates {
                map.insert(
                    "verify".into(),
                    gates.get(id).cloned().unwrap_or(Value::Null),
                );
            }
            map.extend(link_fields(links, "package", &format!("{plan_id}/{id}")));
            Value::Object(map)
        })
        .collect();
    let plan = &detail["plan"];
    let mut head = Map::new();
    for key in ["planId", "tier", "state", "title"] {
        head.insert(key.into(), field(plan, key));
    }
    if let Some(last) = last_reassignment(plan) {
        head.insert("lastReassignment".into(), last);
    }
    head.extend(link_fields(links, "plan", plan_id));
    json!({"plan": head, "packages": rows})
}

fn show(env: &CommandEnv<'_>, call: &CommandCall<'_>, rest: &[String]) -> Outcome {
    let brief = rest.iter().any(|a| a == "--brief");
    let plain: Vec<String> = rest.iter().filter(|a| *a != "--brief").cloned().collect();
    let rest = plain.as_slice();
    if rest.len() > 1 || rest.first().is_some_and(|id| !is_safe_agent_id(id)) {
        return Ok(fail(
            ErrorCode::InvalidRequest,
            "plan show takes an optional plan id",
        ));
    }
    let credential = call.credential.to_string();
    let links: Vec<Value> = {
        let credential = credential.clone();
        let scope = rest.first().cloned();
        env.deps
            .kernel
            .run(move |core| core.plan_links(&credential, scope.as_deref()))?
            .as_array()
            .cloned()
            .unwrap_or_default()
    };
    let Some(plan_id) = rest.first() else {
        if brief {
            let brief_links = links.clone();
            let listed = env
                .deps
                .kernel
                .run(move |core| brief_plans(core, &credential, &brief_links))?;
            return Ok(ok(json!({ "plans": listed })));
        }
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
    // The gate of each package and signed-off integration, read from the ledger only when the project has one.
    let gates: Option<Map<String, Value>> = if env.deps.verify.is_some() {
        let (controller, shown) = (env.deps.credential().to_string(), detail.clone());
        Some(
            env.deps
                .kernel
                .run(move |core| gates_of(core, &controller, &shown).map(Value::Object))?
                .as_object()
                .cloned()
                .unwrap_or_default(),
        )
    } else {
        None
    };
    if brief {
        return Ok(ok(brief_plan(&detail, plan_id, &links, gates.as_ref())));
    }
    let revision = if detail["revision"].is_null() {
        Value::Null
    } else {
        let revision = &detail["revision"];
        let body: Value = serde_json::from_str(revision["bodyJson"].as_str().unwrap_or("null"))
            .map_err(KernelError::from)?;
        let mut shown = json!({
            "revision": field(revision, "revision"),
            "baseSha": field(revision, "baseSha"),
            "authorAgentId": field(revision, "authorAgentId"),
            "createdAt": field(revision, "createdAt"),
            "body": body,
        });
        // An amended revision says what was added and which review it answers.
        if !revision["amendment"].is_null() {
            shown["amendment"] = field(revision, "amendment");
            shown["amendsReviewId"] = field(revision, "amendsReviewId");
        }
        shown
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
            if let Some(gates) = &gates {
                map.insert(
                    "verify".into(),
                    gates
                        .get(package["packageId"].as_str().unwrap_or(""))
                        .cloned()
                        .unwrap_or(Value::Null),
                );
            }
            Value::Object(map)
        })
        .collect();
    result.insert("packages".into(), Value::Array(packages));
    let (overrides, agent_models) = {
        let (id, shown) = (plan_id.clone(), detail.clone());
        env.deps
            .kernel
            .run(move |core| model_members(core, &id, &shown))?
    };
    if !overrides.is_empty() {
        result.insert("modelOverrides".into(), Value::Array(overrides));
    }
    if !agent_models.is_empty() {
        result.insert("agentModels".into(), Value::Array(agent_models));
    }
    let mut signoffs = field(&detail, "signoffs");
    if let Some(gates) = &gates {
        for signoff in signoffs.as_array_mut().into_iter().flatten() {
            let key = format!(
                "integration:{}",
                signoff["integrationId"].as_str().unwrap_or("")
            );
            if let Some(map) = signoff.as_object_mut() {
                map.insert(
                    "verify".into(),
                    gates.get(&key).cloned().unwrap_or(Value::Null),
                );
            }
        }
    }
    result.insert("signoffs".into(), signoffs);
    Ok(ok(Value::Object(result)))
}

// ------------------------------------------------------------------------------------------------ model overrides

/// The role kinds an override may name (a PM is launched, never spawned).
pub(super) const KINDS: [&str; 3] = ["Developer", "Verifier", "Supervisor"];
/// What a caller that may not set an override is told.
pub(super) const MODEL_REFUSED: &str =
    "only the user, the PM or the operator may set a model override";

/// `--model <id>` and `--model <Kind>=<id>` flags out of `rest`: the other arguments, then the flag values in order.
fn take_model_flags(rest: &[String]) -> Result<(Vec<String>, Vec<String>), CommandResponse> {
    let (mut others, mut flags) = (Vec::new(), Vec::new());
    let mut index = 0;
    while index < rest.len() {
        if rest[index] == "--model" {
            let Some(value) = rest.get(index + 1) else {
                return Err(fail(ErrorCode::InvalidRequest, "--model needs a model id"));
            };
            flags.push(value.clone());
            index += 2;
        } else {
            others.push(rest[index].clone());
            index += 1;
        }
    }
    Ok((others, flags))
}

/// The `modelOverrides` of an `openPlan` input from the flag values, and the fit warnings. Nothing is written here.
fn checked_overrides(
    env: &CommandEnv<'_>,
    flags: &[String],
) -> Result<(Vec<Value>, Vec<String>), CommandResponse> {
    let mut seen: Vec<Option<&str>> = Vec::new();
    let mut rows = Vec::new();
    let mut warnings = Vec::new();
    for flag in flags {
        let (kind, model) = match flag.split_once('=') {
            Some((kind, model)) => match KINDS.iter().find(|known| **known == kind) {
                Some(known) => (Some(*known), model),
                None => {
                    return Err(fail(
                        ErrorCode::InvalidRequest,
                        format!(
                        "--model {kind}=... must name Developer, Verifier or Supervisor exactly"
                    ),
                    ))
                }
            },
            None => (None, flag.as_str()),
        };
        if seen.contains(&kind) {
            return Err(fail(
                ErrorCode::InvalidRequest,
                match kind {
                    Some(kind) => format!("--model names {kind} twice"),
                    None => "--model without a kind is given twice".to_string(),
                },
            ));
        }
        seen.push(kind);
        warnings.extend(check_fit(env, kind, model)?);
        rows.push(json!({"roleKind": kind, "model": model}));
    }
    Ok((rows, warnings))
}

/// The refusal of a model value that no role of the kind (all non-PM roles without one) can run, else one warning for each
/// configured role that cannot. Spawn checks again with the configuration of that moment and is the authority.
pub(super) fn check_fit(
    env: &CommandEnv<'_>,
    kind: Option<&str>,
    model: &str,
) -> Result<Vec<String>, CommandResponse> {
    if let Err(why) = capstan_config::check_model_value(model) {
        return Err(fail(ErrorCode::InvalidRequest, why));
    }
    let Some(config) = env.config() else {
        return Ok(Vec::new());
    };
    let (mut fitting, mut first_refusal, mut warnings) = (0, None, Vec::new());
    for role in config
        .roles
        .iter()
        .filter(|role| role.kind != "PM" && kind.is_none_or(|kind| role.kind == kind))
    {
        match capstan_config::fit_for_role(config, &role.name, model) {
            Ok(()) => fitting += 1,
            Err(why) => {
                let host_kind = config
                    .hosts
                    .iter()
                    .find(|host| host.name == role.host)
                    .map_or("unknown", |host| host.kind.as_str());
                warnings.push(format!(
                    "{} on {} ({}) will refuse to spawn with this model",
                    role.name, role.host, host_kind
                ));
                first_refusal.get_or_insert(why);
            }
        }
    }
    match (fitting, first_refusal) {
        (0, Some(why)) => Err(fail(
            ErrorCode::Rejected,
            format!("model_host_mismatch: {why}"),
        )),
        _ => Ok(warnings),
    }
}

/// The active agents an override of this scope does not reach (they keep the model they started with).
fn running_agents(
    core: &capstan_kernel::Core,
    credential: &str,
    scope_type: &str,
    scope_id: &str,
    kind: Option<&str>,
) -> Result<Vec<Value>, KernelError> {
    let mut candidates: Vec<String> = Vec::new();
    if scope_type == "plan" {
        let detail = core.plan_record(credential, scope_id)?;
        candidates.extend(
            detail["plan"]["architectAgentId"]
                .as_str()
                .map(String::from),
        );
        for package in detail["packages"].as_array().into_iter().flatten() {
            candidates.extend(package["assigneeAgentId"].as_str().map(String::from));
        }
    } else {
        for link in core
            .external_links(credential)?
            .as_array()
            .into_iter()
            .flatten()
        {
            if link["refKind"] == "requirement" && link["refId"] == scope_id {
                candidates.extend(link["boundAgentId"].as_str().map(String::from));
            }
        }
    }
    let agents = core.list_agents()?;
    let mut running = Vec::new();
    for agent in agents.as_array().into_iter().flatten() {
        let id = agent["agentId"].as_str().unwrap_or("");
        let kind_ok = kind.is_none_or(|kind| agent["kind"] == kind);
        if agent["state"] == "active"
            && agent["kind"] != "PM"
            && kind_ok
            && candidates.iter().any(|candidate| candidate == id)
            && !running.iter().any(|row: &Value| row["agentId"] == id)
        {
            running.push(
                json!({"agentId": id, "model": agent.get("model").cloned().unwrap_or(Value::Null)}),
            );
        }
    }
    Ok(running)
}

/// `modelOverrides` and `agentModels` of `plan show`, each empty when there is nothing to show.
fn model_members(
    core: &capstan_kernel::Core,
    plan_id: &str,
    detail: &Value,
) -> Result<(Vec<Value>, Vec<Value>), KernelError> {
    let overrides = core
        .model_overrides("plan", plan_id)?
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut ids: Vec<String> = Vec::new();
    ids.extend(
        detail["plan"]["architectAgentId"]
            .as_str()
            .map(String::from),
    );
    for package in detail["packages"].as_array().into_iter().flatten().take(9) {
        if let Some(id) = package["assigneeAgentId"].as_str() {
            if !ids.iter().any(|known| known == id) {
                ids.push(id.to_string());
            }
        }
    }
    let agents = core.list_agents()?;
    let mut models = Vec::new();
    for id in ids.iter().take(9) {
        let Some(found) = core.agent_model(id)? else {
            continue;
        };
        let role = agents
            .as_array()
            .into_iter()
            .flatten()
            .find(|agent| agent["agentId"] == id.as_str())
            .map_or(Value::Null, |agent| agent["roleName"].clone());
        models.push(json!({
            "agentId": id,
            "role": role,
            "model": found["model"],
            "modelSource": found["source"],
        }));
    }
    Ok((overrides, models))
}

/// `plan model <plan-id> [<Kind>] [<id> | --clear]` and `link model <requirement-ref-id> ...`: show, set or clear.
pub(super) fn model_command(
    env: &CommandEnv<'_>,
    call: &CommandCall<'_>,
    scope_type: &'static str,
    scope_id: &str,
    tail: &[String],
) -> Result<CommandResponse, KernelError> {
    let scope_key = if scope_type == "plan" {
        "planId"
    } else {
        "refId"
    };
    if tail.is_empty() {
        let id = scope_id.to_string();
        let rows = env
            .deps
            .kernel
            .run(move |core| core.model_overrides(scope_type, &id))?;
        return Ok(ok(json!({scope_key: scope_id, "modelOverrides": rows})));
    }
    if env.worker_manager(call.identity).is_none() {
        return Ok(fail(ErrorCode::Forbidden, MODEL_REFUSED));
    }
    let named = || {
        fail(
            ErrorCode::InvalidRequest,
            "name the kind before the model: Developer, Verifier or Supervisor",
        )
    };
    let is_value = |value: &str| value != "--clear" && !value.starts_with("--");
    let (kind, value): (Option<&str>, &str) = match tail {
        [only] => {
            if KINDS.contains(&only.as_str()) || (only != "--clear" && only.starts_with("--")) {
                return Ok(named());
            }
            (None, only.as_str())
        }
        [kind, value] => {
            if !KINDS.contains(&kind.as_str()) {
                return Ok(named());
            }
            if value != "--clear" && value.starts_with("--") {
                return Ok(named());
            }
            (Some(kind.as_str()), value.as_str())
        }
        _ => {
            return Ok(fail(
                ErrorCode::InvalidRequest,
                "model takes an optional kind and a model id or --clear",
            ))
        }
    };
    let clearing = !is_value(value);
    let warnings = if clearing {
        Vec::new()
    } else {
        match check_fit(env, kind, value) {
            Ok(warnings) => warnings,
            Err(refusal) => return Ok(refusal),
        }
    };
    env.log(
        "model_override_requested",
        json!({"scopeType": scope_type, "scopeId": scope_id, "roleKind": kind, "clear": clearing}),
    );
    let mut input = Map::new();
    input.insert("scopeType".into(), json!(scope_type));
    input.insert("scopeId".into(), json!(scope_id));
    input.insert("roleKind".into(), json!(kind));
    if !clearing {
        input.insert("model".into(), json!(value));
    }
    let credential = call.credential.to_string();
    let id = scope_id.to_string();
    let kind_owned = kind.map(String::from);
    let answer = env.deps.kernel.run(move |core| {
        let context = new_context(core, &credential)?;
        let input = Value::Object(input);
        let done = if input.get("model").is_some() {
            core.set_model_override(&context, &input)?
        } else {
            core.clear_model_override(&context, &input)?
        };
        let running = running_agents(core, &credential, scope_type, &id, kind_owned.as_deref())?;
        Ok((done, running))
    })?;
    let (done, running) = answer;
    let mut result = Map::new();
    result.insert(scope_key.into(), json!(scope_id));
    result.insert("roleKind".into(), json!(kind));
    if clearing {
        result.insert("cleared".into(), done["cleared"].clone());
    } else {
        result.insert("model".into(), json!(value));
    }
    result.insert("previous".into(), done["previous"].clone());
    if !warnings.is_empty() {
        result.insert("warnings".into(), json!(warnings));
    }
    result.insert("running".into(), Value::Array(running));
    Ok(ok(Value::Object(result)))
}

fn plan_model(env: &CommandEnv<'_>, call: &CommandCall<'_>, rest: &[String]) -> Outcome {
    let Some(plan_id) = rest.first().filter(|id| is_safe_agent_id(id)) else {
        return Ok(fail(
            ErrorCode::InvalidRequest,
            "plan model needs a plan id, then optionally a kind and a model id or --clear",
        ));
    };
    if rest.len() == 1 {
        let (credential, id) = (call.credential.to_string(), plan_id.clone());
        let known = env
            .deps
            .kernel
            .run(move |core| core.plan_record(&credential, &id))?;
        if known.is_null() {
            return Ok(fail(
                ErrorCode::Rejected,
                format!("unknown_plan: no plan {plan_id}"),
            ));
        }
    }
    Ok(model_command(env, call, "plan", plan_id, &rest[1..])?)
}
