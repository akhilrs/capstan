//! `cstan dash`: starts the daemon, finds `cstan-dash` (`CSTAN_DASH_BIN`, beside `cstan`, the data directory, then `PATH`,
//! the resolver order of `src/dash/launch.ts`) and replaces this process with it. There is no other dashboard.
use std::ffi::OsString;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::agent::var;
use crate::jsops::JsError;
use crate::{invalid, take_flag, take_interval_seconds, Cli, ExecPlan, Fail, Flow, EXIT_USAGE};

pub const DASH_BIN_NAME: &str = "cstan-dash";

/// `NOT_A_TERMINAL_MESSAGE`.
pub const NOT_A_TERMINAL: &str =
    "dash needs an interactive terminal; use \"cstan status\" (or \"cstan status --watch\") for scripts and pipes";

/// The one line `cstan dash` gives when no `cstan-dash` can be found, with the usage exit code.
pub const MISSING: &str =
    "cstan-dash was not found; install it beside cstan, put it on PATH or set CSTAN_DASH_BIN to its absolute path (docs/reference/install.md)";

fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        && access_x(path)
}

fn access_x(path: &Path) -> bool {
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `c` is a valid NUL-terminated path.
    unsafe { libc::access(c.as_ptr(), libc::X_OK) == 0 }
}

/// `probeDash`: a candidate must run, `--version` printing `cstan-dash <version>`, before it is trusted.
fn probe(bin: &Path) -> bool {
    let Ok(mut child) = Command::new(bin)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let mut stdout = child.stdout.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(2)),
            Err(_) => break None,
        }
    };
    let text = reader.join().unwrap_or_default();
    status.is_some_and(|s| s.success())
        && text
            .strip_prefix("cstan-dash ")
            .is_some_and(|rest| rest.chars().next().is_some_and(|c| !c.is_whitespace()))
}

/// `candidatePaths`, in search order.
fn candidate_paths(env: &[(OsString, OsString)], cli_path: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(override_path) = var(env, "CSTAN_DASH_BIN").filter(|v| !v.is_empty()) {
        let path = PathBuf::from(override_path);
        if path.is_absolute() {
            out.push(path);
        }
    }
    let real = std::fs::canonicalize(cli_path).unwrap_or_else(|_| cli_path.to_path_buf());
    if let Some(dir) = real.parent() {
        out.push(dir.join(DASH_BIN_NAME));
    }
    let data = match var(env, "XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(data) => PathBuf::from(data),
        None => PathBuf::from(var(env, "HOME").unwrap_or_default()).join(".local/share"),
    };
    out.push(data.join("capstan/current/bin").join(DASH_BIN_NAME));
    if let Some(path) = var(env, "PATH") {
        for dir in std::env::split_paths(path) {
            if !dir.as_os_str().is_empty() && dir.is_absolute() {
                out.push(dir.join(DASH_BIN_NAME));
            }
        }
    }
    out
}

/// `resolveDash`: the first candidate that is an executable file and runs as `cstan-dash`. `CSTAN_DASH=rust` demands one.
fn resolve_dash(env: &[(OsString, OsString)], cli_path: &Path) -> Result<Option<PathBuf>, Fail> {
    let mut broken: Option<PathBuf> = None;
    for candidate in candidate_paths(env, cli_path) {
        if !is_executable_file(&candidate) {
            continue;
        }
        if probe(&candidate) {
            return Ok(Some(candidate));
        }
        broken.get_or_insert(candidate);
    }
    if var(env, "CSTAN_DASH").is_some_and(|v| v == "rust") {
        return Err(Fail::Error(JsError::runtime(match broken {
            Some(path) => format!(
                "CSTAN_DASH=rust but {} does not run as {DASH_BIN_NAME} (`--version` failed); rebuild or reinstall it (docs/reference/install.md)",
                path.display()
            ),
            None => format!(
                "CSTAN_DASH=rust but no executable {DASH_BIN_NAME} was found; set CSTAN_DASH_BIN to its absolute path or install it (docs/reference/install.md)"
            ),
        })));
    }
    Ok(None)
}

fn has_terminal(declared: Option<bool>) -> bool {
    declared.unwrap_or_else(|| {
        // SAFETY: isatty only inspects the descriptor.
        unsafe { libc::isatty(0) == 1 && libc::isatty(1) == 1 }
    })
}

pub fn missing() -> Fail {
    Fail::Error(JsError {
        exit: EXIT_USAGE,
        message: MISSING.into(),
    })
}

impl Cli<'_> {
    /// `cstan dash [--interval <seconds>] [--no-color] [--reduced-motion]`.
    pub fn dash(&mut self, rest: &[String]) -> Result<Flow, Fail> {
        let mut flags = rest.to_vec();
        let interval_seconds = take_interval_seconds(&mut flags)?;
        let no_color = take_flag(&mut flags, "--no-color");
        let reduced_motion = take_flag(&mut flags, "--reduced-motion")
            || var(&self.ctx.env, "CSTAN_REDUCED_MOTION").is_some_and(|v| v == "1");
        if !flags.is_empty() {
            return Err(Fail::Usage);
        }
        let in_agent_shell = self.agent_environment()?.is_some();
        if !has_terminal(self.ctx.terminal) {
            self.io.err_line(&format!("cstan: {NOT_A_TERMINAL}"));
            return Ok(Flow::Code(EXIT_USAGE));
        }
        if in_agent_shell {
            return Err(invalid(
                "dash is an operator tool; run it without CAPSTAN_TOKEN and CAPSTAN_SOCKET",
            ));
        }
        let operator = self.ensure_running()?;
        let worker_limit = capstan_config::load_role_config(&self.ctx.cwd)
            .ok()
            .map(|config| config.limits.max_workers);
        let Some(bin) = resolve_dash(&self.ctx.env, &self.ctx.cli_path)? else {
            return Err(missing());
        };
        let mut args: Vec<OsString> = vec![
            "--socket".into(),
            operator.socket_path.clone().into(),
            "--interval".into(),
            interval_seconds.to_string().into(),
        ];
        if let Some(limit) = worker_limit {
            args.push("--worker-limit".into());
            args.push(limit.to_string().into());
        }
        if no_color {
            args.push("--no-color".into());
        }
        if reduced_motion {
            args.push("--reduced-motion".into());
        }
        let mut env: Vec<(OsString, OsString)> = self
            .ctx
            .env
            .iter()
            .filter(|(key, _)| key != "CAPSTAN_TOKEN" && key != "CAPSTAN_SOCKET")
            .cloned()
            .collect();
        env.retain(|(key, _)| key != "CSTAN_DASH_CREDENTIAL");
        env.push(("CSTAN_DASH_CREDENTIAL".into(), operator.credential.into()));
        Ok(Flow::Exec(ExecPlan {
            program: bin,
            args,
            env,
        }))
    }
}
