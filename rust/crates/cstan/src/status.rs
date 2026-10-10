//! `cstan status` and `cstan inspect` without an agent environment: asked of the running controller over its control
//! socket, or read from the ledger when it is not running (`inspectController` and the status branch of `runCli`).
use std::path::Path;
use std::time::Duration;

use capstan_daemon::run::ProjectConfig;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::types::InitialProject;
use capstan_kernel::{Core, SystemEnv};
use capstan_wire::js::{self, JsStr, Value};
use capstan_wire::{call, AfterSend, Timeout, WireError};
use serde_json::{json, Map, Value as Json};

use crate::config_cmd::kernel_failure;
use crate::render;
use crate::{invalid, output, parse_options, runtime, to_js, Cli, Fail, Flow};

const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);

enum Ask<'a> {
    Status,
    Inspect(&'a str),
}

/// `requestControl`: one request of the control protocol; `Ok(None)` when the controller is not reachable (the connect
/// failed), so the caller reads the ledger instead.
fn request_control(socket: &Path, credential: &str, ask: &Ask) -> Result<Option<Json>, Fail> {
    let (action, id) = match ask {
        Ask::Status => ("status", None),
        Ask::Inspect(id) => ("inspect", Some(*id)),
    };
    let mut members = vec![
        (JsStr::from("token"), Value::String(credential.into())),
        (JsStr::from("action"), Value::String(action.into())),
    ];
    if let Some(id) = id.filter(|id| !id.is_empty()) {
        members.push((JsStr::from("id"), Value::String(id.into())));
    }
    let mut frame = js::stringify(&Value::Object(members), 0)
        .to_utf8_lossy()
        .into_bytes();
    frame.push(b'\n');
    let line = match call(socket, &frame, Timeout::Idle(CONTROL_TIMEOUT)) {
        Ok(line) => line,
        Err(WireError::NotSent { .. }) => return Ok(None),
        Err(WireError::AfterSend(error)) => {
            return Err(runtime(match error {
                AfterSend::TimedOut => "control request timed out".to_string(),
                AfterSend::TooLarge => "control response exceeds limit".to_string(),
                AfterSend::Closed => "connection closed".to_string(),
                AfterSend::Malformed => "malformed reply".to_string(),
                AfterSend::Io(code) => format!("read {code}"),
            }))
        }
    };
    let text = std::str::from_utf8(&line).map_err(|e| invalid(e.to_string()))?;
    let response: Json = serde_json::from_str(text).map_err(|e| invalid(e.to_string()))?;
    match response.get("error") {
        Some(Json::String(message)) if !message.is_empty() => Err(runtime(message.clone())),
        _ => Ok(Some(response.get("result").cloned().unwrap_or(Json::Null))),
    }
}

/// The status of a project whose ledger does not exist yet.
fn not_started(config: &ProjectConfig) -> Json {
    let roles: Vec<Json> = ["PM", "Developer", "Verifier", "Supervisor"]
        .iter()
        .map(|role| {
            json!({
                "role": role,
                "seatId": null,
                "seatState": "not_created",
                "actorActive": false,
                "sessionState": null,
                "assignmentId": null,
            })
        })
        .collect();
    json!({
        "projectId": config.project_id,
        "run": {"state": "not_started", "stateVersion": 0},
        "stateVersion": 0,
        "inputRevision": 0,
        "roles": roles,
        "work": [],
        "findings": [],
        "evidence": [],
        "finalVerification": [],
    })
}

