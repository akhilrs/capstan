//! The op command (src/commands/operator.ts): the Operator agent's proposals and grants. The Operator service is reached
//! through the `OperatorService` trait of the operator crate (`deps.operator`).

use super::shared::{
    describe_full_auto, describe_grant_record, describe_proposal, fail, is_safe_agent_id, ok,
    CommandCall, CommandEnv, CommandResponse, ErrorCode, MAX_STATUS_PROPOSALS,
};
use super::HandlerMap;
use capstan_kernel::helpers::js_trim;
use capstan_kernel::records::RESTART_COMMAND_TEXT;
use capstan_operator::api::{
    DecideInput, Decision, FullAutoOnInput, GrantKind, ListFilter, OperatorError, OperatorService,
    ProposalKind, ProposeInput, SessionGrantInput,
};
use serde_json::{json, Value};

/// Registers the handlers of the routes this module serves (op).
pub fn register(map: &mut HandlerMap) {
    map.insert("op", op);
}

/// `refuse` of the Node handler for what the service raises.
fn refuse(error: &OperatorError) -> CommandResponse {
    fail(
        ErrorCode::Rejected,
        format!("{}: {}", error.code, error.message),
    )
}

fn answered(result: Result<Value, OperatorError>) -> CommandResponse {
    match result {
        Ok(proposal) => ok(describe_proposal(&proposal)),
        Err(error) => refuse(&error),
    }
}

/// Who is calling, as the sub-commands tell them apart.
struct Who {
    /// The designated operator agent.
    operator_agent: bool,
    active_pm: bool,
    /// The operator at the command line.
    cli: bool,
    agent_id: Option<String>,
}

fn op(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let config = env.config();
    let service = env.deps.operator.as_ref();
    let (Some(config), Some(service)) = (config.filter(|c| c.operator.enabled), service) else {
        return Some(fail(
            ErrorCode::NotConfigured,
            "operator commands need [operator] enabled = true in capstan.toml",
        ));
    };
    let sub = call.args.first().map(String::as_str);
    let rest: &[String] = call.args.get(1..).unwrap_or(&[]);
    if !matches!(
        sub,
        Some("propose" | "decide" | "show" | "cancel" | "grants" | "revoke" | "full-auto")
    ) {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "op needs propose, decide, show, cancel, grants, revoke or full-auto",
        ));
    }
    let caller = env.agent_of(call.identity);
    let who = Who {
        operator_agent: caller.is_some_and(|c| {
            c.state == "active" && c.kind == "Developer" && c.role_name == config.operator.role
        }),
        active_pm: caller.is_some_and(|c| c.state == "active" && c.kind == "PM"),
        cli: call.identity.role == "operator",
        agent_id: caller.map(|c| c.agent_id.clone()),
    };
    let service: &dyn OperatorService = service.as_ref();
    let credential = call.credential;
    Some(match sub {
        Some("propose") => propose(env, service, &who, credential, rest),
        Some("decide") => decide(service, &who, credential, rest),
        Some("grants") => grants(service, &who, rest),
        Some("revoke") => revoke(service, &who, credential, rest),
        Some("full-auto") => full_auto(service, &who, credential, rest),
        Some("cancel") => cancel(service, &who, credential, rest),
        _ => show(service, &who, rest),
    })
}

fn propose(
    env: &CommandEnv<'_>,
    service: &dyn OperatorService,
    who: &Who,
    credential: &str,
    rest: &[String],
) -> CommandResponse {
    if !who.operator_agent {
        return fail(
            ErrorCode::Forbidden,
            "only the designated operator agent proposes a command",
        );
    }
    let mut kind = ProposalKind::Command;
    let mut force_restart = false;
    let (command, reason);
    if rest.first().map(String::as_str) == Some("--restart") {
        kind = ProposalKind::Restart;
        let mut tail = &rest[1..];
        if tail.first().map(String::as_str) == Some("--force") {
            force_restart = true;
            tail = &tail[1..];
        }
        if tail.len() != 1 {
            return fail(
                ErrorCode::InvalidRequest,
                "op propose --restart needs [--force] and one reason",
            );
        }
        command = RESTART_COMMAND_TEXT.to_string();
        reason = tail[0].clone();
    } else {
        if rest.len() != 2 {
            return fail(
                ErrorCode::InvalidRequest,
                "op propose needs \"<command>\" and \"<reason>\"",
            );
        }
        if rest[0].starts_with('-') {
            return fail(
                ErrorCode::InvalidRequest,
                "a command cannot start with -; the only options are --restart and --force",
            );
        }
        command = rest[0].clone();
        reason = rest[1].clone();
    }
    env.log(
        "operator_propose_requested",
        json!({"agentId": who.agent_id, "kind": kind.as_str()}),
    );
    answered(service.propose(
        credential,
        ProposeInput {
            kind,
            command,
            reason,
            force_restart,
        },
    ))
}

