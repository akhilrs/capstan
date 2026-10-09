//! What every handler, loop and server thread of the daemon is built on: the `KernelHandle`, the `Deps` bundle of trait
//! objects and options, the `Loops` trait that d2b-loops implements, and the small ports (git, the driver's snapshot)
//! the handlers lean on.
//!
//! # The one-kernel-thread rule
//!
//! `capstan_kernel::Core` owns a rusqlite connection, which is `Send` but not `Sync`, and its `Env` is not `Send`. One
//! thread owns the `Core` (it opens it there) and every access is a closure posted to that thread:
//! `KernelHandle::call(|core| ...)` runs it and waits for the answer. The kernel thread is single threaded, so a
//! closure that blocks blocks every other caller. **No blocking I/O inside a kernel closure**: no herdr, no git, no
//! `ps`/`/proc` sampling, no operator command, no sleep. Loops, the launcher, the operator and `wait` run on their own
//! threads and take the kernel once per ledger step, where Node awaits.
//!
//! A call that moved the ledger's state version wakes the long-poll condvar (`KernelHandle::wait_change`), so `wait`
//! never polls.

use crate::handlers::shared::{AbortSignal, CommandState};
use capstan_config::RoleConfig;
use capstan_herdr::api::HerdrAdapter;
use capstan_kernel::types::{InitialProject, MutationContext};
use capstan_kernel::{Core, KernelError, KernelResult};
use capstan_launcher::api::LauncherService;
use capstan_operator::api::{OperatorService, RestartCoordinator};
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const SOCKET_NAME: &str = "control.sock";
pub const PID_NAME: &str = "daemon.pid";

// ------------------------------------------------------------------------------------------------ the log

/// `LogEntry` of src/daemon.ts.
#[derive(Clone, Debug, PartialEq)]
pub struct LogEntry {
    pub command: String,
    pub actor_id: Option<String>,
    pub role: Option<String>,
    pub code: String,
    pub ms: i64,
    pub arg_count: Option<usize>,
    pub arg_bytes: Option<usize>,
    pub detail: Option<Value>,
}

impl LogEntry {
    pub fn new(command: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            actor_id: None,
            role: None,
            code: code.into(),
            ms: 0,
            arg_count: None,
            arg_bytes: None,
            detail: None,
        }
    }

    /// The JSON members in the order `JSON.stringify` writes the Node entry: `command`, `actorId`, `role`, `code`, `ms`,
    /// then `argCount`, `argBytes` and `detail` when present.
    pub fn to_map(&self) -> Map<String, Value> {
        let mut map = Map::new();
        map.insert("command".into(), json!(self.command));
        map.insert("actorId".into(), json!(self.actor_id));
        map.insert("role".into(), json!(self.role));
        map.insert("code".into(), json!(self.code));
        map.insert("ms".into(), json!(self.ms));
        if let Some(count) = self.arg_count {
            map.insert("argCount".into(), json!(count));
        }
        if let Some(bytes) = self.arg_bytes {
            map.insert("argBytes".into(), json!(bytes));
        }
        if let Some(detail) = &self.detail {
            map.insert("detail".into(), detail.clone());
        }
        map
    }
}

/// `Logger`: where the daemon's log entries go (the process writes them to its standard output, one JSON line each).
pub type Logger = Arc<dyn Fn(&LogEntry) + Send + Sync>;

/// The line the process writes for a log entry: `{"ts": <now as an ISO time>, ...entry}`, as src/cli.ts writes it. Reading
/// the time is one reading of the kernel's clock, as `new Date()` is in Node, so a seeded run consumes the clock the same way.
pub fn render_log_line(kernel: &KernelHandle, entry: &LogEntry) -> String {
    let mut map = Map::new();
    map.insert(
        "ts".into(),
        json!(capstan_ledger::iso_from_millis(kernel.now_ms())),
    );
    map.extend(entry.to_map());
    Value::Object(map).to_string()
}

// ------------------------------------------------------------------------------------------------ shutdown

/// The request to stop (`stopRequested` of runDaemon): set by a signal, by the `shutdown` command and by a restart.
#[derive(Clone, Default)]
pub struct ShutdownHandle {
    inner: Arc<(Mutex<bool>, Condvar)>,
}

