//! A running verify's scratch counts toward the cache cap, and a cycle follows each run. Steps run through a fake step
//! runner that blocks until the test releases it; the cache loop runs on a test clock.

mod common;

use capstan_cache::Runtime;
use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::cache::{runtime_of_deps, status_member};
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::shared::AbortSignal;
use capstan_daemon::handlers::MAX_RESPONSE_BYTES;
use capstan_daemon::loops::cache::{run_with_trigger, CacheTrigger, Timing};
use capstan_daemon::loops::driver::DriverLog;
use capstan_daemon::loops::StopFlag;
use capstan_daemon::verify::{StepRequest, StepRunner, VerifyService};
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
use std::fs::File;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

const PROJECT: &str = "pscratch";
const GB: u64 = 1 << 30;
const MIN: u64 = 60_000;
const START_MS: u64 = 10_000_000_000;

static WORLDS: AtomicUsize = AtomicUsize::new(0);

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

/// The gate: steps wait here until released.
#[derive(Default)]
struct Gate {
    open: Mutex<bool>,
}

struct World {
    dir: common::PrivateDir,
    cache_root: common::PrivateDir,
    scratch: common::PrivateDir,
    deps: Deps,
    service: Arc<VerifyService>,
    credentials: HashMap<String, String>,
    heads: BTreeMap<String, String>,
    gate: Arc<Gate>,
    clock: Arc<AtomicU64>,
    trigger: Arc<CacheTrigger>,
}