impl Cli<'_> {
    /// `inspectController`.
    fn inspect_controller(
        &self,
        config: &ProjectConfig,
        credential: &str,
        ask: Ask,
    ) -> Result<Json, Fail> {
        let socket = config.state_directory.join("control.sock");
        if let Some(result) = request_control(&socket, credential, &ask)? {
            return Ok(result);
        }
        let database = config.state_directory.join("controller.sqlite");
        if !database.exists() {
            return match ask {
                Ask::Inspect(_) => Err(runtime("controller record does not exist")),
                Ask::Status => Ok(not_started(config)),
            };
        }
        let project = InitialProject {
            project_id: config.project_id.clone(),
            name: config.name.clone(),
            owner_credential: credential.to_string(),
            initial_inputs: Vec::new(),
        };
        let options = KernelOptions {
            workspace_root: Some(self.ctx.cwd.clone()),
            ..KernelOptions::default()
        };
        let core = Core::open_read_only(
            &config.state_directory,
            &project,
            &options,
            Box::new(SystemEnv),
        )
        .map_err(|e| kernel_failure(&e))?;
        let result = match ask {
            Ask::Status => core.status_snapshot(),
            Ask::Inspect(id) => core.inspect(id),
        };
        core.close();
        result.map_err(|e| kernel_failure(&e))
    }

    /// `cstan status` with no agent environment.
    pub fn status_offline(&mut self, rest: &[String]) -> Result<Flow, Fail> {
        let (config, credential) = self.load_config()?;
        let (positional, json) = parse_options(rest);
        if !positional.is_empty() {
            return Err(Fail::Usage);
        }
        let snapshot = self.inspect_controller(&config, &credential, Ask::Status)?;
        let work: Vec<Json> = snapshot
            .get("work")
            .and_then(Json::as_array)
            .cloned()
            .ok_or_else(|| {
                crate::Fail::Error(crate::jsops::JsError::type_error(
                    "Cannot read properties of undefined (reading 'filter')",
                ))
            })?;
        let blockers: Vec<Json> = work
            .iter()
            .filter(|item| item.get("state") != Some(&json!("accepted")))
            .map(|item| {
                json!({
                    "workItemId": item.get("workItemId").cloned().unwrap_or(Json::Null),
                    "state": item.get("state").cloned().unwrap_or(Json::Null),
                    "blockers": item.get("blockers").cloned().unwrap_or(Json::Null),
                })
            })
            .collect();
        let mut next_legal_actions: Vec<Json> = Vec::new();
        for item in &work {
            if let Some(actions) = item.get("nextLegalActions").and_then(Json::as_array) {
                for action in actions {
                    if !next_legal_actions.contains(action) {
                        next_legal_actions.push(action.clone());
                    }
                }
            }
        }
        let ownership: Vec<Json> = work
            .iter()
            .map(|item| {
                json!({
                    "workItemId": item.get("workItemId").cloned().unwrap_or(Json::Null),
                    "role": item.get("role").cloned().unwrap_or(Json::Null),
                    "owner": item.get("owner").cloned().unwrap_or(Json::Null),
                })
            })
            .collect();
        let ledger_path = capstan_ledger::resolve_database_path(&config.state_directory)
            .map_err(|e| invalid(e.to_string()))?;
        let mut status = Map::new();
        status.insert("schemaVersion".into(), json!(1));
        if let Json::Object(members) = &snapshot {
            for (key, value) in members {
                status.insert(key.clone(), value.clone());
            }
        }
        status.insert("ownership".into(), Json::Array(ownership));
        status.insert("blockers".into(), Json::Array(blockers));
        status.insert(
            "limits".into(),
            json!({
                "maxSlices": config.max_slices,
                "maxRunMs": config.max_run_ms,
                "maxDispatches": config.max_dispatches,
            }),
        );
        status.insert("nextLegalActions".into(), Json::Array(next_legal_actions));
        status.insert(
            "controller".into(),
            json!({
                "pid": std::process::id(),
                "projectRoot": self.ctx.cwd.to_string_lossy(),
                "ledgerPath": ledger_path.to_string_lossy(),
            }),
        );
        let status = to_js(&Json::Object(status));
        if !json {
            let mut lines = render::controller_lines(Some(&status));
            lines.extend(render::pause_lines(Some(&status), self.ctx.now_ms)?);
            for line in &lines {
                self.io.out_js_line(line);
            }
        }
        output(&mut self.io.out, Some(&status), json);
        Ok(Flow::Code(0))
    }

    /// `cstan inspect <id> [--json]`.
    pub fn inspect(&mut self, rest: &[String]) -> Result<Flow, Fail> {
        let (positional, json) = parse_options(rest);
        let id = match positional.as_slice() {
            [id] if !id.is_empty() => id.clone(),
            _ => return Err(Fail::Usage),
        };
        let (config, credential) = self.load_config()?;
        let inspected = self.inspect_controller(&config, &credential, Ask::Inspect(&id))?;
        if json {
            const KINDS: [&str; 6] = [
                "work_item",
                "assignment",
                "candidate",
                "finding",
                "recovery",
                "report",
            ];
            let kind = inspected.get("kind").and_then(Json::as_str);
            let record = inspected.get("record").filter(|r| r.is_object());
            let id = inspected.get("id").and_then(Json::as_str);
            let (Some(kind), Some(id), Some(record)) = (kind, id, record) else {
                return Err(contract_failure());
            };
            if !KINDS.contains(&kind) {
                return Err(contract_failure());
            }
            let out = json!({"schemaVersion": 1, "kind": kind, "id": id, "record": record});
            output(&mut self.io.out, Some(&to_js(&out)), true);
        } else {
            output(&mut self.io.out, Some(&to_js(&inspected)), false);
        }
        Ok(Flow::Code(0))
    }
}

fn contract_failure() -> Fail {
    runtime("Controller returned an inspect record outside the cstan inspect JSON contract")
}
