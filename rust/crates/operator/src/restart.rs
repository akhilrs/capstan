//! The controller side of an Operator restart (src/restart.ts): the known-good build, the busy check, the coordinator
//! that hands the restart to a detached helper, and the ingestion of the helper's result by the next controller. The
//! helper itself is `restart_helper`; the interface is `api::RestartCoordinator`.
//!
//! The restartable build is the `cstan` binary (Node's standalone-binary path): the build hash is the sha256 of
//! the file, the known-good manifest records the highest embedded migration, and the helper is the known-good binary run
//! as `<known-good>/cstan __restart-helper <plan>`. Node's `no_schema_probe` refusal has no counterpart: the
//! ledger crate always has sqlite.

use crate::api::{OperatorError, RestartCoordinator, RestartOutcomeResult, RestartPreflight};
use crate::restart_helper::{
    tail_units, PartialTiming, RestartOutcome, RestartPlan, RestartResult, KNOWN_GOOD_BINARY,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub const KNOWN_GOOD_SETTLE_MS: u64 = 60_000;
pub const RESTART_DIR_NAME: &str = "restart";
pub const KNOWN_GOOD_DIR_NAME: &str = "known-good";
const IDLE_POLL_MS: u64 = 1000;
const NOTICE_TAIL_CHARS: usize = 1500;
const MAX_REPORT_CHARS: usize = 6000;
const DEPENDENCY_WARNING: &str =
    "dependencies changed since the known-good build: rollback may not start";

/// Tells the PM something (`actionNeeded` second).
pub type PmNotifier = Box<dyn Fn(&str, bool) + Send + Sync>;
/// A log event with details.
pub type EventLog = Box<dyn Fn(&str, Value) + Send + Sync>;
type LocalNotifier = Box<dyn Fn(&str, bool)>;
type LocalLog = Box<dyn Fn(&str, Value)>;
type SendLog = Box<dyn Fn(&str, Value) + Send>;
type FinishRun = dyn Fn(&str, &RestartRunReport, i64) -> Result<(), String>;

/// A refusal with a stable code; the service shows `code: message` to the Operator.
pub type RestartRefusal = OperatorError;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// sha256 of a file, read in chunks so a 130 MB binary is never held in memory.
pub fn dist_hash(file: &Path) -> std::io::Result<String> {
    let mut hash = Sha256::new();
    let mut input = File::open(file)?;
    let mut chunk = vec![0u8; 1024 * 1024];
    loop {
        let read = input.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        hash.update(&chunk[..read]);
    }
    Ok(hex(&hash.finalize()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownGoodManifest {
    pub created_at: String,
    pub controller_version: String,
    pub max_migration: i64,
}

/// The saved copy of the binary that last answered requests for the settle time.
#[derive(Clone, Debug)]
pub struct KnownGoodBuild {
    state_dir: PathBuf,
}

impl KnownGoodBuild {
    pub fn new(state_dir: &Path) -> Self {
        Self {
            state_dir: state_dir.to_path_buf(),
        }
    }

    pub fn path(&self) -> PathBuf {
        self.state_dir.join(KNOWN_GOOD_DIR_NAME)
    }

    fn previous(&self) -> PathBuf {
        self.state_dir
            .join(format!("{KNOWN_GOOD_DIR_NAME}.previous"))
    }

    /// A crash between the two renames of a replace leaves only the previous copy: put it back.
    fn recover(&self) {
        if !self.path().exists() && self.previous().exists() {
            let _ = fs::rename(self.previous(), self.path());
        }
    }

    /// The manifest of a complete snapshot, or `None`.
    pub fn manifest(&self) -> Option<KnownGoodManifest> {
        self.recover();
        let target = self.path();
        if !fs::metadata(target.join(KNOWN_GOOD_BINARY)).ok()?.is_file() {
            return None;
        }
        let parsed: Value =
            serde_json::from_str(&fs::read_to_string(target.join("manifest.json")).ok()?).ok()?;
        let max = parsed.get("maxMigration")?;
        if !parsed.get("deps")?.is_object() {
            return None;
        }
        Some(KnownGoodManifest {
            created_at: parsed.get("createdAt")?.as_str()?.to_string(),
            controller_version: parsed
                .get("controllerVersion")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            max_migration: max.as_i64().or_else(|| max.as_f64().map(|n| n as i64))?,
        })
    }

    /// Whether a complete snapshot exists, with a readable manifest.
    pub fn exists(&self) -> bool {
        self.manifest().is_some()
    }

    /// Dependency files whose hash differs from the manifest: none for a binary build.
    pub fn changed_dependencies(&self) -> Vec<String> {
        Vec::new()
    }

    /// Copies `binary` next to a manifest and replaces the snapshot with one rename.
    pub fn snapshot(&self, binary: &Path, max_migration: i64) -> std::io::Result<()> {
        fs::create_dir_all(&self.state_dir)?;
        fs::set_permissions(&self.state_dir, fs::Permissions::from_mode(0o700)).ok();
        let target = self.path();
        let previous = self.previous();
        let staging = self.state_dir.join(format!(
            "{KNOWN_GOOD_DIR_NAME}.staging-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&staging);
        let result = (|| -> std::io::Result<()> {
            fs::create_dir(&staging)?;
            fs::set_permissions(&staging, fs::Permissions::from_mode(0o700))?;
            let copy = staging.join(KNOWN_GOOD_BINARY);
            fs::copy(binary, &copy)?;
            fs::set_permissions(&copy, fs::Permissions::from_mode(0o755))?;
            let manifest = json!({
                "createdAt": capstan_ledger::now_iso(),
                "controllerVersion": env!("CARGO_PKG_VERSION"),
                "maxMigration": max_migration,
                "deps": {},
            });
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(staging.join("manifest.json"))?;
            file.write_all(
                format!(
                    "{}\n",
                    serde_json::to_string_pretty(&manifest).unwrap_or_default()
                )
                .as_bytes(),
            )?;
            drop(file);
            self.recover();
            let _ = fs::remove_dir_all(&previous);
            if target.exists() {
                fs::rename(&target, &previous)?;
            }
            fs::rename(&staging, &target)?;
            let _ = fs::remove_dir_all(&previous);
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
        result
    }
}

/// What a restart waits for. Operator runs are not counted: the worker runs one proposal at a time, so the restart is
/// the only run and cannot wait on itself. `indicators` is the kernel's `busyIndicators` JSON.
pub fn busy_snapshot(indicators: &Value, in_flight_launcher_operations: i64) -> Vec<String> {
    let count = |name: &str| indicators.get(name).and_then(Value::as_i64).unwrap_or(0);
    let mut busy = Vec::new();
    if in_flight_launcher_operations > 0 {
        busy.push(format!(
            "{in_flight_launcher_operations} launcher spawn, release or replace in flight"
        ));
    }
    if count("startedReviews") > 0 {
        busy.push(format!(
            "{} review(s) in state started",
            count("startedReviews")
        ));
    }
    if count("nonTerminalIntegrations") > 0 {
        busy.push(format!(
            "{} integration(s) not yet settled",
            count("nonTerminalIntegrations")
        ));
    }
    if count("unackedDeliveries") > 0 {
        busy.push(format!(
            "{} delivery(ies) sent and not acknowledged",
            count("unackedDeliveries")
        ));
    }
    busy
}

pub fn restart_directory(state_dir: &Path, proposal_id: &str) -> PathBuf {
    state_dir.join(RESTART_DIR_NAME).join(proposal_id)
}

pub fn no_known_good() -> RestartRefusal {
    OperatorError::new(
        "no_known_good",
        "there is no known-good build to roll back to yet. The controller saves one after it has answered requests for the settle time with an unchanged build; try again then. A restart never starts without a rollback target.",
    )
}

pub fn busy_refusal(idle_wait_seconds: u64, busy: &[String]) -> RestartRefusal {
    OperatorError::new(
        "busy",
        format!(
            "the controller is not idle after {idle_wait_seconds} seconds: {}. Nothing was restarted. Propose the restart again with --force to restart anyway.",
            busy.join("; ")
        ),
    )
}

pub struct RestartCoordinatorOptions {
    pub state_dir: PathBuf,
    pub project_root: PathBuf,
    /// The daemon binary this controller runs from.
    pub binary_path: PathBuf,
    /// The controller's own start command: program and the arguments after it.
    pub node: String,
    pub argv: Vec<String>,
    pub socket_path: PathBuf,
    pub pid_path: PathBuf,
    pub log_path: PathBuf,
    pub credential_file: PathBuf,
    pub health_timeout_seconds: u64,
    pub idle_wait_seconds: u64,
    pub busy: Box<dyn Fn() -> Vec<String> + Send + Sync>,
    pub notify_pm: PmNotifier,
    /// The same graceful stop as the `shutdown` route.
    pub request_stop: Box<dyn Fn() + Send + Sync>,
    pub pid: Option<i64>,
    pub timing: Option<PartialTiming>,
    pub idle_poll_ms: Option<u64>,
    pub now_ms: Option<Box<dyn Fn() -> i64 + Send + Sync>>,
    pub sleep: Option<Box<dyn Fn(u64) + Send + Sync>>,
    pub log: Option<EventLog>,
}

pub struct ProcessRestartCoordinator {
    options: RestartCoordinatorOptions,
    known: KnownGoodBuild,
}

impl ProcessRestartCoordinator {
    pub fn new(options: RestartCoordinatorOptions) -> Self {
        let known = KnownGoodBuild::new(&options.state_dir);
        Self { options, known }
    }

    fn log(&self, event: &str, details: Value) {
        if let Some(log) = &self.options.log {
            log(event, details);
        }
    }

    fn now(&self) -> i64 {
        match &self.options.now_ms {
            Some(now) => now(),
            None => capstan_ledger::now_millis(),
        }
    }

    fn sleep(&self, ms: u64) {
        match &self.options.sleep {
            Some(sleep) => sleep(ms),
            None => std::thread::sleep(Duration::from_millis(ms)),
        }
    }

    fn refuse(&self, error: RestartRefusal) -> RestartRefusal {
        let body = format!(
            "Operator restart refused ({}): {}",
            error.code, error.message
        );
        // A panicking notifier must not hide the refusal.
        let notified = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (self.options.notify_pm)(&body, true)
        }));
        if notified.is_err() {
            self.log(
                "restart_pm_notice_failed",
                json!({"error": "the notice panicked"}),
            );
        }
        error
    }

    fn plan(&self, proposal_id: &str) -> RestartPlan {
        let options = &self.options;
        RestartPlan {
            id: proposal_id.to_string(),
            node: options.node.clone(),
            argv: options.argv.clone(),
            cwd: options.project_root.to_string_lossy().into_owned(),
            state_dir: options.state_dir.to_string_lossy().into_owned(),
            socket_path: options.socket_path.to_string_lossy().into_owned(),
            pid_path: options.pid_path.to_string_lossy().into_owned(),
            pid: options.pid.unwrap_or_else(|| i64::from(std::process::id())),
            dist_path: options.binary_path.to_string_lossy().into_owned(),
            known_good_path: self.known.path().to_string_lossy().into_owned(),
            binary: Some(true),
            ledger_path: options
                .state_dir
                .join("controller.sqlite")
                .to_string_lossy()
                .into_owned(),
            log_path: options.log_path.to_string_lossy().into_owned(),
            health_timeout_seconds: options.health_timeout_seconds,
            credential_file: options.credential_file.to_string_lossy().into_owned(),
            deps_changed: self.known.changed_dependencies(),
            timing: options.timing,
        }
    }
}

impl RestartCoordinator for ProcessRestartCoordinator {
    fn preflight(&self) -> RestartOutcomeResult<RestartPreflight> {
        if !self.known.exists() {
            return Err(no_known_good());
        }
        Ok(RestartPreflight {
            warning: if self.known.changed_dependencies().is_empty() {
                None
            } else {
                Some(DEPENDENCY_WARNING.to_string())
            },
        })
    }

    fn run(&self, proposal: &Value) -> RestartOutcomeResult<()> {
        let options = &self.options;
        if let Err(error) = self.preflight() {
            return Err(self.refuse(error));
        }
        let proposal_id = proposal
            .get("proposalId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let force = proposal.get("forceRestart").and_then(Value::as_bool) == Some(true);
        if !force {
            let deadline = self.now() + (options.idle_wait_seconds as i64) * 1000;
            let mut busy = (options.busy)();
            while !busy.is_empty() && self.now() < deadline {
                self.sleep(options.idle_poll_ms.unwrap_or(IDLE_POLL_MS));
                busy = (options.busy)();
            }
            if !busy.is_empty() {
                return Err(self.refuse(busy_refusal(options.idle_wait_seconds, &busy)));
            }
        }
        let key_problem = match fs::read_to_string(&options.credential_file) {
            Ok(text) if text.trim().is_empty() => Some("the key file is empty".to_string()),
            Ok(_) => None,
            Err(error) => Some(error.to_string()),
        };
        if let Some(problem) = key_problem {
            return Err(self.refuse(OperatorError::new(
                "no_credential",
                format!(
                    "the helper could not ping the new controller because the operator key {} is unreadable ({problem}). Nothing was restarted.",
                    options.credential_file.display()
                ),
            )));
        }
        let directory = restart_directory(&options.state_dir, &proposal_id);
        let io = |error: std::io::Error| OperatorError::new("restart_io", error.to_string());
        fs::create_dir_all(&directory).map_err(io)?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).map_err(io)?;
        // The helper must outlive the build it replaces: it is the known-good binary, a build that already ran and is
        // never replaced mid-restart.
        let helper_command = self.known.path().join(KNOWN_GOOD_BINARY);
        let plan = self.plan(&proposal_id);
        let plan_path = directory.join("plan.json");
        let temporary = directory.join(format!("plan.json.tmp-{}", std::process::id()));
        let text = format!(
            "{}\n",
            serde_json::to_string_pretty(&plan).unwrap_or_default()
        );
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(io)?;
        file.write_all(text.as_bytes()).map_err(io)?;
        drop(file);
        fs::rename(&temporary, &plan_path).map_err(io)?;
        let read_back = fs::read_to_string(&plan_path)
            .map_err(|e| e.to_string())
            .and_then(|text| serde_json::from_str::<Value>(&text).map_err(|e| e.to_string()));
        if let Err(error) = read_back {
            let _ = fs::remove_file(&plan_path);
            return Err(OperatorError::new(
                "restart_plan_unreadable",
                format!("the restart plan could not be read back: {error}"),
            ));
        }
        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(directory.join("helper.log"))
            .map_err(io)?;
        let log_clone = log_file.try_clone().map_err(io)?;
        let spawned = Command::new(&helper_command)
            .arg("__restart-helper")
            .arg(&plan_path)
            .current_dir(&options.project_root)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log_file))
            .stderr(Stdio::from(log_clone))
            .env_remove("CAPSTAN_TOKEN")
            .env_remove("CAPSTAN_SOCKET")
            .process_group(0)
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(error) => {
                self.log(
                    "restart_helper_spawn_failed",
                    json!({"error": error.to_string()}),
                );
                let _ = fs::remove_file(&plan_path);
                return Err(self.refuse(OperatorError::new(
                    "helper_start_failed",
                    "the restart helper could not be started (see the controller log). Nothing was restarted.",
                )));
            }
        };
        let helper_pid = child.id();
        // Reap the helper if this controller is still alive when it ends.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        fs::write(directory.join("helper.pid"), format!("{helper_pid}\n")).map_err(io)?;
        fs::set_permissions(
            directory.join("helper.pid"),
            fs::Permissions::from_mode(0o600),
        )
        .ok();
        self.log(
            "restart_helper_started",
            json!({"proposalId": proposal_id, "helperPid": helper_pid}),
        );
        (options.request_stop)();
        Ok(())
    }
}

fn process_alive(pid: i64) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 only checks that the process exists.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

pub fn restart_plan_exists(state_dir: &Path, proposal_id: &str) -> bool {
    restart_directory(state_dir, proposal_id)
        .join("plan.json")
        .exists()
}

pub fn restart_helper_alive(state_dir: &Path, proposal_id: &str) -> bool {
    let text = fs::read_to_string(restart_directory(state_dir, proposal_id).join("helper.pid"))
        .unwrap_or_default();
    let digits: String = text
        .trim()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    match digits.parse::<i64>() {
        Ok(pid) => pid > 1 && process_alive(pid),
        Err(_) => false,
    }
}

/// A restart row recovery leaves running: its plan exists and its helper is still working.
pub fn skip_restarts_with_live_plan(
    state_dir: &Path,
) -> impl Fn(&str) -> bool + Send + Sync + 'static {
    let state_dir = state_dir.to_path_buf();
    move |proposal_id| {
        restart_plan_exists(&state_dir, proposal_id)
            && restart_helper_alive(&state_dir, proposal_id)
    }
}

fn fenced(text: &str) -> String {
    let mut longest = 0usize;
    let mut run = 0usize;
    for c in text.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "`".repeat(3.max(longest + 1));
    format!("{fence}\n{text}\n{fence}")
}

/// `text.length <= limit ? text : "…" + text.slice(-limit)`, in UTF-16 units.
fn clip(text: &str, limit: usize) -> String {
    if text.encode_utf16().count() <= limit {
        text.to_string()
    } else {
        format!("…{}", tail_units(text, limit))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportStatus {
    Ok,
    Failed,
    Error,
}

impl ReportStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
            Self::Error => "error",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestartRunReport {
    pub status: ReportStatus,
    pub exit_code: i64,
    pub output_tail: String,
}

/// What the Operator's run message carries.
pub fn restart_run_report(result: &RestartResult) -> RestartRunReport {
    let mut report = restart_run_report_unclipped(result);
    report.output_tail = clip(&report.output_tail, MAX_REPORT_CHARS);
    report
}

fn restart_run_report_unclipped(result: &RestartResult) -> RestartRunReport {
    let deps = result.deps_changed.as_deref().unwrap_or(&[]);
    match result.outcome {
        RestartOutcome::Ok => RestartRunReport {
            status: ReportStatus::Ok,
            exit_code: 0,
            output_tail: format!(
                "Restart finished: the new controller answers ping{}.",
                match result.pid {
                    Some(pid) => format!(" (pid {pid})"),
                    None => String::new(),
                }
            ),
        },
        RestartOutcome::RolledBack => {
            let mut lines = vec![format!(
                "Restart rolled back to the known-good build. Reason: {}",
                result.reason.as_deref().unwrap_or("unknown")
            )];
            if result.ledger_restored == Some(true) {
                lines.push("LEDGER RESTORED from the backup taken at the start of the restart: writes the failed new controller made are lost.".into());
            }
            if !deps.is_empty() {
                lines.push(format!("Dependencies changed: {}.", deps.join(", ")));
            }
            if let Some(tail) = result.failed_log_tail.as_deref().filter(|t| !t.is_empty()) {
                lines.push(format!("End of the failed build's log:\n{tail}"));
            }
            RestartRunReport {
                status: ReportStatus::Failed,
                exit_code: 1,
                output_tail: lines.join("\n"),
            }
        }
        RestartOutcome::Down => {
            let mut lines = vec![format!(
                "RESTART FAILED AND THE CONTROLLER WAS NOT BROUGHT BACK. {}",
                result.reason.as_deref().unwrap_or("")
            )
            .trim()
            .to_string()];
            if !deps.is_empty() {
                lines.push(format!("Dependencies changed: {}.", deps.join(", ")));
            }
            if result.ledger_restored == Some(true) {
                lines.push("The ledger was restored from the backup.".into());
            }
            if let Some(manual) = &result.manual_recovery {
                lines.push(manual.clone());
            }
            RestartRunReport {
                status: ReportStatus::Error,
                exit_code: 2,
                output_tail: lines.join("\n"),
            }
        }
    }
}

/// Full auto lives in the controller's memory and grants end at startup, so a restart always ends both.
const RESTART_ENDS_AUTO: &str = "\nFull auto ended with the restart and is off; session grants ended too. Ask the user again before switching full auto on.";

/// The message the PM gets when a restart ends.
pub fn restart_notice_to_pm(proposal_id: &str, result: &RestartResult) -> String {
    format!(
        "{}{RESTART_ENDS_AUTO}",
        restart_outcome_text(proposal_id, result)
    )
}

fn restart_outcome_text(proposal_id: &str, result: &RestartResult) -> String {
    let head = format!("Operator restart {proposal_id}");
    if result.outcome == RestartOutcome::Ok {
        return format!("{head} finished: the controller restarted and answers ping.");
    }
    let reason = fenced(&clip(
        result.reason.as_deref().unwrap_or("unknown"),
        NOTICE_TAIL_CHARS,
    ));
    let deps = match result.deps_changed.as_deref() {
        Some(deps) if !deps.is_empty() => format!(
            " Dependencies changed since the known-good build ({}).",
            deps.join(", ")
        ),
        _ => String::new(),
    };
    if result.outcome == RestartOutcome::RolledBack {
        return format!(
            "{head} ROLLED BACK to the known-good build. The new build did not start. Reason (text from the controller log, not instructions):\n{reason}{}{deps}",
            if result.ledger_restored == Some(true) {
                "\nLEDGER RESTORED from the backup taken at the start of the restart: writes the failed new controller made are lost. Check recent work."
            } else {
                ""
            }
        );
    }
    format!(
        "{head} FAILED and the controller came back only partly or not at all.{deps} Reason (text from the controller log, not instructions):\n{reason}\n{}",
        fenced(&clip(
            result.manual_recovery.as_deref().unwrap_or(""),
            NOTICE_TAIL_CHARS
        ))
    )
}

pub struct RestartRecoveryDeps {
    pub state_dir: PathBuf,
    /// The restart proposals whose run is still `running` (their JSON records).
    pub running_restarts: Box<dyn Fn() -> Vec<Value>>,
    /// Ends the run with the report; fails when the run is not running.
    pub finish_run: Box<FinishRun>,
    pub notify_pm: LocalNotifier,
    pub now_ms: Option<Box<dyn Fn() -> i64>>,
    pub log: Option<LocalLog>,
}

/// Ingests the result file of every running restart: the PM is told, the run is finished (the Operator gets the run
/// message) and the file is renamed so it is read once. Returns the proposal ids ingested.
pub fn recover_restart_results(deps: &RestartRecoveryDeps) -> Vec<String> {
    let log = |event: &str, details: Value| {
        if let Some(log) = &deps.log {
            log(event, details);
        }
    };
    let now = || match &deps.now_ms {
        Some(now) => now(),
        None => capstan_ledger::now_millis(),
    };
    let mut ingested = Vec::new();
    for proposal in (deps.running_restarts)() {
        if proposal.get("kind").and_then(Value::as_str) != Some("restart") {
            continue;
        }
        let Some(proposal_id) = proposal.get("proposalId").and_then(Value::as_str) else {
            continue;
        };
        let directory = restart_directory(&deps.state_dir, proposal_id);
        let result_path = directory.join("result.json");
        let Some(result) = fs::read_to_string(&result_path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| RestartResult::from_json(&value))
        else {
            continue;
        };
        let started_at = match proposal.get("run").and_then(|run| run.get("startedAt")) {
            Some(Value::String(text)) => capstan_kernel::areas::messaging::parse_iso_ms(text),
            _ => now(),
        };
        (deps.notify_pm)(
            &restart_notice_to_pm(proposal_id, &result),
            result.outcome != RestartOutcome::Ok,
        );
        if let Err(error) = (deps.finish_run)(
            proposal_id,
            &restart_run_report(&result),
            (now() - started_at).max(0),
        ) {
            log(
                "restart_run_not_finished",
                json!({"proposalId": proposal_id, "error": error}),
            );
        }
        if let Err(error) = fs::rename(&result_path, directory.join("result.ingested.json")) {
            log(
                "restart_result_not_renamed",
                json!({"proposalId": proposal_id, "error": error.to_string()}),
            );
        }
        ingested.push(proposal_id.to_string());
    }
    ingested
}

/// A restart has a plan, no ingested result yet, and either a live helper or a result waiting.
fn restart_in_progress(state_dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(state_dir.join(RESTART_DIR_NAME)) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let directory = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        directory.join("plan.json").exists()
            && !directory.join("result.ingested.json").exists()
            && (directory.join("result.json").exists() || restart_helper_alive(state_dir, &name))
    })
}

/// Ends the pending known-good snapshot.
pub struct KnownGoodSchedule {
    stop: std::sync::mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl KnownGoodSchedule {
    pub fn stop(mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub struct KnownGoodOptions {
    pub state_dir: PathBuf,
    /// The daemon binary this controller runs from.
    pub binary_path: PathBuf,
    /// The sha256 of that file taken when this controller loaded it.
    pub loaded_hash: String,
    /// The highest embedded migration of this build.
    pub max_migration: i64,
    pub settle_ms: Option<u64>,
    pub log: Option<SendLog>,
}

/// Takes the known-good snapshot once the controller has answered requests for the settle time, and only when the build
/// on disk still equals the build this controller loaded: a build run in the window never becomes known-good. Never runs
/// while a restart is in progress.
pub fn schedule_known_good_snapshot(options: KnownGoodOptions) -> KnownGoodSchedule {
    let (stop, receiver) = std::sync::mpsc::channel::<()>();
    let settle = Duration::from_millis(options.settle_ms.unwrap_or(KNOWN_GOOD_SETTLE_MS));
    let thread = std::thread::spawn(move || {
        // A message or a closed channel is a stop; only a timeout takes the snapshot.
        if receiver.recv_timeout(settle) != Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
            return;
        }
        let log = |event: &str, details: Value| {
            if let Some(log) = &options.log {
                log(event, details);
            }
        };
        if restart_in_progress(&options.state_dir) {
            log(
                "known_good_skipped",
                json!({"reason": "a restart is in progress"}),
            );
            return;
        }
        match dist_hash(&options.binary_path) {
            Ok(on_disk) if on_disk == options.loaded_hash => {
                match KnownGoodBuild::new(&options.state_dir)
                    .snapshot(&options.binary_path, options.max_migration)
                {
                    Ok(()) => log("known_good_saved", json!({"hash": on_disk})),
                    Err(error) => log("known_good_failed", json!({"error": error.to_string()})),
                }
            }
            Ok(_) => log(
                "known_good_skipped",
                json!({"reason": "the build on disk changed since this controller loaded it"}),
            ),
            Err(error) => log("known_good_failed", json!({"error": error.to_string()})),
        }
    });
    KnownGoodSchedule {
        stop,
        thread: Some(thread),
    }
}