fn decide(
    service: &dyn OperatorService,
    who: &Who,
    credential: &str,
    rest: &[String],
) -> CommandResponse {
    let (proposal_id, decision) = (rest.first(), rest.get(1).map(String::as_str));
    let (Some(proposal_id), Some(decision @ ("approve" | "deny"))) = (proposal_id, decision) else {
        return fail(
            ErrorCode::InvalidRequest,
            "op decide needs a proposal id and approve --hash <hash12> or deny [\"<note>\"]",
        );
    };
    let tail: &[String] = rest.get(2..).unwrap_or(&[]);
    if decision == "approve" {
        if who.cli {
            return fail(
                ErrorCode::Rejected,
                "approve_requires_pm: only an active PM agent approves an operator proposal; the operator may deny, cancel and show",
            );
        }
        if !who.active_pm {
            return fail(
                ErrorCode::Forbidden,
                "only an active PM approves a proposal",
            );
        }
        let usage = fail(
            ErrorCode::InvalidRequest,
            "op decide approve needs --hash <hash12> and may add --session exact or --session prefix=\"<words>\"; it takes no --force, the force value is part of the proposal",
        );
        if tail.first().map(String::as_str) != Some("--hash") || tail.get(1).is_none() {
            return usage;
        }
        let mut session = None;
        if tail.len() > 2 {
            let wanted = tail.get(3);
            let (true, Some(wanted)) = (
                tail.len() == 4 && tail[2] == "--session",
                wanted.map(String::as_str),
            ) else {
                return usage;
            };
            if wanted == "exact" {
                session = Some(SessionGrantInput {
                    kind: GrantKind::Exact,
                    text: None,
                });
            } else if let Some(text) = wanted.strip_prefix("prefix=") {
                session = Some(SessionGrantInput {
                    kind: GrantKind::Prefix,
                    text: Some(text.to_string()),
                });
            } else {
                return usage;
            }
        }
        return answered(service.decide(
            credential,
            DecideInput {
                proposal_id: proposal_id.clone(),
                decision: Decision::Approve,
                hash: Some(tail[1].clone()),
                note: None,
                session,
            },
        ));
    }
    if !who.cli && !who.active_pm {
        return fail(ErrorCode::Forbidden, "only the PM or the operator denies");
    }
    if tail.len() > 1 {
        return fail(
            ErrorCode::InvalidRequest,
            "op decide deny takes at most one note; there is no --force at decide time",
        );
    }
    answered(service.decide(
        credential,
        DecideInput {
            proposal_id: proposal_id.clone(),
            decision: Decision::Deny,
            hash: None,
            note: tail.first().cloned(),
            session: None,
        },
    ))
}

fn grants(service: &dyn OperatorService, who: &Who, rest: &[String]) -> CommandResponse {
    if !who.cli && !who.active_pm {
        return fail(
            ErrorCode::Forbidden,
            "only the PM or the operator lists grants",
        );
    }
    if !rest.is_empty() {
        return fail(ErrorCode::InvalidRequest, "op grants takes no argument");
    }
    let described: Vec<Value> = service.grants().iter().map(describe_grant_record).collect();
    ok(json!({ "grants": described }))
}

fn revoke(
    service: &dyn OperatorService,
    who: &Who,
    credential: &str,
    rest: &[String],
) -> CommandResponse {
    if !who.cli && !who.active_pm {
        return fail(ErrorCode::Forbidden, "only the PM or the operator revokes");
    }
    if rest.len() != 1 || !is_safe_agent_id(&rest[0]) {
        return fail(ErrorCode::InvalidRequest, "op revoke needs one grant id");
    }
    match service.revoke_grant(credential, &rest[0]) {
        Ok(grant) => ok(describe_grant_record(&grant)),
        Err(error) => refuse(&error),
    }
}

