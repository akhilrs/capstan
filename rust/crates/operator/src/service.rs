//! The Operator service (src/operator.ts, with the command runner of src/command-runner.ts): proposals, decisions, session
//! grants, full auto, and the worker that runs approved commands one at a time. The approval of exact text is the
//! control; this module never builds a shell string from a proposal field.
//!
//! Limits that are stated, not hidden: an approved command runs as the user and with the user's full authority, in the
//! project root. The guarantee is on the controller side: only an active PM agent approves, approval is bound to the
//! exact text and the force flag, a command runs once, no approval is taken while a run is in progress, and the child
//! gets no `CAPSTAN_` variable (the service removes them itself) and no `cstan` wrapper on PATH (the `environment`
//! closure builds PATH without the launcher's wrapper directory).
//!
//! The ledger is reached through the `OperatorLedger` port, one closure per ledger step; the daemon implements it over
//! its kernel thread (`d2b-shadow`, daemon/src/ports.rs). No closure runs a command, sleeps or does other blocking I/O.

use crate::api::{
    DecideInput, Decision, FullAutoOnInput, FullAutoStatus, ListFilter, OperatorError,
    OperatorFailure, OperatorResult, OperatorService, ProposalKind, ProposeInput,
    RestartCoordinator, OPERATOR_TICK_MS, ORPHAN_KILL_WAIT_MS,
};
use crate::policy::{
    auto_decision, match_session_grant, session_rule, GrantRef, GrantVerdict, FULL_AUTO_RULE,
};
use capstan_kernel::helpers::strip_terminal_sequences;
use capstan_kernel::types::MutationContext;
use capstan_kernel::{Core, KernelResult};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::Read;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, LazyLock, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use unicode_segmentation::UnicodeSegmentation;

// ------------------------------------------------------------------------------------------------ the command runner

pub const RUN_KILL_GRACE_MS: u64 = 2000;
/// The raw window kept while a command runs; sanitising only shrinks it, so the final tail still has the requested size.
const RAW_WINDOW_FACTOR: usize = 4;
const MIN_RAW_WINDOW_BYTES: usize = 16384;
const MIN_SECRET_VALUE_CHARS: usize = 8;
pub const REDACTED: &str = "[redacted]";
const RUN_POLL_MS: u64 = 10;

static SECRET_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)TOKEN|SECRET|KEY|PASSWORD|PASSWD|CREDENTIAL").expect("regex")
});

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a fixed pattern compiles")
}

/// Shapes that look like a credential: provider key prefixes, JWTs, bearer headers and long mixed letter-digit runs (a 40
/// or 64 hex commit or digest is left alone). `\b` is ASCII and `\s` is JavaScript's, as in the Node patterns.
static CREDENTIAL_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    let space = r"[\t\n\x0b\x0c\r \x{a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}\x{feff}]";
    vec![
        regex(&format!(
            r"(?-u:\b)(?:Bearer|Basic){space}+[A-Za-z0-9._~+/=-]{{8,}}"
        )),
        regex(r"(?-u:\b)(?:sk|pk|rk)-[A-Za-z0-9_-]{16,}"),
        regex(r"(?-u:\b)gh[pousr]_[A-Za-z0-9]{20,}"),
        regex(r"(?-u:\b)github_pat_[A-Za-z0-9_]{20,}"),
        regex(r"(?-u:\b)xox[abprs]-[A-Za-z0-9-]{10,}"),
        regex(r"(?-u:\b)(?:AKIA|ASIA)[A-Z0-9]{16}(?-u:\b)"),
        regex(r"(?-u:\b)eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}"),
        regex(r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?(?:-----END [A-Z ]*PRIVATE KEY-----|$)"),
    ]
});
static LONG_RUN: LazyLock<Regex> = LazyLock::new(|| regex(r"[A-Za-z0-9_-]{32,}"));
static INVISIBLE: LazyLock<Regex> = LazyLock::new(|| regex(r"[\p{Cf}\p{Zl}\p{Zp}]"));
static CONTROLS: LazyLock<Regex> = LazyLock::new(|| regex(r"[\x00-\x08\x0b-\x1f\x{7f}-\x{9f}]"));