impl ShutdownHandle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn request(&self) {
        let (flag, condvar) = &*self.inner;
        *flag.lock().unwrap_or_else(|p| p.into_inner()) = true;
        condvar.notify_all();
    }

    pub fn requested(&self) -> bool {
        *self.inner.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Blocks until a stop is requested.
    pub fn wait(&self) {
        let (flag, condvar) = &*self.inner;
        let mut guard = flag.lock().unwrap_or_else(|p| p.into_inner());
        while !*guard {
            guard = condvar.wait(guard).unwrap_or_else(|p| p.into_inner());
        }
    }

    /// Blocks until a stop is requested or `timeout` passes; true when it was requested.
    pub fn wait_timeout(&self, timeout: Duration) -> bool {
        let (flag, condvar) = &*self.inner;
        let guard = flag.lock().unwrap_or_else(|p| p.into_inner());
        let (guard, _) = condvar
            .wait_timeout_while(guard, timeout, |stopped| !*stopped)
            .unwrap_or_else(|p| p.into_inner());
        *guard
    }
}

// ------------------------------------------------------------------------------------------------ the kernel handle

/// The kernel thread is gone (closed, or it failed to open).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KernelGone;

impl std::fmt::Display for KernelGone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the kernel is closed")
    }
}

impl std::error::Error for KernelGone {}

type Job = Box<dyn FnOnce(&Core) + Send>;

enum Message {
    Run(Job),
    Close,
}

#[derive(Default)]
struct Wake {
    counter: Mutex<u64>,
    changed: Condvar,
}

struct Inner {
    sender: Mutex<Option<Sender<Message>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    wake: Arc<Wake>,
}

/// A cloneable, `Send` handle to the thread that owns the `Core`.
#[derive(Clone)]
pub struct KernelHandle {
    inner: Arc<Inner>,
}

impl KernelHandle {
    /// Starts the kernel thread and runs `open` on it (the `Core` and its `Env` are created where they live). Returns the
    /// error `open` returned, so a held project lock surfaces here.
    pub fn spawn<F>(open: F) -> KernelResult<KernelHandle>
    where
        F: FnOnce() -> KernelResult<Core> + Send + 'static,
    {
        let (sender, receiver) = channel::<Message>();
        let (opened_tx, opened_rx) = channel::<KernelResult<()>>();
        let wake = Arc::new(Wake::default());
        let thread_wake = Arc::clone(&wake);
        let thread = std::thread::Builder::new()
            .name("kernel".into())
            .spawn(move || {
                let core = match open() {
                    Ok(core) => {
                        let _ = opened_tx.send(Ok(()));
                        core
                    }
                    Err(error) => {
                        let _ = opened_tx.send(Err(error));
                        return;
                    }
                };
                let mut version = core.state_version().unwrap_or(0);
                while let Ok(message) = receiver.recv() {
                    match message {
                        Message::Run(job) => {
                            job(&core);
                            // A call that moved the state version wakes everything waiting on the ledger.
                            if let Ok(now) = core.state_version() {
                                if now != version {
                                    version = now;
                                    *thread_wake
                                        .counter
                                        .lock()
                                        .unwrap_or_else(|p| p.into_inner()) += 1;
                                    thread_wake.changed.notify_all();
                                }
                            }
                        }
                        Message::Close => break,
                    }
                }
                core.close();
                // Waiters must not sleep on a ledger that is gone.
                *thread_wake
                    .counter
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()) += 1;
                thread_wake.changed.notify_all();
            })
            .map_err(|e| KernelError::Other(format!("the kernel thread did not start: {e}")))?;
        match opened_rx.recv() {
            Ok(Ok(())) => Ok(KernelHandle {
                inner: Arc::new(Inner {
                    sender: Mutex::new(Some(sender)),
                    thread: Mutex::new(Some(thread)),
                    wake,
                }),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(KernelError::Other(
                    "the kernel thread ended while opening".into(),
                ))
            }
        }
    }

