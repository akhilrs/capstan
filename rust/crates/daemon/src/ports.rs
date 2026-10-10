//! The real services behind the daemon's ports: the kernel handle as the launcher's and the Operator's ledger, git for the
//! project's repository, and `wiring`, which builds the Herdr adapter, launcher, Operator service, restart coordinator and
//! loops into `Deps` the way `runDaemon` of src/daemon.ts does.
//!
//! The build runs after the role sync and before the server starts (`Wiring::build`), so every thread it starts (the
//! Operator's timer, the restart result poll) exists before the socket binds and none of them touches the ledger except
//! through `KernelHandle`.

use crate::deps::{
    new_context, CommitInspection, DaemonOptions, Deps, GitError, GitPort, GitResult,
    InspectCommitInput, KernelHandle, NewCommit, NewCommitMessages, NewCommitMessagesInput,
};
use crate::loops::RustLoops;
use crate::run::{Built, RunError, Wiring, EXIT_INVALID};
use capstan_herdr::adapter::{Adapter, AdapterOptions};
use capstan_herdr::api::{HerdrAdapter, LauncherAdapter};
use capstan_herdr::naming::{project_display_name, project_slug};
use capstan_herdr::runner::{ProcessRunner, RunnerOptions};
use capstan_kernel::integrate::GitRepo;
use capstan_kernel::{Core, KernelResult};
use capstan_launcher::api::LauncherService;
use capstan_launcher::kernel::{KernelGone, LedgerPort};
use capstan_launcher::launcher::Launcher;
use capstan_launcher::shared::{FrontEndSite, LauncherOptions};
use capstan_operator::api::{OperatorService, RestartCoordinator};
use capstan_operator::restart::{
    busy_snapshot, dist_hash, recover_restart_results, schedule_known_good_snapshot,
    skip_restarts_with_live_plan, KnownGoodOptions, ProcessRestartCoordinator,
    RestartCoordinatorOptions, RestartRecoveryDeps,
};
use capstan_operator::service::{
    LedgerStep, OperatorLedger, OperatorServiceOptions, ProcessOperatorService,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

// ------------------------------------------------------------------------------------------------ the ledger ports

impl LedgerPort for KernelHandle {
    fn run_job(&self, job: Box<dyn FnOnce(&Core) + Send>) -> Result<(), KernelGone> {
        self.call(job).map_err(|_| KernelGone)
    }
}

impl OperatorLedger for KernelHandle {
    fn run(&self, step: LedgerStep) -> KernelResult<Value> {
        KernelHandle::run(self, step)
    }
}

// ------------------------------------------------------------------------------------------------ git

/// `GitPort` over the project's repository: `GitRepo` for what the kernel's pipeline has, and the two checks of src/git.ts
/// that report uses (`inspectCommit`, `newCommitMessages`), run with a clean environment as Node runs them. Merging and
/// coverage are the kernel's own (`capstan_kernel::integrate`) and are not served through the port.
pub struct ProjectGit {
    root: PathBuf,
    repo: GitRepo,
}

impl ProjectGit {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            repo: GitRepo::new(root),
        }
    }

    /// `runGit`: the exit code and standard output.
    fn run(&self, args: &[&str]) -> GitResult<(i32, String)> {
        let output = std::process::Command::new("git")
            .arg("--no-replace-objects")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .env_clear()
            .env(
                "PATH",
                std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()),
            )
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| failure(&format!("git could not be run ({e})")))?;
        let code = output
            .status
            .code()
            .ok_or_else(|| failure("git could not be run"))?;
        Ok((code, String::from_utf8_lossy(&output.stdout).into_owned()))
    }

    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> GitResult<bool> {
        match self
            .run(&["merge-base", "--is-ancestor", ancestor, descendant])?
            .0
        {
            0 => Ok(true),
            1 => Ok(false),
            _ => Err(failure("git could not compare the commits")),
        }
    }
}

fn failure(message: &str) -> GitError {
    GitError {
        message: message.to_string(),
    }
}

fn from_integrate(error: capstan_kernel::integrate::IntegrateError) -> GitError {
    GitError {
        message: error.message(),
    }
}