fn looks_like_secret_run(run: &str) -> bool {
    run.bytes().any(|b| b.is_ascii_alphabetic())
        && run.bytes().any(|b| b.is_ascii_digit())
        && !run.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The values of the environment variables whose names say they are secrets, longest first.
pub fn secret_values_of(environments: &[&[(String, String)]]) -> Vec<String> {
    let mut values: Vec<String> = Vec::new();
    for environment in environments {
        for (name, value) in *environment {
            if SECRET_NAME.is_match(name)
                && value.encode_utf16().count() >= MIN_SECRET_VALUE_CHARS
                && !values.contains(value)
            {
                values.push(value.clone());
            }
        }
    }
    values.sort_by_key(|value| std::cmp::Reverse(value.encode_utf16().count()));
    values
}

fn tail_of(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let segments: Vec<&str> = text.graphemes(true).collect();
    let mut bytes = 0usize;
    let mut start = segments.len();
    while start > 0 {
        let size = segments[start - 1].len();
        if bytes + size > max_bytes {
            break;
        }
        bytes += size;
        start -= 1;
    }
    segments[start..].concat()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SanitisedOutput {
    pub text: String,
    pub truncated: bool,
}

/// Terminal escapes and C0 controls (other than newline and tab) are removed, credential shapes and the given secret
/// values are replaced by a marker, and the last `max_bytes` bytes are kept.
pub fn sanitise_output(raw: &str, max_bytes: usize, secret_values: &[String]) -> SanitisedOutput {
    let stripped = strip_terminal_sequences(raw);
    let mut text = stripped.replace("\r\n", "\n").replace('\r', "\n");
    text = CONTROLS.replace_all(&text, "").into_owned();
    text = INVISIBLE.replace_all(&text, "").into_owned();
    for value in secret_values {
        if !value.is_empty() {
            text = text.replace(value.as_str(), REDACTED);
        }
    }
    for pattern in CREDENTIAL_PATTERNS.iter() {
        text = pattern.replace_all(&text, REDACTED).into_owned();
    }
    text = LONG_RUN
        .replace_all(&text, |caps: &regex::Captures<'_>| {
            let run = &caps[0];
            if looks_like_secret_run(run) {
                REDACTED.to_string()
            } else {
                run.to_string()
            }
        })
        .into_owned();
    let kept = tail_of(&text, max_bytes);
    SanitisedOutput {
        truncated: kept != text,
        text: kept,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunStatus {
    Ok,
    Failed,
    Timeout,
    Error,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
            Self::Timeout => "timeout",
            Self::Error => "error",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunResult {
    pub status: RunStatus,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub duration_ms: i64,
    pub output_tail: String,
    /// True when output was dropped, either while the command ran or when the tail was cut.
    pub truncated: bool,
}

pub struct RunOptions {
    pub command: String,
    pub cwd: PathBuf,
    pub timeout_ms: u64,
    /// The whole environment of the child; nothing is inherited.
    pub environment: Vec<(String, String)>,
    pub output_tail_bytes: usize,
    /// Called once with the process group leader's pid as soon as the process exists.
    pub on_spawn: Option<Box<dyn Fn(u32) + Send + Sync>>,
    /// Setting the flag kills the process group like a timeout does; the status is then `error`.
    pub abort: Option<Arc<AtomicBool>>,
}

fn kill_group(pgid: i64, signal: libc::c_int) {
    if pgid > 1 {
        // SAFETY: a plain signal to the process group of a child this function started.
        unsafe { libc::kill(-(pgid as libc::pid_t), signal) };
    }
}

fn collect(
    mut stream: impl Read + Send + 'static,
    raw: Arc<Mutex<(Vec<u8>, bool)>>,
    window: usize,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => return,
                Ok(read) => {
                    let mut guard = raw.lock().unwrap_or_else(|p| p.into_inner());
                    guard.0.extend_from_slice(&chunk[..read]);
                    if guard.0.len() > window {
                        let excess = guard.0.len() - window;
                        guard.0.drain(..excess);
                        guard.1 = true;
                    }
                }
            }
        }
    })
}

/// Runs one shell command: `sh -c` in its own process group, a timeout that ends the whole group, and a capped,
/// sanitised tail of the output. The command, the working directory, the environment and the timeout are spawn options;
/// nothing is interpolated into a shell string.
pub fn run_command(options: RunOptions) -> RunResult {
    let started = Instant::now();
    let window = MIN_RAW_WINDOW_BYTES.max(options.output_tail_bytes * RAW_WINDOW_FACTOR);
    let process_env: Vec<(String, String)> = std::env::vars().collect();
    let secrets = secret_values_of(&[&options.environment, &process_env]);
    let finish = |status: RunStatus,
                  exit_code: Option<i32>,
                  signal: Option<i32>,
                  raw: &[u8],
                  dropped: bool,
                  extra: &str| {
        let text = format!("{}{extra}", String::from_utf8_lossy(raw));
        let clean = sanitise_output(&text, options.output_tail_bytes, &secrets);
        RunResult {
            status,
            exit_code,
            signal,
            duration_ms: started.elapsed().as_millis() as i64,
            output_tail: clean.text,
            truncated: dropped || clean.truncated,
        }
    };
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(&options.command)
        .current_dir(&options.cwd)
        .env_clear()
        .envs(options.environment.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return finish(RunStatus::Error, None, None, &[], false, &error.to_string()),
    };
    let pgid = i64::from(child.id());
    let raw = Arc::new(Mutex::new((Vec::new(), false)));
    let readers = [
        child
            .stdout
            .take()
            .map(|out| collect(out, raw.clone(), window)),
        child
            .stderr
            .take()
            .map(|err| collect(err, raw.clone(), window)),
    ];
    if let Some(on_spawn) = &options.on_spawn {
        on_spawn(child.id());
    }
    let (mut timed_out, mut aborted) = (false, false);
    let mut stopped_at: Option<Instant> = None;
    let mut hard_killed = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if !timed_out && started.elapsed() >= Duration::from_millis(options.timeout_ms) {
            timed_out = true;
            kill_group(pgid, libc::SIGTERM);
            stopped_at.get_or_insert_with(Instant::now);
        }
        if !aborted
            && options
                .abort
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::SeqCst))
        {
            aborted = true;
            kill_group(pgid, libc::SIGTERM);
            stopped_at.get_or_insert_with(Instant::now);
        }
        if let Some(at) = stopped_at {
            if !hard_killed && at.elapsed() >= Duration::from_millis(RUN_KILL_GRACE_MS) {
                hard_killed = true;
                kill_group(pgid, libc::SIGKILL);
            }
        }
        std::thread::sleep(Duration::from_millis(RUN_POLL_MS));
    };
    // A group member that ignored SIGTERM must still die.
    if timed_out || aborted {
        kill_group(pgid, libc::SIGKILL);
    }
    for reader in readers.into_iter().flatten() {
        let _ = reader.join();
    }
    let (bytes, dropped) = {
        let guard = raw.lock().unwrap_or_else(|p| p.into_inner());
        (guard.0.clone(), guard.1)
    };
    let (code, signal) = match &status {
        Some(status) => (status.code(), status.signal()),
        None => (None, None),
    };
    if timed_out {
        finish(RunStatus::Timeout, code, signal, &bytes, dropped, "")
    } else if aborted {
        finish(RunStatus::Error, code, signal, &bytes, dropped, "\nstopped")
    } else if code == Some(0) {
        finish(RunStatus::Ok, code, signal, &bytes, dropped, "")
    } else {
        finish(RunStatus::Failed, code, signal, &bytes, dropped, "")
    }
}

