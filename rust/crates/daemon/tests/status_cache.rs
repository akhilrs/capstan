//! The status answer is cached by the project's `state_version` (plus the caller and the driver's view) and for at most
//! `[daemon] status_cache_ms`. Statements are counted on the kernel connection (`Core::statements_run`).

mod common;

use capstan_daemon::deps::{DaemonOptions, Deps, DriverSnapshot, DriverView, KernelHandle, Logger};
use capstan_daemon::handlers::shared::{AbortSignal, CommandResponse, TtlLru};
use capstan_daemon::handlers::status::{InputClass, StatusInputs};
use capstan_daemon::handlers::{dispatch, Request};
use capstan_daemon::loops::retention::run_pass;
use capstan_daemon::loops::StopFlag;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::types::{AgentRecord, Identity as CallerIdentity};
use capstan_kernel::{Core, Env, SeededEnv};
use common::project::Identity;
use common::{private_tempdir, PrivateDir};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;

const DAY_MS: i64 = 86_400_000;
const T0: i64 = 1_800_000_000_000;

struct ClockEnv {
    clock: Arc<AtomicI64>,
    random: SeededEnv,
}

impl Env for ClockEnv {
    fn now(&self) -> i64 {
        self.clock.load(Ordering::SeqCst)
    }
    fn uuid(&self) -> String {
        self.random.uuid()
    }
    fn random_bytes(&self, n: usize) -> Vec<u8> {
        self.random.random_bytes(n)
    }
}

const CONFIG: &str = r#"schema_version = 1
[nexora]
track = "never"
[hosts.claude]
kind = "claude"
[roles.pm]
kind = "PM"
host = "claude"
"#;

/// A driver whose view the test changes.
struct MovingDriver(AtomicU64);

impl DriverView for MovingDriver {
    fn snapshot(&self) -> DriverSnapshot {
        DriverSnapshot {
            stalled_agent_ids: vec![format!("agent-{}", self.0.load(Ordering::SeqCst))],
            ..DriverSnapshot::default()
        }
    }
}

struct World {
    _dir: PrivateDir,
    deps: Deps,
    clock: Arc<AtomicI64>,
    driver: Arc<MovingDriver>,
    serial: AtomicI64,
}

impl World {
    fn new(extra: &str) -> World {
        let dir = private_tempdir();
        let identity = Identity {
            project_id: "pstatus".into(),
            name: "Status".into(),
            credential: format!("operator-{}", "s".repeat(40)),
        };
        let state = common::project::write_project(dir.path(), &identity);
        let project = identity.initial_project();
        let clock = Arc::new(AtomicI64::new(T0));
        let kernel = {
            let (state, project, clock) = (state.clone(), project.clone(), clock.clone());
            KernelHandle::spawn(move || {
                Core::open(
                    &state,
                    &project,
                    &KernelOptions::default(),
                    Box::new(ClockEnv {
                        clock,
                        random: SeededEnv::new("status-cache"),
                    }),
                )
            })
            .expect("the kernel opens")
        };
        let sink: Logger = Arc::new(|_| {});
        let mut options = DaemonOptions::new(state, project, dir.path().to_path_buf());
        let text = format!("{CONFIG}{extra}");
        options.capstan = Some(Arc::new(
            capstan_config::parse_config(text.as_bytes(), dir.path()).expect("the config loads"),
        ));
        let driver = Arc::new(MovingDriver(AtomicU64::new(0)));
        let mut deps = Deps::new(kernel, options, sink);
        deps.driver = driver.clone();
        World {
            _dir: dir,
            deps,
            clock,
            driver,
            serial: AtomicI64::new(1),
        }
    }

    fn operator() -> CallerIdentity {
        CallerIdentity {
            actor_id: "operator".into(),
            role: "operator".into(),
            capabilities: vec![],
            agent: None,
        }
    }

    fn agent(id: &str) -> CallerIdentity {
        CallerIdentity {
            actor_id: id.into(),
            role: "developer".into(),
            capabilities: vec![],
            agent: Some(AgentRecord {
                agent_id: id.into(),
                role_name: "developer".into(),
                kind: "Developer".into(),
                seat_id: format!("seat-{id}"),
                actor_id: id.into(),
                generation: 1,
                state: "active".into(),
                last_activity_at: "2026-01-01T00:00:00.000Z".into(),
            }),
        }
    }

    fn status(&self, who: &CallerIdentity) -> Value {
        let credential = self.deps.credential().to_string();
        let request = Request {
            command: "status",
            credential: &credential,
            args: None,
        };
        let out = dispatch(&self.deps, who, &request, &AbortSignal::new());
        match out.response.expect("a response") {
            CommandResponse::Ok(value) => value,
            _ => panic!("status refused"),
        }
    }

