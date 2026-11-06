//! One verify run, from the claim to the finish.
use super::log::CappedLog;
use super::scratch;
use super::{LeaseFault, StepRequest, VerifyService};
use capstan_cache::{GateLeases, GateScope, LeaseError};
use capstan_herdr::claude_args::build_agent_environment;
use capstan_launcher::cache_env::cache_extras;
use capstan_operator::service::{process_start_time, run_command, RunOptions, RunStatus};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How a run ended, before it is written to the ledger.
pub(crate) struct Outcome {
    pub state: &'static str,
    pub exit_code: Option<i64>,
    pub failed_step: Option<i64>,
    pub tail: String,
    pub truncated: bool,
}

/// A step that was not run, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Skipped {
    pub step: i64,
    pub command: String,
    pub reason: String,
}

impl Skipped {
    pub(crate) fn to_json(&self) -> Value {
        json!({"step": self.step, "command": self.command, "reason": self.reason})
    }
}

/// The steps of `commands` whose path filter excludes every path of `changed`; `filters` is `[verify]`'s filter list.
/// A run whose commands differ from the configured ones (the config changed after it was queued) skips nothing.
pub(crate) fn skipped_steps(
    commands: &[String],
    configured: &[String],
    filters: &[Option<Vec<String>>],
    changed: Option<&[String]>,
) -> Vec<Skipped> {
    if commands != configured || filters.len() != commands.len() {
        return Vec::new();
    }
    let mut skipped = Vec::new();
    for (index, command) in commands.iter().enumerate() {
        let filter = filters[index].as_deref();
        if capstan_config::step_runs(filter, changed) {
            continue;
        }
        skipped.push(Skipped {
            step: (index + 1) as i64,
            command: command.clone(),
            reason: format!(
                "the diff touches none of {}",
                filter.unwrap_or_default().join(", ")
            ),
        });
    }
    skipped
}