    /// Runs `f` on the kernel thread and waits for its answer.
    ///
    /// `f` must not block on anything but the ledger (see the module documentation), and must not call back into the
    /// handle (it would wait for itself).
    pub fn call<R, F>(&self, f: F) -> Result<R, KernelGone>
    where
        R: Send + 'static,
        F: FnOnce(&Core) -> R + Send + 'static,
    {
        let (answer_tx, answer_rx) = channel::<R>();
        let job: Job = Box::new(move |core| {
            let _ = answer_tx.send(f(core));
        });
        {
            let sender = self.inner.sender.lock().unwrap_or_else(|p| p.into_inner());
            match sender.as_ref() {
                Some(sender) => sender.send(Message::Run(job)).map_err(|_| KernelGone)?,
                None => return Err(KernelGone),
            }
        }
        answer_rx.recv().map_err(|_| KernelGone)
    }

    /// `call` for a closure that returns a kernel result, flattened into one: a closed kernel is a `ControllerError` with
    /// the message Node's closed controller gives.
    pub fn run<R, F>(&self, f: F) -> KernelResult<R>
    where
        R: Send + 'static,
        F: FnOnce(&Core) -> KernelResult<R> + Send + 'static,
    {
        self.call(f)
            .map_err(|_| KernelError::controller("controller is closed"))?
    }

    /// The milliseconds the kernel's clock reads now (one reading of its `Env`, as `Date.now()` is in Node).
    pub fn now_ms(&self) -> i64 {
        self.call(|core| core.kernel().env.now()).unwrap_or(0)
    }

    /// `newContext` of src/context.ts: a context for exactly one mutation, built at the moment of the call.
    pub fn new_context(&self, credential: &str) -> KernelResult<MutationContext> {
        let credential = credential.to_string();
        self.run(move |core| new_context(core, &credential))
    }

    /// A number that grows whenever a call moved the ledger's state version (or the kernel closed).
    pub fn change_counter(&self) -> u64 {
        *self
            .inner
            .wake
            .counter
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    /// Blocks until the change counter is past `seen`, `timeout` passes or `signal` aborts (it is checked every
    /// `slice`); returns the counter now. A caller that finds nothing new simply waits again.
    pub fn wait_change(&self, seen: u64, timeout: Duration, signal: Option<&AbortSignal>) -> u64 {
        let deadline = Instant::now() + timeout;
        let mut guard = self
            .inner
            .wake
            .counter
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        loop {
            if *guard > seen || signal.is_some_and(|s| s.reason().is_some()) {
                return *guard;
            }
            let now = Instant::now();
            if now >= deadline {
                return *guard;
            }
            let slice = (deadline - now).min(Duration::from_millis(50));
            guard = self
                .inner
                .wake
                .changed
                .wait_timeout(guard, slice)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
    }

    /// Closes the `Core` (releasing the project lock) and joins the kernel thread. Later calls fail with `KernelGone`.
    pub fn close(&self) {
        let sender = self
            .inner
            .sender
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some(sender) = sender {
            let _ = sender.send(Message::Close);
        }
        let thread = self
            .inner
            .thread
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }
}

/// `newContext(core, credential)`: the ids come from the kernel's `Env` (one uuid), the counters from the ledger.
pub fn new_context(core: &Core, credential: &str) -> KernelResult<MutationContext> {
    let id = core.kernel().env.uuid();
    Ok(MutationContext {
        credential: credential.to_string(),
        request_id: format!("req-{id}"),
        idempotency_key: format!("idem-{id}"),
        expected_version: core.state_version()?,
        input_revision: core.input_revision()?,
    })
}

// ------------------------------------------------------------------------------------------------ git

/// A failure of a git operation (the port reports it; the handler words it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitError {
    pub message: String,
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for GitError {}

pub type GitResult<T> = Result<T, GitError>;

/// `CommitInspection` of src/git.ts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitInspection {
    pub commit_exists: bool,
    pub branch_tip: Option<String>,
    pub is_ancestor_of_tip: bool,
    pub is_ancestor_of_base: bool,
    /// The commit's committer time as an ISO string; None when the commit does not exist.
    pub committed_at: Option<String>,
}

pub struct InspectCommitInput {
    pub branch: String,
    pub base_sha: Option<String>,
    pub sha: String,
}

pub struct NewCommitMessagesInput {
    pub sha: String,
    pub base_sha: String,
    pub own_branch: String,
    pub limit: usize,
}

/// One new commit behind a reported commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewCommit {
    pub sha: String,
    pub parents: usize,
    pub message: String,
}

pub enum NewCommitMessages {
    Commits(Vec<NewCommit>),
    TooMany,
}