fn is_full_sha(text: &str) -> bool {
    text.len() == 40
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl GitPort for ProjectGit {
    fn commit_exists(&self, sha: &str) -> GitResult<bool> {
        self.repo.commit_exists(sha).map_err(from_integrate)
    }

    fn inspect_commit(&self, input: &InspectCommitInput) -> GitResult<CommitInspection> {
        if !is_full_sha(&input.sha) {
            return Err(failure("the commit id must be 40 lowercase hex characters"));
        }
        if input
            .base_sha
            .as_deref()
            .is_some_and(|base| !is_full_sha(base))
        {
            return Err(failure("the base commit id is not a full id"));
        }
        let (code, format) = self.run(&["rev-parse", "--show-object-format"])?;
        if code != 0 {
            return Err(failure(
                "git could not report the repository's object format (git 2.29 or newer is needed)",
            ));
        }
        if format.trim() != "sha1" {
            return Err(failure("only sha1 repositories are supported"));
        }
        let reference = format!("refs/heads/{}", input.branch);
        if self.run(&["check-ref-format", &reference])?.0 != 0 {
            return Err(failure("the recorded branch name is not a valid ref"));
        }
        let (code, tip_text) = self.run(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{reference}^{{commit}}"),
        ])?;
        if code != 0 && code != 1 {
            return Err(failure("git could not read the branch"));
        }
        let tip = (code == 0).then(|| tip_text.trim().to_string());
        if tip.as_deref().is_some_and(|tip| !is_full_sha(tip)) {
            return Err(failure("git gave an unexpected branch tip"));
        }
        let (code, _) = self.run(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{}^{{commit}}", input.sha),
        ])?;
        if code != 0 && code != 1 {
            return Err(failure("git could not look the commit up"));
        }
        let commit_exists = code == 0;
        let mut committed_at = None;
        if commit_exists {
            let (code, stamp) = self.run(&[
                "show",
                "-s",
                "--format=%ct",
                &format!("{}^{{commit}}", input.sha),
            ])?;
            match (code, stamp.trim().parse::<i64>()) {
                (0, Ok(seconds)) => {
                    committed_at = Some(capstan_ledger::iso_from_millis(seconds * 1000));
                }
                _ => return Err(failure("git could not read the commit time")),
            }
        }
        let is_ancestor_of_tip = match (&tip, commit_exists) {
            (Some(tip), true) => self.is_ancestor(&input.sha, tip)?,
            _ => false,
        };
        let is_ancestor_of_base = match (&input.base_sha, commit_exists) {
            (Some(base), true) => self.is_ancestor(&input.sha, base)?,
            _ => false,
        };
        Ok(CommitInspection {
            commit_exists,
            branch_tip: tip,
            is_ancestor_of_tip,
            is_ancestor_of_base,
            committed_at,
        })
    }

    fn new_commit_messages(&self, input: &NewCommitMessagesInput) -> GitResult<NewCommitMessages> {
        if !is_full_sha(&input.sha) || !is_full_sha(&input.base_sha) {
            return Err(failure("the commit ids must be full ids"));
        }
        let reference = format!("refs/heads/{}", input.own_branch);
        if self.run(&["check-ref-format", &reference])?.0 != 0 {
            return Err(failure("the recorded branch name is not a valid ref"));
        }
        let mut excluded = String::new();
        for c in input.own_branch.chars() {
            if matches!(c, '*' | '?' | '[' | '\\') {
                excluded.push('\\');
            }
            excluded.push(c);
        }
        let (code, listed) = self.run(&[
            "rev-list",
            "--reverse",
            &format!("--max-count={}", input.limit + 1),
            &input.sha,
            &format!("^{}", input.base_sha),
            "^HEAD",
            "--not",
            &format!("--exclude={excluded}"),
            "--branches",
        ])?;
        if code != 0 {
            return Err(failure("git could not list the new commits"));
        }
        let shas: Vec<&str> = listed.split('\n').filter(|line| !line.is_empty()).collect();
        if shas.len() > input.limit {
            return Ok(NewCommitMessages::TooMany);
        }
        let mut out = Vec::new();
        for sha in shas {
            if !is_full_sha(sha) {
                return Err(failure("git gave an unexpected commit id"));
            }
            let (code, shown) = self.run(&["show", "-s", "--format=%P%x00%B", sha])?;
            if code != 0 {
                return Err(failure("git could not read a commit message"));
            }
            let Some(cut) = shown.find('\0') else {
                return Err(failure("git gave an unexpected commit"));
            };
            out.push(NewCommit {
                sha: sha.to_string(),
                parents: shown[..cut].split(' ').filter(|p| !p.is_empty()).count(),
                message: shown[cut + 1..].to_string(),
            });
        }
        Ok(NewCommitMessages::Commits(out))
    }

    fn head_commit(&self) -> GitResult<String> {
        self.repo.head_commit().map_err(from_integrate)
    }

    fn commit_subject(&self, sha: &str) -> GitResult<Option<String>> {
        self.repo.commit_subject(sha).map_err(from_integrate)
    }

    fn merge(&self, _input: &Value) -> GitResult<Value> {
        Err(failure("merges run through capstan_kernel::integrate"))
    }

    fn branch_tip(&self, branch: &str) -> GitResult<Option<String>> {
        self.repo.branch_tip(branch).map_err(from_integrate)
    }

    fn is_in_head(&self, sha: &str) -> GitResult<bool> {
        self.repo.is_in_head(sha).map_err(from_integrate)
    }

    fn delete_branch(&self, branch: &str, sha: &str) -> GitResult<bool> {
        self.repo
            .delete_branch_at(branch, sha)
            .map_err(from_integrate)
    }

    fn covered_reports(&self, _head: &str, _reports: &Value, _options: &Value) -> GitResult<Value> {
        Err(failure("coverage runs through capstan_kernel::integrate"))
    }
}