fn full_auto(
    service: &dyn OperatorService,
    who: &Who,
    credential: &str,
    rest: &[String],
) -> CommandResponse {
    let action = rest.first().map(String::as_str);
    let mut args: &[String] = rest.get(1..).unwrap_or(&[]);
    match action {
        Some("status") => {
            if !who.cli && !who.active_pm && !who.operator_agent {
                return fail(ErrorCode::Forbidden, "the caller may not read full auto");
            }
            if !args.is_empty() {
                return fail(
                    ErrorCode::InvalidRequest,
                    "op full-auto status takes no argument",
                );
            }
            ok(describe_full_auto(service.full_auto_status()))
        }
        Some("off") => {
            if !who.cli && !who.active_pm {
                return fail(
                    ErrorCode::Forbidden,
                    "only the PM or the operator switches full auto off",
                );
            }
            if !args.is_empty() {
                return fail(
                    ErrorCode::InvalidRequest,
                    "op full-auto off takes no argument",
                );
            }
            match service.full_auto_off(credential) {
                Ok(status) => ok(describe_full_auto(status)),
                Err(error) => refuse(&error),
            }
        }
        Some("on") => {
            if who.cli {
                return fail(
                    ErrorCode::Rejected,
                    "full_auto_requires_pm: the user switches full auto off; only the PM switches it on, after asking the user",
                );
            }
            if !who.active_pm {
                return fail(
                    ErrorCode::Forbidden,
                    "only an active PM switches full auto on",
                );
            }
            let mut minutes = None;
            if let Some(first) = args.first() {
                if !first.is_empty() && first.bytes().all(|b| b.is_ascii_digit()) {
                    minutes = Some(first.parse::<i64>().unwrap_or(i64::MAX));
                    args = &args[1..];
                }
            }
            if args.len() != 2 || args[0] != "--asked-user" || js_trim(&args[1]).is_empty() {
                return fail(
                    ErrorCode::InvalidRequest,
                    "op full-auto on [<minutes>] --asked-user \"<what the user said>\"",
                );
            }
            match service.full_auto_on(
                credential,
                FullAutoOnInput {
                    minutes,
                    asked_user: args[1].clone(),
                },
            ) {
                Ok(status) => ok(describe_full_auto(status)),
                Err(error) => refuse(&error),
            }
        }
        _ => fail(
            ErrorCode::InvalidRequest,
            "op full-auto needs on, off or status",
        ),
    }
}

fn cancel(
    service: &dyn OperatorService,
    who: &Who,
    credential: &str,
    rest: &[String],
) -> CommandResponse {
    if rest.len() != 1 || !is_safe_agent_id(&rest[0]) {
        return fail(ErrorCode::InvalidRequest, "op cancel needs one proposal id");
    }
    if !who.cli && !who.active_pm && !who.operator_agent {
        return fail(ErrorCode::Forbidden, "the caller may not cancel a proposal");
    }
    answered(service.cancel(credential, &rest[0]))
}

fn show(service: &dyn OperatorService, who: &Who, rest: &[String]) -> CommandResponse {
    if !who.cli && !who.active_pm && !who.operator_agent {
        return fail(ErrorCode::Forbidden, "the caller may not show proposals");
    }
    if rest.len() > 1 {
        return fail(ErrorCode::InvalidRequest, "op show takes at most one id");
    }
    let own = who.operator_agent && who.agent_id.is_some();
    if let Some(id) = rest.first() {
        if !is_safe_agent_id(id) {
            return fail(ErrorCode::InvalidRequest, "the proposal id is not valid");
        }
        let proposal = service.show(id).filter(|proposal| {
            !own || proposal["proposerAgentId"].as_str() == who.agent_id.as_deref()
        });
        return match proposal {
            Some(proposal) => ok(describe_proposal(&proposal)),
            None => fail(
                ErrorCode::Rejected,
                format!("unknown_proposal: no proposal {id}"),
            ),
        };
    }
    let proposals = service.list(&ListFilter {
        states: None,
        proposer_agent_id: if own { who.agent_id.clone() } else { None },
        limit: Some(MAX_STATUS_PROPOSALS),
    });
    let described: Vec<Value> = proposals.iter().map(describe_proposal).collect();
    ok(json!({ "proposals": described }))
}
