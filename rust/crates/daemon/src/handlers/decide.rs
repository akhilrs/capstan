//! The decide command (Rust only, operator only): `decide <requestId> --expect <hex> --nonce <n> [--client <label>] <verb> ...`,
//! one remote decision on a request id of the event feed (`areas::remote_decisions`). The handler parses the call arguments and
//! makes one kernel call, which applies the decision or forwards it to the active PM as a message; it keeps no state and
//! never reaches the launcher, so no pane receives anything. Request ids are `kind:id` and are taken whole: `decide` has no
//! entry in id_prefix.rs.

use super::shared::{
    fail, map_kernel_error, ok, CommandCall, CommandEnv, CommandResponse, ErrorCode,
};
use super::HandlerMap;
use capstan_kernel::areas::remote_decisions::{self, ERROR_CODES};
use capstan_kernel::{KernelError, KernelResult};
use serde_json::Value;

/// Registers the handler of the route this module serves (decide).
pub fn register(map: &mut HandlerMap) {
    map.insert("decide", decide);
}

/// A refusal of the kernel (`code: message`, the code one of `ERROR_CODES`) answers with that code; the other failures map
/// as everywhere.
fn refused(error: &KernelError) -> CommandResponse {
    if let KernelError::Controller(message) = error {
        if let Some((code, rest)) = message.split_once(':') {
            if ERROR_CODES.contains(&code) {
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
    let credential = call.credential.to_string();
    Some(answer(env.deps.kernel.run(move |core| {
        let context = crate::deps::new_context(core, &credential)?;
        core.decide(&context, &input)
    })))
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