// ------------------------------------------------------------------------------------------------ the environment

/// `CAPSTAN_LAUNCH=off` (also `0`, `false`, `no`) keeps the daemon away from Herdr, as `launchDisabled` of src/cli.ts does.
pub fn launch_disabled() -> bool {
    launch_disabled_in(std::env::var("CAPSTAN_LAUNCH").ok().as_deref())
}

pub fn launch_disabled_in(value: Option<&str>) -> bool {
    matches!(
        value.map(|v| v.trim().to_lowercase()).as_deref(),
        Some("off" | "0" | "false" | "no")
    )
}

/// The `cstan` executable agents run: this binary when it is `cstan`, beside it a `cstan` when it runs as `cstan-daemon`.
/// A `cstan-daemon` with no `cstan` beside it has nothing to point agents at, which is an error.
fn daemon_cstan(exe: &Path) -> Result<String, String> {
    if !exe.file_name().is_some_and(|name| name == "cstan-daemon") {
        return Ok(exe.to_string_lossy().into_owned());
    }
    let sibling = exe.with_file_name("cstan");
    if sibling.is_file() {
        Ok(sibling.to_string_lossy().into_owned())
    } else {
        Err(format!(
            "cstan-daemon has no cstan beside it ({}): agents run cstan, so install cstan next to cstan-daemon or start the daemon with `cstan daemon`",
            sibling.display()
        ))
    }
}

/// Where agents' `cstan` runs: the `cstan` executable of `daemon_cstan`, for the agent wrapper and the watch pane's command.
pub fn cli_site(
    env: &HashMap<String, String>,
    exe: &Path,
) -> Result<(String, String, FrontEndSite), String> {
    let cstan = daemon_cstan(exe)?;
    Ok((
        cstan.clone(),
        cstan.clone(),
        FrontEndSite {
            env: env.clone(),
            exec_path: cstan.clone(),
            sea: true,
            cstan,
        },
    ))
}

// ------------------------------------------------------------------------------------------------ the wiring

const RESULT_POLL_MS: u64 = 2000;

fn invalid(message: impl Into<String>) -> RunError {
    RunError::new(EXIT_INVALID, message)
}

/// What `build` hands the restart result poll and the known-good snapshot: both end in `stop_restart_watch`.
struct RestartWatch {
    stop: Arc<AtomicBool>,
    poll: Option<std::thread::JoinHandle<()>>,
    snapshot: Option<capstan_operator::restart::KnownGoodSchedule>,
}

