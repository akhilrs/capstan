//! `cstan decide [--project <root|projectId>] [--json] <requestId> <decide argv>`: one remote decision on a waiting item of
//! a local project, sent to the `decide` route of that project's controller with the operator key.
//!
//! The command never reads `CAPSTAN_SOCKET` or `CAPSTAN_TOKEN` and never starts a daemon: it finds the project (a root, or
//! a project id among the running controllers), reads the operator key the way the daemon checks its own files and makes
//! one call. The words after the request id go to the route unchanged, and `capstan_kernel`'s parser checks them first, so
//! the command line and the route share one grammar.
use std::path::{Path, PathBuf};

use capstan_client::{call_daemon, Response};
use capstan_kernel::areas::remote_decisions;
use capstan_kernel::helpers::clean_text;
use capstan_wire::js;
use serde_json::{json, Value};

use crate::controllers::Proc;
use crate::projects::{
    credential, discover, project_id, socket_of, Credential, EndpointState, ProjectProbe,
};

/// The codes this command gives itself, besides the ones the route gives (`remote_decisions::ERROR_CODES`, `forbidden`,
/// `unauthorized`).
pub const CLI_ERROR_CODES: &[&str] = &["project_not_found", "no_operator_key", "not_answering"];

/// How long the controller may take to answer.
pub const TIMEOUT_MS: u64 = 10_000;

/// A command line that passed the checks of the front: the project, the output form and the arguments of the route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecidePlan {
    pub project: String,
    pub json: bool,
    /// `<requestId> --expect ... --nonce ... <verb> ...`, as given.
    pub argv: Vec<String>,
}

/// What a run prints and the exit code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub stdout: String,
    pub stderr: String,
    pub exit: i32,
}

/// Parses the words after `decide`. `--project` and `--json` come only before the request id, each at most once; the
/// rest goes to the grammar of the route. `None` is a usage error.
pub fn plan(args: &[String]) -> Option<DecidePlan> {
    let mut project: Option<&String> = None;
    let mut json = false;
    let mut at = 0;
    while let Some(word) = args.get(at) {
        match word.as_str() {
            "--project" => {
                let value = args
                    .get(at + 1)
                    .filter(|v| !v.is_empty() && !v.starts_with("--"))?;
                if project.replace(value).is_some() {
                    return None;
                }
                at += 2;
            }
            "--json" => {
                if json {
                    return None;
                }
                json = true;
                at += 1;
            }
            flag if flag.starts_with("--") => return None,
            _ => break,
        }
    }
    let argv = args.get(at..)?.to_vec();
    remote_decisions::parse(&argv).ok()?;
    Some(DecidePlan {
        project: project?.clone(),
        json,
        argv,
    })
}

/// Finds the keys of project ids without asking any controller: the key file is all that is read.
struct KeyOnly;

impl ProjectProbe for KeyOnly {
    fn key(&self, root: &Path) -> Option<Credential> {
        credential(root)
    }

    fn ask(&self, _: &Path, _: &Credential) -> EndpointState {
        EndpointState::Ready
    }
}

/// A refusal: one of `CLI_ERROR_CODES` or a code of the route, and a text for people.
struct Refused {
    code: String,
    message: String,
}

fn refuse(code: &str, message: impl Into<String>) -> Refused {
    Refused {
        code: code.to_string(),
        message: clean_text(&message.into()),
    }
}

/// The root a `--project` value names. A value starting with `/` is a root; any other value is a project id.
fn resolve(value: &str, procs: &[Proc]) -> Result<PathBuf, Refused> {
    if value.starts_with('/') {
        let root = std::fs::canonicalize(value)
            .ok()
            .filter(|root| root.join(".capstan").is_dir())
            .ok_or_else(|| refuse("project_not_found", format!("no project at {value}")))?;
        return Ok(root);
    }
    discover(procs, &[], &KeyOnly)
        .into_iter()
        .find(|endpoint| {
            endpoint.state != EndpointState::Gone
                && project_id(&endpoint.root).as_deref() == Some(value)
        })
        .map(|endpoint| endpoint.root)
        .ok_or_else(|| {
            refuse(
                "project_not_found",
                format!("no running controller has the project id {value}"),
            )
        })
}

fn call(root: &Path, argv: &[String], timeout_ms: u64) -> Result<Value, Refused> {
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
    match call_daemon(&socket_of(root), key.expose(), "decide", argv, timeout_ms) {
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

/// The readable line of an answer: `<requestId> <verb>: <outcome>`, with `(repeat)` and the message id when there are
/// any.
fn line_of(result: &Value) -> String {
    let text = |key: &str| result.get(key).and_then(Value::as_str).unwrap_or("");
    let mut line = format!(
        "{} {}: {}",
        text("requestId"),
        text("verb"),
        text("outcome")
    );
    if result.get("repeat").and_then(Value::as_bool) == Some(true) {
        line.push_str(" (repeat)");
    }
    if let Some(id) = result.get("messageId").and_then(Value::as_str) {
        line.push_str(&format!(", message {id}"));
    }
    clean_text(&line)
}

/// Makes the call of `plan` and words the outcome. `procs` are the running controllers, for a project id.
pub fn run(plan: &DecidePlan, procs: &[Proc], timeout_ms: u64) -> Reply {
    let answer = resolve(&plan.project, procs).and_then(|root| call(&root, &plan.argv, timeout_ms));
    match answer {
        Ok(result) if plan.json => Reply {
            stdout: format!("{result}\n"),
            stderr: String::new(),
            exit: 0,
        },
        Ok(result) => Reply {
            stdout: format!("{}\n", line_of(&result)),
            stderr: String::new(),
            exit: 0,
        },
        Err(Refused { code, message }) if plan.json => Reply {
            stdout: format!(
                "{}\n",
                json!({"v": 1, "error": {"code": code, "message": message}})
            ),
            stderr: String::new(),
            exit: 1,
        },
        Err(Refused { code, message }) => Reply {
            stdout: String::new(),
            stderr: format!("cstan decide: {code}: {message}\n"),
            exit: 1,
        },
    }
}
