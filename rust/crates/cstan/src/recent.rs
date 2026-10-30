//! `cstan recent --project <root|projectId> --epoch <hex> --head <n> [--limit <n>] [--json]`: the newest outcome events of
//! a local project at or below a head, from the `feed recent` route of that project's controller with the operator key.
//!
//! It is a native arm like `decide`: it never reads `CAPSTAN_SOCKET` or `CAPSTAN_TOKEN`, never starts a daemon and never
//! prints the key. The project is found the way `decide` finds it, and the answer is printed as the route gave it: a page
//! (exit 0, even when it is empty or truncated) or the ledger's own position when the epoch or head does not fit it (a
//! resync answer, exit 1).
use std::path::Path;

use capstan_client::{call_daemon, Response};
use capstan_kernel::areas::event_feed::RECENT_MAX_LIMIT;
use capstan_kernel::helpers::clean_text;
use capstan_wire::js;
use serde_json::{json, Value};

use crate::controllers::Proc;
use crate::decide::{refuse, resolve, Refused, Reply};
use crate::projects::{credential, socket_of};

/// The codes this command gives itself, besides the ones the route gives (`rejected` for a rate limit, `invalid_request`,
/// `forbidden`, `unauthorized`).
pub const CLI_ERROR_CODES: &[&str] = &["project_not_found", "no_operator_key", "not_answering"];

/// How long the controller may take to answer.
pub const TIMEOUT_MS: u64 = 10_000;

/// A command line that passed the checks of the front.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentPlan {
    pub project: String,
    pub epoch: String,
    pub head: u64,
    pub limit: Option<u32>,
    pub json: bool,
}

/// A project id is a plain word; anything else that is not an absolute path (`./x`, `../x`, `a/b`, `~`) is a relative
/// path and a usage error, never a project id.
fn project_value(value: &str) -> bool {
    value.starts_with('/')
        || (!value.is_empty()
            && value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
}

fn number<T: std::str::FromStr>(text: &str) -> Option<T> {
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

/// Parses the words after `recent`: each flag at most once, in any order. `None` is a usage error.
pub fn plan(args: &[String]) -> Option<RecentPlan> {
    let (mut project, mut epoch, mut head, mut limit) = (None, None, None, None);
    let mut json = false;
    let mut at = 0;
    while let Some(word) = args.get(at) {
        if word == "--json" {
            if json {
                return None;
            }
            json = true;
            at += 1;
            continue;
        }
        let slot = match word.as_str() {
            "--project" => &mut project,
            "--epoch" => &mut epoch,
            "--head" => &mut head,
            "--limit" => &mut limit,
            _ => return None,
        };
        let value = args.get(at + 1).filter(|v| !v.starts_with("--"))?;
        if slot.replace(value).is_some() {
            return None;
        }
        at += 2;
    }
    let project = project.filter(|p| project_value(p))?.clone();
    let epoch = epoch
        .filter(|e| e.len() == 16 && e.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))?
        .clone();
    let head: u64 = number(head?)?;
    let limit = match limit {
        Some(text) => Some(number::<u32>(text).filter(|n| (1..=RECENT_MAX_LIMIT).contains(n))?),
        None => None,
    };
    Some(RecentPlan {
        project,
        epoch,
        head,
        limit,
        json,
    })
}

fn call(root: &Path, plan: &RecentPlan, timeout_ms: u64) -> Result<Value, Refused> {
    let key = credential(root).ok_or_else(|| {
        refuse(
            "no_operator_key",
            format!("no usable operator key in {}/.capstan", root.display()),
        )
    })?;
    let silent = || {
        refuse(
            "not_answering",
            format!("the controller of {} does not answer", root.display()),
        )
    };
    let mut argv = vec![
        "recent".to_string(),
        "--epoch".to_string(),
        plan.epoch.clone(),
        "--head".to_string(),
        plan.head.to_string(),
    ];
    if let Some(limit) = plan.limit {
        argv.extend(["--limit".to_string(), limit.to_string()]);
    }
    match call_daemon(&socket_of(root), key.expose(), "feed", &argv, timeout_ms) {
        Ok(Response::Ok { result }) => {
            let text = result
                .map(|value| js::stringify(&value, 0).to_utf8_lossy())
                .unwrap_or_else(|| "{}".to_string());
            serde_json::from_str::<Value>(&text)
                .ok()
                .filter(Value::is_object)
                .ok_or_else(silent)
        }
        Ok(Response::Refused { code, message }) => Err(Refused {
            code: clean_text(&code.to_utf8_lossy()),
            message: clean_text(&message.to_utf8_lossy()),
        }),
        _ => Err(silent()),
    }
}

/// The readable line of one event: `<seq>.<k> <ts> <type> <entity type>:<entity id>`, with `<from> -> <to>` when it has.
fn line_of(event: &Value) -> String {
    let text = |value: &Value, key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let number = |key: &str| event.get(key).and_then(Value::as_u64).unwrap_or(0);
    let entity = event.get("entity").cloned().unwrap_or(Value::Null);
    let mut line = format!(
        "{}.{} {} {} {}:{}",
        number("seq"),
        number("k"),
        text(event, "ts"),
        text(event, "type"),
        text(&entity, "type"),
        text(&entity, "id")
    );
    let (from, to) = (text(event, "from"), text(event, "to"));
    if !to.is_empty() {
        line.push_str(&format!(" {from} -> {to}"));
    }
    clean_text(&line)
}

/// Makes the call of `plan` and words the outcome. `procs` are the running controllers, for a project id.
pub fn run(plan: &RecentPlan, procs: &[Proc], timeout_ms: u64) -> Reply {
    let answer = resolve(&plan.project, procs).and_then(|root| call(&root, plan, timeout_ms));
    let reply = |stdout: String, stderr: String, exit: i32| Reply {
        stdout,
        stderr,
        exit,
    };
    match answer {
        Ok(result) => {
            let events = result.get("events").and_then(Value::as_array);
            match (events, plan.json) {
                (Some(_), true) => reply(format!("{result}\n"), String::new(), 0),
                (Some(events), false) => {
                    let lines: String = events.iter().map(|e| line_of(e) + "\n").collect();
                    reply(lines, String::new(), 0)
                }
                (None, true) => reply(format!("{result}\n"), String::new(), 1),
                (None, false) => {
                    let text =
                        |key: &str| result.get(key).map(Value::to_string).unwrap_or_default();
                    reply(
                        String::new(),
                        clean_text(&format!(
                            "cstan recent: resync: the ledger is at epoch {} head {}",
                            text("epoch").trim_matches('"'),
                            text("head")
                        )) + "\n",
                        1,
                    )
                }
            }
        }
        Err(Refused { code, message }) if plan.json => reply(
            format!(
                "{}\n",
                json!({"v": 1, "error": {"code": code, "message": message}})
            ),
            String::new(),
            1,
        ),
        Err(Refused { code, message }) => reply(
            String::new(),
            format!("cstan recent: {code}: {message}\n"),
            1,
        ),
    }
}