    fn statements(&self) -> u64 {
        self.deps
            .kernel
            .run(|core| Ok(core.statements_run()))
            .unwrap()
    }

    /// The statements one status call ran, and its answer.
    fn measured(&self, who: &CallerIdentity) -> (u64, Value) {
        let before = self.statements();
        let answer = self.status(who);
        (self.statements() - before, answer)
    }

    /// What a bare kernel job that reads `state_version` costs: the one statement of the read, plus what the kernel thread
    /// itself runs around every job.
    fn version_job(&self) -> u64 {
        let before = self.statements();
        self.deps.kernel.run(|core| core.state_version()).unwrap();
        self.statements() - before
    }

    fn mutate(&self) {
        let owner = self.deps.credential().to_string();
        let hash = format!("{:064x}", self.serial.fetch_add(1, Ordering::SeqCst));
        self.deps
            .kernel
            .run(move |core| {
                let context = capstan_daemon::deps::new_context(core, &owner)?;
                core.dispatch(
                    "syncRoleDefinitions",
                    &[
                        serde_json::to_value(context).unwrap(),
                        json!([{"name": "pm", "kind": "PM", "host": "claude", "configHash": hash}]),
                    ],
                )
            })
            .expect("a mutation");
    }
}

#[test]
fn every_status_input_is_classified() {
    let inputs = StatusInputs {
        is_operator: false,
        is_pm: false,
        config: None,
        agents: Value::Null,
        snapshot: Value::Null,
        supervision_activity: Value::Null,
        location: Default::default(),
        now: 0,
        messages: None,
        supervision_reason: None,
        pm_open_messages: None,
        driver: None,
        input_clears: None,
        operator_service: None,
        panes: None,
        reviews: None,
        integrations: None,
        pipeline_counts: None,
        active_tasks: None,
        awaiting_confirm: None,
        findings: None,
        reports: None,
        integration_sets: None,
        set_verify: None,
        launcher: None,
        prompt_relay: None,
        sync_drift: None,
        cache: None,
        verify: None,
        ntfy: None,
    };
    let classes = inputs.classes();
    // No `..`: a field added to StatusInputs fails to compile here until the list below names it.
    let StatusInputs {
        is_operator: _,
        is_pm: _,
        config: _,
        agents: _,
        snapshot: _,
        supervision_activity: _,
        location: _,
        now: _,
        messages: _,
        supervision_reason: _,
        pm_open_messages: _,
        driver: _,
        input_clears: _,
        operator_service: _,
        panes: _,
        reviews: _,
        integrations: _,
        pipeline_counts: _,
        active_tasks: _,
        awaiting_confirm: _,
        findings: _,
        reports: _,
        integration_sets: _,
        set_verify: _,
        launcher: _,
        prompt_relay: _,
        sync_drift: _,
        cache: _,
        verify: _,
        ntfy: _,
    } = inputs;
    let named: BTreeSet<&str> = classes.iter().map(|(name, _)| *name).collect();
    assert_eq!(named.len(), classes.len(), "a field is classified twice");
    assert_eq!(named.len(), 30);
    let class_of = |name: &str| classes.iter().find(|(n, _)| *n == name).unwrap().1;
    // Fed by rows written without a state_version bump, or by things outside the ledger.
    for name in [
        "agents",
        "supervision_activity",
        "messages",
        "supervision_reason",
        "pm_open_messages",
        "input_clears",
        "panes",
        "prompt_relay",
        "verify",
        "set_verify",
        "ntfy",
        "cache",
        "launcher",
        "operator_service",
        "now",
        "location",
    ] {
        assert_eq!(class_of(name), InputClass::Ttl, "{name}");
    }
    assert_eq!(class_of("snapshot"), InputClass::Keyed);
    assert_eq!(class_of("driver"), InputClass::Keyed);
}

#[test]
fn a_second_call_costs_the_state_version_read_only() {
    let w = World::new("[daemon]\nstatus_cache_ms = 2000\n");
    for who in [World::operator(), World::agent("dev-1")] {
        let (first, answer) = w.measured(&who);
        assert!(first > 5, "the first call reads the ledger: {first}");
        let (second, again) = w.measured(&who);
        assert_eq!(again, answer);
        let job = w.version_job();
        if who.agent.is_none() {
            // The state_version read and nothing else.
            assert_eq!(second, job, "operator hit");
        } else {
            // An agent's reply also asks for its unread mail (`with_notice`: one more kernel job).
            assert!(
                second > job && second < first,
                "agent hit {second}, miss {first}"
            );
            assert_eq!(w.measured(&who).0, second, "a hit costs the same each time");
        }
        eprintln!(
            "{}: miss {first}, hit {second}, bare version job {job}",
            who.role
        );
    }
}