/// The git operations the handlers use (`inspectCommit`, `newCommitMessages`, `commitExists` of `CommandDependencies`
/// and `IntegrationGit` of src/integration.ts). Every method runs git: never call one inside a kernel closure. Records
/// without a fixed shape (merge inputs and results, covered reports) travel as JSON in the shapes of src/git.ts.
pub trait GitPort: Send + Sync {
    fn commit_exists(&self, sha: &str) -> GitResult<bool>;
    fn inspect_commit(&self, input: &InspectCommitInput) -> GitResult<CommitInspection>;
    fn new_commit_messages(&self, input: &NewCommitMessagesInput) -> GitResult<NewCommitMessages>;
    fn head_commit(&self) -> GitResult<String>;
    /// The first line of a commit's message; None for a git that cannot read it.
    fn commit_subject(&self, sha: &str) -> GitResult<Option<String>>;
    /// `mergeIntoBranch`: input and `MergeResult` as JSON.
    fn merge(&self, input: &Value) -> GitResult<Value>;
    fn branch_tip(&self, branch: &str) -> GitResult<Option<String>>;
    fn is_in_head(&self, sha: &str) -> GitResult<bool>;
    fn delete_branch(&self, branch: &str, sha: &str) -> GitResult<bool>;
    /// `coveredReports`: the covered reports as JSON.
    fn covered_reports(&self, head: &str, reports: &Value, options: &Value) -> GitResult<Value>;
}

/// A git port that refuses everything, for a daemon built without one (and for in-process tests).
pub struct NoGit;

impl NoGit {
    fn unavailable<T>() -> GitResult<T> {
        Err(GitError {
            message: "git is not available in this daemon".into(),
        })
    }
}

impl GitPort for NoGit {
    fn commit_exists(&self, _sha: &str) -> GitResult<bool> {
        Self::unavailable()
    }
    fn inspect_commit(&self, _input: &InspectCommitInput) -> GitResult<CommitInspection> {
        Self::unavailable()
    }
    fn new_commit_messages(&self, _input: &NewCommitMessagesInput) -> GitResult<NewCommitMessages> {
        Self::unavailable()
    }
    fn head_commit(&self) -> GitResult<String> {
        Self::unavailable()
    }
    fn commit_subject(&self, _sha: &str) -> GitResult<Option<String>> {
        Self::unavailable()
    }
    fn merge(&self, _input: &Value) -> GitResult<Value> {
        Self::unavailable()
    }
    fn branch_tip(&self, _branch: &str) -> GitResult<Option<String>> {
        Self::unavailable()
    }
    fn is_in_head(&self, _sha: &str) -> GitResult<bool> {
        Self::unavailable()
    }
    fn delete_branch(&self, _branch: &str, _sha: &str) -> GitResult<bool> {
        Self::unavailable()
    }
    fn covered_reports(&self, _head: &str, _reports: &Value, _options: &Value) -> GitResult<Value> {
        Self::unavailable()
    }
}

// ------------------------------------------------------------------------------------------------ the driver's view

/// `DriverSnapshot` of src/commands/shared.ts: what status shows of the delivery driver.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DriverSnapshot {
    pub stalled_agent_ids: Vec<String>,
    /// Agents whose pane Herdr no longer finds, as the driver sees them now.
    pub lost_agent_ids: Option<Vec<String>>,
    /// (message id, reason)
    pub stuck: Vec<(String, String)>,
    /// The open stale-mail episode of the PM, as JSON.
    pub pm_stale: Option<Value>,
}

/// The delivery driver as the status command sees it (`driverSnapshot`).
pub trait DriverView: Send + Sync {
    fn snapshot(&self) -> DriverSnapshot;
}

/// No driver runs: nothing is stalled or stuck.
pub struct NoDriver;

impl DriverView for NoDriver {
    fn snapshot(&self) -> DriverSnapshot {
        DriverSnapshot::default()
    }
}

// ------------------------------------------------------------------------------------------------ loops

