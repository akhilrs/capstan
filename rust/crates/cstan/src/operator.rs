//! The operator side: the project's credential and socket (`loadOperator`, `loadConfig`), starting the daemon on demand
//! (`ensureRunning`), and `cstan start` / `cstan stop`.
use std::path::{Path, PathBuf};

use capstan_client::{ClientError, EnsureOptions, Reason};
use capstan_daemon::run::{load_project, ProjectConfig};
use capstan_wire::js::{JsStr, Value};

use crate::jsops::JsError;
use crate::{blocked, invalid, output, parse_options, runtime, Cli, Fail, Flow};

const WRAP: &str =
    "operator commands need the operator credential in .capstan of the working directory: ";
const LAUNCHER_CLIENT_TIMEOUT_MS: u64 = 600_000;

/// What `loadOperator` returns.
pub struct Operator {
    pub config: ProjectConfig,
    pub credential: String,
    pub socket_path: PathBuf,
    pub log_path: PathBuf,
}

pub fn object(members: Vec<(&str, Value)>) -> Value {
    Value::Object(
        members
            .into_iter()
            .map(|(key, value)| (JsStr::from(key), value))
            .collect(),
    )
}

pub fn number(n: f64) -> Value {
    Value::Number(n)
}

pub fn string(text: &str) -> Value {
    Value::String(JsStr::from(text))
}

/// `CAPSTAN_LAUNCH=off` (or `0`, `false`, `no`) keeps the daemon away from Herdr.
pub fn launch_disabled(env: &[(std::ffi::OsString, std::ffi::OsString)]) -> bool {
    let value = crate::agent::var(env, "CAPSTAN_LAUNCH").map(|v| v.to_string_lossy().into_owned());
    capstan_daemon::ports::launch_disabled_in(value.as_deref())
}

/// The first steps of `loadConfig`, worded as Node's `fs.lstatSync` words a failure: the project directory, then the
/// config file, then the key. The checks of what they are come from `load_project`; the directory's come before the files
/// are looked at, so a directory that fails them is left for it to word.
fn first_lstats(cwd: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let lstat = |path: &Path| {
        std::fs::symlink_metadata(path).map_err(|e| crate::init::fs_error(&e, "lstat", path))
    };
    let directory = lstat(&cwd.join(".capstan"))?;
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    if !directory.is_dir()
        || directory.file_type().is_symlink()
        || directory.uid() != uid
        || directory.mode() & 0o077 != 0
    {
        return Ok(());
    }
    lstat(&cwd.join(".capstan/project.json"))?;
    lstat(&cwd.join(".capstan/operator.key"))?;
    Ok(())
}

