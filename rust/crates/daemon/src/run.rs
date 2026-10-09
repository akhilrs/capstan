//! The daemon's run function (`runDaemon` of src/daemon.ts and the `daemon` command of src/cli.ts): load the project of the
//! working directory, open the kernel with the project lock (exit 4 when another controller holds it), run the startup
//! sequence, serve until a stop is requested, and shut down in Node's order.
//!
//! This is the initial version handed to d2b-srv: opening, the startup sequence, signals and the ordered shutdown are
//! here; `serve` is a stub that keeps the lock and waits for the stop (it logs `daemon:serve_not_implemented`), so a
//! daemon built from this code already excludes a Node daemon and the other way round.

use crate::deps::{
    render_log_line, startup_sequence, DaemonOptions, Deps, KernelHandle, LogEntry, Logger,
};
use capstan_config::RoleConfig;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::types::{InitialProject, ProjectInput};
use capstan_kernel::{Core, KernelError, SystemEnv};
use capstan_ledger::LedgerError;
use serde_json::{json, Value};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// `EXIT` of src/cli.ts.
pub const EXIT_OK: i32 = 0;
pub const EXIT_USAGE: i32 = 2;
pub const EXIT_INVALID: i32 = 3;
/// Another controller holds the project (the lost-lock-race code `ensureDaemon` waits out).
pub const EXIT_BLOCKED: i32 = 4;
pub const EXIT_RUNTIME: i32 = 5;

/// The log event of a daemon whose server is not ported yet.
pub const SERVE_STUB_EVENT: &str = "serve_not_implemented";

/// Why the daemon ended with a failure: the exit code and the line printed after `cstan-daemon: `.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunError {
    pub exit_code: i32,
    pub message: String,
}

impl RunError {
    pub fn new(exit_code: i32, message: impl Into<String>) -> Self {
        Self {
            exit_code,
            message: message.into(),
        }
    }

    /// The exit code `cstan`'s catch-all gives a failure of this kind: a held or unusable project lock is `blocked`.
    pub fn from_kernel(error: &KernelError) -> Self {
        let code = match error {
            KernelError::Ledger(LedgerError::ProjectLockHeld | LedgerError::Ownership(_)) => {
                EXIT_BLOCKED
            }
            KernelError::Type(_) => EXIT_INVALID,
            _ => EXIT_RUNTIME,
        };
        Self::new(code, error.message())
    }
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RunError {}

// ------------------------------------------------------------------------------------------------ the project

pub const CONFIG_NAME: &str = ".capstan/project.json";
pub const KEY_NAME: &str = ".capstan/operator.key";
pub const CONFIG_FILE_NAME: &str = "capstan.toml";

/// `Config` of src/cli.ts: `.capstan/project.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectConfig {
    pub project_id: String,
    pub name: String,
    pub state_directory: PathBuf,
    pub max_slices: i64,
    pub max_run_ms: i64,
    pub max_dispatches: i64,
}

fn invalid(message: impl Into<String>) -> RunError {
    RunError::new(EXIT_INVALID, message)
}