#[test]
fn a_keyed_change_is_seen_at_once_and_a_ttl_input_within_the_ttl() {
    let w = World::new("[daemon]\nstatus_cache_ms = 2000\n");
    let who = World::operator();
    let (_, before) = w.measured(&who);
    w.mutate();
    let (cost, after) = w.measured(&who);
    assert!(cost > 5, "a mutation drops the cached answer: {cost}");
    assert_ne!(after["stateVersion"], before["stateVersion"]);
    // The driver's view is part of the key.
    w.driver.0.store(1, Ordering::SeqCst);
    let (cost, changed) = w.measured(&who);
    assert!(cost > 5);
    assert_eq!(changed["stalledAgentIds"], json!(["agent-1"]));
    // Unchanged: a hit, and after the TTL a fresh read.
    assert_eq!(w.measured(&who).0, w.version_job());
    w.clock.fetch_add(1999, Ordering::SeqCst);
    assert_eq!(w.measured(&who).0, w.version_job());
    w.clock.fetch_add(1, Ordering::SeqCst);
    assert!(
        w.measured(&who).0 > 5,
        "an entry older than the TTL is read again"
    );
}

#[test]
fn a_retention_pass_does_not_drop_the_cached_answer_until_the_ttl_ends() {
    let w = World::new("[daemon]\nstatus_cache_ms = 2000\n");
    let who = World::operator();
    w.mutate();
    w.mutate();
    w.clock.fetch_add(40 * DAY_MS, Ordering::SeqCst);
    let (_, cached) = w.measured(&who);
    let credential = w.deps.credential().to_string();
    let rows_before = {
        let credential = credential.clone();
        w.deps
            .kernel
            .run(move |core| {
                let _ = credential;
                core.state_version()
            })
            .unwrap()
    };
    let pass = run_pass(&w.deps.kernel, &credential, 30, &StopFlag::new());
    assert!(pass.deleted >= 2 && pass.error.is_none(), "{pass:?}");
    assert_eq!(
        w.deps.kernel.run(|core| core.state_version()).unwrap(),
        rows_before,
        "retention does not bump state_version"
    );
    w.clock.fetch_add(500, Ordering::SeqCst);
    let (cost, within) = w.measured(&who);
    assert_eq!(cost, w.version_job());
    assert_eq!(within, cached);
    w.clock.fetch_add(2000, Ordering::SeqCst);
    let (cost, fresh) = w.measured(&who);
    assert!(cost > 5);
    assert_eq!(fresh["run"], cached["run"]);
    assert_eq!(fresh["roles"], cached["roles"]);
}

#[test]
fn with_the_cache_off_every_call_reads_the_ledger() {
    let w = World::new("[daemon]\nstatus_cache_ms = 0\n");
    let who = World::operator();
    let (first, a) = w.measured(&who);
    let (second, b) = w.measured(&who);
    assert!(first > 5 && second == first, "{first} {second}");
    assert_eq!(a, b);
    let guard = w.deps.commands.status_cache.lock().unwrap();
    assert!(
        guard.as_ref().is_none_or(|c| c.is_empty()),
        "nothing is stored"
    );
}

#[test]
fn two_actors_never_share_an_entry() {
    let w = World::new("[daemon]\nstatus_cache_ms = 2000\n");
    let (a, b) = (World::agent("dev-a"), World::agent("dev-b"));
    assert!(w.measured(&a).0 > 5);
    assert!(w.measured(&b).0 > 5, "b does not read a's answer");
    let hit = w.measured(&a).0;
    assert!(hit < 10, "a hits its own entry: {hit}");
    assert_eq!(w.measured(&b).0, hit);
    assert!(w.measured(&World::operator()).0 > 5);
}

#[test]
fn the_cache_keeps_64_entries_and_drops_the_least_recently_used() {
    let mut cache: TtlLru<u32, u32> = TtlLru::new(64, 1000);
    for key in 0..64 {
        cache.insert(key, key, 0);
    }
    assert_eq!(
        cache.get(&0, 1),
        Some(0),
        "key 0 is now the most recently used"
    );
    cache.insert(64, 64, 2);
    assert_eq!(cache.len(), 64);
    assert_eq!(cache.get(&1, 3), None, "key 1 was the least recently used");
    assert_eq!(cache.get(&0, 3), Some(0));
    assert_eq!(cache.get(&64, 3), Some(64));
    assert_eq!(cache.get(&2, 3), Some(2));
    // Past the TTL an entry is gone; so is one stored in the future of the clock.
    assert_eq!(cache.get(&2, 1002), None);
}