impl Cli<'_> {
    /// `loadConfig`: the project config and the operator credential, or the refusal.
    pub fn load_config(&self) -> Result<(ProjectConfig, String), Fail> {
        if let Err(message) = first_lstats(&self.ctx.cwd) {
            return Err(invalid(message));
        }
        load_project(&self.ctx.cwd).map_err(|error| {
            let message = error.message.strip_prefix(WRAP).unwrap_or(&error.message);
            invalid(message)
        })
    }

    /// `loadOperator`.
    pub fn load_operator(&self) -> Result<Operator, Fail> {
        if let Err(message) = first_lstats(&self.ctx.cwd) {
            return Err(invalid(format!("{WRAP}{message}")));
        }
        let (config, credential) =
            load_project(&self.ctx.cwd).map_err(|error| invalid(error.message))?;
        let socket_path = config.state_directory.join("control.sock");
        Ok(Operator {
            config,
            credential,
            socket_path,
            log_path: self.ctx.cwd.join(".capstan").join("daemon.log"),
        })
    }

    fn ensure_options(&self, operator: &Operator) -> EnsureOptions {
        EnsureOptions {
            socket_path: operator.socket_path.clone(),
            credential: operator.credential.clone(),
            project_root: self.ctx.cwd.clone(),
            log_path: operator.log_path.clone(),
            cli_path: self.ctx.cli_path.clone(),
            env: self.ctx.env.clone(),
            timeout_ms: None,
        }
    }

    /// `ensureRunning`: the operator, with the daemon answering.
    pub fn ensure_running(&mut self) -> Result<Operator, Fail> {
        let operator = self.load_operator()?;
        capstan_client::ensure_daemon(&self.ensure_options(&operator))
            .map_err(controller_unavailable)?;
        Ok(operator)
    }

    /// `cstan start`.
    pub fn start(&mut self, rest: &[String]) -> Result<Flow, Fail> {
        let (positional, json) = parse_options(rest);
        if !positional.is_empty() {
            return Err(Fail::Usage);
        }
        let operator = self.load_operator()?;
        self.require_git()?;
        let started = capstan_client::ensure_daemon(&self.ensure_options(&operator))
            .map_err(controller_unavailable)?;
        let mut launch: Option<Value> = None;
        let mut failed = false;
        if self.ctx.cwd.join(capstan_config::CONFIG_FILE_NAME).exists()
            && !launch_disabled(&self.ctx.env)
        {
            // The daemon is up whatever happens here, so a launch that cannot be reported is shown as the launch's own
            // failure, not as a missing controller.
            match capstan_client::call_daemon(
                &operator.socket_path,
                &operator.credential,
                "launch",
                &[],
                LAUNCHER_CLIENT_TIMEOUT_MS,
            ) {
                Ok(capstan_wire::Response::Ok { result }) => {
                    failed = matches!(
                        result.as_ref().and_then(|r| r.get("state")),
                        Some(Value::String(s)) if s.to_utf8_lossy() == "failed"
                    );
                    launch = result;
                }
                Ok(capstan_wire::Response::Refused { code, message }) => {
                    let hint = if code.to_utf8_lossy() == "not_configured" {
                        " (a daemon started before capstan.toml existed reads it only at start: run cstan stop and cstan start)"
                    } else {
                        ""
                    };
                    launch = Some(Value::String(crate::jsops::concat(&[
                        &code,
                        &": ".into(),
                        &message,
                        &hint.into(),
                    ])));
                    failed = true;
                }
                Ok(capstan_wire::Response::Malformed) => {
                    unreachable!("call_daemon maps it to an error")
                }
                Err(error) => {
                    launch = Some(string(&format!("launch: {}", error.message)));
                    failed = true;
                }
            }
        }
        let mut members = vec![
            ("running", Value::Bool(true)),
            ("pid", Value::Number(started.pid as f64)),
            ("started", Value::Bool(started.started)),
        ];
        if let Some(launch) = launch {
            members.push(("launch", launch));
        }
        output(&mut self.io.out, Some(&object(members)), json);
        Ok(Flow::Code(if failed {
            crate::jsops::EXIT_BLOCKED
        } else {
            0
        }))
    }

    /// `cstan stop`.
    pub fn stop(&mut self, rest: &[String]) -> Result<Flow, Fail> {
        let (positional, json) = parse_options(rest);
        if !positional.is_empty() {
            return Err(Fail::Usage);
        }
        let operator = self.load_operator()?;
        let stopped =
            capstan_client::stop_daemon(&operator.socket_path, &operator.credential, None)
                .map_err(controller_unavailable)?;
        let result = string(if stopped { "stopped" } else { "not_running" });
        output(
            &mut self.io.out,
            Some(&object(vec![
                ("running", Value::Bool(false)),
                ("result", result),
            ])),
            json,
        );
        Ok(Flow::Code(0))
    }
}

/// `controllerUnavailable`: a refusal is blocked, every other failure a plain error; a request too large for the frame is
/// the `TypeError` the Node client throws.
pub fn controller_unavailable(error: ClientError) -> Fail {
    match error {
        ClientError::Unavailable(unavailable) => {
            if unavailable.reason == Reason::Refused {
                blocked(unavailable.message)
            } else {
                runtime(unavailable.message)
            }
        }
        ClientError::Io(message) => runtime(message),
        ClientError::Call(call) if call.code == "ERR_INVALID_ARG_VALUE" => invalid(call.message),
        ClientError::Call(call) => Fail::Error(JsError::runtime(call.message)),
    }
}
