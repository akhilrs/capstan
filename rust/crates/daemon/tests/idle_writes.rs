//! An idle daemon writes no ledger rows. One kernel holds a PM and two active workers with no due message and no open
//! approval window; the delivery driver, supervision, the auto-answer loop and the report relay tick against it with a
//! Herdr adapter that shows every agent in the same state each time. After one tick of each to settle, three more ticks
//! of each leave the `controller_events` and `mutation_requests` row counts as they were.
//!
//! Left out, with the reason: the operator service tick (guarded by operator/tests/service.rs), recovery (runs once at
//! start), the cache and warm-release loops (they reach no `Kernel::mutate` without a due warm reviewer) and the ledger
//! retention pass (it writes neither table).
//!
//! The second test is the guard's own check: the same fixture with one worker's state alternating on each driver tick
//! must show the counts growing, so the first test can fail.

mod common;

use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, Logger};
use capstan_daemon::loops::auto_answer::{AutoAnswer, AutoAnswerOptions};
use capstan_daemon::loops::driver::{Driver, DriverOptions, Steps};
use capstan_daemon::loops::notifier::Notifier;
use capstan_daemon::loops::relay::{Relay, RelayOptions};
use capstan_daemon::loops::supervision::{Supervision, SupervisionOptions};
use capstan_herdr::api::{
    AdapterError, AdapterResult, ClearInput, ClearOutcome, DriverAdapter, GuardedSendInput,
    HerdrState, NotifierAdapter, PaneEntry, PaneRegistry, SendOutcome, WakeOutcome, WakePmInput,
};
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::types::InitialProject;
use capstan_kernel::Core;
use capstan_launcher::api::StubLauncher;
use common::private_tempdir;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const OWNER: &str = "owner-credential-0123456789-abcdefghijklmnopqrstuvwxyz";
/// 2026-01-01T00:00:00.000Z.
const EPOCH_MS: i64 = 1_767_225_600_000;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

/// A Herdr adapter that answers each agent's observed state from a table the test sets, and counts the observations it
/// was asked for.
struct FixedAdapter {
    states: Mutex<HashMap<String, &'static str>>,
    observations: AtomicUsize,
}

impl PaneRegistry for FixedAdapter {
    fn pane_for_agent(&self, agent_id: &str) -> Option<String> {
        Some(format!("pane-{agent_id}"))
    }

    fn pane_entry(&self, _pane_id: &str) -> Option<PaneEntry> {
        None
    }

    fn agent_observation(&self, agent_id: &str) -> AdapterResult<HerdrState> {
        self.observations.fetch_add(1, Ordering::SeqCst);
        let state = lock(&self.states).get(agent_id).copied().unwrap_or("idle");
        Ok(HerdrState::of(state))
    }
}

impl DriverAdapter for FixedAdapter {
    fn guarded_send(&self, _input: GuardedSendInput<'_>) -> AdapterResult<SendOutcome> {
        Err(AdapterError::InvalidArgument(
            "an idle fixture sends nothing".into(),
        ))
    }

    fn wake_pm(&self, _input: WakePmInput<'_>) -> AdapterResult<WakeOutcome> {
        Err(AdapterError::InvalidArgument(
            "an idle fixture wakes nobody".into(),
        ))
    }

    fn clear_after_deferral(&self, _input: ClearInput<'_>) -> AdapterResult<ClearOutcome> {
        Err(AdapterError::InvalidArgument(
            "an idle fixture clears nothing".into(),
        ))
    }
}

struct NoNotify;

impl NotifierAdapter for NoNotify {
    fn notify(&self, _title: &str, _body: &str) -> AdapterResult<()> {
        Ok(())
    }
}

struct Fixture {
    _dir: common::PrivateDir,
    database: PathBuf,
    adapter: Arc<FixedAdapter>,
    driver: Driver,
    supervision: Supervision,
    auto_answer: AutoAnswer,
    relay: Relay,
}

fn ledger(kernel: &KernelHandle, op: &str, args: Vec<Value>) -> Value {
    let op = op.to_string();
    kernel
        .run(move |core| {
            let context = capstan_daemon::deps::new_context(core, OWNER)?;
            let mut all = vec![serde_json::to_value(context).unwrap()];
            all.extend(args);
            core.dispatch(&op, &all)
        })
        .unwrap_or_else(|e| panic!("ledger step failed: {e}"))
}