/// A JSON text without duplicate object members (`parseJsonWithoutDuplicateMembers`): the value, or the refusal.
fn parse_without_duplicates(text: &str) -> Result<Value, String> {
    use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
    struct Strict(Value, usize);
    impl<'de> Deserialize<'de> for Strict {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct V;
            impl<'de> Visitor<'de> for V {
                type Value = Strict;
                fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    f.write_str("a JSON value")
                }
                fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Strict, A::Error> {
                    let mut out = serde_json::Map::new();
                    let mut depth = 0;
                    while let Some(key) = map.next_key::<String>()? {
                        if out.contains_key(&key) {
                            return Err(serde::de::Error::custom(format!(
                                "duplicate JSON member: {key}"
                            )));
                        }
                        let Strict(value, below) = map.next_value::<Strict>()?;
                        depth = depth.max(below);
                        out.insert(key, value);
                    }
                    if depth >= 256 {
                        return Err(serde::de::Error::custom(
                            "JSON nesting depth exceeds the limit",
                        ));
                    }
                    Ok(Strict(Value::Object(out), depth + 1))
                }
                fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Strict, A::Error> {
                    let mut out = Vec::new();
                    let mut depth = 0;
                    while let Some(Strict(value, below)) = seq.next_element::<Strict>()? {
                        depth = depth.max(below);
                        out.push(value);
                    }
                    if depth >= 256 {
                        return Err(serde::de::Error::custom(
                            "JSON nesting depth exceeds the limit",
                        ));
                    }
                    Ok(Strict(Value::Array(out), depth + 1))
                }
                fn visit_bool<E>(self, v: bool) -> Result<Strict, E> {
                    Ok(Strict(Value::Bool(v), 0))
                }
                fn visit_i64<E>(self, v: i64) -> Result<Strict, E> {
                    Ok(Strict(json!(v), 0))
                }
                fn visit_u64<E>(self, v: u64) -> Result<Strict, E> {
                    Ok(Strict(json!(v), 0))
                }
                fn visit_f64<E>(self, v: f64) -> Result<Strict, E> {
                    Ok(Strict(json!(v), 0))
                }
                fn visit_str<E>(self, v: &str) -> Result<Strict, E> {
                    Ok(Strict(Value::String(v.to_string()), 0))
                }
                fn visit_unit<E>(self) -> Result<Strict, E> {
                    Ok(Strict(Value::Null, 0))
                }
            }
            deserializer.deserialize_any(V)
        }
    }
    serde_json::from_str::<Strict>(text)
        .map(|Strict(value, _)| value)
        .map_err(|e| e.to_string())
}

fn read_json(file: &Path) -> Result<Value, RunError> {
    let bytes = std::fs::read(file).map_err(|e| invalid(e.to_string()))?;
    let text = String::from_utf8(bytes).map_err(|e| invalid(e.to_string()))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    parse_without_duplicates(text).map_err(invalid)
}

fn parse_config(value: &Value) -> Result<ProjectConfig, RunError> {
    let Some(raw) = value.as_object() else {
        return Err(invalid("project config must be an object"));
    };
    let mut keys: Vec<&str> = raw.keys().map(String::as_str).collect();
    keys.sort_unstable();
    if keys.join(",")
        != "maxDispatches,maxRunMs,maxSlices,name,projectId,schemaVersion,stateDirectory"
    {
        return Err(invalid("project config has unknown or missing fields"));
    }
    let project_id = raw["projectId"].as_str().unwrap_or("");
    let id = project_id.as_bytes();
    let id_ok = raw["schemaVersion"] == 1
        && (1..=64).contains(&id.len())
        && id[0].is_ascii_alphanumeric()
        && id[1..]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if !id_ok {
        return Err(invalid(
            "project config has an invalid schemaVersion or projectId",
        ));
    }
    let name = raw["name"].as_str().unwrap_or("");
    if capstan_kernel::helpers::js_trim(name).is_empty() || name.encode_utf16().count() > 256 {
        return Err(invalid("project config name is invalid"));
    }
    let state = raw["stateDirectory"].as_str().unwrap_or("");
    if !Path::new(state).is_absolute() {
        return Err(invalid("project config stateDirectory must be absolute"));
    }
    let mut numbers = [0i64; 3];
    for (slot, (key, min, max)) in numbers.iter_mut().zip([
        ("maxSlices", 2, 8),
        ("maxRunMs", 1_000, 86_400_000),
        ("maxDispatches", 2, 64),
    ]) {
        let number = raw[key].as_i64().filter(|n| (min..=max).contains(n));
        match number {
            Some(n) => *slot = n,
            None => {
                return Err(invalid(format!(
                    "project config {key} must be an integer from {min} to {max}"
                )))
            }
        }
    }
    Ok(ProjectConfig {
        project_id: project_id.to_string(),
        name: name.to_string(),
        state_directory: PathBuf::from(state),
        max_slices: numbers[0],
        max_run_ms: numbers[1],
        max_dispatches: numbers[2],
    })
}

