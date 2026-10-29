//! The verify service and the verify route through `handlers::handle_frame` on a scratch git project, a real ledger and
//! portable shell commands as the gate (no cargo): pass, fail, timeout, interruption, recovery, concurrency, cleanup, the
//! bounded log, the reviewer brief, the gates of request-review and integrate, and the automatic runs.

mod common;

use capstan_config::Verify;
use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::shared::AbortSignal;
use capstan_daemon::handlers::MAX_RESPONSE_BYTES;
use capstan_daemon::verify::{scratch, VerifyService};
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::types::InitialProject;
use capstan_kernel::Core;
use capstan_launcher::api::{LauncherService, Placement, SpawnResult, StubLauncher};
use common::project::{build_repo, write_project, Identity};
use common::replay::ReplayGit;
use common::{private_tempdir, seeded_env};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

const CONFIG: &str = r#"schema_version = 1

[hosts.claude]
kind = "claude"

[architect]
enabled = true

[roles.pm]
kind = "PM"
host = "claude"

[roles.developer]
kind = "Developer"
host = "claude"

[roles.architect]
kind = "Developer"
host = "claude"

[roles.reviewer]
kind = "Verifier"
host = "claude"
"#;

/// Run ids come from the kernel's seeded randomness, so every world needs its own seed: the processes of a run are found
/// by its id.
static WORLDS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

const AGENTS: [(&str, &str, &str); 5] = [
    ("pm-1", "pm", "PM"),
    ("arch-1", "architect", "Developer"),
    ("dev-1", "developer", "Developer"),
    ("dev-2", "developer", "Developer"),
    ("rev-1", "reviewer", "Verifier"),
];

struct World {
    dir: common::PrivateDir,
    scratch: common::PrivateDir,
    deps: Deps,
    service: Option<Arc<VerifyService>>,
    credentials: HashMap<String, String>,
    heads: BTreeMap<String, String>,
    launcher: Arc<StubLauncher>,
}

type Prefill = Box<dyn Fn(&Path)>;
type Tweak = Box<dyn Fn(&mut Verify)>;

/// What a test changes in the `[verify]` section.
#[derive(Default)]
struct Spec {
    /// The `[verify]` table text after `commands` and `scratch_dir`; `None` leaves the section out.
    verify: Option<String>,
    commands: Vec<String>,
    tweak: Option<Tweak>,
    /// Runs on the scratch directory before the service starts.
    prefill: Option<Prefill>,
}

fn spec(commands: &[&str]) -> Spec {
    Spec {
        verify: Some(String::new()),
        commands: commands.iter().map(|c| c.to_string()).collect(),
        tweak: None,
        prefill: None,
    }
}

impl Spec {
    fn extra(mut self, text: &str) -> Self {
        self.verify.as_mut().unwrap().push_str(text);
        self
    }
    fn prefill(mut self, f: impl Fn(&Path) + 'static) -> Self {
        self.prefill = Some(Box::new(f));
        self
    }
    fn tweak(mut self, f: impl Fn(&mut Verify) + 'static) -> Self {
        self.tweak = Some(Box::new(f));
        self
    }
}

impl World {
    fn new(spec: Spec) -> World {
        World::new_with_toml(spec, "")
    }

    /// `more` is appended to capstan.toml (other sections, such as `[worktree]`).
    fn new_with_toml(spec: Spec, more: &str) -> World {
        let dir = private_tempdir();
        let scratch_dir = private_tempdir();
        let identity = Identity {
            project_id: "pverify".into(),
            name: "Verify".into(),
            credential: format!("operator-{}", "v".repeat(40)),
        };
        let state = write_project(dir.path(), &identity);
        let mut config = CONFIG.to_string();
        if let Some(extra) = &spec.verify {
            let commands: Vec<String> = spec.commands.iter().map(|c| format!("{c:?}")).collect();
            config.push_str(&format!(
                "\n[verify]\ncommands = [{}]\nscratch_dir = \"{}\"\n{extra}\n",
                commands.join(", "),
                scratch_dir.path().display()
            ));
        }
        config.push_str(more);
        let file = dir.path().join("capstan.toml");
        std::fs::write(&file, config).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let heads = build_repo(
            dir.path(),
            &json!({
                "files": {"a.txt": "a\n"},
                "branches": {
                    "feat/dev-1": {"files": {"one.txt": "one\n"}, "message": "feat: one"},
                    "feat/dev-2": {"files": {"a.txt": "two\n"}, "message": "feat: two"},
                },
            }),
        );
        let database = capstan_ledger::resolve_database_path(&state).unwrap();
        capstan_ledger::open_database(&database, &capstan_ledger::OpenOptions::default()).unwrap();
        let project = identity.initial_project();
        let kernel = {
            let (state, project, root): (_, InitialProject, _) =
                (state.clone(), project.clone(), dir.path().to_path_buf());
            KernelHandle::spawn(move || {
                Core::open(
                    &state,
                    &project,
                    &KernelOptions {
                        workspace_root: Some(root),
                        ..KernelOptions::default()
                    },
                    seeded_env(&format!(
                        "verify-{}-{}",
                        std::process::id(),
                        WORLDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    )),
                )
            })
            .expect("the kernel opens")
        };
        let sink: Logger = Arc::new(|_: &LogEntry| {});
        let mut options = DaemonOptions::new(state, project, dir.path().to_path_buf());
        let role_config = capstan_config::load_role_config(dir.path()).unwrap();
        let mut verify = role_config.verify.clone();
        options.capstan = Some(Arc::new(role_config));
        let launcher = Arc::new(StubLauncher::new());
        let mut deps = Deps::new(kernel, options, sink);
        deps.git = Arc::new(ReplayGit::new(dir.path()));
        deps.launcher = Some(launcher.clone() as Arc<dyn LauncherService>);
        capstan_daemon::deps::startup_sequence(&deps);
        let mut service = None;
        if let Some(verify) = verify.as_mut() {
            if let Some(tweak) = &spec.tweak {
                tweak(verify);
            }
            let made = VerifyService::with_config(&deps, verify.clone());
            deps.verify = Some(made.clone());
            service = Some(made);
        }
        if let Some(prefill) = &spec.prefill {
            prefill(scratch_dir.path());
        }
        let mut world = World {
            dir,
            scratch: scratch_dir,
            deps,
            service,
            credentials: HashMap::new(),
            heads,
            launcher,
        };
        world
            .credentials
            .insert("operator".into(), identity.credential);
        world.populate();
        if let Some(service) = &world.service {
            service.start();
        }
        world
    }

