//! The verify service: runs the project's configured gate commands (`[verify]`) against the commit of a report or an
//! integration, in a scratch git worktree outside the project checkout, and records how each run ended in the ledger.
//!
//! `max_parallel` worker threads claim queued runs from the ledger (`claimVerifyRun`). A run is: a detached worktree at
//! the commit, the configured `[worktree]` setup, each command through the operator's command runner (own process group,
//! scrubbed environment, the whole-run timeout shared by the steps), cleanup (group, marked processes, teardown, worktree
//! and directory), and then one `finishVerifyRun`. Cleanup comes before the finish, so a client that sees a final state
//! also finds the scratch gone.
//!
//! Nothing here runs inside a kernel closure: every ledger step is one `KernelHandle` call, git and the gate run on the
//! worker's own thread.
mod log;
mod recover;
mod run;
pub mod scratch;

use crate::deps::{new_context, Deps, KernelHandle, LogEntry, Logger};
use capstan_config::{Verify, Worktree};
use capstan_kernel::types::MutationContext;
use capstan_kernel::{Core, KernelResult};
use capstan_operator::service::RealProcesses;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// How long a worker sleeps without a wake-up before it looks at the queue again.
const IDLE_CHECK: Duration = Duration::from_secs(5);

pub struct VerifyService {
    pub(crate) kernel: KernelHandle,
    credential: String,
    pub(crate) verify: Verify,
    pub(crate) worktree: Option<Worktree>,
    pub(crate) env_pass: Vec<String>,
    pub(crate) cache: Option<capstan_cache::Runtime>,
    pub(crate) project_root: PathBuf,
    pub(crate) scratch_root: PathBuf,
    pub(crate) project_id: String,
    log: Logger,
    pub(crate) processes: RealProcesses,
    stopping: AtomicBool,
    wake: (Mutex<u64>, Condvar),
    aborts: Mutex<HashMap<String, Arc<AtomicBool>>>,
    cancelled: Mutex<HashSet<String>>,
    threads: Mutex<Vec<JoinHandle<()>>>,
    /// Why the scratch directory cannot be used; no run starts while it is set.
    problem: Mutex<Option<String>>,
    /// A test hook: milliseconds a worker waits between claiming a run and starting it.
    pub claim_delay_ms: std::sync::atomic::AtomicU64,
}

/// `~/x` against `$HOME`, otherwise the path as it is.
fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) if !home.is_empty() => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

/// `<cache base>/projects/<project id>/verify` when the configuration names no scratch directory.
fn default_scratch(project_id: &str) -> PathBuf {
    let base = match std::env::var("XDG_CACHE_HOME") {
        Ok(xdg) if xdg.starts_with('/') => PathBuf::from(xdg).join("capstan"),
        _ => match std::env::var("HOME") {
            Ok(home) if !home.is_empty() => PathBuf::from(home).join(".cache").join("capstan"),
            // SAFETY: getuid has no preconditions.
            _ => PathBuf::from(format!("/tmp/capstan-cache-{}", unsafe { libc::getuid() })),
        },
    };
    let segment: String = project_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    base.join("projects").join(segment).join("verify")
}

impl VerifyService {
    /// The service for the daemon's `[verify]` section; `None` without one.
    pub fn new(deps: &Deps) -> Option<Arc<Self>> {
        let verify = deps.options.capstan.as_ref()?.verify.clone()?;
        Some(Self::with_config(deps, verify))
    }

    /// The service for `verify` (a test may choose limits the configuration does not allow).
    pub fn with_config(deps: &Deps, verify: Verify) -> Arc<Self> {
        let config = deps.options.capstan.as_deref();
        let scratch_root = verify
            .scratch_dir
            .as_deref()
            .map(expand_home)
            .unwrap_or_else(|| default_scratch(&deps.options.project.project_id));
        Arc::new(Self {
            kernel: deps.kernel.clone(),
            credential: deps.credential().to_string(),
            worktree: config.and_then(|c| c.worktree.clone()),
            env_pass: config.map(|c| c.env_pass.clone()).unwrap_or_default(),
            cache: crate::handlers::cache::runtime_of_deps(deps),
            project_root: deps.options.workspace_root.clone(),
            scratch_root,
            project_id: deps.options.project.project_id.clone(),
            log: deps.log.clone(),
            processes: RealProcesses,
            verify,
            stopping: AtomicBool::new(false),
            wake: (Mutex::new(0), Condvar::new()),
            aborts: Mutex::new(HashMap::new()),
            cancelled: Mutex::new(HashSet::new()),
            threads: Mutex::new(Vec::new()),
            problem: Mutex::new(None),
            claim_delay_ms: std::sync::atomic::AtomicU64::new(0),
        })
    }

    pub fn config(&self) -> &Verify {
        &self.verify
    }

    /// Why runs cannot start (the scratch directory is refused), if they cannot.
    pub fn problem(&self) -> Option<String> {
        self.problem
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn commands_sha(&self) -> String {
        self.verify.commands_sha()
    }

    pub(crate) fn detail_log(&self, event: &str, detail: Value) {
        let mut entry = LogEntry::new(format!("daemon:{event}"), "info");
        entry.detail = Some(detail);
        (self.log)(&entry);
    }

    /// One ledger step as the daemon's own credential, with a fresh mutation context.
    pub(crate) fn step<R: Send + 'static>(
        &self,
        f: impl FnOnce(&Core, &MutationContext) -> KernelResult<R> + Send + 'static,
    ) -> KernelResult<R> {
        let credential = self.credential.clone();
        self.kernel.run(move |core| {
            let context = new_context(core, &credential)?;
            f(core, &context)
        })
    }

