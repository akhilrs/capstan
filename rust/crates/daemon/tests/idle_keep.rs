//! `cstan keep` and the idle facts of `cstan status`, through `handlers::handle_frame` like a connection, on a real ledger
//! with a test clock: who may keep which agent, what a keep does to the idle-release loop, the status members of an idle,
//! a warned, a kept, a busy and a worker with open work, and a restart of the kernel.

mod common;

use capstan_daemon::deps::{new_context, DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::shared::AbortSignal;
use capstan_daemon::handlers::MAX_RESPONSE_BYTES;
use capstan_daemon::loops::driver::Steps;
use capstan_daemon::loops::idle_release::{IdleRelease, IdleReleaseOptions, ToolProcessSampler};
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::Core;
use capstan_launcher::api::StubLauncher;
use common::project::{write_project, Identity};
use common::{private_tempdir, PrivateDir};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
const BASE: i64 = 1_800_000_000_000;
const IDLE: i64 = 120;

const CONFIG: &str = r#"schema_version = 1

[hosts.claude]
kind = "claude"

[roles.pm]
kind = "PM"
host = "claude"

[roles.developer]
kind = "Developer"
host = "claude"

[roles.supervisor]
kind = "Supervisor"
host = "claude"
"#;

const AGENTS: [(&str, &str, &str); 5] = [
    ("pm-1", "pm", "PM"),
    ("sup-1", "supervisor", "Supervisor"),
    ("dev-1", "developer", "Developer"),
    ("dev-2", "developer", "Developer"),
    ("dev-3", "developer", "Developer"),
];

struct TestEnv {
    clock: Arc<AtomicI64>,
    inner: capstan_kernel::SeededEnv,
}

impl capstan_kernel::Env for TestEnv {
    fn now(&self) -> i64 {
        self.clock.load(Ordering::SeqCst)
    }
    fn uuid(&self) -> String {
        self.inner.uuid()
    }
    fn random_bytes(&self, n: usize) -> Vec<u8> {
        self.inner.random_bytes(n)
    }
}

struct NoProcess;

impl ToolProcessSampler for NoProcess {
    fn tool_process_alive(&self, _agent_id: &str) -> Result<bool, String> {
        Ok(false)
    }
}

struct World {
    _dir: PrivateDir,
    state: PathBuf,
    root: PathBuf,
    deps: Option<Deps>,
    trust: capstan_config::TrustStore,
    clock: Arc<AtomicI64>,
    credentials: HashMap<String, String>,
}

fn open_kernel(
    state: &std::path::Path,
    root: &std::path::Path,
    clock: &Arc<AtomicI64>,
    identity: &Identity,
) -> KernelHandle {
    let (state, root, clock) = (state.to_path_buf(), root.to_path_buf(), Arc::clone(clock));
    let project = identity.initial_project();
    KernelHandle::spawn(move || {
        Core::open(
            &state,
            &project,
            &KernelOptions {
                workspace_root: Some(root),
                ..KernelOptions::default()
            },
            Box::new(TestEnv {
                clock,
                inner: capstan_kernel::SeededEnv::new("idle-keep"),
            }),
        )
    })
    .expect("the kernel opens")
}

fn deps_for(
    kernel: KernelHandle,
    state: &std::path::Path,
    root: &std::path::Path,
    identity: &Identity,
    trust: &capstan_config::TrustStore,
) -> Deps {
    let sink: Logger = Arc::new(|_: &LogEntry| {});
    let mut options = DaemonOptions::new(
        state.to_path_buf(),
        identity.initial_project(),
        root.to_path_buf(),
    );
    options.trust_store = trust.clone();
    options.capstan = Some(Arc::new(
        capstan_config::load_role_config_with(root, trust).unwrap(),
    ));
    let deps = Deps::new(kernel, options, sink);
    capstan_daemon::deps::startup_sequence(&deps);
    deps
}

fn iso(ms: i64) -> String {
    capstan_ledger::iso_from_millis(ms)
}

impl World {
    /// `agents_section` is the text of the `[agents]` section (empty for the default).
    fn new(agents_section: &str) -> World {
        let dir = private_tempdir();
        let identity = Identity {
            project_id: "pkeep".into(),
            name: "Keep".into(),
            credential: format!("operator-{}", "k".repeat(40)),
        };
        let state = write_project(dir.path(), &identity);
        let file = dir.path().join("capstan.toml");
        std::fs::write(&file, format!("{CONFIG}\n{agents_section}\n")).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let trust = common::kept_trust_store();
        common::trusted_project(dir.path(), &trust);
        let database = capstan_ledger::resolve_database_path(&state).unwrap();
        capstan_ledger::open_database(&database, &capstan_ledger::OpenOptions::default()).unwrap();
        let clock = Arc::new(AtomicI64::new(BASE));
        let root = dir.path().to_path_buf();
        let kernel = open_kernel(&state, &root, &clock, &identity);
        let deps = deps_for(kernel, &state, &root, &identity, &trust);
        let mut world = World {
            _dir: dir,
            state,
            root,
            deps: Some(deps),
            trust,
            clock,
            credentials: HashMap::new(),
        };
        world
            .credentials
            .insert("operator".into(), identity.credential);
        world.populate();
        world
    }

    fn ledger(&self, op: &str, args: Vec<Value>) -> Value {
        let (credential, name) = (self.credentials["operator"].clone(), op.to_string());
        self.deps()
            .kernel
            .run(move |core| {
                let context = new_context(core, &credential)?;
                let mut all = vec![serde_json::to_value(context).unwrap()];
                all.extend(args);
                core.dispatch(&name, &all)
            })
            .unwrap_or_else(|e| panic!("{op}: {e}"))
    }

    fn populate(&mut self) {
        let roles: Vec<Value> = [("pm", "PM"), ("developer", "Developer"), ("supervisor", "Supervisor")]
            .iter()
            .map(|(name, kind)| {
                json!({"name": name, "kind": kind, "host": "claude", "configHash": "a".repeat(64)})
            })
            .collect();
        self.ledger("syncRoleDefinitions", vec![json!(roles)]);
        for (agent, role, kind) in AGENTS {
            let seat = format!("seat-{agent}");
            self.ledger(
                "createSeat",
                vec![json!({"seatId": seat, "name": seat, "role": kind})],
            );
            let actor = self.ledger(
                "createActor",
                vec![json!({"displayName": agent, "role": kind, "seatId": seat})],
            );
            self.ledger(
                "registerAgent",
                vec![json!({"agentId": agent, "roleName": role, "seatId": seat, "actorId": actor["actorId"]})],
            );
            self.credentials
                .insert(agent.into(), actor["credential"].as_str().unwrap().into());
        }
    }

    fn deps(&self) -> &Deps {
        self.deps.as_ref().expect("deps")
    }

    fn now(&self) -> i64 {
        self.clock.load(Ordering::SeqCst)
    }

    fn advance(&self, ms: i64) {
        self.clock.fetch_add(ms, Ordering::SeqCst);
    }

    fn call(&self, who: &str, command: &str, args: &[&str]) -> Value {
        let credential = self.credentials[who].clone();
        let frame = json!({"v": 1, "credential": credential, "command": command, "args": args});
        let outcome = handle_frame(
            self.deps(),
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

    fn refused(&self, who: &str, command: &str, args: &[&str], code: &str, text: &str) {
        let answer = self.call(who, command, args);
        assert_eq!(answer["ok"], false, "{command} {args:?}: {answer}");
        assert_eq!(answer["code"], code, "{command} {args:?}: {answer}");
        let message = answer["message"].as_str().unwrap();
        assert!(message.starts_with(text), "{message}");
    }

    /// The status agent row of `agent`.
    fn agent(&self, agent: &str) -> Value {
        let status = self.ok("operator", "status", &[]);
        status["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["agentId"] == agent)
            .unwrap_or_else(|| panic!("{agent} in status"))
            .clone()
    }

    fn observe(&self, agent: &str, state: &str) {
        let (credential, agent, state) = (
            self.credentials["operator"].clone(),
            agent.to_string(),
            state.to_string(),
        );
        self.deps()
            .kernel
            .run(move |core| {
                let context = new_context(core, &credential)?;
                core.record_agent_observation(&context, &agent, &json!(state))
            })
            .unwrap();
    }

    fn idle_loop(&self, launcher: Arc<StubLauncher>) -> IdleRelease {
        let clock = Arc::clone(&self.clock);
        IdleRelease::new(IdleReleaseOptions {
            steps: Steps::new(
                self.deps().kernel.clone(),
                self.deps().credential().to_string(),
            ),
            launcher,
            idle_minutes: IDLE,
            warn_minutes: 10,
            now: Arc::new(move || clock.load(Ordering::SeqCst)),
            log: Arc::new(|_, _| {}),
            child_activity: Arc::new(Mutex::new(HashMap::new())),
            sampler: Arc::new(NoProcess),
        })
    }

    fn pm_notes(&self) -> Vec<String> {
        let messages = self
            .deps()
            .kernel
            .run(|core| core.messages_for("pm-1"))
            .unwrap();
        messages
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["body"].as_str().unwrap().to_string())
            .collect()
    }

    /// Closes the kernel and opens it again on the same ledger, as a restart of the daemon does.
    fn restart(&mut self) {
        let identity = Identity {
            project_id: "pkeep".into(),
            name: "Keep".into(),
            credential: self.credentials["operator"].clone(),
        };
        drop(self.deps.take());
        let fresh = open_kernel(&self.state, &self.root, &self.clock, &identity);
        self.deps = Some(deps_for(
            fresh,
            &self.state,
            &self.root,
            &identity,
            &self.trust,
        ));
    }
}

fn members(agent: &Value) -> [bool; 5] {
    [
        "idleSince",
        "warnedAt",
        "autoReleaseAt",
        "keptUntil",
        "openWork",
    ]
    .map(|k| agent.get(k).is_some())
}

#[test]
fn status_carries_the_idle_members_of_each_kind_of_worker() {
    let w = World::new("[agents]\nidle_release_minutes = 120\n");
    w.advance(200 * MINUTE);
    // dev-1 idle, dev-2 busy, dev-3 holding a message it has not read.
    w.observe("dev-2", "working");
    w.ok("pm-1", "send", &["dev-3", "start on the task"]);
    let idle = w.agent("dev-1");
    assert_eq!(idle["idleSince"], iso(BASE));
    assert_eq!(idle["autoReleaseAt"], iso(BASE + IDLE * MINUTE));
    assert_eq!(idle["keptUntil"], Value::Null);
    assert_eq!(idle["warnedAt"], Value::Null);
    assert_eq!(idle["openWork"], json!([]));
    let busy = w.agent("dev-2");
    assert_eq!(busy["idleSince"], Value::Null);
    assert_eq!(busy["autoReleaseAt"], Value::Null);
    let holding = w.agent("dev-3");
    assert_eq!(holding["autoReleaseAt"], Value::Null);
    assert_eq!(holding["openWork"].as_array().unwrap().len(), 1);
    // The PM is no worker: no members.
    assert_eq!(members(&w.agent("pm-1")), [false; 5]);

    // Kept: the keep time shows and nothing is scheduled.
    let kept = w.ok("pm-1", "keep", &["dev-1", "3"]);
    assert_eq!(kept["until"], iso(w.now() + 3 * HOUR));
    let row = w.agent("dev-1");
    assert_eq!(row["keptUntil"], iso(w.now() + 3 * HOUR));
    assert_eq!(row["autoReleaseAt"], Value::Null);
    // Cleared: back to the schedule.
    w.ok("operator", "keep", &["dev-1", "0"]);
    let row = w.agent("dev-1");
    assert_eq!(row["keptUntil"], Value::Null);
    assert_eq!(row["autoReleaseAt"], iso(BASE + IDLE * MINUTE));
    // The default is 24 hours.
    let kept = w.ok("pm-1", "keep", &["dev-1"]);
    assert_eq!(kept["until"], iso(w.now() + 24 * HOUR));
}

#[test]
fn a_warned_worker_shows_its_release_time() {
    let w = World::new("");
    w.advance(200 * MINUTE);
    let release_at = iso(w.now() + 10 * MINUTE);
    let (credential, input) = (
        w.credentials["operator"].clone(),
        json!({"agentId": "dev-1", "role": "developer", "idleSince": iso(BASE), "releaseAt": release_at}),
    );
    w.deps()
        .kernel
        .run(move |core| {
            let context = new_context(core, &credential)?;
            core.record_idle_warning(&context, &input)
        })
        .unwrap();
    let row = w.agent("dev-1");
    assert_eq!(row["autoReleaseAt"], release_at);
    assert!(row["warnedAt"].is_string());
}

#[test]
fn nothing_is_added_while_the_release_is_off() {
    let w = World::new("[agents]\nidle_release_minutes = 0\n");
    w.advance(200 * MINUTE);
    assert_eq!(members(&w.agent("dev-1")), [false; 5]);
}

#[test]
fn every_refusal_has_its_code_and_text() {
    let w = World::new("");
    w.refused(
        "dev-1",
        "keep",
        &["dev-2"],
        "forbidden",
        "not_allowed: only the PM or the operator",
    );
    w.refused("sup-1", "keep", &["dev-2"], "forbidden", "not_allowed:");
    w.refused(
        "pm-1",
        "keep",
        &["nobody"],
        "unknown_agent",
        "unknown_agent: nobody",
    );
    w.refused("pm-1", "keep", &["pm-1"], "rejected", "not_a_worker: pm-1");
    w.refused(
        "pm-1",
        "keep",
        &["sup-1"],
        "rejected",
        "not_a_worker: sup-1",
    );
    for hours in ["169", "-1", "x", "1.5"] {
        w.refused(
            "pm-1",
            "keep",
            &["dev-1", hours],
            "invalid_request",
            "bad_hours:",
        );
    }
    w.refused("pm-1", "keep", &[], "invalid_request", "keep takes");
    w.refused(
        "pm-1",
        "keep",
        &["dev-1", "1", "2"],
        "invalid_request",
        "keep takes",
    );
    // The boundaries are fine.
    w.ok("pm-1", "keep", &["dev-1", "1"]);
    w.ok("pm-1", "keep", &["dev-1", "168"]);
}

#[test]
fn a_keep_blocks_a_due_release_and_ends_the_warned_episode() {
    let w = World::new("");
    let launcher = Arc::new(StubLauncher::new());
    let mut idle = w.idle_loop(launcher.clone());
    let released = |launcher: &StubLauncher, agent: &str| {
        launcher
            .calls()
            .iter()
            .filter(|c| c.starts_with(&format!("release {agent}")))
            .count()
    };
    // The workers are in their grace for 120 minutes; the warning goes out when it ends.
    for _ in 0..125 {
        w.advance(MINUTE);
        idle.tick();
    }
    let warned = |w: &World, agent: &str| {
        w.pm_notes()
            .iter()
            .filter(|n| n.starts_with(agent) && n.contains("has been idle since"))
            .count()
    };
    for agent in ["dev-1", "dev-2", "dev-3"] {
        assert_eq!(warned(&w, agent), 1, "{agent}");
    }
    // dev-1 and dev-2 are kept during the warning (by the PM and by the operator); dev-3 is not.
    w.ok("pm-1", "keep", &["dev-1", "24"]);
    w.ok("operator", "keep", &["dev-2", "24"]);
    for _ in 0..30 {
        w.advance(MINUTE);
        idle.tick();
    }
    // The stub launcher does not end the agent in the ledger, so the loop may release it again.
    assert!(released(&launcher, "dev-3") >= 1, "{:?}", launcher.calls());
    assert_eq!(released(&launcher, "dev-1"), 0);
    assert_eq!(released(&launcher, "dev-2"), 0);
    let notes = w.pm_notes();
    assert!(notes.iter().any(|n| n.starts_with("dev-3 was released")));
    assert!(!notes.iter().any(|n| n.starts_with("dev-1 was released")));
    assert!(!notes.iter().any(|n| n.starts_with("dev-2 was released")));
}

#[test]
fn a_keep_survives_a_restart() {
    let mut w = World::new("");
    w.advance(200 * MINUTE);
    let kept = w.ok("pm-1", "keep", &["dev-1", "5"]);
    w.restart();
    let row = w.agent("dev-1");
    assert_eq!(row["keptUntil"], kept["until"]);
    assert_eq!(row["autoReleaseAt"], Value::Null);
}

/// A driver that reports child-process activity of dev-1 at a fixed time.
struct BusyBuild(i64);

impl capstan_daemon::deps::DriverView for BusyBuild {
    fn snapshot(&self) -> capstan_daemon::deps::DriverSnapshot {
        capstan_daemon::deps::DriverSnapshot {
            child_activity: vec![("dev-1".to_string(), self.0)],
            ..Default::default()
        }
    }
}

#[test]
fn recent_child_activity_moves_the_idle_time_and_the_release() {
    let mut w = World::new("[daemon]\nstatus_cache_ms = 0\n");
    w.advance(200 * MINUTE);
    // Without the driver's history the worker looks due long ago.
    assert_eq!(w.agent("dev-1")["autoReleaseAt"], iso(BASE + IDLE * MINUTE));
    let recent = w.now() - 5 * MINUTE;
    w.deps.as_mut().unwrap().driver = Arc::new(BusyBuild(recent));
    let row = w.agent("dev-1");
    assert_eq!(row["idleSince"], iso(recent));
    // Not due: the release is a full idle period after the build's last sign of life.
    assert_eq!(row["autoReleaseAt"], iso(recent + IDLE * MINUTE));
    // A worker without child activity is unchanged.
    assert_eq!(w.agent("dev-2")["autoReleaseAt"], iso(BASE + IDLE * MINUTE));
}

#[test]
fn a_cleared_keep_needs_a_new_warning_before_a_release() {
    let w = World::new("");
    let launcher = Arc::new(StubLauncher::new());
    let mut idle = w.idle_loop(launcher.clone());
    for _ in 0..125 {
        w.advance(MINUTE);
        idle.tick();
    }
    let warnings = |w: &World| {
        w.pm_notes()
            .iter()
            .filter(|n| n.starts_with("dev-1") && n.contains("has been idle since"))
            .count()
    };
    let released = |l: &StubLauncher| {
        l.calls()
            .iter()
            .filter(|c| c.starts_with("release dev-1"))
            .count()
    };
    assert_eq!(warnings(&w), 1);
    // Keep during the warning, then clear it at once: the old warning does not count any more.
    w.ok("pm-1", "keep", &["dev-1", "24"]);
    w.advance(MINUTE);
    idle.tick();
    w.ok("pm-1", "keep", &["dev-1", "0"]);
    assert!(w.agent("dev-1")["warnedAt"].is_null());
    // The next tick may only warn again; the release waits the warn time (10 minutes) after the new warning.
    w.advance(MINUTE);
    idle.tick();
    assert_eq!(released(&launcher), 0, "{:?}", launcher.calls());
    assert_eq!(warnings(&w), 2);
    for _ in 0..8 {
        w.advance(MINUTE);
        idle.tick();
    }
    assert_eq!(released(&launcher), 0);
    for _ in 0..3 {
        w.advance(MINUTE);
        idle.tick();
    }
    assert!(released(&launcher) >= 1);
}

/// A driver that started at the given time (a daemon restart).
struct Restarted(i64);

impl capstan_daemon::deps::DriverView for Restarted {
    fn snapshot(&self) -> capstan_daemon::deps::DriverSnapshot {
        capstan_daemon::deps::DriverSnapshot {
            grace_floor_ms: self.0,
            ..Default::default()
        }
    }
}

#[test]
fn status_shows_no_release_time_while_the_agent_is_in_the_restart_grace() {
    let mut w = World::new("[daemon]\nstatus_cache_ms = 0\n");
    w.advance(200 * MINUTE);
    assert_eq!(w.agent("dev-1")["autoReleaseAt"], iso(BASE + IDLE * MINUTE));
    let restart = w.now();
    w.deps.as_mut().unwrap().driver = Arc::new(Restarted(restart));
    assert!(w.agent("dev-1")["autoReleaseAt"].is_null());
}
