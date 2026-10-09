//! The finding and review commands (src/commands/findings.ts): a supervisor's findings and a reviewer's verdicts.

use super::shared::{
    fail, finding_answer, map_kernel_error, ok, CommandCall, CommandEnv, CommandResponse, ErrorCode,
};
use super::HandlerMap;
use crate::deps::new_context;
use crate::reviews::{release_reviewer_later, review_text};
use capstan_kernel::KernelError;
use serde_json::{json, Value};

/// Registers the handlers of the routes this module serves (finding, review).
pub fn register(map: &mut HandlerMap) {
    map.insert("finding", finding);
    map.insert("review", review);
}

fn field(value: &Value, key: &str) -> Value {
    value.get(key).cloned().unwrap_or(Value::Null)
}

/// `refuse` of the finding handler: a refusal of the controller is `finding_refused`.
fn refuse_finding(error: &KernelError) -> CommandResponse {
    if error.is_controller_error() {
        fail(
            ErrorCode::Rejected,
            format!("finding_refused: {}", error.message()),
        )
    } else {
        map_kernel_error(error)
    }
}

fn finding(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let caller = env.agent_of(call.identity);
    let Some(caller) = caller.filter(|c| c.kind == "Supervisor" && c.state == "active") else {
        return Some(fail(
            ErrorCode::Forbidden,
            "only a Supervisor can raise or check a finding",
        ));
    };
    let args = call.args;
    let credential = call.credential.to_string();
    if args.first().map(String::as_str) == Some("check") && args.len() != 5 {
        if args.len() != 4 {
            return Some(fail(
                ErrorCode::InvalidRequest,
                "finding check needs a finding id, resolved or unresolved, and the evidence",
            ));
        }
        let input = json!({"findingId": args[1], "result": args[2], "evidence": args[3]});
        let checked = env.deps.kernel.run(move |core| {
            let context = new_context(core, &credential)?;
            core.check_finding(&context, &input)
        });
        return Some(match checked {
            Ok(finding) => {
                env.log(
                    "finding_checked",
                    json!({
                        "findingId": field(&finding, "findingId"),
                        "state": field(&finding, "state"),
                        "interventions": field(&finding, "interventions"),
                    }),
                );
                ok(finding_answer(&finding))
            }
            Err(error) => refuse_finding(&error),
        });
    }
    if args.len() != 5 {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "finding needs an agent id, a severity, the evidence, the requested correction and the done-when condition",
        ));
    }
    let now = env.deps.kernel.now_ms();
    let allowed = env
        .state
        .finding_limiter
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .allow(&caller.agent_id, now);
    if !allowed {
        return Some(fail(
            ErrorCode::Rejected,
            "finding_rate_limit: too many findings; wait a minute",
        ));
    }
    let input = json!({
        "targetAgentId": args[0],
        "severity": args[1],
        "evidence": args[2],
        "correction": args[3],
        "doneWhen": args[4],
    });
    let raised = env.deps.kernel.run(move |core| {
        let context = new_context(core, &credential)?;
        core.raise_finding(&context, &input)
    });
    Some(match raised {
        Ok(finding) => {
            env.log(
                "finding_raised",
                json!({
                    "findingId": field(&finding, "findingId"),
                    "targetAgentId": field(&finding, "targetAgentId"),
                    "severity": field(&finding, "severity"),
                }),
            );
            ok(finding_answer(&finding))
        }
        Err(error) => refuse_finding(&error),
    })
}

fn review(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let caller = env.agent_of(call.identity);
    if !caller.is_some_and(|c| c.kind == "Verifier" && c.state == "active") {
        return Some(fail(
            ErrorCode::Forbidden,
            "only a reviewer can answer a review",
        ));
    }
    if call.args.len() != 2 {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "review needs a verdict (pass or findings) and a text",
        ));
    }
    let verdict = call.args[0].as_str();
    if verdict != "pass" && verdict != "findings" {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "the verdict must be pass or findings",
        ));
    }
    let text = review_text(&call.args[1]);
    if text.is_empty() {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "the review text must not be empty",
        ));
    }
    let credential = call.credential.to_string();
    let input = json!({"verdict": verdict, "text": text});
    let completed = env.deps.kernel.run(move |core| {
        let context = new_context(core, &credential)?;
        core.complete_review(&context, &input)
    });
    Some(match completed {
        Ok(review) => {
            env.log(
                "review_completed",
                json!({"reviewId": field(&review, "reviewId"), "state": field(&review, "state")}),
            );
            release_reviewer_later(env.deps, &review);
            ok(json!({
                "reviewId": field(&review, "reviewId"),
                "round": field(&review, "round"),
                "state": field(&review, "state"),
                "announced": !field(&review, "notifiedMessageId").is_null(),
            }))
        }
        Err(error) if error.is_controller_error() => fail(
            ErrorCode::Rejected,
            format!("review_refused: {}", error.message()),
        ),
        Err(error) => map_kernel_error(&error),
    })
}