/// The background work of the daemon, implemented by d2b-loops (the delivery driver, supervision, the report relay,
/// review and integration recovery, the operator's tick). The daemon's run function drives it in Node's order:
///
/// 1. the socket binds and the launcher adopts its panes in the background;
/// 2. when adoption is done: `start_ticks` (driver, supervision), and `recover_and_relay` (reviews' reviewers
///    released, integrations recovered, the report relay started);
/// 3. on stop: `stop_relay`, then (after the adoption has ended) `stop_ticks`.
///
/// Every loop runs on its own thread and takes the kernel (`deps.kernel`) once per ledger step.
pub trait Loops: Send + Sync {
    fn start_ticks(&self, deps: &Deps);
    fn recover_and_relay(&self, deps: &Deps);
    fn stop_relay(&self);
    fn stop_ticks(&self);
}

/// Loops that do nothing (a daemon without a launcher, and tests).
pub struct NoLoops;

impl Loops for NoLoops {
    fn start_ticks(&self, _deps: &Deps) {}
    fn recover_and_relay(&self, _deps: &Deps) {}
    fn stop_relay(&self) {}
    fn stop_ticks(&self) {}
}

// ------------------------------------------------------------------------------------------------ options and deps

/// What announces `ready`, `lock_released` and `socket_removed`: called with the event and the process id.
pub type Announce = Arc<dyn Fn(&str, u32) + Send + Sync>;

/// `DaemonOptions` of src/daemon.ts.
#[derive(Clone)]
pub struct DaemonOptions {
    pub state_directory: PathBuf,
    pub project: InitialProject,
    pub workspace_root: PathBuf,
    /// The configuration of capstan.toml; without it only the commands run (no driver, launcher or operator).
    pub capstan: Option<Arc<RoleConfig>>,
    /// How often the report relay and the driver tick; None is the default of each.
    pub tick_ms: Option<u64>,
    /// How often the supervision tick looks; a test makes it short.
    pub supervision_tick_ms: Option<u64>,
    /// Absolute path of the executable the launched agents' `cstan` wrapper runs.
    pub cli_path: Option<PathBuf>,
    /// The announcements `ready`, `lock_released` and `socket_removed` (one JSON line each on the process's output).
    pub announce: Option<Announce>,
}

impl DaemonOptions {
    pub fn new(state_directory: PathBuf, project: InitialProject, workspace_root: PathBuf) -> Self {
        Self {
            state_directory,
            project,
            workspace_root,
            capstan: None,
            tick_ms: None,
            supervision_tick_ms: None,
            cli_path: None,
            announce: None,
        }
    }

    /// The control socket's path.
    pub fn socket_path(&self) -> PathBuf {
        self.state_directory.join(SOCKET_NAME)
    }

    pub fn pid_path(&self) -> PathBuf {
        self.state_directory.join(PID_NAME)
    }

    /// The project (operator) credential.
    pub fn credential(&self) -> &str {
        &self.project.owner_credential
    }
}

/// Everything a handler, loop or server thread needs, cheap to clone and `Send`.
#[derive(Clone)]
pub struct Deps {
    pub kernel: KernelHandle,
    pub options: Arc<DaemonOptions>,
    pub log: Logger,
    pub shutdown: ShutdownHandle,
    /// The per-instance state of the command handlers (rate limiters, waits).
    pub commands: Arc<CommandState>,
    /// The launcher, when capstan.toml configures one and launching is on.
    pub launcher: Option<Arc<dyn LauncherService>>,
    /// The Operator service, when `[operator]` is enabled.
    pub operator: Option<Arc<dyn OperatorService>>,
    pub restart: Option<Arc<dyn RestartCoordinator>>,
    pub adapter: Option<Arc<dyn HerdrAdapter>>,
    pub git: Arc<dyn GitPort>,
    pub driver: Arc<dyn DriverView>,
    pub loops: Arc<dyn Loops>,
}

impl Deps {
    /// Deps with the kernel only: no launcher, operator or adapter, a git that refuses, no driver and no loops.
    pub fn new(kernel: KernelHandle, options: DaemonOptions, log: Logger) -> Self {
        Self {
            kernel,
            options: Arc::new(options),
            log,
            shutdown: ShutdownHandle::new(),
            commands: Arc::new(CommandState::default()),
            launcher: None,
            operator: None,
            restart: None,
            adapter: None,
            git: Arc::new(NoGit),
            driver: Arc::new(NoDriver),
            loops: Arc::new(NoLoops),
        }
    }

    /// The project (operator) credential.
    pub fn credential(&self) -> &str {
        self.options.credential()
    }

