//! The verify gate's cache wiring: its slot directories, the slot lease, the infrastructure rerun, the scopes of the
//! environment and the start-up sweep. Steps run through a fake step runner, so no real build is involved.

mod common;

use capstan_cache::lease::GateLease;
use capstan_cache::{GateScope, Runtime};
use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::cache::runtime_of_deps;
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::shared::AbortSignal;
use capstan_daemon::handlers::MAX_RESPONSE_BYTES;
use capstan_daemon::verify::{LeaseFault, StepRequest, StepRunner, VerifyService};
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::types::InitialProject;
use capstan_kernel::Core;
use capstan_launcher::api::{LauncherService, StubLauncher};
use capstan_operator::service::{RunResult, RunStatus};
use common::project::{build_repo, write_project, Identity};
use common::replay::ReplayGit;
use common::{private_tempdir, seeded_env};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PROJECT: &str = "pgate";

const CONFIG: &str = r#"schema_version = 1

[hosts.claude]
kind = "claude"

[roles.pm]
kind = "PM"
host = "claude"

[roles.developer]
kind = "Developer"
host = "claude"
"#;

static WORLDS: AtomicUsize = AtomicUsize::new(0);

type Prepare = Box<dyn FnOnce(&Path)>;
type Calls = Arc<Mutex<Vec<(String, BTreeMap<String, String>)>>>;
type Behave = Arc<dyn Fn(&str, usize, &BTreeMap<String, String>) -> RunResult + Send + Sync>;

fn result(status: RunStatus, tail: &str) -> RunResult {
    RunResult {
        status,
        exit_code: Some(if status == RunStatus::Ok { 0 } else { 1 }),
        signal: None,
        duration_ms: 1,
        output_tail: tail.to_string(),
        truncated: false,
    }
}

fn ok() -> RunResult {
    result(RunStatus::Ok, "ok")
}

fn failed(tail: &str) -> RunResult {
    result(RunStatus::Failed, tail)
}

#[derive(Default)]
struct Cfg {
    commands: Vec<String>,
    /// Text after the `[cache]` table's own keys; `None` leaves `[cache]` out.
    cache: Option<String>,
    verify_extra: String,
    more_toml: String,
    max_parallel: Option<i64>,
    timeout_seconds: Option<i64>,
    /// Runs on the rust entry directory before the service starts.
    prepare: Option<Prepare>,
}

struct World {
    dir: common::PrivateDir,
    #[allow(dead_code)]
    cache_root: common::PrivateDir,
    scratch: common::PrivateDir,
    deps: Deps,
    service: Arc<VerifyService>,
    credentials: HashMap<String, String>,
    heads: BTreeMap<String, String>,
    logs: Arc<Mutex<Vec<LogEntry>>>,
    calls: Calls,
}

fn rust_cache(extra: &str) -> Option<String> {
    Some(format!(
        "presets = [\"rust\", \"python\"]\nmax_size_gb = 1\nmax_age_days = 30\n[cache.rust]\nmode = \"shared-build-dir\"\ntarget_dir = true\n{extra}"
    ))
}

