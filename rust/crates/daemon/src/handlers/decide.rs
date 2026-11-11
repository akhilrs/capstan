//! The decide command (Rust only, operator only): `decide <requestId> --expect <hex> --nonce <n> [--client <label>] <verb> ...`,
//! one remote decision on a request id of the event feed (`areas::remote_decisions`). The handler parses the call arguments and
//! makes one kernel call, which applies the decision or forwards it to the active PM as a message; it keeps no state and
//! never reaches the launcher, so no pane receives anything. Request ids are `kind:id` and are taken whole: `decide` has no
//! entry in id_prefix.rs.

use super::shared::{
    fail, map_kernel_error, ok, CommandCall, CommandEnv, CommandResponse, ErrorCode,
};
use super::HandlerMap;
use capstan_config::{Remote, RoleConfig};
use capstan_kernel::areas::integrations;
use capstan_kernel::areas::remote_decisions::{
    self, DecideOptions, Kind, ERROR_CODES, MERGE_ERROR_CODES,
};
use capstan_kernel::integrate::GitRepo;
use capstan_kernel::types::MutationContext;
use capstan_kernel::{KernelError, KernelResult};
use capstan_operator::api::OperatorFailure;
use serde_json::Value;

/// Registers the handlers of the routes this module serves (decide and decide-merge).
pub fn register(map: &mut HandlerMap) {
    map.insert("decide", decide);
    map.insert("decide-merge", super::decide_merge::decide_merge);
}

/// The configuration as `capstan.toml` has it now, read with the loader of the daemon's start-up; `Err` is the text of
/// the failure.
pub(super) fn fresh_config(env: &CommandEnv<'_>) -> Result<RoleConfig, String> {
    capstan_config::load_role_config_with(
        &env.deps.options.workspace_root,
        &env.deps.options.trust_store,
    )
    .map_err(|e| e.to_string())
}

/// The options of a decision by `[remote]`: without a section (or without a readable file) the defaults.
pub(super) fn options_of(remote: Option<&Remote>) -> DecideOptions {
    match remote {
        Some(remote) => DecideOptions {
            messages_per_hour: remote.messages_per_hour,
            ..DecideOptions::default()
        },
        None => DecideOptions::default(),
    }
}

/// A refusal of the kernel (`code: message`, the code one of `ERROR_CODES` or `MERGE_ERROR_CODES`) answers with that
/// code; the other failures map as everywhere.
pub(super) fn refused(error: &KernelError) -> CommandResponse {
    if let KernelError::Controller(message) = error {
        if let Some((code, rest)) = message.split_once(':') {
            if ERROR_CODES.contains(&code) || MERGE_ERROR_CODES.contains(&code) {
                if let Some(code) = ErrorCode::parse(code) {
                    return fail(code, rest.trim());
                }
            }
        }
    }
    map_kernel_error(error)
}

fn answer(result: KernelResult<Value>) -> CommandResponse {
    match result {
        Ok(value) => ok(value),
        Err(error) => refused(&error),
    }
}

fn decide(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let input = match remote_decisions::parse(call.args) {
        Ok(input) => input,
        Err(refusal) => {
            return Some(match ErrorCode::parse(refusal.code) {
                Some(code) => fail(code, refusal.message),
                None => fail(ErrorCode::InvalidRequest, refusal.message),
            })
        }
    };
    if input.kind == Kind::Integration && input.verb == "merge" {
        return Some(fail(
            ErrorCode::InvalidRequest,
            "an integration merge goes through decide-merge",
        ));
    }
    let options = match fresh_config(env) {
        Ok(config) => options_of(Some(&config.remote)),
        Err(error) => {
            env.log("decide_config_failed", serde_json::json!({"error": error}));
            options_of(None)
        }
    };
    let credential = call.credential.to_string();
    let reply_to =
        (input.kind == Kind::Message && input.verb == "reply").then(|| input.target.clone());
    // A discard removes the integration's branch once the decision is applied: its name and head, read before.
    let discard_id =
        (input.kind == Kind::Integration && input.verb == "discard").then(|| input.target.clone());
    let discarding = if let Some(id) = discard_id.clone() {
        env.deps
            .kernel
            .run(move |core| {
                let row = integrations::integration_row(core.kernel(), &id)?;
                let repo = core.repo_of_integration(&id)?;
                Ok(row.map(|row| (row, repo)))
            })
            .ok()
            .flatten()
            .and_then(|(row, repo)| {
                // A set member's branch is in its own repo.
                row.head_sha
                    .clone()
                    .filter(|_| row.state == "merged")
                    .map(|sha| (row.branch.clone(), sha, repo))
            })
    } else {
        None
    };
    let op_target = (input.kind == Kind::Op).then(|| input.target.clone());
    let decided = if input.kind == Kind::Op && input.verb == "approve-as" {
        // The choice makes grants and windows: the Operator service decides it, with its configuration and the floor now.
        match env.deps.operator.clone() {
            Some(operator) => {
                operator
                    .remote_decide(&credential, input, options)
                    .map_err(|failure| match failure {
                        OperatorFailure::Kernel(error) => error,
                        OperatorFailure::Operator(error) => {
                            KernelError::controller(format!("{}: {}", error.code, error.message))
                        }
                    })
            }
            None => Err(remote_decisions::Refusal {
                code: "needs_terminal",
                message: "the Operator is not enabled on this host; approve at the terminal".into(),
            }
            .into()),
        }
    } else {
        env.deps.kernel.run(move |core| {
            let context = crate::deps::new_context(core, &credential)?;
            remote_decisions::decide_with(core, &context, &input, options)
        })
    };
    if let (Ok(result), Some(proposal_id)) = (&decided, op_target) {
        if result["repeat"] == false
            && matches!(result["outcome"].as_str(), Some("applied" | "forwarded"))
        {
            close_operator_picker(env, &proposal_id);
        }
    }
    if let (Ok(result), Some(message_id)) = (&decided, reply_to) {
        if result["outcome"] == "applied" {
            close_answered_picker(env, &message_id);
        }
    }
    if let (Ok(result), Some(id)) = (&decided, discard_id) {
        if result["outcome"] == "applied" && result["repeat"] != true {
            discard_member_after(env, &id);
        }
    }
    if let (Ok(result), Some((branch, head_sha, member_repo))) = (&decided, discarding) {
        if result["outcome"] == "applied" && result["repeat"] != true {
            let repo = GitRepo::new(env.deps.repo_root(member_repo.as_deref().unwrap_or("")));
            let removed = repo.delete_branch_at(&branch, &head_sha).unwrap_or(false);
            if !removed {
                // `cstan cleanup` offers the branch of a discarded integration again.
                env.log(
                    "integration_branch_not_removed",
                    serde_json::json!({"branch": branch}),
                );
            }
        }
    }
    Some(answer(decided))
}