impl Fixture {
    fn new() -> Fixture {
        let dir = private_tempdir();
        let state = dir.path().to_path_buf();
        let database = capstan_ledger::resolve_database_path(&state).expect("the ledger path");
        capstan_ledger::open_database(&database, &capstan_ledger::OpenOptions::default())
            .expect("the ledger migrates");
        let project = InitialProject {
            project_id: "pidle".into(),
            name: "Idle".into(),
            owner_credential: OWNER.into(),
            initial_inputs: capstan_daemon::run::placeholder_inputs(),
        };
        let kernel = {
            let (state, project) = (state.clone(), project.clone());
            KernelHandle::spawn(move || {
                Core::open(
                    &state,
                    &project,
                    &KernelOptions {
                        workspace_root: Some(state.clone()),
                        ..KernelOptions::default()
                    },
                    common::seeded_env("idle-writes"),
                )
            })
            .expect("the kernel opens")
        };
        let roles: Vec<Value> = [
            ("pm", "PM"),
            ("developer", "Developer"),
            ("supervisor", "Supervisor"),
        ]
        .iter()
        .map(|(name, kind)| {
            json!({"name": name, "kind": kind, "host": "claude", "configHash": "a".repeat(64)})
        })
        .collect();
        ledger(&kernel, "syncRoleDefinitions", vec![json!(roles)]);
        for (agent, role, kind) in [
            ("pm-1", "pm", "PM"),
            ("dev-1", "developer", "Developer"),
            ("dev-2", "developer", "Developer"),
        ] {
            let seat = format!("seat-{agent}");
            ledger(
                &kernel,
                "createSeat",
                vec![json!({"seatId": seat, "name": seat, "role": kind})],
            );
            let actor = ledger(
                &kernel,
                "createActor",
                vec![json!({"displayName": agent, "role": kind, "seatId": seat})],
            );
            ledger(
                &kernel,
                "registerAgent",
                vec![
                    json!({"agentId": agent, "roleName": role, "seatId": seat, "actorId": actor["actorId"]}),
                ],
            );
        }

        let adapter = Arc::new(FixedAdapter {
            states: Mutex::new(HashMap::new()),
            observations: AtomicUsize::new(0),
        });
        let clock = Arc::new(AtomicI64::new(EPOCH_MS));
        let now = {
            let clock = Arc::clone(&clock);
            Arc::new(move || clock.load(Ordering::SeqCst))
        };
        let log: capstan_daemon::loops::driver::DriverLog = Arc::new(|_, _| {});
        let notifier = Arc::new(Notifier::new(
            Arc::new(NoNotify),
            true,
            false,
            state.join("notifications.jsonl"),
            now.clone(),
            Arc::new(|_, _| {}),
        ));
        let hour = 3600;
        let driver = Driver::new(DriverOptions {
            kernel: kernel.clone(),
            adapter: adapter.clone(),
            timers: json!({
                "maxDeferralSeconds": hour, "maxBusyDeferralSeconds": hour, "pmAckTimeoutSeconds": hour,
                "pmNotifyAfterSeconds": hour, "notifyIntervalSeconds": hour, "stallAfterSeconds": hour,
                "workerAckTimeoutSeconds": hour, "pmWakeAfterSeconds": hour, "pmWakeIntervalSeconds": hour,
            }),
            notifier,
            credential: OWNER.into(),
            now: now.clone(),
            log: log.clone(),
            tick_ms: None,
            process_probe: None,
            pm_stale_seconds: 1200,
            liveness: None,
            stall_child_max_seconds: 3600,
            snapshot: None,
        });
        let supervision = Supervision::new(SupervisionOptions {
            steps: Steps::new(kernel.clone(), OWNER.into()),
            launcher: Arc::new(StubLauncher::new()),
            enabled: true,
            check_seconds: 60.0,
            supervisor_role: Some("supervisor".into()),
            now: now.clone(),
            log: log.clone(),
        });
        let sink: Logger = Arc::new(|_| {});
        let deps = Deps::new(
            kernel.clone(),
            DaemonOptions::new(state.clone(), project, state.clone()),
            sink,
        );
        let auto_answer = AutoAnswer::new(AutoAnswerOptions {
            deps,
            log: log.clone(),
        });
        let relay = Relay::new(RelayOptions {
            steps: Steps::new(kernel, OWNER.into()),
            credential: OWNER.into(),
            now,
            finding_check_seconds: None,
            log,
        });
        Fixture {
            _dir: dir,
            database,
            adapter,
            driver,
            supervision,
            auto_answer,
            relay,
        }
    }

    /// The row counts of `controller_events` and `mutation_requests`.
    fn counts(&self) -> (i64, i64) {
        let connection = rusqlite::Connection::open_with_flags(
            &self.database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .expect("the ledger opens");
        let count = |table: &str| -> i64 {
            connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap()
        };
        (count("controller_events"), count("mutation_requests"))
    }

    /// One tick of every loop the guard covers.
    fn tick_all(&mut self) {
        self.driver.tick();
        self.supervision.tick();
        self.auto_answer.tick();
        self.relay.tick();
    }
}

#[test]
fn an_idle_daemon_writes_no_ledger_rows() {
    let mut fixture = Fixture::new();
    fixture.tick_all();
    let settled = fixture.counts();
    let observed = fixture.adapter.observations.load(Ordering::SeqCst);
    assert!(settled.0 > 0 && settled.1 > 0, "the fixture has a ledger");
    for _ in 0..3 {
        fixture.tick_all();
    }
    // The driver did tick: it asked the adapter about the PM and both workers each time.
    assert_eq!(
        fixture.adapter.observations.load(Ordering::SeqCst),
        observed * 4,
        "every idle tick observed every agent"
    );
    assert_eq!(
        fixture.counts(),
        settled,
        "(controller_events, mutation_requests) grew on idle ticks"
    );
}

#[test]
fn the_guard_sees_writes_when_an_observed_state_changes() {
    let mut fixture = Fixture::new();
    fixture.tick_all();
    let settled = fixture.counts();
    let observed = fixture.adapter.observations.load(Ordering::SeqCst);
    for round in 0..3 {
        let state = if round % 2 == 0 { "working" } else { "idle" };
        lock(&fixture.adapter.states).insert("dev-1".into(), state);
        fixture.tick_all();
    }
    assert_eq!(
        fixture.adapter.observations.load(Ordering::SeqCst),
        observed * 4,
        "the driver ticked each time"
    );
    let after = fixture.counts();
    assert!(
        after.0 > settled.0 && after.1 > settled.1,
        "a changing state must write: {settled:?} -> {after:?}"
    );
}
