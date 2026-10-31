//! `cstan merge-targets --project <root|projectId> [--json]`: the branches a remote merge of that project may go into,
//! from the `decide-merge targets` route of its controller with the operator key. It changes nothing.
//!
//! A native arm like `decide` and `recent`: it never reads `CAPSTAN_SOCKET` or `CAPSTAN_TOKEN`, never starts a daemon
//! and never prints the key. The answer is `{enabled, defaultBranch, checkedOut, branches}`; without `--json` one line
//! per branch, the checked-out one and the default marked.
use std::path::Path;

use capstan_client::{call_daemon, Response};
use capstan_kernel::helpers::clean_text;
use capstan_wire::js;
use serde_json::{json, Value};

use crate::controllers::Proc;
use crate::decide::{refuse, resolve, Refused, Reply};
use crate::projects::{credential, socket_of};

/// How long the controller may take to answer (a few git calls).
pub const TIMEOUT_MS: u64 = 20_000;

/// A command line that passed the checks of the front.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetsPlan {
    pub project: String,
    pub json: bool,
}

/// Parses the words after `merge-targets`. `None` is a usage error.
pub fn plan(args: &[String]) -> Option<TargetsPlan> {
    let (mut project, mut json) = (None, false);
    let mut at = 0;
    while let Some(word) = args.get(at) {
        match word.as_str() {
            "--json" if !json => {
                json = true;
                at += 1;
            }
            "--project" if project.is_none() => {
                let value = args.get(at + 1).filter(|v| !v.starts_with("--"))?;
                project = Some(value.clone());
                at += 2;
            }
            _ => return None,
        }
    }
    let project = project.filter(|p| {
        p.starts_with('/')
            || (!p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
    })?;
    Some(TargetsPlan { project, json })
}

fn call(root: &Path, timeout_ms: u64) -> Result<Value, Refused> {
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
    match call_daemon(
        &socket_of(root),
        key.expose(),
        "decide-merge",
        &["targets".to_string()],
        timeout_ms,
    ) {
        Ok(Response::Ok { result }) => {
            let text = result
                .map(|value| js::stringify(&value, 0).to_utf8_lossy())
                .unwrap_or_else(|| "{}".to_string());
            serde_json::from_str::<Value>(&text)
                .ok()
                .filter(|value| value.get("branches").is_some_and(Value::is_array))
                .ok_or_else(silent)
        }
        Ok(Response::Refused { code, message }) => Err(Refused {
            code: clean_text(&code.to_utf8_lossy()),
            message: clean_text(&message.to_utf8_lossy()),
        }),
        _ => Err(silent()),
    }
}

/// Makes the call of `plan` and words the outcome. `procs` are the running controllers, for a project id.
pub fn run(plan: &TargetsPlan, procs: &[Proc], timeout_ms: u64) -> Reply {
    let reply = |stdout: String, stderr: String, exit: i32| Reply {
        stdout,
        stderr,
        exit,
    };
    match resolve(&plan.project, procs).and_then(|root| call(&root, timeout_ms)) {
        Ok(result) if plan.json => reply(format!("{result}\n"), String::new(), 0),
        Ok(result) => {
            let text = |key: &str| result.get(key).and_then(Value::as_str).map(str::to_string);
            let (checked_out, default) = (text("checkedOut"), text("defaultBranch"));
            let mut out = String::new();
            if result.get("enabled") != Some(&Value::Bool(true)) {
                out.push_str("merging from a remote client is off ([remote] merge)\n");
            }
            for branch in result["branches"].as_array().into_iter().flatten() {
                let Some(name) = branch.as_str() else { continue };
                let mut marks = Vec::new();
                if checked_out.as_deref() == Some(name) {
                    marks.push("checked out");
                }
                if default.as_deref() == Some(name) {
                    marks.push("default");
                }
                let line = if marks.is_empty() {
                    name.to_string()
                } else {
                    format!("{name} ({})", marks.join(", "))
                };
                out.push_str(&clean_text(&line));
                out.push('\n');
            }
            reply(out, String::new(), 0)
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
            format!("cstan merge-targets: {code}: {message}\n"),
            1,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn the_front_takes_a_project_and_json_once_each() {
        assert_eq!(
            plan(&words("--project /srv/a --json")),
            Some(TargetsPlan {
                project: "/srv/a".into(),
                json: true
            })
        );
        assert_eq!(plan(&words("--json --project ims-api")).map(|p| p.project), Some("ims-api".into()));
        assert_eq!(plan(&words("")), None);
        assert_eq!(plan(&words("--project")), None);
        assert_eq!(plan(&words("--project a --project b")), None);
        assert_eq!(plan(&words("--project ./a")), None);
        assert_eq!(plan(&words("--project a --into main")), None);
    }
}