/// The message to the PM when the steps after a member's discard failed.
fn discard_failure_text(set_id: &str, repo: &str, integration_id: &str, error: &str) -> String {
    format!(
        "The remote discard of integration {integration_id} (repo {repo} of set {set_id}) was applied, but resetting the set afterwards failed: {error}. The discard is idempotent: rerun cstan integrate discard {set_id} --repo {repo} to finish it."
    )
}

/// A discarded set member resets its set's gates, cancels its runs and may end the set; a plain integration is not one.
/// This runs after the decision was applied, so a failure cannot undo it: it is logged and the PM is told the set and the
/// command to rerun. The discard is idempotent: the user can rerun `cstan integrate discard <set> --repo <r>`, which skips
/// the member already discarded and repeats the cancel, reset and set move.
fn discard_member_after(env: &CommandEnv<'_>, integration_id: &str) {
    let id = integration_id.to_string();
    let service = env.deps.verify.clone();
    let controller = env.deps.credential().to_string();
    let result = env.deps.kernel.run(move |core| {
        let abort = |run_id: &str| {
            if let Some(service) = &service {
                service.abort(run_id);
            }
        };
        integrations::after_member_discard(
            core.kernel(),
            &|| {
                crate::deps::new_context(core, &controller).unwrap_or_else(|_| MutationContext {
                    credential: controller.clone(),
                    request_id: String::new(),
                    idempotency_key: String::new(),
                    expected_version: 0,
                    input_revision: 0,
                })
            },
            &id,
            &abort,
        )
    });
    if let Err(error) = result {
        env.log(
            "integration_set_discard_failed",
            serde_json::json!({"integrationId": integration_id, "error": error.message()}),
        );
        let id = integration_id.to_string();
        let credential = env.deps.credential().to_string();
        let text = error.message().to_string();
        let told = env.deps.kernel.run(move |core| {
            let (Some(set_id), Some(repo)) = (
                core.set_of_integration(&id)?,
                core.repo_of_integration(&id)?,
            ) else {
                return Ok(false);
            };
            let Some(parties) =
                capstan_kernel::areas::message_notices::notice_parties(core.kernel())?
            else {
                return Ok(false);
            };
            let context = crate::deps::new_context(core, &credential)?;
            core.enqueue_message(
                &context,
                &serde_json::json!({
                    "recipientAgentId": parties.pm.agent_id,
                    "body": discard_failure_text(&set_id, &repo, &id, &text),
                    "actionNeeded": true,
                }),
            )?;
            Ok(true)
        });
        if !matches!(told, Ok(true)) {
            env.log(
                "integration_set_discard_not_told",
                serde_json::json!({"integrationId": integration_id}),
            );
        }
    }
}