/// The start time field of /proc/<pid>/stat (clock ticks since boot), or `None` when the process does not exist or the
/// file cannot be read. It tells a live leader from a pid that was reused.
pub fn process_start_time(pid: i64) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name is in parentheses and may contain spaces or parentheses itself.
    let after = &stat[stat.rfind(')')? + 2..];
    let start = after.split(' ').nth(19)?;
    (!start.is_empty() && start.bytes().all(|b| b.is_ascii_digit())).then(|| start.to_string())
}

/// Whether a process group still has a member.
pub fn group_is_alive(pgid: i64) -> bool {
    if pgid <= 1 {
        return false;
    }
    // SAFETY: signal 0 only checks that the group exists.
    let result = unsafe { libc::kill(-(pgid as libc::pid_t), 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

/// The variables a command may see: everything except `CAPSTAN_*`, so an agent token or socket can never reach it even
/// when the caller's environment closure forgot to remove them.
pub fn scrubbed_environment(environment: Vec<(String, String)>) -> Vec<(String, String)> {
    environment
        .into_iter()
        .filter(|(name, _)| !name.starts_with("CAPSTAN_"))
        .collect()
}

// ------------------------------------------------------------------------------------------------ ports

/// One ledger step: a closure the daemon posts to the thread that owns the kernel and waits for.
pub type LedgerStep = Box<dyn FnOnce(&Core) -> KernelResult<Value> + Send>;

/// The daemon's kernel thread, as the service sees it.
pub trait OperatorLedger: Send + Sync {
    fn run(&self, step: LedgerStep) -> KernelResult<Value>;
}

/// Process queries and signals the orphan check uses.
pub trait ProcessOperations: Send + Sync {
    /// Whether any member of the process group is alive.
    fn group_alive(&self, pgid: i64) -> bool;
    /// The start time of the process with this pid, or `None`.
    fn start_time(&self, pid: i64) -> Option<String>;
    fn kill_group(&self, pgid: i64);
}

pub struct RealProcesses;

impl ProcessOperations for RealProcesses {
    fn group_alive(&self, pgid: i64) -> bool {
        group_is_alive(pgid)
    }
    fn start_time(&self, pid: i64) -> Option<String> {
        process_start_time(pid)
    }
    fn kill_group(&self, pgid: i64) {
        kill_group(pgid, libc::SIGKILL);
    }
}

pub type Environment = Arc<dyn Fn() -> Vec<(String, String)> + Send + Sync>;
pub type PmNotice = Arc<dyn Fn(&str, bool) + Send + Sync>;
pub type EventSink = Arc<dyn Fn(&str, Value) + Send + Sync>;
pub type Runner = Arc<dyn Fn(RunOptions) -> RunResult + Send + Sync>;

pub struct OperatorServiceOptions {
    pub ledger: Arc<dyn OperatorLedger>,
    pub config: capstan_config::Operator,
    /// The project credential the controller uses for its own ledger writes.
    pub controller_credential: String,
    /// The directory every command runs in: the project root, never configurable.
    pub project_root: PathBuf,
    /// The environment a command runs in; the launcher builds it without any agent token.
    pub environment: Environment,
    pub run_command: Option<Runner>,
    /// Starts a restart; without it restart proposals are refused.
    pub restart: Option<Arc<dyn RestartCoordinator>>,
    /// Sends a controller message to the active PM.
    pub notify_pm: Option<PmNotice>,
    pub processes: Option<Arc<dyn ProcessOperations>>,
    /// Milliseconds from a source that never runs backwards; full auto is timed with it so a wall-clock jump cannot
    /// lengthen it.
    pub monotonic_now: Option<Arc<dyn Fn() -> i64 + Send + Sync>>,
    pub orphan_kill_wait_ms: Option<u64>,
    pub log: Option<EventSink>,
}

fn real_monotonic_ms() -> i64 {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN.get_or_init(Instant::now).elapsed().as_millis() as i64
}

/// `newContext(core, credential)` of src/context.ts.
fn new_context(core: &Core, credential: &str) -> KernelResult<MutationContext> {
    let id = core.kernel().env.uuid();
    Ok(MutationContext {
        credential: credential.to_string(),
        request_id: format!("req-{id}"),
        idempotency_key: format!("idem-{id}"),
        expected_version: core.state_version()?,
        input_revision: core.input_revision()?,
    })
}

#[derive(Default)]
struct State {
    full_auto: Option<(i64, i64)>,
    stopped: bool,
    draining: bool,
    ticking: bool,
    abort: Option<Arc<AtomicBool>>,
    grant_deadlines: HashMap<String, i64>,
    timer: Option<(Sender<()>, JoinHandle<()>)>,
}

struct Inner {
    options: OperatorServiceOptions,
    run: Runner,
    processes: Arc<dyn ProcessOperations>,
    state: Mutex<State>,
    drained: Condvar,
}

/// The Operator service over a kernel thread.
pub struct ProcessOperatorService {
    inner: Arc<Inner>,
}

impl ProcessOperatorService {
    pub fn new(options: OperatorServiceOptions) -> Self {
        let run: Runner = options
            .run_command
            .clone()
            .unwrap_or_else(|| Arc::new(run_command));
        let processes = options
            .processes
            .clone()
            .unwrap_or_else(|| Arc::new(RealProcesses));
        Self {
            inner: Arc::new(Inner {
                options,
                run,
                processes,
                state: Mutex::new(State::default()),
                drained: Condvar::new(),
            }),
        }
    }
}

fn limits_json(config: &capstan_config::Operator) -> Value {
    json!({
        "proposalTtlMinutes": config.proposal_ttl_minutes,
        "approvalTtlMinutes": config.approval_ttl_minutes,
    })
}

fn array(value: Value) -> Vec<Value> {
    match value {
        Value::Array(items) => items,
        _ => Vec::new(),
    }
}

impl Inner {
    fn config(&self) -> &capstan_config::Operator {
        &self.options.config
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn log(&self, event: &str, details: Value) {
        if let Some(log) = &self.options.log {
            log(event, details);
        }
    }

    fn now(&self) -> i64 {
        match &self.options.monotonic_now {
            Some(now) => now(),
            None => real_monotonic_ms(),
        }
    }

    /// One ledger step as the given credential.
    fn step(
        &self,
        credential: &str,
        step: impl FnOnce(&Core, &MutationContext) -> KernelResult<Value> + Send + 'static,
    ) -> KernelResult<Value> {
        let credential = credential.to_string();
        self.options.ledger.run(Box::new(move |core| {
            let context = new_context(core, &credential)?;
            step(core, &context)
        }))
    }

    fn as_controller(
        &self,
        step: impl FnOnce(&Core, &MutationContext) -> KernelResult<Value> + Send + 'static,
    ) -> KernelResult<Value> {
        self.step(&self.options.controller_credential, step)
    }

    fn read(
        &self,
        step: impl FnOnce(&Core) -> KernelResult<Value> + Send + 'static,
    ) -> KernelResult<Value> {
        self.options.ledger.run(Box::new(step))
    }

    fn full_auto_status(&self) -> FullAutoStatus {
        let state = self.state();
        match state.full_auto {
            None => FullAutoStatus::Off,
            Some((ends_at_ms, minutes)) => {
                let remaining_ms = ends_at_ms - self.now();
                if remaining_ms <= 0 {
                    FullAutoStatus::Off
                } else {
                    FullAutoStatus::On {
                        minutes,
                        remaining_seconds: (remaining_ms + 999) / 1000,
                    }
                }
            }
        }
    }

    /// True while full auto is on; the first look after the time box ends it and records the expiry.
    fn check_full_auto(&self) -> bool {
        let minutes = {
            let state = self.state();
            match state.full_auto {
                None => return false,
                Some((_, minutes)) => minutes,
            }
        };
        if matches!(self.full_auto_status(), FullAutoStatus::On { .. }) {
            return true;
        }
        self.state().full_auto = None;
        let role = self.config().role.clone();
        let recorded = self.as_controller(move |core, context| {
            core.record_operator_full_auto(
                context,
                &json!({"change": "expired", "operatorRole": role, "minutes": minutes}),
            )
        });
        if let Err(error) = recorded {
            self.log(
                "operator_full_auto_expiry_not_recorded",
                json!({"error": error.message()}),
            );
        }
        false
    }

    fn live_grants(&self) -> Vec<Value> {
        let active = match self.read(|core| core.active_operator_grants()) {
            Ok(grants) => array(grants),
            Err(_) => return Vec::new(),
        };
        let now = self.now();
        let state = self.state();
        active
            .into_iter()
            .filter(|grant| {
                let id = grant["grantId"].as_str().unwrap_or_default();
                now < state.grant_deadlines.get(id).copied().unwrap_or(i64::MAX)
            })
            .collect()
    }

    fn clear_orphans(&self) -> KernelResult<()> {
        let orphans = array(self.read(|core| core.uncleared_operator_orphans())?);
        for orphan in orphans {
            let proposal_id = orphan["proposalId"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let pgid = orphan["pgid"].as_i64().unwrap_or(0);
            let leader_start = orphan["leaderStart"].as_str().map(str::to_string);
            let start = self.processes.start_time(pgid);
            let reused = matches!((&start, &leader_start), (Some(now), Some(then)) if now != then);
            if reused || !self.processes.group_alive(pgid) {
                self.clear_orphan(&proposal_id)?;
                continue;
            }
            self.processes.kill_group(pgid);
            let wait = self
                .options
                .orphan_kill_wait_ms
                .unwrap_or(ORPHAN_KILL_WAIT_MS);
            let deadline = Instant::now() + Duration::from_millis(wait);
            while self.processes.group_alive(pgid) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(100));
            }
            if self.processes.group_alive(pgid) {
                self.log(
                    "operator_orphan_alive",
                    json!({"proposalId": proposal_id, "pgid": pgid}),
                );
                continue;
            }
            self.clear_orphan(&proposal_id)?;
            self.log(
                "operator_orphan_killed",
                json!({"proposalId": proposal_id, "pgid": pgid}),
            );
        }
        Ok(())
    }

    fn clear_orphan(&self, proposal_id: &str) -> KernelResult<()> {
        let id = proposal_id.to_string();
        self.as_controller(move |core, context| core.clear_operator_orphan(context, &id))
            .map(|_| ())
    }

    fn finish_run(&self, proposal_id: &str, result: &RunResult) -> KernelResult<()> {
        let input = json!({
            "proposalId": proposal_id,
            "status": result.status.as_str(),
            "exitCode": result.exit_code,
            "durationMs": result.duration_ms,
            "outputTail": result.output_tail,
            "truncated": result.truncated,
        });
        self.as_controller(move |core, context| core.finish_operator_run(context, &input))
            .map(|_| ())
    }

    fn finish_with_error(&self, proposal_id: &str, tail: &str) -> KernelResult<()> {
        self.finish_run(
            proposal_id,
            &RunResult {
                status: RunStatus::Error,
                exit_code: None,
                signal: None,
                duration_ms: 0,
                output_tail: tail.to_string(),
                truncated: false,
            },
        )
    }

    fn execute(self: &Arc<Self>, proposal: &Value) -> KernelResult<()> {
        let proposal_id = proposal["proposalId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if proposal["kind"].as_str() == Some("restart") {
            let Some(restart) = &self.options.restart else {
                return self.finish_with_error(&proposal_id, "restart_not_available");
            };
            if let Err(error) = restart.run(proposal) {
                self.log(
                    "operator_restart_failed",
                    json!({"proposalId": proposal_id, "error": error.message}),
                );
                let tail = format!("{}: {}", error.code, error.message);
                return self.finish_with_error(&proposal_id, &tail);
            }
            return Ok(());
        }
        let config = self.config();
        let timeout_seconds = config
            .timeout_seconds
            .min(config.max_timeout_seconds)
            .max(0) as u64;
        let abort = Arc::new(AtomicBool::new(false));
        {
            let mut state = self.state();
            if state.stopped {
                abort.store(true, Ordering::SeqCst);
            }
            state.abort = Some(abort.clone());
        }
        let on_spawn: Box<dyn Fn(u32) + Send + Sync> = {
            let inner = self.clone();
            let id = proposal_id.clone();
            Box::new(move |pid| {
                let leader_start = inner.processes.start_time(i64::from(pid));
                let input = json!({"proposalId": id, "pgid": pid, "leaderStart": leader_start});
                let recorded = inner.as_controller(move |core, context| {
                    core.record_operator_run_process(context, &input)
                });
                if let Err(error) = recorded {
                    inner.log(
                        "operator_process_not_recorded",
                        json!({"proposalId": id, "error": error.message()}),
                    );
                }
            })
        };
        let result = (self.run)(RunOptions {
            command: proposal["command"].as_str().unwrap_or_default().to_string(),
            cwd: self.options.project_root.clone(),
            timeout_ms: timeout_seconds * 1000,
            environment: scrubbed_environment((self.options.environment)()),
            output_tail_bytes: config.output_tail_bytes.max(0) as usize,
            on_spawn: Some(on_spawn),
            abort: Some(abort),
        });
        self.state().abort = None;
        self.finish_run(&proposal_id, &result)
    }

    fn run_approved(self: &Arc<Self>) -> KernelResult<()> {
        loop {
            if self.state().stopped {
                return Ok(());
            }
            if !array(self.read(|core| core.uncleared_operator_orphans())?).is_empty() {
                return Ok(());
            }
            let approved = array(self.read(|core| core.approved_operator_proposals())?);
            let Some(next) = approved.first() else {
                return Ok(());
            };
            let proposal_id = next["proposalId"].as_str().unwrap_or_default().to_string();
            if next["autoRule"].as_str() == Some(FULL_AUTO_RULE) && !self.check_full_auto() {
                let id = proposal_id.clone();
                self.as_controller(move |core, context| core.end_full_auto_proposal(context, &id))?;
                continue;
            }
            let mut input = limits_json(self.config());
            input["proposalId"] = json!(proposal_id);
            let claim =
                self.as_controller(move |core, context| core.claim_operator_run(context, &input))?;
            if claim["claimed"].as_bool() != Some(true) {
                if claim["reason"].as_str() == Some("busy") {
                    return Ok(());
                }
                continue;
            }
            self.execute(&claim["proposal"])?;
        }
    }

    /// Runs approved proposals now; returns when the worker has nothing left to start. A second caller waits for the
    /// worker that is already running.
    fn drain(self: &Arc<Self>) {
        let mut state = self.state();
        if state.draining {
            while state.draining {
                state = self.drained.wait(state).unwrap_or_else(|p| p.into_inner());
            }
            return;
        }
        state.draining = true;
        drop(state);
        if let Err(error) = self.run_approved() {
            self.log("operator_worker_failed", json!({"error": error.message()}));
        }
        self.state().draining = false;
        self.drained.notify_all();
    }

    fn kick(self: &Arc<Self>) {
        if self.state().stopped {
            return;
        }
        let inner = self.clone();
        std::thread::spawn(move || inner.drain());
    }

    fn tick(self: &Arc<Self>) {
        {
            let mut state = self.state();
            if state.ticking || state.stopped {
                return;
            }
            state.ticking = true;
        }
        let outcome = (|| -> KernelResult<()> {
            self.check_full_auto();
            let now = self.now();
            let active = array(self.read(|core| core.active_operator_grants())?);
            let late: Vec<String> = {
                let state = self.state();
                active
                    .iter()
                    .filter_map(|grant| grant["grantId"].as_str())
                    .filter(|id| now >= state.grant_deadlines.get(*id).copied().unwrap_or(i64::MAX))
                    .map(str::to_string)
                    .collect()
            };
            let probe = late.clone();
            let due = array(self.read(move |core| core.due_operator_grant_expiries(Some(&probe)))?);
            if !due.is_empty() {
                self.as_controller(move |core, context| {
                    core.expire_operator_grants(context, Some(&late))
                })?;
            }
            let limits = limits_json(self.config());
            let probe = limits.clone();
            let due = array(self.read(move |core| core.due_operator_expiries(&probe))?);
            if !due.is_empty() {
                self.as_controller(move |core, context| {
                    core.expire_operator_proposals(context, &limits)
                })?;
            }
            self.clear_orphans()?;
            Ok(())
        })();
        if let Err(error) = outcome {
            self.log("operator_tick_failed", json!({"error": error.message()}));
        } else {
            self.drain();
        }
        self.state().ticking = false;
    }
}

impl OperatorService for ProcessOperatorService {
    fn propose(&self, credential: &str, input: ProposeInput) -> OperatorResult<Value> {
        let inner = &self.inner;
        if input.kind == ProposalKind::Restart && inner.options.restart.is_none() {
            return Err(OperatorError::new(
                "restart_not_available",
                "restarting the controller from an Operator proposal is not available yet",
            )
            .into());
        }
        let restart_warning = match (&input.kind, &inner.options.restart) {
            (ProposalKind::Restart, Some(restart)) => restart.preflight()?.warning,
            _ => None,
        };
        let config = inner.config();
        let verdict = auto_decision(
            input.kind,
            &input.command,
            &config.auto_approve,
            &config.auto_approve_prefix,
        );
        let mut auto_rule: Option<String> = if inner.check_full_auto() {
            Some(FULL_AUTO_RULE.to_string())
        } else if verdict.auto {
            verdict.rule
        } else {
            None
        };
        if auto_rule.is_none() {
            let grants: Vec<GrantRef> = inner
                .live_grants()
                .iter()
                .filter_map(|grant| {
                    Some(GrantRef {
                        grant_id: grant["grantId"].as_str()?.to_string(),
                        kind: match grant["kind"].as_str()? {
                            "exact" => crate::api::GrantKind::Exact,
                            "prefix" => crate::api::GrantKind::Prefix,
                            _ => return None,
                        },
                        text: grant["text"].as_str()?.to_string(),
                    })
                })
                .collect();
            if let GrantVerdict::Matched { grant_id } =
                match_session_grant(input.kind, &input.command, &grants)
            {
                auto_rule = Some(session_rule(&grant_id));
            }
        }
        let body = json!({
            "kind": input.kind.as_str(),
            "command": input.command,
            "reason": input.reason,
            "forceRestart": input.force_restart,
            "autoRule": auto_rule,
            "maxPending": config.max_pending_proposals,
        });
        let record = inner
            .step(credential, move |core, context| {
                core.propose_operator_action(context, &body)
            })
            .map_err(OperatorFailure::from)?;
        if let Some(warning) = restart_warning {
            if let Some(notify) = &inner.options.notify_pm {
                notify(
                    &format!(
                        "Operator proposal {} (restart): {warning}.",
                        record["proposalId"].as_str().unwrap_or_default()
                    ),
                    true,
                );
            }
        }
        if record["state"].as_str() == Some("approved") {
            inner.kick();
        }
        Ok(record)
    }

    fn decide(&self, credential: &str, input: DecideInput) -> OperatorResult<Value> {
        let inner = &self.inner;
        let config = inner.config();
        let mut body = Map::new();
        body.insert("proposalId".into(), json!(input.proposal_id));
        body.insert(
            "decision".into(),
            json!(match input.decision {
                Decision::Approve => "approve",
                Decision::Deny => "deny",
            }),
        );
        if let Some(hash) = &input.hash {
            body.insert("hash".into(), json!(hash));
        }
        if let Some(note) = &input.note {
            body.insert("note".into(), json!(note));
        }
        body.insert(
            "proposalTtlMinutes".into(),
            json!(config.proposal_ttl_minutes),
        );
        if let Some(session) = &input.session {
            let mut grant = Map::new();
            grant.insert("kind".into(), json!(session.kind.as_str()));
            if let Some(text) = &session.text {
                grant.insert("text".into(), json!(text));
            }
            grant.insert("maxMinutes".into(), json!(config.session_grant_max_minutes));
            body.insert("session".into(), Value::Object(grant));
        }
        let body = Value::Object(body);
        let record = inner
            .step(credential, move |core, context| {
                core.decide_operator_proposal(context, &body)
            })
            .map_err(OperatorFailure::from)?;
        if let Some(grant_id) = record["sessionGrant"]["grantId"].as_str() {
            let deadline = inner.now() + config.session_grant_max_minutes * 60_000;
            inner
                .state()
                .grant_deadlines
                .insert(grant_id.to_string(), deadline);
        }
        if record["state"].as_str() == Some("approved") {
            inner.kick();
        }
        Ok(record)
    }

    fn cancel(&self, credential: &str, proposal_id: &str) -> OperatorResult<Value> {
        let id = proposal_id.to_string();
        self.inner
            .step(credential, move |core, context| {
                core.cancel_operator_proposal(context, &id)
            })
            .map_err(OperatorFailure::from)
    }

    fn show(&self, proposal_id: &str) -> Option<Value> {
        let id = proposal_id.to_string();
        match self.inner.read(move |core| core.operator_proposal(&id)) {
            Ok(Value::Null) | Err(_) => None,
            Ok(record) => Some(record),
        }
    }

    fn list(&self, filter: &ListFilter) -> Vec<Value> {
        let mut body = Map::new();
        if let Some(states) = &filter.states {
            body.insert("states".into(), json!(states));
        }
        if let Some(agent) = &filter.proposer_agent_id {
            body.insert("proposerAgentId".into(), json!(agent));
        }
        if let Some(limit) = filter.limit {
            body.insert("limit".into(), json!(limit));
        }
        let body = Value::Object(body);
        self.inner
            .read(move |core| core.list_operator_proposals(Some(&body)))
            .map(array)
            .unwrap_or_default()
    }

    fn grants(&self) -> Vec<Value> {
        self.inner.live_grants()
    }

    fn list_grants(&self) -> Vec<Value> {
        self.inner
            .read(|core| core.list_operator_grants(None))
            .map(array)
            .unwrap_or_default()
    }

    fn revoke_grant(&self, credential: &str, grant_id: &str) -> OperatorResult<Value> {
        let id = grant_id.to_string();
        self.inner
            .step(credential, move |core, context| {
                core.revoke_operator_grant(context, &id)
            })
            .map_err(OperatorFailure::from)
    }

    fn full_auto_on(
        &self,
        credential: &str,
        input: FullAutoOnInput,
    ) -> OperatorResult<FullAutoStatus> {
        let inner = &self.inner;
        let config = inner.config();
        let minutes = input.minutes.unwrap_or(config.full_auto_default_minutes);
        if minutes < 1 || minutes > config.full_auto_max_minutes {
            return Err(OperatorError::new(
                "full_auto_minutes",
                format!(
                    "full auto lasts 1 to {} minutes",
                    config.full_auto_max_minutes
                ),
            )
            .into());
        }
        let body = json!({
            "change": "on",
            "operatorRole": config.role,
            "minutes": minutes,
            "askedUser": input.asked_user,
        });
        inner
            .step(credential, move |core, context| {
                core.record_operator_full_auto(context, &body)
            })
            .map_err(OperatorFailure::from)?;
        inner.state().full_auto = Some((inner.now() + minutes * 60_000, minutes));
        Ok(inner.full_auto_status())
    }

    fn full_auto_off(&self, credential: &str) -> OperatorResult<FullAutoStatus> {
        let inner = &self.inner;
        if !inner.check_full_auto() {
            return Ok(FullAutoStatus::Off);
        }
        let minutes = inner.state().full_auto.map_or(0, |(_, minutes)| minutes);
        let body =
            json!({"change": "off", "operatorRole": inner.config().role, "minutes": minutes});
        inner
            .step(credential, move |core, context| {
                core.record_operator_full_auto(context, &body)
            })
            .map_err(OperatorFailure::from)?;
        inner.state().full_auto = None;
        inner.kick();
        Ok(FullAutoStatus::Off)
    }

    fn full_auto_status(&self) -> FullAutoStatus {
        self.inner.full_auto_status()
    }

    fn drain(&self) {
        self.inner.drain();
    }

    fn recover(&self, skip_restarts_with_plan: Option<&dyn Fn(&str) -> bool>) {
        let inner = &self.inner;
        // The filter is not `Send`, so it is applied here: the restart proposals it names are looked up first.
        let skipped: Vec<String> = match skip_restarts_with_plan {
            None => Vec::new(),
            Some(skip) => {
                let running = inner
                    .read(|core| {
                        core.list_operator_proposals(Some(&json!({"states": ["running"]})))
                    })
                    .map(array)
                    .unwrap_or_default();
                running
                    .iter()
                    .filter_map(|proposal| proposal["proposalId"].as_str())
                    .filter(|id| skip(id))
                    .map(str::to_string)
                    .collect()
            }
        };
        let has_filter = skip_restarts_with_plan.is_some();
        let steps = [
            inner.as_controller(move |core, context| {
                let filter = |id: &str| skipped.iter().any(|skipped| skipped == id);
                core.abandon_running_operator_runs(context, has_filter.then_some(&filter))
            }),
            inner.as_controller(|core, context| core.end_operator_grants_for_restart(context)),
        ];
        for outcome in steps {
            if let Err(error) = outcome {
                inner.log("operator_recover_failed", json!({"error": error.message()}));
            }
        }
        let role = inner.config().role.clone();
        if let Err(error) = inner.as_controller(move |core, context| {
            core.record_operator_full_auto(
                context,
                &json!({"change": "startup", "operatorRole": role}),
            )
        }) {
            inner.log("operator_recover_failed", json!({"error": error.message()}));
        }
        if let Err(error) = inner.clear_orphans() {
            inner.log("operator_recover_failed", json!({"error": error.message()}));
        }
    }

    fn tick(&self) {
        self.inner.tick();
    }

    fn start(&self, interval_ms: Option<u64>) {
        let inner = &self.inner;
        let mut state = inner.state();
        if state.timer.is_some() {
            return;
        }
        let interval = Duration::from_millis(interval_ms.unwrap_or(OPERATOR_TICK_MS));
        let (stop, receiver) = channel::<()>();
        let worker = inner.clone();
        let handle = std::thread::spawn(move || {
            while receiver.recv_timeout(interval) == Err(RecvTimeoutError::Timeout) {
                worker.tick();
            }
        });
        state.timer = Some((stop, handle));
    }

    fn stop(&self) {
        let inner = &self.inner;
        let (timer, abort) = {
            let mut state = inner.state();
            state.stopped = true;
            (state.timer.take(), state.abort.clone())
        };
        if let Some(abort) = abort {
            abort.store(true, Ordering::SeqCst);
        }
        if let Some((stop, handle)) = timer {
            let _ = stop.send(());
            let _ = handle.join();
        }
        let mut state = inner.state();
        while state.draining {
            state = inner.drained.wait(state).unwrap_or_else(|p| p.into_inner());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitising_removes_escapes_and_secrets_and_keeps_the_end() {
        let secrets = vec!["supersecretvalue".to_string()];
        let out = sanitise_output(
            "\x1b[31mred\x1b[0m token supersecretvalue\u{200b} Bearer abcdefgh12345\r\nend",
            1000,
            &secrets,
        );
        assert!(out.text.contains("red token [redacted]"), "{}", out.text);
        assert!(out.text.contains("[redacted]\nend"), "{}", out.text);
        assert!(!out.text.contains('\x1b'));
        let cut = sanitise_output("0123456789", 4, &[]);
        assert_eq!(cut.text, "6789");
        assert!(cut.truncated);
        let digest = "a".repeat(40);
        assert_eq!(sanitise_output(&digest, 100, &[]).text, digest);
        assert_eq!(
            sanitise_output(&format!("xyz1{}", "d".repeat(40)), 100, &[]).text,
            "[redacted]"
        );
    }

    #[test]
    fn a_command_runs_with_only_the_given_environment() {
        let result = run_command(RunOptions {
            command: "echo \"$ONLY\" ${HOME:-nohome}".into(),
            cwd: std::env::temp_dir(),
            timeout_ms: 5000,
            environment: vec![
                ("ONLY".into(), "x".into()),
                ("PATH".into(), "/usr/bin:/bin".into()),
            ],
            output_tail_bytes: 1000,
            on_spawn: None,
            abort: None,
        });
        assert_eq!(result.status, RunStatus::Ok);
        assert_eq!(result.output_tail, "x nohome\n");
    }

    #[test]
    fn a_timeout_ends_the_group_and_an_abort_is_an_error() {
        let base = |command: &str, timeout_ms: u64, abort: Option<Arc<AtomicBool>>| RunOptions {
            command: command.into(),
            cwd: std::env::temp_dir(),
            timeout_ms,
            environment: vec![("PATH".into(), "/usr/bin:/bin".into())],
            output_tail_bytes: 1000,
            on_spawn: None,
            abort,
        };
        let timed = run_command(base("sleep 30", 100, None));
        assert_eq!(timed.status, RunStatus::Timeout);
        let flag = Arc::new(AtomicBool::new(true));
        let aborted = run_command(base("sleep 30", 60_000, Some(flag)));
        assert_eq!(aborted.status, RunStatus::Error);
        assert!(aborted.output_tail.ends_with("stopped"));
        assert_eq!(
            run_command(base("exit 3", 5000, None)).status,
            RunStatus::Failed
        );
    }

    #[test]
    fn capstan_variables_never_reach_a_command() {
        let kept = scrubbed_environment(vec![
            ("CAPSTAN_TOKEN".into(), "t".into()),
            ("CAPSTAN_SOCKET".into(), "s".into()),
            ("PATH".into(), "/bin".into()),
        ]);
        assert_eq!(kept, vec![("PATH".to_string(), "/bin".to_string())]);
    }
}
