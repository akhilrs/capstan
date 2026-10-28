//! The typed calls of the daemon's `feed` command (operator only): `FeedCall` is one subcommand with its values, `args`
//! the argument list the daemon parses, `call_feed` one call on a fresh connection.

use crate::{call_daemon, CallError};
use capstan_wire::Response;
use std::path::Path;

/// One subcommand of `feed`. Optional values are left out of the arguments when `None`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeedCall {
    Head,
    Page {
        after_seq: u64,
        after_k: u32,
        limit: u32,
        anchor: Option<String>,
    },
    Snapshot,
    Waiting {
        after: Option<String>,
        limit: u32,
        kind: Option<String>,
    },
    Agents {
        after: Option<String>,
        limit: u32,
    },
    Plans {
        after: Option<String>,
        limit: u32,
    },
    Item {
        section: String,
        key: String,
    },
    Hash {
        request_id: String,
    },
}

fn flag(args: &mut Vec<String>, name: &str, value: &Option<String>) {
    if let Some(value) = value {
        args.push(name.to_string());
        args.push(value.clone());
    }
}

impl FeedCall {
    /// The arguments of the `feed` command for this call.
    pub fn args(&self) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();
        match self {
            FeedCall::Head => args.push("head".into()),
            FeedCall::Snapshot => args.push("snapshot".into()),
            FeedCall::Page {
                after_seq,
                after_k,
                limit,
                anchor,
            } => {
                args.extend([
                    "page".into(),
                    "--after".into(),
                    format!("{after_seq}:{after_k}"),
                ]);
                args.extend(["--limit".into(), limit.to_string()]);
                flag(&mut args, "--anchor", anchor);
            }
            FeedCall::Waiting { after, limit, kind } => {
                args.extend(["waiting".into(), "--limit".into(), limit.to_string()]);
                flag(&mut args, "--after", after);
                flag(&mut args, "--kind", kind);
            }
            FeedCall::Agents { after, limit } | FeedCall::Plans { after, limit } => {
                let name = if matches!(self, FeedCall::Agents { .. }) {
                    "agents"
                } else {
                    "plans"
                };
                args.extend([name.into(), "--limit".into(), limit.to_string()]);
                flag(&mut args, "--after", after);
            }
            FeedCall::Item { section, key } => {
                args.extend(["item".into(), section.clone(), key.clone()]);
            }
            FeedCall::Hash { request_id } => args.extend(["hash".into(), request_id.clone()]),
        }
        args
    }
}

/// `feed <call>` on the daemon at `socket`. A refusal (`forbidden`, `invalid_request`, ...) is `Response::Refused`.
pub fn call_feed(
    socket: &Path,
    credential: &str,
    call: &FeedCall,
    timeout_ms: u64,
) -> Result<Response, CallError> {
    call_daemon(socket, credential, "feed", &call.args(), timeout_ms)
}