/// `loadConfig` of src/cli.ts, with its checks: a private `.capstan`, regular files of mode 0600 owned by the user, a
/// project-local state directory and an operator key of at least 32 characters. Failures are exit code 3 and say that
/// operator commands need the credential, as the Node CLI does.
pub fn load_project(cwd: &Path) -> Result<(ProjectConfig, String), RunError> {
    let wrap = |message: String| {
        invalid(format!(
            "operator commands need the operator credential in .capstan of the working directory: {message}"
        ))
    };
    let attempt = || -> Result<(ProjectConfig, String), String> {
        let directory = cwd.join(".capstan");
        let config_path = cwd.join(CONFIG_NAME);
        let key_path = cwd.join(KEY_NAME);
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        let directory_stat = std::fs::symlink_metadata(&directory).map_err(|e| e.to_string())?;
        if !directory_stat.is_dir()
            || directory_stat.file_type().is_symlink()
            || directory_stat.uid() != uid
            || directory_stat.mode() & 0o077 != 0
        {
            return Err(
                "Capstan project directory must be private and owned by the current user".into(),
            );
        }
        let config_stat = std::fs::symlink_metadata(&config_path).map_err(|e| e.to_string())?;
        let key_stat = std::fs::symlink_metadata(&key_path).map_err(|e| e.to_string())?;
        if !config_stat.is_file()
            || config_stat.file_type().is_symlink()
            || !key_stat.is_file()
            || key_stat.file_type().is_symlink()
        {
            return Err("Capstan config and operator key must be regular files".into());
        }
        if config_stat.mode() & 0o077 != 0 || key_stat.mode() & 0o077 != 0 {
            return Err("Capstan config and operator key permissions must be 0600".into());
        }
        if config_stat.uid() != uid || key_stat.uid() != uid {
            return Err("Capstan files must be owned by the current user".into());
        }
        let value = read_json(&config_path).map_err(|e| e.message)?;
        let config = parse_config(&value).map_err(|e| e.message)?;
        if config.state_directory != directory.join("state") {
            return Err("Capstan stateDirectory must remain project-local".into());
        }
        let key = std::fs::read(&key_path).map_err(|e| e.to_string())?;
        let credential = String::from_utf8_lossy(&key).trim().to_string();
        if credential.encode_utf16().count() < 32 {
            return Err("operator key is missing or invalid".into());
        }
        Ok((config, credential))
    };
    attempt().map_err(wrap)
}

/// `PLACEHOLDER_INPUTS` of src/daemon.ts: a fresh project has no brief, criteria or plan yet, but the ledger needs one
/// revision of each durable input kind to exist; they are marked so nothing can mistake them for real inputs.
pub fn placeholder_inputs() -> Vec<ProjectInput> {
    vec![
        ProjectInput {
            kind: "project_config".into(),
            content: json!({"placeholder": true}),
        },
        ProjectInput {
            kind: "task_brief".into(),
            content: json!({"placeholder": true}),
        },
        ProjectInput {
            kind: "acceptance_criteria".into(),
            content: json!(["(no acceptance criteria recorded yet)"]),
        },
        ProjectInput {
            kind: "policy".into(),
            content: json!({"placeholder": true}),
        },
        ProjectInput {
            kind: "plan".into(),
            content: json!({"placeholder": true}),
        },
    ]
}

/// The project the daemon opens: the config's id and name, the operator credential and the placeholder inputs.
pub fn initial_project(config: &ProjectConfig, credential: &str) -> InitialProject {
    InitialProject {
        project_id: config.project_id.clone(),
        name: config.name.clone(),
        owner_credential: credential.to_string(),
        initial_inputs: placeholder_inputs(),
    }
}

/// The options of the daemon of the project in `cwd`: its config, operator key and, when `capstan.toml` exists, the
/// role configuration (a refusal of it is exit code 3). Warnings of the configuration are returned for the caller to log.
pub fn daemon_options(cwd: &Path) -> Result<(DaemonOptions, Vec<String>), RunError> {
    let (config, credential) = load_project(cwd)?;
    let mut options = DaemonOptions::new(
        config.state_directory.clone(),
        initial_project(&config, &credential),
        cwd.to_path_buf(),
    );
    let mut warnings = Vec::new();
    if cwd.join(CONFIG_FILE_NAME).exists() {
        let capstan: RoleConfig =
            capstan_config::load_role_config(cwd).map_err(|e| invalid(e.to_string()))?;
        warnings = capstan.warnings.clone();
        options.capstan = Some(Arc::new(capstan));
    }
    options.cli_path = std::env::current_exe().ok();
    Ok((options, warnings))
}

// ------------------------------------------------------------------------------------------------ signals

static STOP_SIGNALLED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP_SIGNALLED.store(true, Ordering::SeqCst);
}