    fn ledger(&self, credential: &str, op: &str, args: Vec<Value>) -> Value {
        let credential = credential.to_string();
        let name = op.to_string();
        self.deps
            .kernel
            .run(move |core| {
                let context = capstan_daemon::deps::new_context(core, &credential)?;
                let mut all = vec![serde_json::to_value(context).unwrap()];
                all.extend(args);
                core.dispatch(&name, &all)
            })
            .unwrap_or_else(|e| panic!("{op}: {e}"))
    }

    /// A read of the ledger by operation name (no context).
    fn read(&self, op: &str, args: Vec<Value>) -> Value {
        let name = op.to_string();
        self.deps
            .kernel
            .run(move |core| core.dispatch(&name, &args))
            .unwrap_or_else(|e| panic!("{op}: {e}"))
    }

    fn populate(&mut self) {
        let owner = self.credentials["operator"].clone();
        let roles: Vec<Value> = [
            ("pm", "PM"),
            ("developer", "Developer"),
            ("architect", "Developer"),
            ("reviewer", "Verifier"),
        ]
        .iter()
        .map(|(name, kind)| {
            json!({"name": name, "kind": kind, "host": "claude", "configHash": "a".repeat(64)})
        })
        .collect();
        self.ledger(&owner, "syncRoleDefinitions", vec![json!(roles)]);
        for (agent, role, kind) in AGENTS {
            let seat = format!("seat-{agent}");
            self.ledger(
                &owner,
                "createSeat",
                vec![json!({"seatId": seat, "name": seat, "role": kind})],
            );
            let actor = self.ledger(
                &owner,
                "createActor",
                vec![json!({"displayName": agent, "role": kind, "seatId": seat})],
            );
            self.ledger(
                &owner,
                "registerAgent",
                vec![json!({"agentId": agent, "roleName": role, "seatId": seat, "actorId": actor["actorId"]})],
            );
            self.credentials
                .insert(agent.into(), actor["credential"].as_str().unwrap().into());
        }
        let internal = self.deps.credential().to_string();
        for (agent, branch) in [("dev-1", "feat/dev-1"), ("dev-2", "feat/dev-2")] {
            self.ledger(
                &internal,
                "recordAgentPane",
                vec![json!({
                    "agentId": agent,
                    "workspaceId": "ws-1",
                    "paneId": format!("pane-{agent}"),
                    "worktreePath": format!("/tmp/wt/{agent}"),
                    "branch": branch,
                    "baseSha": self.heads["main"],
                })],
            );
        }
    }

    fn call(&self, who: &str, command: &str, args: &[&str]) -> Value {
        let credential = self
            .credentials
            .get(who)
            .cloned()
            .unwrap_or_else(|| who.into());
        let frame = json!({"v": 1, "credential": credential, "command": command, "args": args});
        let outcome = handle_frame(
            &self.deps,
            frame.to_string().as_bytes(),
            &AbortSignal::new(),
            MAX_RESPONSE_BYTES,
        );
        serde_json::from_str(&outcome.response.expect("an answer")).expect("JSON")
    }

    fn ok(&self, who: &str, command: &str, args: &[&str]) -> Value {
        let answer = self.call(who, command, args);
        assert_eq!(answer["ok"], true, "{command} {args:?}: {answer}");
        answer["result"].clone()
    }

    fn refused(&self, who: &str, command: &str, args: &[&str], code: &str) -> String {
        let answer = self.call(who, command, args);
        assert_eq!(answer["ok"], false, "{command} {args:?}: {answer}");
        assert_eq!(answer["code"], code, "{command} {args:?}: {answer}");
        answer["message"].as_str().unwrap().to_string()
    }

    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(self.dir.path())
            .args(args)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?}");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// A report of dev-1 (branch feat/dev-1) or dev-2; returns (report id, commit).
    fn report(&self, agent: &str) -> (String, String) {
        let branch = format!("feat/{agent}");
        let sha = self.heads[&branch].clone();
        let report = self.ok(agent, "report", &[&sha, "Done"]);
        (report["reportId"].as_str().unwrap().to_string(), sha)
    }

    fn run(&self, id: &str) -> Value {
        self.ok("operator", "verify", &["show", id])
    }

    /// Waits for the run to end; returns its record.
    fn finished(&self, id: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            let run = self.run(id);
            let state = run["state"].as_str().unwrap();
            if state != "queued" && state != "running" {
                return run;
            }
            assert!(Instant::now() < deadline, "run {id} did not end: {run}");
            std::thread::sleep(Duration::from_millis(40));
        }
    }

    fn wait_for(&self, what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn scratch_path(&self, id: &str) -> PathBuf {
        self.scratch.path().join(id)
    }

    /// No scratch worktree, directory or marked process is left for the run.
    fn assert_clean(&self, id: &str) {
        let listed = self.git(&["worktree", "list", "--porcelain"]);
        assert_eq!(
            listed
                .lines()
                .filter(|l| l.starts_with("worktree "))
                .count(),
            1,
            "{listed}"
        );
        assert!(!self.scratch_path(id).exists(), "scratch of {id} is left");
        assert!(
            scratch::marked_pids(&scratch::marker(id), None).is_empty(),
            "a process of {id} is left"
        );
    }

    fn stop(&self) {
        if let Some(service) = &self.service {
            service.stop();
        }
    }
}