    /// Wakes the workers: a run was queued.
    pub fn notify(&self) {
        let (counter, condvar) = &self.wake;
        *counter.lock().unwrap_or_else(|p| p.into_inner()) += 1;
        condvar.notify_all();
    }

    pub(crate) fn stopping(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    /// Asks a running run to end: its process group is killed and the run is finished as interrupted. False when this
    /// service is not running that run.
    pub fn abort(&self, run_id: &str) -> bool {
        let flag = self
            .aborts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(run_id)
            .cloned();
        match flag {
            Some(flag) => {
                self.cancelled
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(run_id.to_string());
                flag.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    /// Recovers what a stopped daemon left, then starts the workers; all on a thread of its own, so the socket does not
    /// wait for the cleanup.
    pub fn start(self: &Arc<Self>) {
        let service = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("verify".into())
            .spawn(move || {
                service.recover();
                if service.stopping() || service.problem().is_some() {
                    return;
                }
                service.spawn_workers();
            });
        match spawned {
            Ok(handle) => self
                .threads
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(handle),
            Err(error) => {
                self.detail_log("verify_start_failed", json!({"error": error.to_string()}))
            }
        }
    }

    fn spawn_workers(self: &Arc<Self>) {
        let mut threads = self.threads.lock().unwrap_or_else(|p| p.into_inner());
        for index in 0..self.verify.max_parallel.max(1) {
            let service = Arc::clone(self);
            match std::thread::Builder::new()
                .name(format!("verify-{index}"))
                .spawn(move || service.work())
            {
                Ok(handle) => threads.push(handle),
                Err(error) => {
                    self.detail_log("verify_start_failed", json!({"error": error.to_string()}))
                }
            }
        }
    }

    /// Stops: running runs are aborted and cleaned, the workers end, and every run still marked running is recorded as
    /// interrupted. Call it before the kernel closes.
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        for flag in self
            .aborts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
        {
            flag.store(true, Ordering::SeqCst);
        }
        self.notify();
        let threads: Vec<_> = self
            .threads
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .drain(..)
            .collect();
        for thread in threads {
            let _ = thread.join();
        }
        if let Err(error) = self.step(|core, context| core.interrupt_active_verify_runs(context)) {
            self.detail_log(
                "verify_interrupt_failed",
                json!({"error": error.to_string()}),
            );
        }
    }

    /// The worker loop: claim the oldest queued run, execute it, look again; sleep when the queue is empty.
    fn work(self: &Arc<Self>) {
        loop {
            if self.stopping() {
                return;
            }
            let seen = *self.wake.0.lock().unwrap_or_else(|p| p.into_inner());
            match self.claim_next() {
                Some((run, abort)) => self.execute(&run, abort),
                None => {
                    let (counter, condvar) = &self.wake;
                    let guard = counter.lock().unwrap_or_else(|p| p.into_inner());
                    if *guard == seen && !self.stopping() {
                        let _ = condvar
                            .wait_timeout(guard, IDLE_CHECK)
                            .unwrap_or_else(|p| p.into_inner());
                    }
                }
            }
        }
    }

    /// The run this worker claimed, if there is a queued one and room for it.
    fn claim_next(&self) -> Option<(Value, Arc<AtomicBool>)> {
        let limit = self.verify.max_parallel;
        let queued = match self
            .kernel
            .run(move |core| core.next_queued_verify_runs(limit))
        {
            Ok(queued) => queued,
            Err(_) => return None,
        };
        for run in queued["runs"].as_array().into_iter().flatten() {
            let run_id = run["runId"].as_str()?.to_string();
            let scratch = self.scratch_root.join(&run_id);
            let log = self.scratch_root.join("logs").join(format!("{run_id}.log"));
            let input = json!({
                "runId": run_id,
                "maxParallel": limit,
                "scratchPath": scratch.to_string_lossy(),
                "logPath": log.to_string_lossy(),
            });
            // The abort flag exists before the run can be seen as running, so a cancel cannot fall between the two.
            let Some(abort) = self.register(&run_id) else {
                continue; // another worker is claiming it
            };
            match self.step(move |core, context| core.claim_verify_run(context, &input)) {
                Ok(claimed) if claimed["claimed"] == true => {
                    let delay = self.claim_delay_ms.load(Ordering::SeqCst);
                    if delay > 0 {
                        std::thread::sleep(Duration::from_millis(delay));
                    }
                    return Some((claimed["run"].clone(), abort));
                }
                Ok(_) => {
                    self.unregister(&run_id);
                    continue;
                }
                Err(error) => {
                    self.unregister(&run_id);
                    self.detail_log(
                        "verify_claim_failed",
                        json!({"runId": run_id, "error": error.to_string()}),
                    );
                    return None;
                }
            }
        }
        None
    }

    /// Takes the abort flag of a run about to be claimed (None when another worker holds it); a stop that came first
    /// aborts it at once.
    fn register(&self, run_id: &str) -> Option<Arc<AtomicBool>> {
        let mut aborts = self.aborts.lock().unwrap_or_else(|p| p.into_inner());
        if aborts.contains_key(run_id) {
            return None;
        }
        let flag = Arc::new(AtomicBool::new(false));
        aborts.insert(run_id.to_string(), flag.clone());
        if self.stopping() {
            flag.store(true, Ordering::SeqCst);
        }
        Some(flag)
    }

    fn unregister(&self, run_id: &str) -> bool {
        self.aborts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(run_id);
        self.cancelled
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(run_id)
    }
}