/// The paths changed between the merge base of `commit` and the project's checked-out HEAD, and `commit`. `None` when
/// they cannot be read, which makes every step run.
pub(crate) fn changed_paths(project_root: &Path, commit: &str) -> Option<Vec<String>> {
    let base = scratch::git(project_root, &["merge-base", commit, "HEAD"]).ok()?;
    let base = base.trim();
    if base.is_empty() {
        return None;
    }
    let list = scratch::git(
        project_root,
        &["diff", "--name-only", "--no-renames", "-z", base, commit],
    )
    .ok()?;
    Some(
        list.split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

impl Outcome {
    fn error(message: impl Into<String>) -> Self {
        Self {
            state: "error",
            exit_code: None,
            failed_step: None,
            tail: message.into(),
            truncated: false,
        }
    }
}

/// What `steps` needs to know about one claimed run.
#[derive(Clone, Copy)]
pub(crate) struct Job<'a> {
    pub run_id: &'a str,
    pub commit: &'a str,
    pub commands: &'a [String],
    pub scratch: &'a Path,
    pub log: &'a CappedLog,
    pub abort: &'a Arc<AtomicBool>,
    pub started: Instant,
    pub slot: usize,
    pub scope: GateScope,
}

/// What a slot directory looked like before a step: its inode and the names of its top-level subdirectories.
struct SlotSnapshot {
    path: PathBuf,
    inode: u64,
    subdirs: BTreeSet<String>,
}

/// How a run's lease attempt went, decided before the worktree exists.
pub(crate) enum LeaseOutcome {
    /// Leased, or nothing to lease.
    Ready,
    /// The run ends before any step: `None` is an abort, `Some` an error outcome.
    Failed(Option<Outcome>),
}

/// What a run leaves that cleanup has to remove.
#[derive(Default)]
pub(crate) struct Left {
    /// Process groups of the commands that were started.
    pub groups: Arc<Mutex<Vec<i64>>>,
    /// Start time (clock ticks) of the first process, the earliest a process of this run can have started.
    pub first_start: Arc<Mutex<Option<u64>>>,
    /// Whether the worktree was created, so a teardown hook and a removal are due.
    pub worktree_added: bool,
    /// Set once the first process of the run is recorded in the ledger (it keeps one, write-once).
    pub recorded: Arc<AtomicBool>,
    /// The steps the path filters left out; recorded with the finish.
    pub skipped: Vec<Skipped>,
}

impl VerifyService {
    /// The environment of the gate commands: a scrubbed base, `[env] pass`, the `[cache]` variables, `[verify].env`, and
    /// the marker of this run.
    pub(crate) fn command_environment(
        &self,
        run_id: &str,
        commit: &str,
        scope: GateScope,
    ) -> Result<Vec<(String, String)>, String> {
        let base: HashMap<String, String> = std::env::vars().collect();
        let mut extras: Vec<(String, String)> = Vec::new();
        if let Some(runtime) = &self.cache {
            match cache_extras(runtime, scope) {
                Ok(cache) => extras.extend(cache),
                Err(message) => {
                    self.detail_log("verify_cache_unavailable", json!({"message": message}))
                }
            }
        }
        extras.extend(self.verify.env.iter().cloned());
        let mut environment = build_agent_environment(&base, &extras, &self.env_pass)
            .map_err(|error| format!("the gate environment is not acceptable: {error}"))?;
        environment.insert("CAPSTAN_VERIFY_RUN_ID".into(), run_id.to_string());
        environment.insert("CAPSTAN_VERIFY_COMMIT".into(), commit.to_string());
        Ok(environment.into_iter().collect())
    }

    #[allow(clippy::too_many_arguments)]
    fn run_step(
        &self,
        command: &str,
        cwd: &Path,
        timeout_ms: u64,
        environment: &[(String, String)],
        abort: &Arc<AtomicBool>,
        log: &CappedLog,
        spawned: Box<dyn Fn(u32) + Send + Sync>,
    ) -> capstan_operator::service::RunResult {
        let runner = self
            .step_runner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        runner(StepRequest {
            command: command.to_string(),
            cwd: cwd.to_path_buf(),
            timeout_ms,
            environment: environment.to_vec(),
            abort: abort.clone(),
            log: log.clone(),
            on_spawn: spawned,
        })
    }

    /// The directories of this run's slot, as they are now.
    fn snapshot_slot(&self, slot: usize) -> Vec<SlotSnapshot> {
        use std::os::unix::fs::MetadataExt;
        let Some(runtime) = &self.cache else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (_, n, dirs) in runtime.gate_slots() {
            if n != slot {
                continue;
            }
            for path in dirs {
                let Ok(meta) = std::fs::metadata(&path) else {
                    continue;
                };
                let subdirs = std::fs::read_dir(&path)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect();
                out.push(SlotSnapshot {
                    path,
                    inode: meta.ino(),
                    subdirs,
                });
            }
        }
        out
    }

    /// True when a slot directory is gone, replaced, or lost a top-level subdirectory it had.
    fn slot_damaged(snapshot: &[SlotSnapshot]) -> bool {
        use std::os::unix::fs::MetadataExt;
        snapshot.iter().any(|s| match std::fs::metadata(&s.path) {
            Err(_) => true,
            Ok(meta) => meta.ino() != s.inode || s.subdirs.iter().any(|n| !s.path.join(n).is_dir()),
        })
    }

    /// The callback that notes each started process group, and records the first one in the ledger (the ledger keeps one
    /// process per run; the others are found by the marker in their environment).
    fn on_spawn(&self, run_id: &str, left: &Left) -> Box<dyn Fn(u32) + Send + Sync> {
        let kernel = self.kernel.clone();
        let credential = self.credential.clone();
        let run_id = run_id.to_string();
        let groups = left.groups.clone();
        let first = left.first_start.clone();
        let log = self.log.clone();
        let recorded = left.recorded.clone();
        Box::new(move |pid| {
            groups
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(i64::from(pid));
            let Some(start) = process_start_time(i64::from(pid)) else {
                return;
            };
            first
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get_or_insert(start.parse().unwrap_or(0));
            if recorded.swap(true, Ordering::SeqCst) {
                return;
            }
            let input = json!({"runId": run_id, "pgid": pid, "leaderStart": start});
            let credential = credential.clone();
            let outcome = kernel.run(move |core| {
                let context = crate::deps::new_context(core, &credential)?;
                core.record_verify_process(&context, &input)
            });
            if let Err(error) = outcome {
                let mut entry =
                    crate::deps::LogEntry::new("daemon:verify_process_not_recorded", "info");
                entry.detail = Some(json!({"runId": run_id, "error": error.to_string()}));
                log(&entry);
            }
        })
    }

    /// Everything between the claim and the cleanup. `None` means the run was aborted (cancelled, or the daemon stops).
    fn steps(
        &self,
        job: &Job<'_>,
        left: &mut Left,
        leases: &mut Option<GateLeases>,
    ) -> Option<Outcome> {
        let Job {
            run_id,
            commit,
            commands,
            scratch,
            log,
            abort,
            started,
            slot,
            scope,
        } = *job;
        let deadline = started + Duration::from_secs(self.verify.timeout_seconds.max(1) as u64);
        let remaining = |now: Instant| deadline.saturating_duration_since(now).as_millis() as u64;
        if let Err(message) = scratch::add_worktree(&self.project_root, scratch, commit) {
            left.worktree_added = scratch.exists();
            return Some(Outcome::error(format!(
                "git worktree add failed: {message}"
            )));
        }
        left.worktree_added = true;
        if abort.load(Ordering::SeqCst) {
            return None;
        }
        let environment = match self.command_environment(run_id, commit, scope) {
            Ok(environment) => environment,
            Err(message) => return Some(Outcome::error(message)),
        };
        if self.verify.use_worktree_hooks {
            if let Some(setup) = self
                .worktree
                .as_ref()
                .and_then(|w| w.setup.clone().map(|s| (s, w.setup_timeout_seconds)))
            {
                log.append(format!("$ [setup] {}\n", setup.0).as_bytes());
                let timeout = remaining(Instant::now()).min(setup.1.max(1) as u64 * 1000);
                let result = self.run_step(
                    &setup.0,
                    scratch,
                    timeout,
                    &environment,
                    abort,
                    log,
                    self.on_spawn(run_id, left),
                );
                if abort.load(Ordering::SeqCst) {
                    return None;
                }
                if result.status != RunStatus::Ok {
                    let how = match result.status {
                        RunStatus::Timeout => "timed out".to_string(),
                        _ => format!(
                            "exit {}",
                            result.exit_code.map_or("none".into(), |c| c.to_string())
                        ),
                    };
                    return Some(Outcome {
                        state: "error",
                        exit_code: result.exit_code.map(i64::from),
                        failed_step: None,
                        tail: format!("worktree setup failed ({how})\n{}", result.output_tail),
                        truncated: result.truncated,
                    });
                }
            }
        }
        let mut last: Option<capstan_operator::service::RunResult> = None;
        let changed = if self.verify.path_filters.iter().any(Option::is_some) {
            changed_paths(&self.project_root, commit)
        } else {
            None
        };
        left.skipped = skipped_steps(
            commands,
            &self.verify.commands,
            &self.verify.path_filters,
            changed.as_deref(),
        );
        let lease_wait = *self.lease_wait.lock().unwrap_or_else(|p| p.into_inner());
        let mut snapshot = if leases.as_ref().is_some_and(|l| !l.is_empty()) {
            self.snapshot_slot(slot)
        } else {
            Vec::new()
        };
        let mut rerun = false;
        let mut first_tail = String::new();
        let mut index = 0;
        while index < commands.len() {
            let command = &commands[index];
            if abort.load(Ordering::SeqCst) {
                return None;
            }
            let step = (index + 1) as i64;
            if let Some(skip) = left.skipped.iter().find(|s| s.step == step) {
                log.append(
                    format!(
                        "$ [{step}/{}] {command}\n  skipped: {}\n",
                        commands.len(),
                        skip.reason
                    )
                    .as_bytes(),
                );
                index += 1;
                continue;
            }
            let left_ms = remaining(Instant::now());
            if left_ms == 0 {
                return Some(Outcome {
                    state: "timeout",
                    exit_code: None,
                    failed_step: Some(step),
                    tail: format!(
                        "the run's {}s ended before step {step} started",
                        self.verify.timeout_seconds
                    ),
                    truncated: false,
                });
            }
            log.append(format!("$ [{step}/{}] {command}\n", commands.len()).as_bytes());
            let result = self.run_step(
                command,
                scratch,
                left_ms,
                &environment,
                abort,
                log,
                self.on_spawn(run_id, left),
            );
            // What a step left behind in its group goes with it.
            for pgid in left.groups.lock().unwrap_or_else(|p| p.into_inner()).iter() {
                scratch::kill_group(*pgid);
            }
            if abort.load(Ordering::SeqCst) {
                return None;
            }
            match result.status {
                RunStatus::Ok => {
                    last = Some(result);
                    index += 1;
                }
                RunStatus::Failed | RunStatus::Error
                    if !snapshot.is_empty() && Self::slot_damaged(&snapshot) =>
                {
                    if rerun {
                        return Some(Outcome {
                            state: "error",
                            exit_code: None,
                            failed_step: Some(step),
                            tail: format!(
                                "infrastructure: the gate build directory was removed during the run\n{first_tail}"
                            ),
                            truncated: result.truncated,
                        });
                    }
                    rerun = true;
                    first_tail = result.output_tail;
                    let Some(held) = leases.as_mut() else {
                        return Some(Outcome::error("infrastructure: no lease to renew"));
                    };
                    match held.reacquire(lease_wait, abort) {
                        Ok(()) => {}
                        Err(LeaseError::Aborted) => return None,
                        Err(error) => return Some(Outcome::error(lease_message(slot, &error))),
                    }
                    snapshot = self.snapshot_slot(slot);
                    log.append(
                        b"infrastructure: the gate build directory was removed; rerunning from step 1\n",
                    );
                    self.detail_log(
                        "verify_infrastructure_rerun",
                        json!({"runId": run_id, "slot": slot, "step": step}),
                    );
                    index = 0;
                }
                RunStatus::Failed => {
                    let tail = if rerun {
                        format!(
                            "after an infrastructure rerun from step 1: {}",
                            result.output_tail
                        )
                    } else {
                        result.output_tail
                    };
                    return Some(Outcome {
                        state: "failed",
                        exit_code: result.exit_code.map(i64::from),
                        failed_step: Some(step),
                        tail,
                        truncated: result.truncated,
                    });
                }
                RunStatus::Timeout => {
                    return Some(Outcome {
                        state: "timeout",
                        exit_code: result.exit_code.map(i64::from),
                        failed_step: Some(step),
                        tail: result.output_tail,
                        truncated: result.truncated,
                    })
                }
                RunStatus::Error => {
                    return Some(Outcome {
                        state: "error",
                        exit_code: None,
                        failed_step: Some(step),
                        tail: result.output_tail,
                        truncated: result.truncated,
                    })
                }
            }
        }
        let (tail, truncated) =
            last.map_or((String::new(), false), |r| (r.output_tail, r.truncated));
        Some(Outcome {
            state: "passed",
            exit_code: Some(0),
            failed_step: None,
            tail,
            truncated,
        })
    }

    /// Removes everything a run left: its process groups, the processes that carry its marker, the teardown hook, the
    /// worktree and the directory.
    pub(crate) fn clean_up(
        &self,
        run_id: &str,
        commit: &str,
        scratch: &Path,
        left: &Left,
        environment: Option<&[(String, String)]>,
        scope: GateScope,
    ) {
        for pgid in left.groups.lock().unwrap_or_else(|p| p.into_inner()).iter() {
            scratch::kill_group(*pgid);
        }
        let min_start = *left.first_start.lock().unwrap_or_else(|p| p.into_inner());
        if self.verify.reap {
            scratch::reap_marked(&scratch::marker(run_id), min_start);
        }
        if left.worktree_added {
            self.teardown(run_id, commit, scratch, environment, scope);
            if self.verify.reap {
                scratch::reap_marked(&scratch::marker(run_id), min_start);
            }
        }
        scratch::remove_worktree(&self.project_root, scratch);
    }

    /// `[worktree].teardown` in the project root, as for an agent's worktree; a failure is logged and never stops the removal.
    pub(crate) fn teardown(
        &self,
        run_id: &str,
        commit: &str,
        scratch: &Path,
        environment: Option<&[(String, String)]>,
        scope: GateScope,
    ) {
        if !self.verify.use_worktree_hooks {
            return;
        }
        let Some(worktree) = &self.worktree else {
            return;
        };
        let Some(command) = &worktree.teardown else {
            return;
        };
        let mut env: Vec<(String, String)> = match environment {
            Some(env) => env.to_vec(),
            None => match self.command_environment(run_id, commit, scope) {
                Ok(env) => env,
                Err(_) => return,
            },
        };
        env.push((
            "CAPSTAN_WORKTREE_PATH".into(),
            scratch.to_string_lossy().into_owned(),
        ));
        let timeout = worktree
            .teardown_timeout_seconds
            .unwrap_or(capstan_config::DEFAULT_WORKTREE_TEARDOWN_TIMEOUT_SECONDS)
            .max(1) as u64
            * 1000;
        let result = run_command(RunOptions {
            command: command.clone(),
            cwd: self.project_root.clone(),
            timeout_ms: timeout,
            environment: env,
            output_tail_bytes: 2000,
            on_spawn: None,
            abort: None,
            log_sink: None,
        });
        if result.status != RunStatus::Ok {
            self.detail_log(
                "verify_teardown_failed",
                json!({"runId": run_id, "status": result.status.as_str(), "output": result.output_tail}),
            );
        }
    }

    /// Keeps the newest `keep_logs` log files.
    pub(crate) fn trim_logs(&self) {
        let dir = self.scratch_root.join("logs");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        let mut logs: Vec<(std::time::SystemTime, PathBuf)> = entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "log"))
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .collect();
        logs.sort_by(|a, b| b.cmp(a));
        for (_, path) in logs.into_iter().skip(self.verify.keep_logs.max(1) as usize) {
            let _ = std::fs::remove_file(path);
        }
    }

    fn open_log(&self, run_id: &str) -> Result<CappedLog, String> {
        use std::os::unix::fs::DirBuilderExt;
        let dir = self.scratch_root.join("logs");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .map_err(|e| format!("the verify log directory could not be created: {e}"))?;
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(dir.join(format!("{run_id}.log")))
            .map_err(|e| format!("the verify log could not be created: {e}"))?;
        Ok(CappedLog::new(
            file,
            self.verify.max_log_bytes.max(2) as usize,
        ))
    }

    /// Leases the run's slot and works out the scope its commands run in.
    fn lease_run(
        &self,
        slot: usize,
        abort: &Arc<AtomicBool>,
    ) -> (GateScope, Option<GateLeases>, LeaseOutcome) {
        let Some(runtime) = &self.cache else {
            return (GateScope::Bare, None, LeaseOutcome::Ready);
        };
        if self.leases_unreliable.load(Ordering::SeqCst) {
            return (GateScope::Shared, None, LeaseOutcome::Ready);
        }
        let wait = *self.lease_wait.lock().unwrap_or_else(|p| p.into_inner());
        let fault = *self.lease_fault.lock().unwrap_or_else(|p| p.into_inner());
        let leased = if fault == LeaseFault::LeaseIo {
            Err(LeaseError::Io("injected lease failure".into()))
        } else {
            runtime.lease_gate(slot, wait, abort)
        };
        match leased {
            Ok(leases) if leases.is_empty() => {
                (GateScope::Gate { slot }, None, LeaseOutcome::Ready)
            }
            Ok(leases) => (GateScope::Gate { slot }, Some(leases), LeaseOutcome::Ready),
            Err(LeaseError::Aborted) => (GateScope::Shared, None, LeaseOutcome::Failed(None)),
            Err(LeaseError::Busy) => (
                GateScope::Shared,
                None,
                LeaseOutcome::Failed(Some(Outcome::error(lease_message(slot, &LeaseError::Busy)))),
            ),
            Err(LeaseError::Io(message)) => {
                self.detail_log(
                    "verify_lease_unavailable",
                    json!({"slot": slot, "message": message}),
                );
                (GateScope::Shared, None, LeaseOutcome::Ready)
            }
        }
    }

    /// Runs a claimed run to its end; a panic in the run ends it as an error (with its slot released) instead of leaving
    /// it recorded as running.
    pub(crate) fn execute_guarded(&self, run: &Value, abort: Arc<AtomicBool>, slot: usize) {
        let started = Instant::now();
        // Owned here so what the run started is still known when it panics.
        let mut left = Left::default();
        // The slot lease is held here, outside the unwind guard: it outlives the cleanup on every path and is
        // dropped last, so the teardown hook never runs unleased.
        let (scope, mut leases, lease_outcome) = self.lease_run(slot, &abort);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.execute(
                run,
                abort,
                &mut left,
                &mut leases,
                started,
                slot,
                scope,
                lease_outcome,
            )
        }));
        let Err(payload) = result else { return };
        let message = payload
            .downcast_ref::<&str>()
            .map(|m| m.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".into());
        let run_id = run["runId"].as_str().unwrap_or("").to_string();
        let commit = run["commitSha"].as_str().unwrap_or("").to_string();
        self.detail_log(
            "verify_worker_panicked",
            json!({"runId": run_id, "message": message}),
        );
        let scratch = self.scratch_root.join(&run_id);
        left.worktree_added |= scratch.exists();
        // The cleanup of the run may itself be what panicked; the finish is recorded either way.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.clean_up(&run_id, &commit, &scratch, &left, None, scope);
            // Whatever `reap` says: the worker is gone and nothing is waiting on this run's processes, as after a crash.
            let min_start = *left.first_start.lock().unwrap_or_else(|p| p.into_inner());
            scratch::reap_marked(&scratch::marker(&run_id), min_start);
        }));
        self.unregister(&run_id);
        let input = json!({
            "runId": run_id,
            "state": "error",
            "exitCode": Value::Null,
            "failedStep": Value::Null,
            "durationMs": started.elapsed().as_millis() as i64,
            "outputTail": format!("the verify worker panicked: {message}"),
            "outputTruncated": false,
        });
        self.finish_once(&run_id, input);
    }

    /// Records the end of a run unless it is already recorded (a second finish is a no-op).
    fn finish_once(&self, run_id: &str, input: Value) {
        let id = run_id.to_string();
        let result = self.step(move |core, context| {
            if core.verify_run(&id)?["state"] != "running" {
                return Ok(());
            }
            core.finish_verify_run(context, &input).map(|_| ())
        });
        if let Err(error) = result {
            self.detail_log(
                "verify_finish_failed",
                json!({"runId": run_id, "error": error.to_string()}),
            );
        }
    }

    /// Runs a claimed run to its end.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn execute(
        &self,
        run: &Value,
        abort: Arc<AtomicBool>,
        left: &mut Left,
        leases: &mut Option<GateLeases>,
        started: Instant,
        slot: usize,
        scope: GateScope,
        lease_outcome: LeaseOutcome,
    ) {
        let run_id = run["runId"].as_str().unwrap_or("").to_string();
        let commit = run["commitSha"].as_str().unwrap_or("").to_string();
        let commands: Vec<String> = run["commands"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c.as_str().map(str::to_string))
            .collect();
        if self.panic_next_run.swap(false, Ordering::SeqCst) {
            panic!("verify worker test panic");
        }
        let scratch = self.scratch_root.join(&run_id);
        let outcome = match self.open_log(&run_id) {
            _ if matches!(lease_outcome, LeaseOutcome::Failed(_)) => match lease_outcome {
                LeaseOutcome::Failed(outcome) => outcome,
                LeaseOutcome::Ready => None,
            },
            Err(message) => Some(Outcome::error(message)),
            Ok(log) => {
                let job = Job {
                    run_id: &run_id,
                    commit: &commit,
                    commands: &commands,
                    scratch: &scratch,
                    log: &log,
                    abort: &abort,
                    started,
                    slot,
                    scope,
                };
                let outcome = self.steps(&job, left, leases);
                log.finish();
                outcome
            }
        };
        if self.panic_after_steps.swap(false, Ordering::SeqCst) {
            panic!("verify worker test panic after the steps");
        }
        self.clean_up(&run_id, &commit, &scratch, left, None, scope);
        self.trim_logs();
        let cancelled = self.unregister(&run_id);
        let delay = self.finish_delay_ms.load(Ordering::SeqCst);
        if delay > 0 {
            std::thread::sleep(Duration::from_millis(delay));
        }
        let duration_ms = started.elapsed().as_millis() as i64;
        let outcome = match outcome {
            Some(outcome) => outcome,
            // Aborted: a cancel finishes the run as interrupted; a stop leaves it for `stop` to record.
            None if cancelled => Outcome {
                state: "interrupted",
                exit_code: None,
                failed_step: None,
                tail: "cancelled".into(),
                truncated: false,
            },
            None => return,
        };
        let input = json!({
            "runId": run_id,
            "state": outcome.state,
            "exitCode": outcome.exit_code,
            "failedStep": outcome.failed_step,
            "durationMs": duration_ms,
            "outputTail": outcome.tail,
            "outputTruncated": outcome.truncated,
            "skippedSteps": left.skipped.iter().map(Skipped::to_json).collect::<Vec<_>>(),
        });
        self.finish_once(&run_id, input);
    }
}