impl World {
    /// `with_cache`: a `[cache]` section (cap 1 GB); `set_trigger`: the service gets the trigger.
    fn new(with_cache: bool, set_trigger: bool) -> World {
        let dir = private_tempdir();
        let cache_root = private_tempdir();
        let scratch = private_tempdir();
        let identity = Identity {
            project_id: PROJECT.into(),
            name: "Scratch".into(),
            credential: format!("operator-{}", "s".repeat(40)),
        };
        let state = write_project(dir.path(), &identity);
        let mut config = CONFIG.to_string();
        if with_cache {
            config.push_str(&format!(
                "\n[cache]\nroot = \"{}\"\npresets = [\"python\"]\nmax_size_gb = 1\nmax_age_days = 30\n",
                cache_root.path().display()
            ));
        }
        config.push_str(&format!(
            "\n[verify]\ncommands = [\"gate\"]\nscratch_dir = \"{}\"\n",
            scratch.path().display()
        ));
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
                        "verify-scratch-{}-{}",
                        std::process::id(),
                        WORLDS.fetch_add(1, Ordering::SeqCst)
                    )),
                )
            })
            .expect("the kernel opens")
        };
        let sink: Logger = Arc::new(|_: &LogEntry| {});
        let mut options = DaemonOptions::new(state, project, dir.path().to_path_buf());
        options.trust_store = trust.clone();
        let role_config = capstan_config::load_role_config_with(dir.path(), &trust).unwrap();
        let verify = role_config.verify.clone().expect("a [verify] section");
        options.capstan = Some(Arc::new(role_config));
        let mut deps = Deps::new(kernel, options, sink);
        deps.git = Arc::new(ReplayGit::new(dir.path()));
        deps.launcher = Some(Arc::new(StubLauncher::new()) as Arc<dyn LauncherService>);
        capstan_daemon::deps::startup_sequence(&deps);
        let service = VerifyService::with_config(&deps, verify);
        deps.verify = Some(service.clone());
        let clock = Arc::new(AtomicU64::new(START_MS));
        let trigger = {
            let clock = clock.clone();
            CacheTrigger::with_clock(Arc::new(move || clock.load(Ordering::SeqCst)))
        };
        if set_trigger {
            service.set_cache_trigger(trigger.clone());
        }
        let gate: Arc<Gate> = Arc::default();
        let runner: StepRunner = {
            let gate = gate.clone();
            Arc::new(move |request: StepRequest| {
                let end = Instant::now() + Duration::from_secs(60);
                while !*gate.open.lock().unwrap()
                    && !request.abort.load(Ordering::SeqCst)
                    && Instant::now() < end
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                RunResult {
                    status: RunStatus::Ok,
                    exit_code: Some(0),
                    signal: None,
                    duration_ms: 1,
                    output_tail: "ok".into(),
                    truncated: false,
                }
            })
        };
        service.set_step_runner(runner);
        service.set_lease_wait(Duration::from_millis(50));
        let mut world = World {
            dir,
            cache_root,
            scratch,
            deps,
            service,
            credentials: HashMap::new(),
            heads,
            gate,
            clock,
            trigger,
        };
        world
            .credentials
            .insert("operator".into(), identity.credential);
        world.populate();
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

    fn queue(&self) -> String {
        let sha = self.heads["feat/dev-1"].clone();
        let report = self.call("dev-1", "report", &[&sha, "Done"]);
        let id = report["reportId"].as_str().unwrap().to_string();
        self.call("operator", "verify", &[&id])["runId"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Queues a run and waits until its scratch directory exists; returns the run id and the directory.
    fn start_run(&self) -> (String, PathBuf) {
        *self.gate.open.lock().unwrap() = false;
        let id = self.queue();
        let dir = self.scratch.path().join(&id);
        wait_for("the run dir", || dir.is_dir());
        (id, dir)
    }

    /// Opens the gate and waits for the run to end.
    fn finish(&self, id: &str) {
        *self.gate.open.lock().unwrap() = true;
        wait_for("the run to end", || {
            let state = self.call("operator", "verify", &["show", id])["state"]
                .as_str()
                .unwrap()
                .to_string();
            state != "queued" && state != "running"
        });
    }

    fn advance(&self, ms: u64) {
        self.clock.fetch_add(ms, Ordering::SeqCst);
    }

    fn runtime(&self) -> Runtime {
        runtime_of_deps(&self.deps).expect("a cache runtime")
    }

    /// The cache loop on its own thread: no periodic cycle for an hour, so only the trigger causes one.
    fn loop_thread(
        &self,
        runtime: Runtime,
        log: DriverLog,
    ) -> (StopFlag, std::thread::JoinHandle<()>) {
        let stop = StopFlag::new();
        let handle = {
            let (stop, trigger) = (stop.clone(), self.trigger.clone());
            std::thread::spawn(move || {
                let timing = Timing {
                    first_delay: Duration::from_secs(3600),
                    interval: Duration::from_secs(3600),
                };
                run_with_trigger(&runtime, &stop, timing, &log, &trigger)
            })
        };
        (stop, handle)
    }

    fn wait_cycles(&self, count: u64) {
        wait_for("a cache cycle", || self.trigger.cycles() >= count);
    }
}

impl Drop for World {
    fn drop(&mut self) {
        *self.gate.open.lock().unwrap() = true;
        self.service.stop();
    }
}

/// Polls for up to 5 s (a minute when the wait is for a run).
fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let limit = if what.contains("run") { 60 } else { 5 };
    let end = Instant::now() + Duration::from_secs(limit);
    while !done() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn big_file(dir: &Path, bytes: u64) {
    File::create(dir.join("big"))
        .unwrap()
        .set_len(bytes)
        .unwrap();
}

fn quiet() -> DriverLog {
    Arc::new(|_, _| {})
}

#[test]
fn a_running_runs_scratch_is_measured_counted_and_a_run_end_triggers_a_cycle() {
    let world = World::new(true, true);
    let runtime = world.runtime();
    let (stop, handle) = world.loop_thread(runtime.clone(), quiet());
    let (id, dir) = world.start_run();
    big_file(&dir, 3 * GB);
    assert_eq!(world.service.scratch_occupants(), vec![]);
    world.advance(2 * MIN);
    let label = format!("verify/{id}");
    wait_for("the first refresh", || {
        world
            .service
            .scratch_occupants()
            .iter()
            .any(|o| o.label == label && o.bytes >= 3 * GB)
    });
    // The measurer saw the store plus the occupants over the 1 GB cap and fired: one cycle writes the manifest.
    world.wait_cycles(1);
    let member = status_member(&runtime);
    assert_eq!(member["occupants"][0]["label"], label.as_str(), "{member}");
    assert!(member["occupants"][0]["bytes"].as_u64().unwrap() >= 3 * GB);
    // The daemon's cache route counts it toward the cap.
    let dry = world.call("pm-1", "cache", &["prune", "--dry-run"]);
    assert!(
        dry["skipped"]
            .as_array()
            .unwrap()
            .contains(&json!({"label": label, "reason": "held by verify"})),
        "{dry}"
    );
    // The stored value is served without touching the disk.
    big_file(&dir, 0);
    assert!(world.service.scratch_occupants()[0].bytes >= 3 * GB);
    // A run end triggers a cycle once five minutes passed since the last one.
    world.advance(5 * MIN);
    let before = world.trigger.cycles();
    world.finish(&id);
    world.wait_cycles(before + 1);
    wait_for("the measurer to end", || {
        world.service.measurer_threads() == 0
    });
    assert_eq!(world.service.scratch_occupants(), vec![]);
    stop.stop();
    handle.join().unwrap();
}

#[test]
fn run_ends_are_debounced_to_one_cycle_per_five_minutes() {
    let world = World::new(true, true);
    let (stop, handle) = world.loop_thread(world.runtime(), quiet());
    let (a, _) = world.start_run();
    world.finish(&a);
    world.wait_cycles(1);
    let (b, _) = world.start_run();
    world.finish(&b);
    // Within five minutes of the test clock the second end only stays pending (a negative check: a short wait).
    std::thread::sleep(Duration::from_millis(2500));
    assert_eq!(world.trigger.cycles(), 1);
    world.advance(5 * MIN);
    world.wait_cycles(2);
    let (c, _) = world.start_run();
    world.advance(5 * MIN);
    world.finish(&c);
    world.wait_cycles(3);
    stop.stop();
    handle.join().unwrap();
}

#[test]
fn without_a_trigger_nothing_is_measured_and_a_run_end_does_nothing() {
    let world = World::new(false, false);
    let (id, dir) = world.start_run();
    big_file(&dir, 3 * GB);
    world.advance(10 * MIN);
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(world.service.measurer_threads(), 0);
    assert_eq!(world.service.scratch_occupants(), vec![]);
    world.finish(&id);
    assert_eq!(world.trigger.cycles(), 0);
    assert_eq!(
        world.trigger.last_store_total.load(Ordering::SeqCst),
        u64::MAX
    );
}

#[test]
fn no_measurer_remains_after_stop() {
    let world = World::new(true, true);
    let (_id, _dir) = world.start_run();
    wait_for("a measurer", || world.service.measurer_threads() == 1);
    world.service.stop();
    assert_eq!(world.service.measurer_threads(), 0);
}

fn git_in(directory: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .output()
        .expect("git runs");
    assert!(output.status.success(), "git {args:?}");
}

#[test]
fn with_the_kill_switch_off_a_triggered_cycle_removes_nothing_and_lists_the_victims() {
    let world = World::new(true, true);
    let mut runtime = world.runtime();
    runtime.evict_while_active = false;
    let entry = runtime.root.join("python");
    std::fs::create_dir_all(&entry).unwrap();
    File::create(entry.join("blob"))
        .unwrap()
        .set_len(2 * GB)
        .unwrap();
    File::create(entry.join(".last-used"))
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(5 * 86_400))
        .unwrap();
    // An agent's linked worktree: the project is not idle.
    let worktree = world.cache_root.path().join("wt");
    git_in(
        world.dir.path(),
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "feat/agent",
            worktree.to_str().unwrap(),
        ],
    );
    let seen: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
    let log: DriverLog = {
        let seen = seen.clone();
        Arc::new(move |event, details| seen.lock().unwrap().push((event.to_string(), details)))
    };
    let (stop, handle) = world.loop_thread(runtime, log);
    let (id, _dir) = world.start_run();
    world.finish(&id);
    world.wait_cycles(1);
    assert!(entry.is_dir(), "nothing was removed");
    let seen = seen.lock().unwrap();
    let pruned = seen
        .iter()
        .find(|(e, _)| e == "cache_pruned")
        .expect("a cycle");
    assert_eq!(pruned.1["idle"], false);
    assert_eq!(pruned.1["cleared"], json!([]));
    assert_eq!(pruned.1["wouldClear"], json!(["python"]), "{:?}", pruned.1);
    drop(seen);
    stop.stop();
    handle.join().unwrap();
}

#[test]
fn a_fire_during_a_running_cycle_stays_pending_and_runs_a_follow_up_cycle() {
    let world = World::new(true, true);
    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    // The log call happens inside the cycle: fire once from there.
    let log: DriverLog = {
        let (trigger, fired) = (world.trigger.clone(), fired.clone());
        Arc::new(move |event, _| {
            if event == "cache_pruned" && !fired.swap(true, Ordering::SeqCst) {
                trigger.fire();
            }
        })
    };
    let (stop, handle) = world.loop_thread(world.runtime(), log);
    world.trigger.fire();
    world.wait_cycles(1);
    assert!(fired.load(Ordering::SeqCst));
    world.advance(5 * MIN);
    world.wait_cycles(2);
    stop.stop();
    handle.join().unwrap();
}