impl RestartWatch {
    fn end(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(poll) = self.poll.take() {
            let _ = poll.join();
        }
        if let Some(snapshot) = self.snapshot.take() {
            snapshot.stop();
        }
    }
}

/// `notifyPm`: a controller message to the active PM, or a log line when there is none.
fn notify_pm(deps: &Deps, body: &str, action_needed: bool) {
    let (credential, body) = (deps.credential().to_string(), body.to_string());
    let outcome = deps.kernel.run(move |core| {
        let agents = core.active_agents()?;
        let pm = agents
            .as_array()
            .and_then(|list| {
                list.iter()
                    .find(|agent| agent.get("kind").and_then(Value::as_str) == Some("PM"))
            })
            .and_then(|agent| agent.get("agentId").and_then(Value::as_str))
            .map(str::to_string);
        let Some(pm) = pm else {
            return Ok(false);
        };
        let context = new_context(core, &credential)?;
        core.enqueue_message(
            &context,
            &json!({"recipientAgentId": pm, "body": body, "actionNeeded": action_needed}),
        )?;
        Ok(true)
    });
    match outcome {
        Ok(true) => {}
        Ok(false) => deps.detail_log(
            "restart_pm_notice_skipped",
            json!({"reason": "no active PM"}),
        ),
        Err(error) => deps.detail_log(
            "restart_pm_notice_failed",
            json!({"error": error.to_string()}),
        ),
    }
}

/// `ingestResults`: the result of a restart that ended is told to the PM and finishes its run.
fn ingest_restart_results(deps: &Deps, state_dir: &Path) {
    let credential = deps.credential().to_string();
    let log_deps = deps.clone();
    let list_deps = deps.clone();
    let finish_deps = deps.clone();
    let notify_deps = deps.clone();
    recover_restart_results(&RestartRecoveryDeps {
        state_dir: state_dir.to_path_buf(),
        running_restarts: Box::new(move || {
            let outcome = list_deps
                .kernel
                .run(|core| core.list_operator_proposals(Some(&json!({"states": ["running"]}))));
            match outcome {
                Ok(Value::Array(list)) => list
                    .into_iter()
                    .filter(|p| p.get("kind").and_then(Value::as_str) == Some("restart"))
                    .collect(),
                _ => Vec::new(),
            }
        }),
        finish_run: Box::new(move |proposal_id, report, duration_ms| {
            let (credential, proposal_id) = (credential.clone(), proposal_id.to_string());
            let input = json!({
                "proposalId": proposal_id,
                "status": report.status.as_str(),
                "exitCode": report.exit_code,
                "durationMs": duration_ms,
                "outputTail": report.output_tail,
                "truncated": false,
            });
            finish_deps
                .kernel
                .run(move |core| {
                    let context = new_context(core, &credential)?;
                    core.finish_operator_run(&context, &input)
                })
                .map(|_| ())
                .map_err(|e| e.to_string())
        }),
        notify_pm: Box::new(move |body, action| notify_pm(&notify_deps, body, action)),
        now_ms: None,
        log: Some(Box::new(move |event, details| {
            log_deps.detail_log(event, details)
        })),
    });
}

/// The arguments that start this daemon again: `daemon` for `cstan`, none for the thin `cstan-daemon`.
fn daemon_argv(executable: &Path) -> Vec<String> {
    if executable
        .file_name()
        .is_some_and(|name| name == "cstan-daemon")
    {
        Vec::new()
    } else {
        vec!["daemon".to_string()]
    }
}

/// The pieces `wiring` needs from the process: the project's configuration and where the daemon runs.
pub struct WiringInput {
    pub cli: (String, String, FrontEndSite),
    pub executable: PathBuf,
    pub launch: bool,
}