    /// `detailLog` of runDaemon: a `daemon:<event>` entry with code `info` and a detail.
    pub fn detail_log(&self, event: &str, detail: Value) {
        let mut entry = LogEntry::new(format!("daemon:{event}"), "info");
        entry.detail = Some(detail);
        (self.log)(&entry);
    }
}

// ------------------------------------------------------------------------------------------------ startup

/// What runDaemon does to the ledger right after it opens, in Node's order and with Node's clock and randomness
/// readings, so a seeded run writes the same rows: close the waits a dead process left, fail the prompt relays that were
/// typing, and (with a configuration) tell the kernel about prompt relay. Each step that fails is logged and goes on.
pub fn startup_sequence(deps: &Deps) {
    let credential = deps.credential().to_string();
    // closeStaleWaits: a wait cannot outlive its connection, so any row still open at start belongs to a dead process.
    match deps.kernel.run({
        let credential = credential.clone();
        move |core| core.open_waits(&credential)
    }) {
        Ok(Value::Array(waits)) => {
            for wait in waits {
                let Some(wait_id) = wait
                    .get("waitId")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                else {
                    continue;
                };
                let credential = credential.clone();
                let outcome = deps.kernel.run({
                    let wait_id = wait_id.clone();
                    move |core| {
                        let context = new_context(core, &credential)?;
                        core.end_wait_as_controller(&context, &wait_id)
                    }
                });
                if let Err(error) = outcome {
                    deps.detail_log(
                        "stale_wait_not_closed",
                        json!({"waitId": wait_id, "error": error.to_string()}),
                    );
                }
            }
        }
        Ok(_) => {}
        Err(error) => deps.detail_log("stale_wait_not_closed", json!({"error": error.to_string()})),
    }
    if let Some(config) = &deps.options.capstan {
        let relay = json!({
            "enabled": config.prompt_relay.enabled,
            "captureTtlSeconds": config.prompt_relay.capture_ttl_seconds,
        });
        if let Err(error) = deps
            .kernel
            .run(move |core| core.configure_prompt_relay(&relay))
        {
            deps.detail_log(
                "prompt_relay_reconcile_failed",
                json!({"error": error.to_string()}),
            );
        }
    }
    // A row left typing by a daemon that stopped may have had keys sent; it is failed, never retried.
    let interrupted = deps.kernel.run({
        let credential = credential.clone();
        move |core| {
            let context = new_context(core, &credential)?;
            core.fail_interrupted_prompt_relays(&context)
        }
    });
    match interrupted {
        Ok(Value::Array(relays)) if !relays.is_empty() => {
            deps.detail_log("prompt_relay_interrupted", json!({"relays": relays}));
        }
        Ok(_) => {}
        Err(error) => deps.detail_log(
            "prompt_relay_reconcile_failed",
            json!({"error": format!("{error}")}),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use capstan_kernel::kernel::KernelOptions;
    use capstan_kernel::SeededEnv;
    use std::os::unix::fs::PermissionsExt;

    pub(crate) fn project() -> InitialProject {
        InitialProject {
            project_id: "proj1".into(),
            name: "Deps Project".into(),
            owner_credential: "owner-credential-0123456789-abcdefghijklmnopqrstuvwxyz".into(),
            initial_inputs: crate::run::placeholder_inputs(),
        }
    }

    fn open(dir: &tempfile::TempDir) -> KernelHandle {
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let state = dir.path().to_path_buf();
        KernelHandle::spawn(move || {
            Core::open(
                &state,
                &project(),
                &KernelOptions {
                    workspace_root: Some(state.clone()),
                    ..KernelOptions::default()
                },
                Box::new(SeededEnv::new("deps")),
            )
        })
        .expect("the kernel opens")
    }

    #[test]
    fn a_second_open_on_the_same_state_directory_reports_the_held_lock() {
        let dir = tempfile::tempdir().unwrap();
        let first = open(&dir);
        let state = dir.path().to_path_buf();
        let second = KernelHandle::spawn(move || {
            Core::open(
                &state,
                &project(),
                &KernelOptions {
                    workspace_root: Some(state.clone()),
                    ..KernelOptions::default()
                },
                Box::new(SeededEnv::new("deps")),
            )
        });
        let error = second.err().expect("the lock is held");
        assert_eq!(error.name(), "ControllerOwnershipError");
        first.close();
    }

    #[test]
    fn calls_run_on_one_thread_and_a_mutation_wakes_the_waiters() {
        let dir = tempfile::tempdir().unwrap();
        let kernel = open(&dir);
        let first = kernel.call(|_| std::thread::current().id()).unwrap();
        let second = kernel.call(|_| std::thread::current().id()).unwrap();
        assert_eq!(first, second);
        assert_ne!(first, std::thread::current().id());

        let seen = kernel.change_counter();
        let waiter = {
            let kernel = kernel.clone();
            std::thread::spawn(move || kernel.wait_change(seen, Duration::from_secs(10), None))
        };
        let credential = project().owner_credential;
        kernel
            .run(move |core| {
                let context = new_context(core, &credential)?;
                core.create_seat(
                    &context,
                    &json!({"seatId": "seat-1", "name": "seat-1", "role": "Developer"}),
                )
            })
            .expect("a mutation");
        assert!(waiter.join().unwrap() > seen);
        // A read changes nothing and wakes nobody.
        let now = kernel.change_counter();
        kernel.run(|core| core.list_agents()).unwrap();
        assert_eq!(kernel.change_counter(), now);
        kernel.close();
    }

    #[test]
    fn a_closed_kernel_answers_with_the_closed_controller_error() {
        let dir = tempfile::tempdir().unwrap();
        let kernel = open(&dir);
        kernel.close();
        assert_eq!(kernel.call(|_| 1), Err(KernelGone));
        let error = kernel.run(|core| core.list_agents()).unwrap_err();
        assert_eq!(error.message(), "controller is closed");
        // A second close is harmless.
        kernel.close();
    }

    #[test]
    fn wait_change_returns_at_the_timeout_and_on_abort() {
        let dir = tempfile::tempdir().unwrap();
        let kernel = open(&dir);
        let seen = kernel.change_counter();
        let started = Instant::now();
        assert_eq!(
            kernel.wait_change(seen, Duration::from_millis(60), None),
            seen
        );
        assert!(started.elapsed() >= Duration::from_millis(50));
        let signal = AbortSignal::new();
        signal.abort("closed");
        let started = Instant::now();
        kernel.wait_change(seen, Duration::from_secs(30), Some(&signal));
        assert!(started.elapsed() < Duration::from_secs(5));
        kernel.close();
    }

    /// The body of one `pub trait <name>` of an api.rs source.
    fn trait_body<'a>(source: &'a str, name: &str) -> &'a str {
        let start = source
            .find(&format!("pub trait {name}"))
            .unwrap_or_else(|| panic!("no trait {name}"));
        let rest = &source[start..];
        let end = rest.find("\n}\n").expect("the end of the trait");
        &rest[..end]
    }

    #[test]
    fn the_api_files_cover_the_readme_list_of_node_methods() {
        let readme = include_str!("../../../../test/daemon-scenarios/README.md");
        let list = readme
            .split("```api-coverage\n")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
            .expect("the api-coverage block");
        let sources = [
            ("herdr", include_str!("../../herdr/src/api.rs")),
            ("launcher", include_str!("../../launcher/src/api.rs")),
            ("operator", include_str!("../../operator/src/api.rs")),
        ];
        let mut checked = 0;
        for line in list.lines().filter(|line| !line.trim().is_empty()) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let [krate, interface, _node, "->", method] = parts[..] else {
                panic!("a malformed coverage line: {line}");
            };
            let source = sources.iter().find(|(name, _)| *name == krate).unwrap().1;
            let body = trait_body(source, interface);
            assert!(
                body.contains(&format!("fn {method}(")),
                "{krate}::{interface} has no fn {method} ({line})"
            );
            checked += 1;
        }
        assert!(checked >= 60, "only {checked} lines");
    }

    #[test]
    fn log_entries_keep_the_members_of_the_node_entry_in_order() {
        let mut entry = LogEntry::new("ping", "ok");
        entry.actor_id = Some("a".into());
        entry.role = Some("operator".into());
        entry.arg_count = Some(0);
        entry.arg_bytes = Some(0);
        let text = serde_json::to_string(&entry.to_map()).unwrap();
        assert_eq!(
            text,
            r#"{"command":"ping","actorId":"a","role":"operator","code":"ok","ms":0,"argCount":0,"argBytes":0}"#
        );
    }
}
