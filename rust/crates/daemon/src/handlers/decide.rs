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
use capstan_kernel::areas::remote_decisions::{
    self, DecideOptions, Kind, ERROR_CODES, MERGE_ERROR_CODES,
};
use capstan_kernel::{KernelError, KernelResult};
use serde_json::Value;

/// Registers the handlers of the routes this module serves (decide and decide-merge).
pub fn register(map: &mut HandlerMap) {
    map.insert("decide", decide);
    map.insert("decide-merge", super::decide_merge::decide_merge);
}

/// The configuration as `capstan.toml` has it now, read with the loader of the daemon's start-up; `Err` is the text of
/// the failure.
pub(super) fn fresh_config(env: &CommandEnv<'_>) -> Result<RoleConfig, String> {
    capstan_config::load_role_config(&env.deps.options.workspace_root).map_err(|e| e.to_string())
}

/// The options of a decision by `[remote]`: without a section (or without a readable file) the defaults.
pub(super) fn options_of(remote: Option<&Remote>) -> DecideOptions {
    match remote {
        Some(remote) => DecideOptions {
            messages_per_hour: remote.messages_per_hour,
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
    let reply_to = (input.kind == Kind::Message && input.verb == "reply").then(|| input.target.clone());
    let decided = env.deps.kernel.run(move |core| {
        let context = crate::deps::new_context(core, &credential)?;
        remote_decisions::decide_with(core, &context, &input, options)
    });
    if let (Ok(result), Some(message_id)) = (&decided, reply_to) {
        if result["outcome"] == "applied" {
            close_answered_picker(env, &message_id);
        }
    }
    Some(answer(decided))
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
            let outcome = launcher.dismiss_question(&row.sender_agent_id, &row.body);
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
}