fn lease_message(slot: usize, error: &LeaseError) -> String {
    match error {
        LeaseError::Busy => {
            format!("infrastructure: verify slot {slot} is held by another process")
        }
        other => format!("infrastructure: verify slot {slot} lease failed: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn filters() -> Vec<Option<Vec<String>>> {
        vec![Some(strings(&["rust/**", "Cargo.lock"])), None]
    }

    #[test]
    fn a_docs_only_diff_skips_the_filtered_step_and_says_why() {
        let commands = strings(&["cargo test", "guard"]);
        let changed = strings(&["docs/a.md"]);
        let skipped = skipped_steps(&commands, &commands, &filters(), Some(&changed));
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].step, 1);
        assert_eq!(skipped[0].command, "cargo test");
        assert_eq!(
            skipped[0].reason,
            "the diff touches none of rust/**, Cargo.lock"
        );
    }

    #[test]
    fn a_matching_unknown_or_empty_diff_skips_nothing() {
        let commands = strings(&["cargo test", "guard"]);
        let hit = strings(&["rust/x.rs"]);
        assert!(skipped_steps(&commands, &commands, &filters(), Some(&hit)).is_empty());
        assert!(skipped_steps(&commands, &commands, &filters(), None).is_empty());
        assert!(skipped_steps(&commands, &commands, &filters(), Some(&[])).is_empty());
    }

    #[test]
    fn a_run_queued_with_other_commands_skips_nothing() {
        let queued = strings(&["cargo test", "guard"]);
        let now = strings(&["cargo test", "other"]);
        let changed = strings(&["docs/a.md"]);
        assert!(skipped_steps(&queued, &now, &filters(), Some(&changed)).is_empty());
    }
}
