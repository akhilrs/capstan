//! The design command: `design approve|changes <plan-id>/<package-id>` records the user's decision on a design package's
//! report, `design show <plan-id>[/<package-id>]` lists the decisions. Only the PM or the operator decides.

use super::shared::{
    fail, is_safe_agent_id, map_kernel_error, ok, CommandCall, CommandEnv, CommandResponse,
    ErrorCode,
};
use super::HandlerMap;
use crate::deps::new_context;
use serde_json::{json, Map};

/// Registers the handler of the route this module serves (design).
pub fn register(map: &mut HandlerMap) {
    map.insert("design", design);
}

fn usage() -> CommandResponse {
    fail(
        ErrorCode::InvalidRequest,
        "design takes approve <plan-id>/<package-id> --link <url> --asked-user \"<what the user said>\", changes <plan-id>/<package-id> --asked-user \"<what the user asked to change>\" [--link <url>], or show <plan-id>[/<package-id>]",
    )
}

fn refused(error: &capstan_kernel::KernelError) -> CommandResponse {
    if error.is_controller_error() {
        fail(
            ErrorCode::Rejected,
            format!("design_refused: {}", error.message()),
        )
    } else {
        map_kernel_error(error)
    }
}

/// `<plan-id>` or `<plan-id>/<package-id>`.
fn target(text: &str) -> Option<(String, Option<String>)> {
    let (plan, package) = match text.split_once('/') {
        Some((plan, package)) => (plan, Some(package)),
        None => (text, None),
    };
    let valid = is_safe_agent_id(plan) && package.is_none_or(is_safe_agent_id);
    valid.then(|| (plan.to_string(), package.map(str::to_string)))
}

fn design(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let args = call.args;
    let (Some(sub), Some(spec)) = (args.first(), args.get(1)) else {
        return Some(usage());
    };
    let Some((plan_id, package_id)) = target(spec) else {
        return Some(usage());
    };
    Some(match sub.as_str() {
        "show" if args.len() == 2 => show(env, &plan_id, package_id),
        "approve" | "changes" => match package_id {
            Some(package_id) => decide(env, call, sub, &plan_id, &package_id),
            None => usage(),
        },
        _ => usage(),
    })
}

fn show(env: &CommandEnv<'_>, plan_id: &str, package_id: Option<String>) -> CommandResponse {
    let plan = plan_id.to_string();
    match env
        .deps
        .kernel
        .run(move |core| core.design_approvals(&plan, package_id.as_deref()))
    {
        Ok(result) => ok(result),
        Err(error) => refused(&error),
    }
}

fn decide(
    env: &CommandEnv<'_>,
    call: &CommandCall<'_>,
    sub: &str,
    plan_id: &str,
    package_id: &str,
) -> CommandResponse {
    let Some(requested_by) = env.worker_manager(call.identity) else {
        return fail(
            ErrorCode::Forbidden,
            "only the PM or the operator records a design decision",
        );
    };
    let mut link = None;
    let mut asked_user = None;
    let mut rest = call.args[2..].iter();
    while let Some(flag) = rest.next() {
        let slot = match flag.as_str() {
            "--link" => &mut link,
            "--asked-user" => &mut asked_user,
            _ => return usage(),
        };
        match rest.next() {
            Some(value) if slot.is_none() => *slot = Some(value.clone()),
            _ => return usage(),
        }
    }
    let Some(asked_user) = asked_user.filter(|text| !text.trim().is_empty()) else {
        return fail(
            ErrorCode::InvalidRequest,
            "--asked-user \"<what the user said>\" is required: ask the user before you record a design decision",
        );
    };
    if sub == "approve" && link.as_deref().is_none_or(|text| text.trim().is_empty()) {
        return fail(
            ErrorCode::InvalidRequest,
            "--link <url-or-repo-path> is required: the design the user approved",
        );
    }
    env.log(
        "design_decision_requested",
        json!({"requestedBy": requested_by, "planId": plan_id, "packageId": package_id, "decision": sub}),
    );
    let mut input = Map::new();
    input.insert("planId".into(), json!(plan_id));
    input.insert("packageId".into(), json!(package_id));
    input.insert(
        "decision".into(),
        json!(if sub == "approve" {
            "approved"
        } else {
            "changes"
        }),
    );
    input.insert("askedUser".into(), json!(asked_user));
    if let Some(link) = link {
        input.insert("link".into(), json!(link));
    }
    let credential = call.credential.to_string();
    match env.deps.kernel.run(move |core| {
        let context = new_context(core, &credential)?;
        core.record_design_decision(&context, &serde_json::Value::Object(input))
    }) {
        Ok(result) => ok(result),
        Err(error) => refused(&error),
    }
}