impl Drop for World {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Whether the process exists and is not a zombie.
fn alive(pid: i64) -> bool {
    // SAFETY: signal 0 only checks that the pid exists.
    let exists = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0
        || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
    exists && !capstan_process::is_zombie(pid)
}

/// Not a test: the process a gate command leaves running in another session. It is this test binary rather than
/// `sleep`, because macOS hides the environment (and so the run's marker) of Apple's own binaries.
#[test]
#[ignore = "a helper process, started by the gate commands of the session tests"]
fn helper_sleeps_in_another_session() {
    std::thread::sleep(Duration::from_secs(60));
}

/// A gate command that starts a process in a new session, as a tool daemon (an sccache server) does, writes its pid to
/// `pid_path` and leaves it running. `setsid` is not installed on macOS; perl's POSIX::setsid does the same there.
fn new_session_command(pid_path: &Path) -> String {
    let quote = |text: &str| format!("'{}'", text.replace('\'', r"'\''"));
    let exe = std::env::current_exe().unwrap();
    let inner = format!(
        "echo $$ > {}; exec {} --ignored --exact helper_sleeps_in_another_session",
        quote(&pid_path.to_string_lossy()),
        quote(&exe.to_string_lossy())
    );
    format!(
        "(if command -v setsid >/dev/null; then setsid sh -c {inner_q}; else perl -MPOSIX -e 'POSIX::setsid(); exec @ARGV' sh -c {inner_q}; fi &) ; sleep 1; test -s {pid}",
        inner_q = quote(&inner),
        pid = quote(&pid_path.to_string_lossy()),
    )
}

fn read_pid(path: &Path) -> Option<i64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

// ------------------------------------------------------------------------------------------------ outcomes

#[test]
fn a_passing_gate_records_passed_with_its_exit_code_duration_and_tail() {
    let world = World::new(spec(&["echo first", "echo second"]));
    let (report, commit) = world.report("dev-1");
    let queued = world.ok("dev-1", "verify", &[&report]);
    assert_eq!(queued["state"], "queued");
    assert_eq!(queued["commitSha"], commit);
    let id = queued["runId"].as_str().unwrap();
    // The same request while the run is active answers that run.
    let again = world.ok("dev-1", "verify", &[&report]);
    if again["state"] == "queued" || again["state"] == "running" {
        assert_eq!(again["runId"], id);
        assert_eq!(again["existing"], true);
    }
    let run = world.finished(id);
    assert_eq!(run["state"], "passed", "{run}");
    assert_eq!(run["exitCode"], 0);
    assert_eq!(run["failedStep"], Value::Null);
    assert!(run["durationMs"].as_i64().unwrap() >= 0);
    assert!(run["outputTail"].as_str().unwrap().contains("second"));
    world.assert_clean(id);
    // Only the log of the run is kept.
    assert!(Path::new(run["logPath"].as_str().unwrap()).is_file());
    let status = world.ok("operator", "status", &[]);
    assert_eq!(status["verify"]["recent"][0]["runId"], id);
}

#[test]
fn a_failing_second_command_records_failed_with_its_step_and_tail() {
    let world = World::new(spec(&["echo one", "echo oops >&2; exit 3", "echo never"]));
    let (report, _) = world.report("dev-1");
    let id = world.ok("operator", "verify", &[&report])["runId"]
        .as_str()
        .unwrap()
        .to_string();
    let run = world.finished(&id);
    assert_eq!(run["state"], "failed", "{run}");
    assert_eq!(run["failedStep"], 2);
    assert_eq!(run["exitCode"], 3);
    let tail = run["outputTail"].as_str().unwrap();
    assert!(tail.contains("oops") && !tail.contains("never"), "{tail}");
    world.assert_clean(&id);
}

#[test]
fn the_gate_runs_in_a_detached_worktree_at_the_commit_with_a_scrubbed_environment() {
    let world = World::new(
        spec(&["test -f one.txt && ! test -f ../a-file-of-the-project && pwd && env | sort > env.out && cat env.out | grep -c . && git rev-parse HEAD"])
            .extra("env = { GATE_MODE = \"fast\" }\n"),
    );
    let (report, commit) = world.report("dev-1");
    let id = world.ok("operator", "verify", &[&report])["runId"]
        .as_str()
        .unwrap()
        .to_string();
    let run = world.finished(&id);
    assert_eq!(run["state"], "passed", "{run}");
    assert!(run["outputTail"].as_str().unwrap().contains(&commit));
    world.assert_clean(&id);
}

#[test]
fn the_environment_has_the_marker_the_configured_variables_and_no_credentials() {
    std::env::set_var("CAPSTAN_TOKEN", "must-not-leak");
    std::env::set_var("SECRET_FROM_DAEMON", "also-not");
    let out = tempfile::tempdir().unwrap();
    let file = out.path().join("env");
    let command = format!(
        "echo run=$CAPSTAN_VERIFY_RUN_ID commit=$CAPSTAN_VERIFY_COMMIT mode=$GATE_MODE token=${{CAPSTAN_TOKEN:-none}} secret=${{SECRET_FROM_DAEMON:-none}} > {}",
        file.display()
    );
    let world = World::new(spec(&[&command]).extra("env = { GATE_MODE = \"fast\" }\n"));
    let (report, commit) = world.report("dev-1");
    let id = world.ok("operator", "verify", &[&report])["runId"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(world.finished(&id)["state"], "passed");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap().trim(),
        format!("run={id} commit={commit} mode=fast token=none secret=none")
    );
}

#[test]
fn a_command_past_the_timeout_records_timeout_and_its_group_is_gone() {
    let pidfile = tempfile::tempdir().unwrap();
    let pid_path = pidfile.path().join("pid");
    let command = format!("echo $$ > {}; sleep 30", pid_path.display());
    let world = World::new(spec(&[&command]).tweak(|v| v.timeout_seconds = 1));
    let (report, _) = world.report("dev-1");
    let id = world.ok("operator", "verify", &[&report])["runId"]
        .as_str()
        .unwrap()
        .to_string();
    let run = world.finished(&id);
    assert_eq!(run["state"], "timeout", "{run}");
    assert_eq!(run["failedStep"], 1);
    let pid = read_pid(&pid_path).expect("the command started");
    assert!(!alive(pid), "the shell of the gate is still running");
    world.assert_clean(&id);
}

#[test]
fn the_timeout_is_shared_by_the_steps_of_a_run() {
    let world = World::new(
        spec(&["sleep 0.7", "sleep 0.7", "echo third"]).tweak(|v| v.timeout_seconds = 1),
    );
    let (report, _) = world.report("dev-1");
    let id = world.ok("operator", "verify", &[&report])["runId"]
        .as_str()
        .unwrap()
        .to_string();
    let run = world.finished(&id);
    assert_eq!(run["state"], "timeout", "{run}");
    assert_eq!(run["failedStep"], 2);
}

#[test]
fn a_log_stays_within_its_cap_and_the_tail_within_its_size() {
    let world = World::new(
        spec(&["head -c 300000 /dev/zero | tr '\\0' x; echo; echo the-end"]).tweak(|v| {
            v.max_log_bytes = 4096;
            v.output_tail_bytes = 1024;
        }),
    );
    let (report, _) = world.report("dev-1");
    let id = world.ok("operator", "verify", &[&report])["runId"]
        .as_str()
        .unwrap()
        .to_string();
    let run = world.finished(&id);
    assert_eq!(run["state"], "passed", "{run}");
    assert!(run["outputTail"].as_str().unwrap().len() <= 1024);
    let log = std::fs::read(run["logPath"].as_str().unwrap()).unwrap();
    assert!(log.len() <= 4096 + 64, "the log is {} bytes", log.len());
    let text = String::from_utf8_lossy(&log);
    assert!(text.contains("bytes cut"), "no marker");
    assert!(text.trim_end().ends_with("the-end"));
    assert!(text.starts_with("$ [1/1]"));
}

#[test]
fn only_keep_logs_log_files_are_kept() {
    let world = World::new(spec(&["echo hi"]).tweak(|v| v.keep_logs = 2));
    let mut ids = Vec::new();
    for agent in ["dev-1", "dev-2"] {
        let (report, _) = world.report(agent);
        for sha in ["a", "b"] {
            let queued = world.ledger(
                &world.credentials["operator"].clone(),
                "queueVerifyRun",
                vec![json!({"subjectId": report, "commands": ["echo hi"], "commandsSha": format!("sha-{agent}-{sha}"), "trigger": "manual"})],
            );
            ids.push(queued["runId"].as_str().unwrap().to_string());
        }
    }
    world.service.as_ref().unwrap().notify();
    for id in &ids {
        world.finished(id);
    }
    let logs = std::fs::read_dir(world.scratch.path().join("logs"))
        .unwrap()
        .count();
    assert_eq!(logs, 2);
}

// ------------------------------------------------------------------------------------------------ interruption

#[test]
fn a_daemon_stopped_mid_run_records_interrupted_and_cleans_up() {
    let pidfile = tempfile::tempdir().unwrap();
    let pid_path = pidfile.path().join("pid");
    let command = format!("echo $$ > {}; sleep 60", pid_path.display());
    let world = World::new(spec(&[&command]));
    let (report, _) = world.report("dev-1");
    let id = world.ok("operator", "verify", &[&report])["runId"]
        .as_str()
        .unwrap()
        .to_string();
    world.wait_for("the command to start", || read_pid(&pid_path).is_some());
    world.stop();
    let run = world.run(&id);
    assert_eq!(run["state"], "interrupted", "{run}");
    let pid = read_pid(&pid_path).unwrap();
    assert!(!alive(pid));
    world.assert_clean(&id);
    // Nothing passed.
    let runs = world.ok("operator", "verify", &["list"]);
    assert!(runs["runs"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["state"] != "passed"));
}

#[test]
fn a_restart_after_a_hard_kill_interrupts_the_stale_run_kills_its_group_and_removes_its_scratch() {
    let world = World::new(spec(&["echo hi"]));
    // The daemon that died: a claimed run, its recorded process, a scratch worktree and a log.
    world.service.as_ref().unwrap().stop();
    let (report, commit) = world.report("dev-1");
    let queued = world.ledger(
        &world.credentials["operator"].clone(),
        "queueVerifyRun",
        vec![json!({"subjectId": report, "commands": ["echo hi"], "commandsSha": "stale", "trigger": "manual"})],
    );
    let id = queued["runId"].as_str().unwrap().to_string();
    let scratch_path = world.scratch_path(&id);
    let log_path = world.scratch.path().join("logs").join(format!("{id}.log"));
    scratch::add_worktree(world.dir.path(), &scratch_path, &commit).unwrap();
    let claimed = world.ledger(
        world.deps.credential(),
        "claimVerifyRun",
        vec![json!({"runId": id, "maxParallel": 1, "scratchPath": scratch_path.to_string_lossy(), "logPath": log_path.to_string_lossy()})],
    );
    assert_eq!(claimed["claimed"], true);
    let mut orphan = Command::new("sh");
    orphan
        .args(["-c", "sleep 60 & wait"])
        .current_dir(&scratch_path)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .env("CAPSTAN_VERIFY_RUN_ID", &id)
        .process_group(0);
    let mut child = orphan.spawn().unwrap();
    let pgid = i64::from(child.id());
    let start = capstan_operator::service::process_start_time(pgid).unwrap();
    world.ledger(
        world.deps.credential(),
        "recordVerifyProcess",
        vec![json!({"runId": id, "pgid": pgid, "leaderStart": start})],
    );
    assert!(scratch_path.exists());
    // The new daemon: a service that recovers.
    let restarted = VerifyService::with_config(
        &world.deps,
        world.service.as_ref().unwrap().config().clone(),
    );
    restarted.recover();
    let run = world.run(&id);
    assert_eq!(run["state"], "interrupted", "{run}");
    let _ = child.wait();
    assert!(!alive(pgid));
    world.assert_clean(&id);
    assert!(!scratch_path.exists());
}

#[test]
fn a_crash_during_step_two_leaves_no_survivor_even_with_reap_off() {
    // The ledger records the first step's group only, so the process of step 2 is found by its marker.
    let world = World::new(spec(&["echo hi"]).tweak(|v| v.reap = false));
    world.service.as_ref().unwrap().stop();
    let (report, commit) = world.report("dev-1");
    let queued = world.ledger(
        &world.credentials["operator"].clone(),
        "queueVerifyRun",
        vec![json!({"subjectId": report, "commands": ["echo hi"], "commandsSha": "step2", "trigger": "manual"})],
    );
    let id = queued["runId"].as_str().unwrap().to_string();
    let scratch_path = world.scratch_path(&id);
    scratch::add_worktree(world.dir.path(), &scratch_path, &commit).unwrap();
    world.ledger(
        world.deps.credential(),
        "claimVerifyRun",
        vec![json!({"runId": id, "maxParallel": 1, "scratchPath": scratch_path.to_string_lossy(), "logPath": "/tmp/none.log"})],
    );
    // Step 1 ended; its group is recorded and gone. Step 2 left a process in a group of its own.
    let mut step_one = Command::new("true");
    step_one.process_group(0);
    let mut first = step_one.spawn().unwrap();
    let first_pgid = i64::from(first.id());
    let first_start =
        capstan_operator::service::process_start_time(first_pgid).unwrap_or("1".into());
    let _ = first.wait();
    world.ledger(
        world.deps.credential(),
        "recordVerifyProcess",
        vec![json!({"runId": id, "pgid": first_pgid, "leaderStart": first_start})],
    );
    let mut step_two = Command::new("sleep");
    step_two
        .arg("60")
        .current_dir(&scratch_path)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .env("CAPSTAN_VERIFY_RUN_ID", &id)
        .process_group(0);
    let mut child = step_two.spawn().unwrap();
    let pid = i64::from(child.id());
    let restarted = VerifyService::with_config(
        &world.deps,
        world.service.as_ref().unwrap().config().clone(),
    );
    restarted.recover();
    let _ = child.wait();
    assert!(!alive(pid));
    assert_eq!(world.run(&id)["state"], "interrupted");
    world.assert_clean(&id);
}

#[test]
fn a_recorded_leader_that_is_another_process_now_is_not_killed() {
    let world = World::new(spec(&["echo hi"]));
    world.service.as_ref().unwrap().stop();
    let (report, commit) = world.report("dev-1");
    let queued = world.ledger(
        &world.credentials["operator"].clone(),
        "queueVerifyRun",
        vec![json!({"subjectId": report, "commands": ["echo hi"], "commandsSha": "reused", "trigger": "manual"})],
    );
    let id = queued["runId"].as_str().unwrap().to_string();
    let scratch_path = world.scratch_path(&id);
    scratch::add_worktree(world.dir.path(), &scratch_path, &commit).unwrap();
    world.ledger(
        world.deps.credential(),
        "claimVerifyRun",
        vec![json!({"runId": id, "maxParallel": 1, "scratchPath": scratch_path.to_string_lossy(), "logPath": "/tmp/none.log"})],
    );
    let mut bystander = Command::new("sleep");
    bystander.arg("30").process_group(0);
    let mut child = bystander.spawn().unwrap();
    let pgid = i64::from(child.id());
    world.ledger(
        world.deps.credential(),
        "recordVerifyProcess",
        vec![json!({"runId": id, "pgid": pgid, "leaderStart": "1"})],
    );
    let restarted = VerifyService::with_config(
        &world.deps,
        world.service.as_ref().unwrap().config().clone(),
    );
    restarted.recover();
    assert_eq!(world.run(&id)["state"], "interrupted");
    assert!(
        alive(pgid),
        "a process that only has the recorded pid was killed"
    );
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn recovery_removes_a_stray_scratch_worktree_and_leaves_queued_runs() {
    let world = World::new(spec(&["echo hi"]));
    world.service.as_ref().unwrap().stop();
    let (report, commit) = world.report("dev-1");
    let stray = world.scratch.path().join("stray-run");
    scratch::add_worktree(world.dir.path(), &stray, &commit).unwrap();
    let queued = world.ok("operator", "verify", &[&report]);
    let restarted = VerifyService::with_config(
        &world.deps,
        world.service.as_ref().unwrap().config().clone(),
    );
    restarted.recover();
    assert!(!stray.exists());
    assert_eq!(
        world.run(queued["runId"].as_str().unwrap())["state"],
        "queued"
    );
    assert_eq!(
        world
            .git(&["worktree", "list", "--porcelain"])
            .lines()
            .filter(|l| l.starts_with("worktree "))
            .count(),
        1
    );
}

#[test]
fn cancelling_a_running_run_kills_its_group_and_records_interrupted() {
    let pidfile = tempfile::tempdir().unwrap();
    let pid_path = pidfile.path().join("pid");
    let command = format!("echo $$ > {}; sleep 60", pid_path.display());
    let world = World::new(spec(&[&command]));
    let (report, _) = world.report("dev-1");
    let id = world.ok("dev-1", "verify", &[&report])["runId"]
        .as_str()
        .unwrap()
        .to_string();
    world.wait_for("the command to start", || read_pid(&pid_path).is_some());
    // dev-2 neither asked for the run nor is its architect.
    world.refused("dev-2", "verify", &["cancel", &id], "forbidden");
    let cancelled = world.ok("dev-1", "verify", &["cancel", &id]);
    assert_eq!(cancelled["state"], "interrupted", "{cancelled}");
    assert!(!alive(read_pid(&pid_path).unwrap()));
    world.assert_clean(&id);
    assert_eq!(world.run(&id)["state"], "interrupted");
}

#[test]
fn the_pm_the_operator_and_the_architect_may_cancel_a_queued_run_and_a_finished_one_is_refused() {
    let world = World::new(spec(&["sleep 60"]).tweak(|v| v.max_parallel = 1));
    let (first, _) = world.report("dev-1");
    let (second, _) = world.report("dev-2");
    // The first run holds the only slot, so the second stays queued.
    let busy = world.ok("dev-1", "verify", &[&first])["runId"]
        .as_str()
        .unwrap()
        .to_string();
    world.wait_for("the first run to start", || {
        world.run(&busy)["state"] == "running"
    });
    let queued = world.ok("dev-2", "verify", &[&second])["runId"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(world.run(&queued)["state"], "queued");
    world.refused("dev-1", "verify", &["cancel", &queued], "forbidden");
    let cancelled = world.ok("pm-1", "verify", &["cancel", &queued]);
    assert_eq!(cancelled["state"], "cancelled");
    let again = world.refused("operator", "verify", &["cancel", &queued], "rejected");
    assert!(again.contains("already cancelled"), "{again}");
    world.ok("operator", "verify", &["cancel", &busy]);
    world.finished(&busy);
}

// ------------------------------------------------------------------------------------------------ concurrency

#[test]
fn with_max_parallel_one_three_queued_runs_never_run_together_and_all_finish() {
    let world = World::new(spec(&["sleep 0.6"]).tweak(|v| v.max_parallel = 1));
    let (report, _) = world.report("dev-1");
    let operator = world.credentials["operator"].clone();
    let ids: Vec<String> = ["a", "b", "c"]
        .iter()
        .map(|sha| {
            world.ledger(
                &operator,
                "queueVerifyRun",
                vec![json!({"subjectId": report, "commands": ["sleep 0.6"], "commandsSha": format!("sha-{sha}"), "trigger": "manual"})],
            )["runId"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    world.service.as_ref().unwrap().notify();
    let (mut most, mut samples) = (0, 0);
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let runs = world.read("verifyRuns", vec![Value::Null, json!(10)]);
        let states: Vec<&str> = runs["runs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["state"].as_str().unwrap())
            .collect();
        let running = states.iter().filter(|s| **s == "running").count();
        most = most.max(running);
        samples += 1;
        if states.len() == 3 && states.iter().all(|s| *s == "passed") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the runs did not finish: {states:?}"
        );
        std::thread::sleep(Duration::from_millis(15));
    }
    assert!(samples > 10);
    assert_eq!(
        most, 1,
        "at most one run at any moment, and one at some moment"
    );
    for id in &ids {
        world.assert_clean(id);
    }
}

#[test]
fn max_parallel_two_runs_two_at_once() {
    let world = World::new(spec(&["sleep 1"]).tweak(|v| v.max_parallel = 2));
    let (report, _) = world.report("dev-1");
    let operator = world.credentials["operator"].clone();
    for sha in ["a", "b", "c"] {
        world.ledger(
            &operator,
            "queueVerifyRun",
            vec![json!({"subjectId": report, "commands": ["sleep 1"], "commandsSha": format!("sha-{sha}"), "trigger": "manual"})],
        );
    }
    world.service.as_ref().unwrap().notify();
    let mut most = 0;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let runs = world.read("verifyRuns", vec![Value::Null, json!(10)]);
        let states: Vec<&str> = runs["runs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["state"].as_str().unwrap())
            .collect();
        most = most.max(states.iter().filter(|s| **s == "running").count());
        if states.iter().all(|s| *s == "passed") && states.len() == 3 {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(15));
    }
    assert_eq!(most, 2);
}

// ------------------------------------------------------------------------------------------------ the route

#[test]
fn without_a_verify_section_nothing_runs_and_verify_is_not_configured() {
    let world = World::new(Spec::default());
    assert!(world.service.is_none());
    let (report, _) = world.report("dev-1");
    let message = world.refused("operator", "verify", &[&report], "rejected");
    assert!(message.starts_with("verify_not_configured:"), "{message}");
    world.refused("dev-1", "verify", &["list"], "rejected");
    let status = world.ok("operator", "status", &[]);
    assert!(status.get("verify").is_none());
    assert_eq!(
        world.read("verifyRuns", vec![Value::Null, json!(5)])["runs"],
        json!([])
    );
}

#[test]
fn the_verify_route_validates_its_arguments_and_accepts_id_prefixes() {
    let world = World::new(spec(&["echo hi"]));
    let (report, _) = world.report("dev-1");
    world.refused("operator", "verify", &[], "invalid_request");
    world.refused("operator", "verify", &["show"], "invalid_request");
    world.refused("operator", "verify", &["show", "a", "b"], "invalid_request");
    world.refused("operator", "verify", &["cancel"], "invalid_request");
    let unknown = world.refused("operator", "verify", &["show", "no-such-run"], "rejected");
    assert!(unknown.starts_with("unknown_run:"), "{unknown}");
    let missing = world.refused("operator", "verify", &["no-such-report"], "rejected");
    assert!(missing.starts_with("verify_refused:"), "{missing}");
    // A unique prefix of the report id and of the run id.
    let queued = world.ok("operator", "verify", &[&report[..8]]);
    let id = queued["runId"].as_str().unwrap().to_string();
    let run = world.ok("operator", "verify", &["show", &id[..8]]);
    assert_eq!(run["runId"], id);
    world.finished(&id);
    let listed = world.ok("operator", "verify", &["list", &report[..8]]);
    assert_eq!(listed["runs"][0]["runId"], id);
    assert!(listed["runs"][0].get("outputTail").is_none());
    let all = world.ok("operator", "verify", &["list"]);
    assert_eq!(all["runs"].as_array().unwrap().len(), 1);
    // A finished run cannot be cancelled.
    let late = world.refused("operator", "verify", &["cancel", &id], "rejected");
    assert!(late.contains("already passed"), "{late}");
}

// ------------------------------------------------------------------------------------------------ automatic runs

#[test]
fn auto_report_queues_exactly_one_run_per_verified_report() {
    let world = World::new(spec(&["echo hi"]).extra("auto = [\"report\"]\n"));
    let (report, commit) = world.report("dev-1");
    let runs = world.ok("operator", "verify", &["list", &report]);
    let runs = runs["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(runs[0]["trigger"], "auto");
    assert_eq!(runs[0]["commitSha"], commit);
    let id = runs[0]["runId"].as_str().unwrap().to_string();
    // A repeated report writes nothing, and queues nothing; a manual request while the run is active answers that run.
    let repeat = world.ok("dev-1", "report", &[&commit, "Done"]);
    assert_eq!(repeat["duplicate"], true);
    let manual = world.ok("dev-1", "verify", &[&report]);
    if manual["existing"] == true {
        assert_eq!(manual["runId"], id);
    }
    world.finished(&id);
    assert_eq!(
        world.ok("operator", "verify", &["list", &report])["runs"]
            .as_array()
            .unwrap()
            .len(),
        if manual["existing"] == true { 1 } else { 2 }
    );
}

#[test]
fn nothing_is_queued_automatically_without_auto() {
    let world = World::new(spec(&["echo hi"]));
    let (report, _) = world.report("dev-1");
    assert_eq!(
        world.ok("operator", "verify", &["list", &report])["runs"],
        json!([])
    );
}

// ------------------------------------------------------------------------------------------------ the gates

fn script_reviewer(launcher: &StubLauncher) {
    launcher.script_spawn(SpawnResult {
        state: "started".into(),
        agent_id: "rev-1".into(),
        pane_id: "w1:p1".into(),
        worktree_path: "/tmp/work/rev-1".into(),
        branch: "capstan/rev-1".into(),
        placement: Placement::Tab,
        placement_note: None,
        hint: None,
        missing_env: None,
        warning: None,
    });
}

fn review_task(world: &World) -> String {
    let messages = world.read("messagesFor", vec![json!("rev-1")]);
    messages
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["body"].as_str().unwrap().to_string())
        .find(|body| body.contains("review"))
        .expect("a review task")
}

#[test]
fn the_reviewer_brief_carries_the_recorded_gate() {
    let world = World::new(spec(&["echo gate-ok"]));
    let (report, commit) = world.report("dev-1");
    let id = world.ok("operator", "verify", &[&report])["runId"]
        .as_str()
        .unwrap()
        .to_string();
    world.finished(&id);
    script_reviewer(&world.launcher);
    world.ok("pm-1", "request-review", &[&report]);
    let task = review_task(&world);
    assert!(
        task.contains(&format!("Gate passed at {commit} (run {id}")),
        "{task}"
    );
}

#[test]
fn the_reviewer_brief_of_a_failed_run_carries_the_failed_line() {
    let world = World::new(spec(&["echo broken >&2; exit 4"]));
    let (report, commit) = world.report("dev-1");
    let id = world.ok("operator", "verify", &[&report])["runId"]
        .as_str()
        .unwrap()
        .to_string();
    world.finished(&id);
    script_reviewer(&world.launcher);
    world.ok("pm-1", "request-review", &[&report]);
    let task = review_task(&world);
    assert!(
        task.contains(&format!("Gate failed at {commit} (run {id}")),
        "{task}"
    );
    assert!(task.contains("broken"), "{task}");
}

#[test]
fn the_reviewer_brief_without_a_run_says_the_gate_did_not_run() {
    let world = World::new(spec(&["echo hi"]));
    let (report, _) = world.report("dev-1");
    script_reviewer(&world.launcher);
    world.ok("pm-1", "request-review", &[&report]);
    let task = review_task(&world);
    assert!(task.contains("No gate run is recorded"), "{task}");
}

#[test]
fn with_require_review_a_request_without_a_passed_run_is_refused_and_a_passed_run_lets_it_through()
{
    let world = World::new(spec(&["echo hi"]).extra("require = [\"review\"]\n"));
    let (report, commit) = world.report("dev-1");
    let message = world.refused("pm-1", "request-review", &[&report], "rejected");
    assert!(message.starts_with("verify_required:"), "{message}");
    assert!(
        message.contains(&report) && message.contains(&commit),
        "{message}"
    );
    assert!(
        message.contains(&format!("cstan verify {report}")),
        "{message}"
    );
    let id = world.ok("pm-1", "verify", &[&report])["runId"]
        .as_str()
        .unwrap()
        .to_string();
    world.finished(&id);
    script_reviewer(&world.launcher);
    let requested = world.ok("pm-1", "request-review", &[&report]);
    assert_eq!(requested["state"], "started");
}

#[test]
fn a_failed_run_does_not_open_the_review_gate_and_changed_commands_make_a_pass_stale() {
    let world = World::new(spec(&["exit 1"]).extra("require = [\"review\"]\n"));
    let (report, _) = world.report("dev-1");
    let id = world.ok("pm-1", "verify", &[&report])["runId"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(world.finished(&id)["state"], "failed");
    let message = world.refused("pm-1", "request-review", &[&report], "rejected");
    assert!(message.contains("did not pass"), "{message}");
}

#[test]
fn request_review_while_the_run_is_running_is_refused_at_once_with_the_run_id() {
    let world = World::new(spec(&["sleep 30"]).extra("require = [\"review\"]\n"));
    let (report, _) = world.report("dev-1");
    let id = world.ok("pm-1", "verify", &[&report])["runId"]
        .as_str()
        .unwrap()
        .to_string();
    world.wait_for("the run to start", || world.run(&id)["state"] == "running");
    let started = Instant::now();
    let message = world.refused("pm-1", "request-review", &[&report], "rejected");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the refusal waited"
    );
    assert!(message.starts_with("verify_required:"), "{message}");
    assert!(
        message.contains(&id) && message.contains("running"),
        "{message}"
    );
    // No reviewer was started for it.
    assert!(!world
        .launcher
        .calls()
        .iter()
        .any(|c| c.starts_with("spawn")));
    world.ok("operator", "verify", &["cancel", &id]);
}

fn pass_review(world: &World, subject: &str) {
    let pm = world.credentials["pm-1"].clone();
    let reviewer = world.credentials["rev-1"].clone();
    world.ledger(
        &pm,
        "beginReview",
        vec![json!({"subjectId": subject, "reviewerRole": "reviewer", "reviewerAgentId": "rev-1"})],
    );
    world.ledger(
        &reviewer,
        "completeReview",
        vec![json!({"verdict": "pass", "text": "Fine."})],
    );
}

#[test]
fn with_require_integrate_integrate_is_refused_without_a_passed_run_and_merges_after_one() {
    let world = World::new(spec(&["echo hi"]).extra("require = [\"integrate\"]\n"));
    let (report, commit) = world.report("dev-1");
    pass_review(&world, &report);
    let message = world.refused("pm-1", "integrate", &[&report], "rejected");
    assert!(message.starts_with("verify_required:"), "{message}");
    assert!(
        message.contains(&report) && message.contains(&commit),
        "{message}"
    );
    let id = world.ok("pm-1", "verify", &[&report])["runId"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(world.finished(&id)["state"], "passed");
    let merged = world.ok("pm-1", "integrate", &[&report]);
    assert_eq!(merged["state"], "merged");
}

#[test]
fn require_review_alone_does_not_gate_integrate() {
    let world = World::new(spec(&["echo hi"]).extra("require = [\"review\"]\n"));
    let (report, _) = world.report("dev-1");
    pass_review(&world, &report);
    let merged = world.ok("pm-1", "integrate", &[&report]);
    assert_eq!(merged["state"], "merged");
}

#[test]
fn auto_integration_queues_a_run_at_the_merged_commit() {
    let world = World::new(spec(&["echo hi"]).extra("auto = [\"integration\"]\n"));
    let (report, _) = world.report("dev-1");
    pass_review(&world, &report);
    let merged = world.ok("pm-1", "integrate", &[&report]);
    let integration = merged["integrationId"].as_str().unwrap().to_string();
    let runs = world.ok("operator", "verify", &["list", &integration]);
    let runs = runs["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(runs[0]["commitSha"], merged["head"]);
    assert_eq!(runs[0]["trigger"], "auto");
    world.finished(runs[0]["runId"].as_str().unwrap());
    world.assert_clean(runs[0]["runId"].as_str().unwrap());
}

// ------------------------------------------------------------------------------------------------ hooks

#[test]
fn the_worktree_setup_and_teardown_hooks_run_for_a_gate() {
    let marks = tempfile::tempdir().unwrap();
    let mark = marks.path().join("marks");
    let config = format!(
        "\n[worktree]\nsetup = \"echo setup-$CAPSTAN_VERIFY_RUN_ID-in-$PWD >> {m}\"\nteardown = \"echo teardown-$CAPSTAN_WORKTREE_PATH-$CAPSTAN_VERIFY_COMMIT >> {m}\"\n",
        m = mark.display()
    );
    let world = World::new_with_toml(spec(&["test -f one.txt"]), &config);
    let (report, commit) = world.report("dev-1");
    let id = world.ok("operator", "verify", &[&report])["runId"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(world.finished(&id)["state"], "passed");
    let marks = std::fs::read_to_string(&mark).unwrap();
    assert!(marks.contains(&format!("setup-{id}-in-")), "{marks}");
    assert!(
        marks.contains(&format!(
            "teardown-{}-{commit}",
            world.scratch_path(&id).display()
        )),
        "{marks}"
    );
    world.assert_clean(&id);
}

#[test]
fn a_failing_setup_is_an_error_run_not_a_gate_result() {
    let world = World::new_with_toml(
        spec(&["echo never"]),
        "\n[worktree]\nsetup = \"echo no-network >&2; exit 9\"\n",
    );
    let (report, _) = world.report("dev-1");
    let id = world.ok("operator", "verify", &[&report])["runId"]
        .as_str()
        .unwrap()
        .to_string();
    let run = world.finished(&id);
    assert_eq!(run["state"], "error", "{run}");
    assert!(
        run["outputTail"].as_str().unwrap().contains("setup failed"),
        "{run}"
    );
    world.assert_clean(&id);
}

#[test]
fn a_process_a_gate_leaves_in_another_session_is_stopped_with_the_run() {
    let pidfile = tempfile::tempdir().unwrap();
    let pid_path = pidfile.path().join("daemon-pid");
    let command = new_session_command(&pid_path);
    let world = World::new(spec(&[&command]));
    let (report, _) = world.report("dev-1");
    let id = world.ok("operator", "verify", &[&report])["runId"]
        .as_str()
        .unwrap()
        .to_string();
    let run = world.finished(&id);
    assert_eq!(run["state"], "passed", "{run}");
    let pid = read_pid(&pid_path).unwrap();
    assert!(
        !alive(pid),
        "the process in another session survived the run"
    );
    world.assert_clean(&id);
}

#[test]
fn with_reap_off_a_process_in_another_session_is_left_alone() {
    let pidfile = tempfile::tempdir().unwrap();
    let pid_path = pidfile.path().join("daemon-pid");
    let command = new_session_command(&pid_path);
    let world = World::new(spec(&[&command]).tweak(|v| v.reap = false));
    let (report, _) = world.report("dev-1");
    let id = world.ok("operator", "verify", &[&report])["runId"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(world.finished(&id)["state"], "passed");
    let pid = read_pid(&pid_path).unwrap();
    let survived = alive(pid);
    // SAFETY: stops the one process this test started.
    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    assert!(survived, "reap = false still stopped the process");
}

#[test]
fn status_lists_active_and_recent_runs_only_with_a_verify_section() {
    let world = World::new(spec(&["sleep 30"]));
    let (report, _) = world.report("dev-1");
    let id = world.ok("operator", "verify", &[&report])["runId"]
        .as_str()
        .unwrap()
        .to_string();
    world.wait_for("the run", || world.run(&id)["state"] == "running");
    let status = world.ok("pm-1", "status", &[]);
    assert_eq!(status["verify"]["active"][0]["runId"], id);
    assert!(status["verify"]["active"][0].get("outputTail").is_none());
    world.ok("operator", "verify", &["cancel", &id]);
    let status = world.ok("operator", "status", &[]);
    assert_eq!(status["verify"]["active"], json!([]));
    assert_eq!(status["verify"]["recent"][0]["state"], "interrupted");
}

#[test]
fn a_scratch_dir_that_holds_other_things_is_refused_and_left_untouched() {
    let world = World::new(spec(&["echo hi"]).prefill(|scratch| {
        let checkout = scratch.join("my-checkout-0001");
        std::fs::create_dir_all(&checkout).unwrap();
        let init = Command::new("git")
            .arg("-C")
            .arg(&checkout)
            .args(["init", "--quiet"])
            .output()
            .unwrap();
        assert!(init.status.success());
        std::fs::write(checkout.join("work.txt"), "precious\n").unwrap();
    }));
    let service = world.service.as_ref().unwrap();
    world.wait_for("the scratch directory to be judged", || {
        service.problem().is_some()
    });
    let problem = service.problem().unwrap();
    assert!(
        problem.contains("not empty") && problem.contains("scratch_dir"),
        "{problem}"
    );
    let (report, _) = world.report("dev-1");
    let message = world.refused("operator", "verify", &[&report], "rejected");
    assert!(message.starts_with("verify_scratch_refused:"), "{message}");
    service.stop();
    let checkout = world.scratch.path().join("my-checkout-0001");
    assert!(checkout.join(".git").exists());
    assert_eq!(
        std::fs::read_to_string(checkout.join("work.txt")).unwrap(),
        "precious\n"
    );
    assert!(!world.scratch.path().join(".capstan-verify").exists());
}

#[test]
fn recovery_leaves_a_repository_of_another_project_in_the_scratch_dir_alone() {
    let world = World::new(spec(&["echo hi"]));
    world.service.as_ref().unwrap().stop();
    // Made by a marker-holding daemon, then an unrelated checkout is put beside it (named like a run).
    let other = world.scratch.path().join("0123456789abcdef");
    std::fs::create_dir_all(&other).unwrap();
    let init = Command::new("git")
        .arg("-C")
        .arg(&other)
        .args(["init", "--quiet"])
        .output()
        .unwrap();
    assert!(init.status.success());
    let restarted = VerifyService::with_config(
        &world.deps,
        world.service.as_ref().unwrap().config().clone(),
    );
    restarted.recover();
    assert!(other.join(".git").exists());
}

#[test]
fn a_cancel_between_the_claim_and_the_start_of_the_run_is_not_lost() {
    let world = World::new(spec(&["sleep 30"]));
    world
        .service
        .as_ref()
        .unwrap()
        .claim_delay_ms
        .store(800, std::sync::atomic::Ordering::SeqCst);
    let (report, _) = world.report("dev-1");
    let id = world.ok("operator", "verify", &[&report])["runId"]
        .as_str()
        .unwrap()
        .to_string();
    world.wait_for("the claim", || world.run(&id)["state"] == "running");
    // The worker is still in the delay: the run is running in the ledger and not started yet.
    let cancelled = world.ok("operator", "verify", &["cancel", &id]);
    assert_eq!(cancelled["state"], "interrupted", "{cancelled}");
    world.assert_clean(&id);
}

#[test]
fn a_worker_that_panics_finishes_its_run_as_error_and_frees_its_slot() {
    let world = World::new(spec(&["echo ok"]).tweak(|v| v.max_parallel = 1));
    world
        .service
        .as_ref()
        .unwrap()
        .panic_next_run
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let (first, _) = world.report("dev-1");
    let (second, _) = world.report("dev-2");
    let id = world.ok("dev-1", "verify", &[&first])["runId"]
        .as_str()
        .unwrap()
        .to_string();
    let run = world.finished(&id);
    assert_eq!(run["state"], "error", "{run}");
    assert!(
        run["outputTail"].as_str().unwrap().contains("panicked"),
        "{run}"
    );
    world.assert_clean(&id);
    // The only slot is free again: the next run goes through.
    let next = world.ok("dev-2", "verify", &[&second])["runId"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(world.finished(&next)["state"], "passed");
}

#[test]
fn a_worker_that_panics_after_starting_a_process_leaves_none_even_with_reap_off() {
    let pidfile = tempfile::tempdir().unwrap();
    let pid_path = pidfile.path().join("daemon-pid");
    let command = new_session_command(&pid_path);
    let world = World::new(spec(&[&command]).tweak(|v| v.reap = false));
    world
        .service
        .as_ref()
        .unwrap()
        .panic_after_steps
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let (report, _) = world.report("dev-1");
    let id = world.ok("operator", "verify", &[&report])["runId"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(world.finished(&id)["state"], "error");
    let pid = read_pid(&pid_path).unwrap();
    let survived = alive(pid);
    if survived {
        // SAFETY: stops the one process this test started.
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    }
    assert!(!survived, "the panicked run left its process");
    world.assert_clean(&id);
}

#[test]
fn a_cancel_that_lands_after_the_steps_ended_answers_the_real_final_state() {
    let world = World::new(spec(&["echo ok"]));
    world
        .service
        .as_ref()
        .unwrap()
        .finish_delay_ms
        .store(1500, std::sync::atomic::Ordering::SeqCst);
    let (report, _) = world.report("dev-1");
    let id = world.ok("operator", "verify", &[&report])["runId"]
        .as_str()
        .unwrap()
        .to_string();
    // The steps end quickly; the finish is held back, so the ledger still says running and nothing can be aborted.
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(world.run(&id)["state"], "running");
    let cancelled = world.ok("operator", "verify", &["cancel", &id]);
    assert_eq!(cancelled["state"], "passed", "{cancelled}");
    assert_eq!(cancelled["alreadyFinished"], true, "{cancelled}");
}