/// Builds the wiring of `runDaemon`: git and the loops always, and with a configuration and Herdr (not `CAPSTAN_LAUNCH=off`)
/// the adapter, launcher and, when `[operator]` is enabled, the Operator service with its restart coordinator.
pub fn wiring(options: &DaemonOptions, input: WiringInput) -> Result<Wiring, RunError> {
    let adapter: Option<Arc<Adapter>> = match (&options.capstan, input.launch) {
        (Some(config), true) => {
            let runner = ProcessRunner::new(RunnerOptions {
                session: config.herdr_session.clone(),
                binary: None,
                timeout_ms: None,
                env: None,
            })
            .map_err(invalid)?;
            let root = options.workspace_root.to_string_lossy().into_owned();
            let mut adapter_options = AdapterOptions::new(Arc::new(runner));
            adapter_options.project_slug = Some(project_slug(&project_display_name(
                config.project_name.as_deref(),
                &root,
            )));
            Some(Arc::new(Adapter::new(adapter_options)))
        }
        _ => None,
    };
    let state_dir = options.state_directory.clone();
    let workspace = options.workspace_root.clone();
    let build: crate::run::BuildFn = Box::new(move |deps: &mut Deps| {
        deps.git = Arc::new(ProjectGit::new(&workspace));
        let loops = RustLoops::new();
        deps.driver = loops.driver_view();
        deps.loops = loops;
        let mut built = Built::default();
        let (Some(config), Some(adapter)) = (deps.options.capstan.clone(), adapter) else {
            return built;
        };
        deps.adapter = Some(Arc::clone(&adapter) as Arc<dyn HerdrAdapter>);
        let sync_deps = deps.clone();
        let (node, cli_path, site) = input.cli;
        let launcher = Arc::new(Launcher::new(LauncherOptions {
            ledger: capstan_launcher::kernel::Ledger::new(
                Arc::new(deps.kernel.clone()),
                deps.credential(),
            ),
            adapter: Arc::clone(&adapter) as Arc<dyn LauncherAdapter>,
            config: (*config).clone(),
            project_root: workspace.to_string_lossy().into_owned(),
            cli_path,
            socket_path: deps.options.socket_path().to_string_lossy().into_owned(),
            credential: deps.credential().to_string(),
            node_path: Some(node),
            base_environment: std::env::vars().collect(),
            git: None,
            now: None,
            log: Some({
                let deps = deps.clone();
                Arc::new(move |event: &str, details: Value| deps.detail_log(event, details))
            }),
            sync_roles: Some(Arc::new(move || {
                crate::run::sync_configured_roles(&sync_deps)
            })),
            run_setup: None,
            sleep: None,
            run_teardown: None,
            site: Some(site),
        }));
        deps.launcher = Some(Arc::clone(&launcher) as Arc<dyn LauncherService>);
        let operator = &config.operator;
        if !operator.enabled {
            return built;
        }
        let executable = input.executable;
        let coordinator = Arc::new(ProcessRestartCoordinator::new(RestartCoordinatorOptions {
            state_dir: state_dir.clone(),
            project_root: workspace.clone(),
            binary_path: executable.clone(),
            node: executable.to_string_lossy().into_owned(),
            argv: daemon_argv(&executable),
            socket_path: deps.options.socket_path(),
            pid_path: deps.options.pid_path(),
            log_path: state_dir
                .parent()
                .map(|dir| dir.join("daemon.log"))
                .unwrap_or_else(|| state_dir.join("daemon.log")),
            credential_file: state_dir
                .parent()
                .map(|dir| dir.join("operator.key"))
                .unwrap_or_else(|| state_dir.join("operator.key")),
            health_timeout_seconds: operator.restart_health_timeout_seconds.max(0) as u64,
            idle_wait_seconds: operator.restart_idle_wait_seconds.max(0) as u64,
            busy: {
                let (deps, launcher) = (deps.clone(), Arc::clone(&launcher));
                Box::new(move || {
                    let indicators = deps
                        .kernel
                        .run(|core| core.busy_indicators())
                        .unwrap_or(Value::Null);
                    busy_snapshot(&indicators, launcher.in_flight_operations() as i64)
                })
            },
            notify_pm: {
                let deps = deps.clone();
                Box::new(move |body, action| notify_pm(&deps, body, action))
            },
            request_stop: {
                let shutdown = deps.shutdown.clone();
                Box::new(move || shutdown.request())
            },
            pid: None,
            timing: None,
            idle_poll_ms: None,
            now_ms: None,
            sleep: None,
            log: Some({
                let deps = deps.clone();
                Box::new(move |event, details| deps.detail_log(event, details))
            }),
        }));
        let service = Arc::new(ProcessOperatorService::new(OperatorServiceOptions {
            ledger: Arc::new(deps.kernel.clone()),
            config: operator.clone(),
            controller_credential: deps.credential().to_string(),
            project_root: workspace.clone(),
            environment: {
                let launcher = Arc::clone(&launcher);
                Arc::new(move || launcher.operator_environment().into_iter().collect())
            },
            run_command: None,
            restart: Some(Arc::clone(&coordinator) as Arc<dyn RestartCoordinator>),
            notify_pm: Some({
                let deps = deps.clone();
                Arc::new(move |body: &str, action: bool| notify_pm(&deps, body, action))
            }),
            processes: None,
            monotonic_now: None,
            orphan_kill_wait_ms: None,
            log: Some({
                let deps = deps.clone();
                Arc::new(move |event: &str, details: Value| deps.detail_log(event, details))
            }),
        }));
        deps.restart = Some(coordinator as Arc<dyn RestartCoordinator>);
        deps.operator = Some(service as Arc<dyn OperatorService>);

        built.before_operator_recover = Some({
            let (deps, state_dir) = (deps.clone(), state_dir.clone());
            Box::new(move || ingest_restart_results(&deps, &state_dir))
        });
        built.skip_restarts_with_plan = Some(Box::new(skip_restarts_with_live_plan(&state_dir)));
        let stop = Arc::new(AtomicBool::new(false));
        let poll = {
            let (deps, state_dir, stop) = (deps.clone(), state_dir.clone(), Arc::clone(&stop));
            std::thread::Builder::new()
                .name("restart-results".into())
                .spawn(move || {
                    while !deps
                        .shutdown
                        .wait_timeout(Duration::from_millis(RESULT_POLL_MS))
                    {
                        if stop.load(Ordering::SeqCst) {
                            return;
                        }
                        ingest_restart_results(&deps, &state_dir);
                    }
                })
                .ok()
        };
        let snapshot = match dist_hash(&executable) {
            Ok(loaded_hash) => {
                let deps = deps.clone();
                Some(schedule_known_good_snapshot(KnownGoodOptions {
                    state_dir: state_dir.clone(),
                    binary_path: executable.clone(),
                    loaded_hash,
                    max_migration: capstan_ledger::max_embedded_migration(),
                    settle_ms: None,
                    log: Some(Box::new(move |event, details| {
                        deps.detail_log(event, details)
                    })),
                }))
            }
            Err(error) => {
                deps.detail_log(
                    "known_good_not_scheduled",
                    json!({"error": error.to_string()}),
                );
                None
            }
        };
        let watch = RestartWatch {
            stop,
            poll,
            snapshot,
        };
        built.stop_restart_watch = Some(Box::new(move || watch.end()));
        built
    });
    Ok(Wiring {
        sync_roles: None,
        build: Some(build),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_is_off_for_the_values_node_turns_it_off_for() {
        for value in ["off", "OFF", " 0 ", "false", "No"] {
            assert!(launch_disabled_in(Some(value)), "{value}");
        }
        for value in ["", "on", "1", "yes", "maybe"] {
            assert!(!launch_disabled_in(Some(value)), "{value}");
        }
        assert!(!launch_disabled_in(None));
    }

    #[test]
    fn the_agents_cstan_is_this_binary_or_the_one_beside_cstan_daemon() {
        let env = HashMap::new();
        let (_, cli, site) = cli_site(&env, Path::new("/opt/bin/cstan")).unwrap();
        assert_eq!(cli, "/opt/bin/cstan");
        assert_eq!(site.cstan, "/opt/bin/cstan");
        let dir = tempfile::tempdir().unwrap();
        let daemon = dir.path().join("cstan-daemon");
        // No cstan beside cstan-daemon: nothing to point the wrapper at.
        let error = cli_site(&env, &daemon).unwrap_err();
        assert!(error.contains("no cstan beside it"), "{error}");
        std::fs::write(dir.path().join("cstan"), "").unwrap();
        let (_, cli, site) = cli_site(&env, &daemon).unwrap();
        assert_eq!(site.cstan, dir.path().join("cstan").to_string_lossy());
        assert_eq!(cli, site.cstan);
    }
}