/// Handlers for SIGTERM and SIGINT go in before the kernel opens, so a signal during migrations or reconcile still ends
/// in a clean close; a thread turns the signal into the stop request. The previous dispositions are restored by
/// `restore_signals`.
fn install_signals(
    deps_shutdown: crate::deps::ShutdownHandle,
) -> Vec<(libc::c_int, libc::sigaction)> {
    let mut previous = Vec::new();
    for signal in [libc::SIGTERM, libc::SIGINT] {
        // SAFETY: the handler only stores into an atomic; the struct is zeroed before its fields are set.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as usize;
            libc::sigemptyset(&mut action.sa_mask);
            let mut old: libc::sigaction = std::mem::zeroed();
            if libc::sigaction(signal, &action, &mut old) == 0 {
                previous.push((signal, old));
            }
        }
    }
    let _ = std::thread::Builder::new()
        .name("signals".into())
        .spawn(move || loop {
            if STOP_SIGNALLED.load(Ordering::SeqCst) {
                deps_shutdown.request();
                return;
            }
            if deps_shutdown.requested() {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        });
    previous
}

fn restore_signals(previous: Vec<(libc::c_int, libc::sigaction)>) {
    for (signal, action) in previous {
        // SAFETY: restores a disposition `sigaction` returned for this signal.
        unsafe {
            libc::sigaction(signal, &action, std::ptr::null_mut());
        }
    }
}

// ------------------------------------------------------------------------------------------------ the pid file

/// `writePidFile`: replaces the pid file with this process's id (mode 0600, created exclusively).
pub fn write_pid_file(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    writeln!(file, "{}", std::process::id())
}

/// `removePidFile`: removes the pid file when it still names this process; the pid file is advisory.
pub fn remove_pid_file(path: &Path) {
    if let Ok(text) = std::fs::read_to_string(path) {
        if text.trim() == std::process::id().to_string() {
            let _ = std::fs::remove_file(path);
        }
    }
}

// ------------------------------------------------------------------------------------------------ serve

/// What `serve` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Served {
    /// The server ran until the stop was requested.
    Stopped,
}

/// Serves the control socket until the stop is requested. A stub until d2b-srv: it only logs that the server is not ported
/// and keeps the lock until the stop request, which is what lets the lock exclusion be tested.
pub fn serve(deps: &Deps) -> Served {
    deps.detail_log(
        SERVE_STUB_EVENT,
        json!({"socket": deps.options.socket_path().to_string_lossy()}),
    );
    deps.shutdown.wait();
    Served::Stopped
}

// ------------------------------------------------------------------------------------------------ run

fn announce(options: &DaemonOptions, event: &str) {
    if let Some(announce) = &options.announce {
        announce(event, std::process::id());
    }
}

/// Runs the daemon until SIGTERM, SIGINT or the shutdown command (`runDaemon`).
pub fn run_daemon(options: DaemonOptions) -> Result<(), RunError> {
    let shutdown = crate::deps::ShutdownHandle::new();
    let signals = install_signals(shutdown.clone());
    let outcome = run_with(options, shutdown);
    restore_signals(signals);
    outcome
}

fn run_with(options: DaemonOptions, shutdown: crate::deps::ShutdownHandle) -> Result<(), RunError> {
    let kernel_options = KernelOptions {
        workspace_root: Some(options.workspace_root.clone()),
        runtime_workspace_path: None,
        keep_migration_backups: options
            .capstan
            .as_ref()
            .and_then(|config| usize::try_from(config.ledger.keep_migration_backups).ok()),
    };
    let kernel = {
        let (state, project) = (options.state_directory.clone(), options.project.clone());
        KernelHandle::spawn(move || {
            Core::open(&state, &project, &kernel_options, Box::new(SystemEnv))
        })
        .map_err(|error| RunError::from_kernel(&error))?
    };
    // The sink reads the kernel's clock for each line's timestamp, as the Node CLI's `stamp()` reads `new Date()`.
    let sink: Logger = {
        let kernel = kernel.clone();
        Arc::new(move |entry: &LogEntry| {
            let _ = writeln!(std::io::stdout(), "{}", render_log_line(&kernel, entry));
        })
    };
    let mut deps = Deps::new(kernel.clone(), options, sink);
    deps.shutdown = shutdown;
    if deps.shutdown.requested() {
        kernel.close();
        return Ok(());
    }
    startup_sequence(&deps);
    // Panes recorded before a restart are re-registered first, in the background, so the socket binds at once; the
    // loops start only after (d2b-srv and d2b-loops fill this in).
    serve(&deps);
    // The order of Node's `finally`: no new ticks, the operator and the launcher's work end, then the kernel closes (the
    // lock is free once it does), then the socket.
    deps.loops.stop_relay();
    deps.loops.stop_ticks();
    if let Some(operator) = &deps.operator {
        operator.stop();
    }
    kernel.close();
    announce(&deps.options, "lock_released");
    remove_pid_file(&deps.options.pid_path());
    Ok(())
}