impl World {
    /// `setup` configures the service before it starts.
    fn new(mut cfg: Cfg, behave: Behave, setup: impl FnOnce(&Arc<VerifyService>)) -> World {
        let dir = private_tempdir();
        let cache_root = private_tempdir();
        let scratch = private_tempdir();
        let identity = Identity {
            project_id: PROJECT.into(),
            name: "Gate".into(),
            credential: format!("operator-{}", "g".repeat(40)),
        };
        let state = write_project(dir.path(), &identity);
        let mut config = CONFIG.to_string();
        if let Some(cache) = &cfg.cache {
            let (head, rest) = cache.split_once("[cache.rust]").unwrap_or((cache, ""));
            config.push_str(&format!(
                "\n[cache]\nroot = \"{}\"\n{head}",
                cache_root.path().display()
            ));
            if !rest.is_empty() {
                config.push_str(&format!("[cache.rust]{rest}"));
            }
        }
        let commands: Vec<String> = cfg.commands.iter().map(|c| format!("{c:?}")).collect();
        config.push_str(&format!(
            "\n[verify]\ncommands = [{}]\nscratch_dir = \"{}\"\n{}\n",
            commands.join(", "),
            scratch.path().display(),
            cfg.verify_extra
        ));
        config.push_str(&cfg.more_toml);
        let file = dir.path().join("capstan.toml");
        std::fs::write(&file, config).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let trust = common::kept_trust_store();
        common::trusted_project(dir.path(), &trust);
        let heads = build_repo(
            dir.path(),
            &json!({
                "files": {"a.txt": "a\n"},
                "branches": {"feat/dev-1": {"files": {"one.txt": "one\n"}, "message": "feat: one"}},
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
                        "gate-env-{}-{}",
                        std::process::id(),
                        WORLDS.fetch_add(1, Ordering::SeqCst)
                    )),
                )
            })
            .expect("the kernel opens")
        };
        let logs: Arc<Mutex<Vec<LogEntry>>> = Arc::default();
        let sink: Logger = {
            let logs = logs.clone();
            Arc::new(move |entry: &LogEntry| logs.lock().unwrap().push(entry.clone()))
        };
        let mut options = DaemonOptions::new(state, project, dir.path().to_path_buf());
        options.trust_store = trust.clone();
        let role_config = capstan_config::load_role_config_with(dir.path(), &trust).unwrap();
        let mut verify = role_config.verify.clone().expect("a [verify] section");
        if let Some(parallel) = cfg.max_parallel {
            verify.max_parallel = parallel;
        }
        if let Some(seconds) = cfg.timeout_seconds {
            verify.timeout_seconds = seconds;
        }
        options.capstan = Some(Arc::new(role_config));
        let mut deps = Deps::new(kernel, options, sink);
        deps.git = Arc::new(ReplayGit::new(dir.path()));
        deps.launcher = Some(Arc::new(StubLauncher::new()) as Arc<dyn LauncherService>);
        capstan_daemon::deps::startup_sequence(&deps);
        let service = VerifyService::with_config(&deps, verify);
        deps.verify = Some(service.clone());
        let calls: Calls = Arc::default();
        let counter: Arc<Mutex<HashMap<String, usize>>> = Arc::default();
        let runner: StepRunner = {
            let calls = calls.clone();
            Arc::new(move |request: StepRequest| {
                let env: BTreeMap<String, String> = request.environment.iter().cloned().collect();
                let nth = {
                    let mut counts = counter.lock().unwrap();
                    let n = counts.entry(request.command.clone()).or_insert(0);
                    *n += 1;
                    *n
                };
                calls
                    .lock()
                    .unwrap()
                    .push((request.command.clone(), env.clone()));
                behave(&request.command, nth, &env)
            })
        };
        service.set_step_runner(runner);
        service.set_lease_wait(Duration::from_millis(50));
        setup(&service);
        let mut world = World {
            dir,
            cache_root,
            scratch,
            deps,
            service,
            credentials: HashMap::new(),
            heads,
            logs,
            calls,
        };
        world
            .credentials
            .insert("operator".into(), identity.credential);
        world.populate();
        if let Some(prepare) = cfg.prepare.take() {
            prepare(&world.entry_dir());
        }
        world.service.start();
        world
    }

    fn ledger(&self, credential: &str, op: &str, args: Vec<Value>) -> Value {
        let (credential, name) = (credential.to_string(), op.to_string());
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

    fn populate(&mut self) {
        let owner = self.credentials["operator"].clone();
        let roles: Vec<Value> = [("pm", "PM"), ("developer", "Developer")]
            .iter()
            .map(|(n, k)| json!({"name": n, "kind": k, "host": "claude", "configHash": "a".repeat(64)}))
            .collect();
        self.ledger(&owner, "syncRoleDefinitions", vec![json!(roles)]);
        for (agent, role, kind) in [("pm-1", "pm", "PM"), ("dev-1", "developer", "Developer")] {
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
        self.ledger(
            &internal,
            "recordAgentPane",
            vec![json!({
                "agentId": "dev-1", "workspaceId": "ws-1", "paneId": "pane-dev-1",
                "worktreePath": "/tmp/wt/dev-1", "branch": "feat/dev-1", "baseSha": self.heads["main"],
            })],
        );
    }

    fn call(&self, who: &str, command: &str, args: &[&str]) -> Value {
        let credential = self.credentials[who].clone();
        let frame = json!({"v": 1, "credential": credential, "command": command, "args": args});
        let outcome = handle_frame(
            &self.deps,
            frame.to_string().as_bytes(),
            &AbortSignal::new(),
            MAX_RESPONSE_BYTES,
        );
        let answer: Value = serde_json::from_str(&outcome.response.expect("an answer")).unwrap();
        assert_eq!(answer["ok"], true, "{command} {args:?}: {answer}");
        answer["result"].clone()
    }

    /// Queues a run for dev-1's report; returns the run id.
    fn queue(&self) -> String {
        let sha = self.heads["feat/dev-1"].clone();
        let report = self.call("dev-1", "report", &[&sha, "Done"]);
        let id = report["reportId"].as_str().unwrap().to_string();
        self.call("operator", "verify", &[&id])["runId"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn show(&self, id: &str) -> Value {
        self.call("operator", "verify", &["show", id])
    }

    fn finished(&self, id: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let run = self.show(id);
            let state = run["state"].as_str().unwrap();
            if state != "queued" && state != "running" {
                return run;
            }
            assert!(Instant::now() < deadline, "run {id} did not end: {run}");
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn run_to_end(&self) -> Value {
        let id = self.queue();
        self.finished(&id)
    }

    fn runtime(&self) -> Runtime {
        runtime_of_deps(&self.deps).expect("a cache runtime")
    }

    fn entry_dir(&self) -> PathBuf {
        self.runtime().entry_dir("rust")
    }

    fn slot_target(&self, slot: usize) -> PathBuf {
        self.entry_dir()
            .join("gate-target")
            .join(PROJECT)
            .join(format!("slot-{slot}"))
    }

    fn slot_build(&self, slot: usize) -> PathBuf {
        self.entry_dir()
            .join("gate-build")
            .join(PROJECT)
            .join(format!("slot-{slot}"))
    }

    fn lease_free(&self, slot: usize) -> bool {
        GateLease::try_acquire(&self.entry_dir(), PROJECT, slot).is_some()
    }

    /// Waits (up to 2 s) for the lease to be free. Parallel tests fork gate steps, and a child holds a copy of the
    /// lease descriptor until its exec, so a released flock can look held for a few milliseconds.
    fn wait_lease_free(&self, slot: usize) -> bool {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if self.lease_free(slot) {
                return true;
            }
            if std::time::Instant::now() >= end {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn logged(&self, event: &str) -> Vec<Value> {
        self.logs
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.command == format!("daemon:{event}"))
            .map(|e| e.detail.clone().unwrap_or(Value::Null))
            .collect()
    }

    fn commands_run(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|(c, _)| c.clone())
            .collect()
    }

    fn env_of(&self, nth: usize) -> BTreeMap<String, String> {
        self.calls.lock().unwrap()[nth].1.clone()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        self.service.stop();
    }
}

fn cfg(commands: &[&str]) -> Cfg {
    Cfg {
        commands: commands.iter().map(|c| c.to_string()).collect(),
        cache: rust_cache(""),
        ..Cfg::default()
    }
}

fn passing() -> Behave {
    Arc::new(|_, _, _| ok())
}

fn nothing(_: &Arc<VerifyService>) {}

fn target_of(env: &BTreeMap<String, String>) -> PathBuf {
    PathBuf::from(&env["CARGO_TARGET_DIR"])
}

#[test]
fn a_gate_builds_in_its_slot_directory_and_the_verify_env_wins() {
    let mut c = cfg(&["gate"]);
    c.verify_extra =
        "env = { CARGO_BUILD_BUILD_DIR = \"/from/verify\", GATE_MODE = \"x\" }\n".into();
    let world = World::new(c, passing(), nothing);
    let run = world.run_to_end();
    assert_eq!(run["state"], "passed", "{run}");
    let env = world.env_of(0);
    assert_eq!(target_of(&env), world.slot_target(0));
    assert_eq!(env["CARGO_BUILD_BUILD_DIR"], "/from/verify");
    assert_eq!(env["GATE_MODE"], "x");
    assert!(
        env.contains_key("PIP_CACHE_DIR"),
        "other cache variables are kept"
    );
    // An agent keeps the shared target.
    let agent: BTreeMap<String, String> =
        capstan_launcher::cache_env::cache_extras(&world.runtime(), GateScope::Agent)
            .unwrap()
            .into_iter()
            .collect();
    assert!(
        agent["CARGO_TARGET_DIR"].ends_with("cargo-target"),
        "{agent:?}"
    );
    assert!(world.wait_lease_free(0));
}

/// A behaviour: `build` always passes; `test` fails once, wiping the slot's target as it does.
fn wipes_once(counter_command: &'static str, fail_tail: &'static str) -> Behave {
    Arc::new(move |command, nth, env| {
        if command == counter_command && nth == 1 {
            let target = target_of(env);
            let _ = std::fs::remove_dir_all(&target);
            return failed(fail_tail);
        }
        ok()
    })
}

#[test]
fn a_step_that_wipes_the_slot_and_fails_reruns_the_list_once_from_step_one() {
    let world = World::new(cfg(&["build", "test"]), wipes_once("test", "boom"), nothing);
    let run = world.run_to_end();
    assert_eq!(run["state"], "passed", "{run}");
    assert_eq!(world.commands_run(), ["build", "test", "build", "test"]);
    assert!(world.wait_lease_free(0));
}

#[test]
fn a_test_step_that_needs_the_build_output_passes_after_the_rerun() {
    let behave: Behave = Arc::new(|command, nth, env| {
        let target = target_of(env);
        match command {
            "build" => {
                std::fs::create_dir_all(target.join("debug")).unwrap();
                std::fs::write(target.join("debug/out"), "x").unwrap();
                ok()
            }
            _ if nth == 1 => {
                let _ = std::fs::remove_dir_all(&target);
                failed("no such file: debug/out")
            }
            _ if target.join("debug/out").is_file() => ok(),
            _ => failed("no build output"),
        }
    });
    let world = World::new(cfg(&["build", "test"]), behave, nothing);
    let run = world.run_to_end();
    assert_eq!(run["state"], "passed", "{run}");
}

#[test]
fn the_lease_is_held_on_the_recreated_directory_during_the_rerun() {
    let held = Arc::new(AtomicBool::new(false));
    let entry: Arc<Mutex<Option<PathBuf>>> = Arc::default();
    let behave: Behave = {
        let (held, entry) = (held.clone(), entry.clone());
        Arc::new(move |command, nth, env| {
            let target = target_of(env);
            if command == "test" && nth == 1 {
                let _ = std::fs::remove_dir_all(&target);
                return failed("gone");
            }
            if command == "test" {
                let dir = entry.lock().unwrap().clone().unwrap();
                held.store(
                    target.is_dir() && GateLease::try_acquire(&dir, PROJECT, 0).is_none(),
                    Ordering::SeqCst,
                );
            }
            ok()
        })
    };
    let world = World::new(cfg(&["build", "test"]), behave, nothing);
    *entry.lock().unwrap() = Some(world.entry_dir());
    assert_eq!(world.run_to_end()["state"], "passed");
    assert!(
        held.load(Ordering::SeqCst),
        "the slot was recreated and leased"
    );
}

#[test]
fn a_second_infrastructure_failure_is_an_error_with_the_original_tail() {
    let behave: Behave = Arc::new(|command, nth, env| {
        if command == "test" {
            let _ = std::fs::remove_dir_all(target_of(env));
            return failed(if nth == 1 {
                "first tail"
            } else {
                "second tail"
            });
        }
        ok()
    });
    let world = World::new(cfg(&["build", "test"]), behave, nothing);
    let run = world.run_to_end();
    assert_eq!(run["state"], "error", "{run}");
    let tail = run["outputTail"].as_str().unwrap();
    assert!(
        tail.starts_with("infrastructure: the gate build directory was removed during the run"),
        "{tail}"
    );
    assert!(tail.contains("first tail"), "{tail}");
    assert!(world.wait_lease_free(0));
}

#[test]
fn a_rerun_that_fails_with_the_slot_intact_is_an_ordinary_failure() {
    let behave: Behave = Arc::new(|command, nth, env| {
        if command == "test" {
            if nth == 1 {
                let _ = std::fs::remove_dir_all(target_of(env));
            }
            return failed("still red");
        }
        ok()
    });
    let world = World::new(cfg(&["build", "test"]), behave, nothing);
    let run = world.run_to_end();
    assert_eq!(run["state"], "failed", "{run}");
    assert!(
        run["outputTail"]
            .as_str()
            .unwrap()
            .starts_with("after an infrastructure rerun from step 1: still red"),
        "{run}"
    );
}

#[test]
fn a_rerun_past_the_deadline_ends_as_timeout() {
    let behave: Behave = Arc::new(|command, nth, env| {
        if command == "test" && nth == 1 {
            let _ = std::fs::remove_dir_all(target_of(env));
            std::thread::sleep(Duration::from_millis(1300));
            return failed("slow");
        }
        ok()
    });
    let mut c = cfg(&["build", "test"]);
    c.timeout_seconds = Some(1);
    let world = World::new(c, behave, nothing);
    let run = world.run_to_end();
    assert_eq!(run["state"], "timeout", "{run}");
    assert!(world.wait_lease_free(0));
}

#[test]
fn failures_with_the_slot_intact_are_not_retried() {
    for case in ["missing-file", "file-replaced", "new-subdirs"] {
        let behave: Behave = Arc::new(move |command, nth, env| {
            let target = target_of(env);
            if command == "build" && nth == 1 {
                std::fs::create_dir_all(target.join("debug")).unwrap();
                std::fs::write(target.join("top-file"), "x").unwrap();
                return ok();
            }
            if command == "test" {
                match case {
                    "file-replaced" => {
                        let _ = std::fs::remove_file(target.join("top-file"));
                        std::fs::write(target.join("top-file"), "y").unwrap();
                    }
                    "new-subdirs" => std::fs::create_dir_all(target.join("fresh/er")).unwrap(),
                    _ => {}
                }
                return failed("real failure");
            }
            ok()
        });
        let world = World::new(cfg(&["build", "test"]), behave, nothing);
        let run = world.run_to_end();
        assert_eq!(run["state"], "failed", "{case}: {run}");
        assert_eq!(run["outputTail"], "real failure", "{case}");
        assert_eq!(world.commands_run(), ["build", "test"], "{case}");
    }
}

#[test]
fn removing_either_directory_of_the_shared_build_dir_mode_retries() {
    for victim in ["target", "build"] {
        let behave: Behave = Arc::new(move |command, nth, env| {
            if command == "test" && nth == 1 {
                let key = if victim == "target" {
                    "CARGO_TARGET_DIR"
                } else {
                    "CARGO_BUILD_BUILD_DIR"
                };
                let _ = std::fs::remove_dir_all(&env[key]);
                return failed("gone");
            }
            ok()
        });
        let world = World::new(cfg(&["build", "test"]), behave, nothing);
        assert_eq!(world.run_to_end()["state"], "passed", "{victim}");
        assert_eq!(world.commands_run().len(), 4, "{victim}");
        assert!(world.slot_target(0).is_dir() && world.slot_build(0).is_dir());
    }
}

#[test]
fn the_lease_is_free_after_pass_failure_abort_and_panics() {
    // pass and failure
    let behave: Behave =
        Arc::new(|command, _, _| if command == "bad" { failed("no") } else { ok() });
    let world = World::new(cfg(&["good"]), behave.clone(), nothing);
    assert_eq!(world.run_to_end()["state"], "passed");
    assert!(world.wait_lease_free(0));
    let world = World::new(cfg(&["bad"]), behave, nothing);
    assert_eq!(world.run_to_end()["state"], "failed");
    assert!(world.wait_lease_free(0));
    // abort: a step that waits for the abort flag
    let started = Arc::new(AtomicBool::new(false));
    let slow: Behave = {
        let started = started.clone();
        Arc::new(move |_, _, _| {
            started.store(true, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(600));
            ok()
        })
    };
    let world = World::new(cfg(&["slow"]), slow, nothing);
    let id = world.queue();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !started.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!world.lease_free(0), "held while the step runs");
    world.call("operator", "verify", &["cancel", &id]);
    assert_eq!(world.finished(&id)["state"], "interrupted");
    assert!(world.wait_lease_free(0));
    // panics
    for next_run in [true, false] {
        let world = World::new(cfg(&["good"]), passing(), |s| {
            if next_run {
                s.panic_next_run.store(true, Ordering::SeqCst);
            } else {
                s.panic_after_steps.store(true, Ordering::SeqCst);
            }
        });
        let run = world.run_to_end();
        assert_eq!(run["state"], "error", "{run}");
        assert!(world.wait_lease_free(0), "next_run={next_run}");
    }
}

#[test]
fn a_panicking_run_tears_down_under_its_lease_with_the_slot_environment() {
    let out = private_tempdir();
    let env_file = out.path().join("env");
    let marker = out.path().join("marker");
    let mut c = cfg(&["good"]);
    c.more_toml = format!(
        "\n[worktree]\nteardown = \"env > {env}; touch {marker}; sleep 2\"\n",
        env = env_file.display(),
        marker = marker.display()
    );
    let world = World::new(c, passing(), |s| {
        s.panic_after_steps.store(true, Ordering::SeqCst)
    });
    let entry = world.entry_dir();
    let poll = {
        let (marker, entry) = (marker.clone(), entry.clone());
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            while !marker.exists() {
                if Instant::now() > deadline {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Some(GateLease::try_acquire(&entry, PROJECT, 0).is_none())
        })
    };
    let run = world.run_to_end();
    assert_eq!(run["state"], "error", "{run}");
    assert_eq!(
        poll.join().unwrap(),
        Some(true),
        "the lease was held during the teardown"
    );
    let text = std::fs::read_to_string(&env_file).unwrap();
    let want = format!("CARGO_TARGET_DIR={}", world.slot_target(0).display());
    assert!(text.lines().any(|l| l == want), "{text}");
    assert!(world.wait_lease_free(0));
}

#[test]
fn a_recovered_orphans_teardown_gets_the_shared_variables() {
    let out = private_tempdir();
    let env_file = out.path().join("env");
    let mut c = cfg(&["good"]);
    c.more_toml = format!(
        "\n[worktree]\nteardown = \"env > {}\"\n",
        env_file.display()
    );
    let world = World::new(c, passing(), nothing);
    world.service.stop();
    let sha = world.heads["feat/dev-1"].clone();
    let report = world.call("dev-1", "report", &[&sha, "Done"]);
    let queued = world.ledger(
        &world.credentials["operator"].clone(),
        "queueVerifyRun",
        vec![json!({"subjectId": report["reportId"], "commands": ["good"], "commandsSha": "x", "trigger": "manual"})],
    );
    let id = queued["runId"].as_str().unwrap().to_string();
    let scratch_path = world.scratch.path().join(&id);
    capstan_daemon::verify::scratch::add_worktree(world.dir.path(), &scratch_path, &sha).unwrap();
    let log_path = world.scratch.path().join("logs").join(format!("{id}.log"));
    world.ledger(
        world.deps.credential(),
        "claimVerifyRun",
        vec![json!({"runId": id, "maxParallel": 1, "scratchPath": scratch_path.to_string_lossy(), "logPath": log_path.to_string_lossy()})],
    );
    let restarted = VerifyService::with_config(&world.deps, world.service.config().clone());
    restarted.recover();
    let text = std::fs::read_to_string(&env_file).unwrap();
    assert!(!text.contains("CARGO_TARGET_DIR="), "{text}");
    assert!(!text.contains("CARGO_BUILD_BUILD_DIR="), "{text}");
    assert!(text.contains("PIP_CACHE_DIR="), "{text}");
}

#[test]
fn a_busy_slot_ends_the_run_as_error_and_the_worker_takes_the_next_run() {
    let world = World::new(cfg(&["good"]), passing(), nothing);
    // Another description holds slot 0 (the run asks for it at once).
    let entry = world.entry_dir();
    std::fs::create_dir_all(&entry).unwrap();
    let held = GateLease::acquire(&entry, PROJECT, 0).unwrap();
    let run = world.run_to_end();
    assert_eq!(run["state"], "error", "{run}");
    assert!(
        run["outputTail"]
            .as_str()
            .unwrap()
            .contains("slot 0 is held by another process"),
        "{run}"
    );
    drop(held);
    assert_eq!(world.run_to_end()["state"], "passed");
}

#[test]
fn an_abort_during_the_lease_wait_ends_the_run_quickly() {
    let world = World::new(cfg(&["good"]), passing(), |s| {
        s.set_lease_wait(Duration::from_secs(10))
    });
    let entry = world.entry_dir();
    std::fs::create_dir_all(&entry).unwrap();
    let _held = GateLease::acquire(&entry, PROJECT, 0).unwrap();
    let id = world.queue();
    let deadline = Instant::now() + Duration::from_secs(30);
    while world.show(&id)["state"] != "running" {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let asked = Instant::now();
    world.call("operator", "verify", &["cancel", &id]);
    let run = world.finished(&id);
    assert!(
        asked.elapsed() < Duration::from_secs(2),
        "took {:?}",
        asked.elapsed()
    );
    assert_eq!(run["state"], "interrupted", "{run}");
    assert!(world.commands_run().is_empty());
}

fn assert_shared_env(world: &World) {
    let env = world.env_of(0);
    assert!(env.contains_key("PIP_CACHE_DIR"), "{env:?}");
    assert!(!env.contains_key("CARGO_TARGET_DIR"), "{env:?}");
    assert!(!env.contains_key("CARGO_BUILD_BUILD_DIR"), "{env:?}");
    assert!(
        !capstan_cache::lease::lease_path(&world.entry_dir(), PROJECT, 0).exists(),
        "no lease file"
    );
}

#[test]
fn scopes_without_a_lease() {
    // The cache off: no cache variable and no lease file.
    let mut c = cfg(&["good"]);
    c.cache = None;
    let world = World::new(c, passing(), nothing);
    assert_eq!(world.run_to_end()["state"], "passed");
    let env = world.env_of(0);
    assert!(
        !env.keys()
            .any(|k| k.starts_with("CARGO_") || k == "PIP_CACHE_DIR" || k == "CAPSTAN_CACHE_DIR"),
        "{env:?}"
    );
    // No entry has gate_dirs (python only): the Agent environment, no lease file.
    let mut c = cfg(&["good"]);
    c.cache = Some("presets = [\"python\"]\nmax_size_gb = 1\nmax_age_days = 30\n".into());
    let world = World::new(c, passing(), nothing);
    assert_eq!(world.run_to_end()["state"], "passed");
    let expected: BTreeMap<String, String> =
        capstan_launcher::cache_env::cache_extras(&world.runtime(), GateScope::Agent)
            .unwrap()
            .into_iter()
            .collect();
    let env = world.env_of(0);
    for (k, v) in &expected {
        assert_eq!(env.get(k), Some(v), "{k}");
    }
    let leases = std::fs::read_dir(world.runtime().root.clone())
        .unwrap()
        .flatten()
        .flat_map(|e| std::fs::read_dir(e.path()).into_iter().flatten().flatten())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".lease"))
        .count();
    assert_eq!(leases, 0);
    // self_test false: Shared, no lease; the sweep removes nothing.
    let world = World::new(cfg(&["good"]), passing(), |s| {
        s.set_self_test_override(Some(false))
    });
    assert_eq!(world.run_to_end()["state"], "passed");
    assert_shared_env(&world);
    assert_eq!(world.logged("cache_lease_unreliable").len(), 1);
    // A lease I/O failure: Shared and logged.
    let world = World::new(cfg(&["good"]), passing(), |s| {
        s.set_lease_fault(LeaseFault::LeaseIo)
    });
    assert_eq!(world.run_to_end()["state"], "passed");
    assert_shared_env(&world);
    assert_eq!(world.logged("verify_lease_unavailable").len(), 1);
}

/// Creates slot `n` of this project under the rust entry, with a lease file and a file in each directory.
fn make_slot(entry: &Path, project: &str, n: usize) {
    for sub in ["gate-target", "gate-build"] {
        let dir = entry.join(sub).join(project).join(format!("slot-{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f"), "data").unwrap();
    }
    drop(GateLease::acquire(entry, project, n).unwrap());
}

fn prepared(
    setup: impl FnOnce(&Arc<VerifyService>),
    prepare: impl FnOnce(&Path) + 'static,
) -> World {
    let mut c = cfg(&["good"]);
    c.max_parallel = Some(2);
    c.prepare = Some(Box::new(prepare));
    World::new(c, passing(), setup)
}

#[test]
fn the_start_up_sweep_removes_unheld_slots_beyond_max_parallel_and_skips_held_ones() {
    let held: Arc<Mutex<Option<GateLease>>> = Arc::default();
    let world = prepared(nothing, {
        let held = held.clone();
        move |entry: &Path| {
            make_slot(entry, PROJECT, 3);
            make_slot(entry, PROJECT, 4);
            make_slot(entry, "otherproj", 3);
            *held.lock().unwrap() = Some(GateLease::acquire(entry, PROJECT, 4).unwrap());
        }
    });
    let entry = world.entry_dir();
    let deadline = Instant::now() + Duration::from_secs(30);
    while world.logged("verify_slot_removed").is_empty()
        || world.logged("verify_slot_skipped").is_empty()
    {
        assert!(Instant::now() < deadline, "no sweep logged");
        std::thread::sleep(Duration::from_millis(20));
    }
    let removed = world.logged("verify_slot_removed");
    assert!(removed.iter().all(|d| d["slot"] == 3), "{removed:?}");
    assert!(removed[0]["bytes"].as_u64().unwrap() >= 8, "{removed:?}");
    let skipped = world.logged("verify_slot_skipped");
    assert!(skipped.iter().all(|d| d["slot"] == 4), "{skipped:?}");
    assert!(!entry
        .join("gate-target")
        .join(PROJECT)
        .join("slot-3")
        .exists());
    assert!(entry
        .join("gate-target")
        .join(PROJECT)
        .join("slot-4")
        .exists());
    // Another project's slot 3 is untouched and never logged.
    assert!(entry
        .join("gate-target")
        .join("otherproj")
        .join("slot-3")
        .exists());
    drop(held);
}

#[test]
fn an_unreliable_lease_test_removes_nothing_and_a_sweep_error_is_logged() {
    let world = prepared(
        |s| s.set_self_test_override(Some(false)),
        |entry| make_slot(entry, PROJECT, 3),
    );
    std::thread::sleep(Duration::from_millis(400));
    assert!(world.logged("verify_slot_removed").is_empty());
    assert!(world
        .entry_dir()
        .join("gate-target")
        .join(PROJECT)
        .join("slot-3")
        .exists());
    assert_eq!(world.logged("cache_lease_unreliable").len(), 1);

    let world = prepared(
        |s| s.set_lease_fault(LeaseFault::RemoveSlotError),
        |entry| make_slot(entry, PROJECT, 3),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while world.logged("verify_slot_cleanup_failed").is_empty() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    // Start-up continued: a run still works.
    assert_eq!(world.run_to_end()["state"], "passed");
}