/// After a proposal was decided (or forwarded) from a remote client the PM may still show its picker: one Esc closes it,
/// only when the screen shows the question of this proposal and hash (the launcher checks). Best effort and advisory:
/// the approval never waits on it, and a busy PM or a picker worded differently stays open. Runs on its own thread.
fn close_operator_picker(env: &CommandEnv<'_>, proposal_id: &str) {
    let id = proposal_id.to_string();
    let lookup = env.deps.kernel.run(move |core| {
        let record = core.operator_proposal(&id)?;
        let pm = capstan_kernel::areas::message_notices::notice_parties(core.kernel())?
            .map(|parties| parties.pm.agent_id);
        Ok((record["commandSha"].as_str().map(str::to_string), pm))
    });
    let (sha, pm) = match lookup {
        Ok((sha, pm)) => (sha, pm),
        Err(error) => {
            env.log(
                "operator_picker_closed",
                serde_json::json!({
                    "agentId": null,
                    "proposalId": proposal_id,
                    "sent": false,
                    "error": error.message(),
                }),
            );
            return;
        }
    };
    let (Some(agent_id), Some(sha)) = (pm, sha) else {
        env.log(
            "operator_picker_closed",
            serde_json::json!({
                "agentId": null,
                "proposalId": proposal_id,
                "sent": false,
                "error": "no_active_pm",
            }),
        );
        return;
    };
    let Some(launcher) = env.deps.launcher.clone() else {
        return;
    };
    let question = capstan_config::operator_texts::fill(
        capstan_config::operator_texts::OPERATOR_PICKER_QUESTION,
        &[
            ("<proposal-id>", proposal_id),
            ("<hash12>", sha.get(..12).unwrap_or(&sha)),
        ],
    );
    let deps = env.deps.clone();
    let proposal_id = proposal_id.to_string();
    let _ = std::thread::Builder::new()
        .name("close-operator-picker".into())
        .spawn(move || {
            let outcome = launcher.dismiss_question(&agent_id, &question, &[]);
            deps.detail_log(
                "operator_picker_closed",
                serde_json::json!({
                    "agentId": agent_id,
                    "proposalId": proposal_id,
                    "sent": matches!(outcome, Ok(true)),
                    "error": outcome.err().map(|error| error.to_string()),
                }),
            );
        });
}

/// After a question was answered from a remote client: the agent that asked may still show it in a picker, which
/// holds the answer back until someone presses a key at the terminal. One Esc closes it, only when the screen shows
/// that question (the launcher checks); the answer is then delivered as a message. Runs on its own thread.
fn close_answered_picker(env: &CommandEnv<'_>, message_id: &str) {
    let Some(launcher) = env.deps.launcher.clone() else {
        return;
    };
    let id = message_id.to_string();
    let Ok(Some(row)) = env.deps.kernel.run(move |core| {
        capstan_kernel::areas::user_messages::user_message_row(core.kernel(), &id)
    }) else {
        return;
    };
    let deps = env.deps.clone();
    let _ = std::thread::Builder::new()
        .name("close-answered-picker".into())
        .spawn(move || {
            let outcome = launcher.dismiss_question(&row.sender_agent_id, &row.body, &row.options);
            deps.detail_log(
                "question_picker_closed",
                serde_json::json!({
                    "agentId": row.sender_agent_id,
                    "sequence": row.sequence,
                    "sent": matches!(outcome, Ok(true)),
                    "error": outcome.err().map(|error| error.to_string()),
                }),
            );
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use capstan_kernel::areas::remote_decisions::Refusal;

    #[test]
    fn every_code_of_the_kernel_parses_and_round_trips() {
        for code in ERROR_CODES {
            let parsed = ErrorCode::parse(code).unwrap_or_else(|| panic!("{code} has no code"));
            assert_eq!(parsed.as_str(), *code);
        }
    }

    #[test]
    fn the_options_take_messages_per_hour_from_remote() {
        let remote = Remote {
            messages_per_hour: 7,
            ..Remote::default()
        };
        assert_eq!(options_of(Some(&remote)).messages_per_hour, 7);
        assert_eq!(options_of(Some(&Remote::default())).messages_per_hour, 30);
        assert_eq!(options_of(None).messages_per_hour, 30);
    }

    #[test]
    fn every_merge_code_parses_and_round_trips() {
        for code in MERGE_ERROR_CODES {
            let parsed = ErrorCode::parse(code).unwrap_or_else(|| panic!("{code} has no code"));
            assert_eq!(parsed.as_str(), *code);
            let error: KernelError = remote_decisions::Refusal {
                code,
                message: "text".into(),
            }
            .into();
            assert_eq!(refused(&error), fail(parsed, "text"));
        }
    }

    #[test]
    fn a_refusal_keeps_its_code_and_text() {
        let error: KernelError = Refusal {
            code: "not_waiting",
            message: "op:x is not waiting".into(),
        }
        .into();
        assert_eq!(
            refused(&error),
            fail(ErrorCode::NotWaiting, "op:x is not waiting")
        );
        let other = KernelError::Controller("something else: broke".into());
        assert_eq!(
            refused(&other),
            fail(ErrorCode::Error, "the command failed")
        );
    }

    #[test]
    fn the_failure_message_names_the_set_and_the_rerun_command() {
        let text = super::discard_failure_text("set-1", "web", "int-9", "disk full");
        assert!(
            text.contains("set set-1") && text.contains("disk full"),
            "{text}"
        );
        assert!(
            text.contains("rerun cstan integrate discard set-1 --repo web"),
            "{text}"
        );
    }
}