/// The announcement the process prints for `ready`, `lock_released` and `socket_removed`: `{"ts", "event", "pid"}`.
pub fn announce_line(event: &str, pid: u32) -> String {
    json!({"ts": capstan_ledger::now_iso(), "event": event, "pid": pid}).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn project_dir(config: &str, key: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let capstan = dir.path().join(".capstan");
        std::fs::create_dir(&capstan).unwrap();
        std::fs::set_permissions(&capstan, std::fs::Permissions::from_mode(0o700)).unwrap();
        for (name, text) in [("project.json", config), ("operator.key", key)] {
            let file = capstan.join(name);
            std::fs::write(&file, text).unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        dir
    }

    fn config_for(dir: &Path) -> String {
        format!(
            r#"{{"schemaVersion":1,"projectId":"p1","name":"n","stateDirectory":"{}","maxSlices":4,"maxRunMs":3600000,"maxDispatches":16}}"#,
            dir.join(".capstan").join("state").display()
        )
    }

    #[test]
    fn a_project_loads_with_its_credential() {
        let probe = tempfile::tempdir().unwrap();
        let dir = project_dir("", "");
        let text = config_for(dir.path());
        std::fs::write(dir.path().join(CONFIG_NAME), &text).unwrap();
        std::fs::write(dir.path().join(KEY_NAME), format!("{}\n", "k".repeat(40))).unwrap();
        let (config, credential) = load_project(dir.path()).unwrap();
        assert_eq!(config.project_id, "p1");
        assert_eq!(config.max_slices, 4);
        assert_eq!(credential, "k".repeat(40));
        let (options, warnings) = daemon_options(dir.path()).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(options.project.initial_inputs.len(), 5);
        assert_eq!(
            options.socket_path(),
            dir.path().join(".capstan/state/control.sock")
        );
        drop(probe);
    }

    #[test]
    fn a_broken_project_is_refused_with_exit_code_3() {
        let dir = project_dir("{}", "short");
        let error = load_project(dir.path()).unwrap_err();
        assert_eq!(error.exit_code, EXIT_INVALID);
        assert!(error
            .message
            .contains("operator commands need the operator credential"));
        let missing = tempfile::tempdir().unwrap();
        assert_eq!(
            load_project(missing.path()).unwrap_err().exit_code,
            EXIT_INVALID
        );
    }

    #[test]
    fn duplicate_members_and_unknown_fields_are_refused() {
        assert!(parse_without_duplicates(r#"{"a":1,"a":2}"#)
            .unwrap_err()
            .contains("duplicate JSON member: a"));
        assert!(parse_without_duplicates(r#"{"a":{"b":1,"b":1}}"#).is_err());
        assert!(parse_without_duplicates(r#"{"a":[{"b":1},{"b":2}]}"#).is_ok());
        assert!(parse_config(&json!({"schemaVersion": 1})).is_err());
    }

    #[test]
    fn a_held_lock_is_exit_code_4() {
        let error = RunError::from_kernel(&KernelError::Ledger(LedgerError::ProjectLockHeld));
        assert_eq!(error.exit_code, EXIT_BLOCKED);
        assert_eq!(
            error.message,
            "another cooperating controller owns this project"
        );
        assert_eq!(
            RunError::from_kernel(&KernelError::type_error("x")).exit_code,
            EXIT_INVALID
        );
        assert_eq!(
            RunError::from_kernel(&KernelError::Other("x".into())).exit_code,
            EXIT_RUNTIME
        );
    }
}
