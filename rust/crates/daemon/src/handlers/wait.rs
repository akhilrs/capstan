//! `wait` (src/commands/wait.ts): blocks an agent until mail arrives.
//!
//! The long poll runs on the connection's own thread and takes the kernel once per ledger step; between steps it sleeps on
//! the ledger's change counter (`KernelHandle::wait_change`), capped at the poll interval of Node so a missed wake-up costs
//! at most that long. The clock is read where Node reads `now()`, so a seeded run consumes the same stream.

use super::env::inbox_reply;
use super::shared::{
    fail, map_kernel_error, ok, CommandCall, CommandEnv, CommandResponse, ErrorCode, WaitEntry,
    WAIT_POLL_MS,
};
use super::HandlerMap;
use crate::deps::new_context;
use capstan_config::DEFAULT_WAIT_TIMEOUT_SECONDS;
use capstan_kernel::KernelError;
use serde_json::json;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Registers the handler of the route this module serves (wait).
pub fn register(map: &mut HandlerMap) {
    map.insert("wait", wait);
}

/// Ends this wait's entry, however the handler returns: removes it from the table when it is still the current one and
/// tells a newer wait that is waiting for it to be done.
struct Finish<'a> {
    env: &'a CommandEnv<'a>,
    agent_id: String,
    done: Arc<(Mutex<bool>, Condvar)>,
}

impl Drop for Finish<'_> {
    fn drop(&mut self) {
        {
            let mut waits = self
                .env
                .state
                .waits
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if waits
                .get(&self.agent_id)
                .is_some_and(|entry| Arc::ptr_eq(&entry.done, &self.done))
            {
                waits.remove(&self.agent_id);
            }
        }
        let (flag, condvar) = &*self.done;
        *flag.lock().unwrap_or_else(|p| p.into_inner()) = true;
        condvar.notify_all();
    }
}

fn wait(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let Some(agent) = call.identity.agent.as_ref() else {
        return Some(fail(ErrorCode::Forbidden, "only an agent waits"));
    };
    // `--digest` (the current client) asks for info mail apart and acknowledged; no argument is the earlier shape.
    // `--limit <seconds>` (sent by `cstan wait --until`) only shortens the host's limit, so the last round of a long
    // wait ends at the caller's deadline.
    let mut digest = false;
    let mut shorter_ms = None;
    let mut rest = call.args;
    if let [first, tail @ ..] = rest {
        if first == "--digest" {
            digest = true;
            rest = tail;
        }
    }
    match rest {
        [] => {}
        [flag, seconds] if flag == "--limit" => match seconds.parse::<u64>() {
            Ok(seconds) if seconds >= 1 => shorter_ms = Some(seconds as i64 * 1000),
            _ => {
                return Some(fail(
                    ErrorCode::InvalidRequest,
                    "wait --limit takes whole seconds",
                ))
            }
        },
        _ => return Some(fail(ErrorCode::InvalidRequest, "wait takes no arguments")),
    }
    let auto_ack_info = env
        .config()
        .is_none_or(|config| config.messages.auto_ack_info);
    let kernel = &env.deps.kernel;
    let signal = call.signal;
    let host_ms = call
        .limit_ms
        .map_or(DEFAULT_WAIT_TIMEOUT_SECONDS * 1000, |ms| ms as i64);
    let deadline = kernel.now_ms() + shorter_ms.map_or(host_ms, |ms| ms.min(host_ms));
    let done = Arc::new((Mutex::new(false), Condvar::new()));
    let previous = {
        let mut waits = env.state.waits.lock().unwrap_or_else(|p| p.into_inner());
        waits.insert(
            agent.agent_id.clone(),
            WaitEntry {
                signal: signal.clone(),
                done: Arc::clone(&done),
            },
        )
    };
    let _finish = Finish {
        env,
        agent_id: agent.agent_id.clone(),
        done,
    };
    if let Some(previous) = &previous {
        previous.signal.abort("superseded");
        // The newer wait starts only once the older one has let go, or when it is itself ended.
        let (flag, condvar) = &*previous.done;
        let mut finished = flag.lock().unwrap_or_else(|p| p.into_inner());
        while !*finished && signal.reason().is_none() {
            finished = condvar
                .wait_timeout(finished, Duration::from_millis(20))
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
    }
    if signal.reason().is_some() {
        return env.abort_reply(signal);
    }
    // Whether mail that wakes the agent under the message policy is pending. Read-only: a wait that is superseded or
    // aborted has pulled nothing.
    let waking = |credential: &str| -> Result<bool, KernelError> {
        let credential = credential.to_string();
        let pending = kernel.run(move |core| core.wake_pending(&credential))?;
        Ok(pending["wake"].as_bool().unwrap_or(false))
    };
    let attempt = || -> Result<Option<CommandResponse>, KernelError> {
        let credential = call.credential.to_string();
        let mut seen = kernel.change_counter();
        let mut woke = waking(&credential)?;
        if !woke && kernel.now_ms() < deadline {
            let begun = kernel.run({
                let credential = credential.clone();
                move |core| {
                    let context = new_context(core, &credential)?;
                    core.begin_wait(&context)
                }
            })?;
            let wait_id = begun["waitId"].as_str().unwrap_or("").to_string();
            let polled = (|| -> Result<bool, KernelError> {
                while !woke && signal.reason().is_none() && kernel.now_ms() < deadline {
                    let left = (deadline - kernel.now_ms()).max(1);
                    let slice = left.min(WAIT_POLL_MS as i64).max(1) as u64;
                    kernel.wait_change(seen, Duration::from_millis(slice), Some(signal));
                    if signal.reason().is_some() {
                        break;
                    }
                    seen = kernel.change_counter();
                    woke = waking(&credential)?;
                }
                Ok(woke)
            })();
            env.end_wait(&credential, &wait_id);
            woke = polled?;
        }
        if signal.reason().is_some() {
            return Ok(None);
        }
        // Delivery happens once, here, also when the wait timed out.
        let (_, mut reply) = kernel.run({
            let credential = credential.clone();
            move |core| inbox_reply(core, &credential, digest, auto_ack_info)
        })?;
        // Timed out means nothing came: no action mail and no info block in the reply.
        let delivered = ["count", "infoCount"]
            .iter()
            .any(|key| reply.get(*key).and_then(|n| n.as_i64()).unwrap_or(0) > 0);
        reply.insert("timedOut".into(), json!(!woke && !delivered));
        Ok(Some(ok(serde_json::Value::Object(reply))))
    };
    match attempt() {
        // An abort during the wait: the reply the reason asks for, or nothing when the client is gone.
        Ok(None) => env.abort_reply(signal),
        Ok(Some(response)) => Some(response),
        Err(error) => {
            if signal.reason().is_some() {
                return env.abort_reply(signal);
            }
            env.log(
                "wait_failed",
                json!({"agentId": agent.agent_id, "error": error.to_string()}),
            );
            Some(map_kernel_error(&error))
        }
    }
}
