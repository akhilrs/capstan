//! The driver asks the adapter for all observations once per tick and reads the process table once per tick.

mod common;

use capstan_daemon::deps::KernelHandle;
use capstan_daemon::loops::driver::{Driver, DriverOptions};
use capstan_daemon::loops::notifier::Notifier;
use capstan_herdr::api::{
    AdapterError, AdapterResult, ClearInput, ClearOutcome, DriverAdapter, GuardedSendInput,
    HerdrState, NotifierAdapter, PaneEntry, PaneRegistry, SendOutcome, WakeOutcome, WakePmInput,
};
use capstan_herdr::api::{HerdrOutput, ProcessActivityProbe, StubRunner};
use capstan_herdr::process_activity::HerdrProcessProbe;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::types::InitialProject;
use capstan_kernel::Core;
use common::private_tempdir;
use serde_json::{json, Value};
use std::collections::HashMap;
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
    batches: AtomicUsize,
}

impl PaneRegistry for FixedAdapter {
    fn pane_for_agent(&self, agent_id: &str) -> Option<String> {
        Some(format!("pane-{agent_id}"))
    }

    fn pane_entry(&self, _pane_id: &str) -> Option<PaneEntry> {
        None
    }

    fn batches_observations(&self) -> bool {
        true
    }

    fn agent_observations(
        &self,
        agent_ids: &[String],
    ) -> HashMap<String, AdapterResult<HerdrState>> {
        self.batches.fetch_add(1, Ordering::SeqCst);
        agent_ids
            .iter()
            .map(|id| {
                let state = lock(&self.states).get(id).copied().unwrap_or("idle");
                (id.clone(), Ok(HerdrState::of(state)))
            })
            .collect()
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

/// Wraps a probe and counts the ticks it was told about.
struct CountingProbe {
    inner: HerdrProcessProbe,
    begins: AtomicUsize,
}

impl ProcessActivityProbe for CountingProbe {
    fn sample(
        &self,
        pane_id: &str,
    ) -> Result<capstan_herdr::api::ProcessSample, capstan_herdr::api::HerdrError> {
        self.inner.sample(pane_id)
    }
    fn host_alive(&self, pane_id: &str) -> Result<Option<bool>, capstan_herdr::api::HerdrError> {
        self.inner.host_alive(pane_id)
    }
    fn begin_tick(&self) {
        self.begins.fetch_add(1, Ordering::SeqCst);
        self.inner.begin_tick();
    }
}

struct Rig {
    _dir: common::PrivateDir,
    adapter: Arc<FixedAdapter>,
    driver: Driver,
    probe: Arc<CountingProbe>,
    reads: Arc<AtomicUsize>,
    clock: Arc<AtomicI64>,
}

fn rig(workers: usize) -> Rig {
    let dir = private_tempdir();
    let state = dir.path().to_path_buf();
    let database = capstan_ledger::resolve_database_path(&state).expect("the ledger path");
    capstan_ledger::open_database(&database, &capstan_ledger::OpenOptions::default())
        .expect("the ledger migrates");
    let project = InitialProject {
        project_id: "pbatch".into(),
        name: "Batch".into(),
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
                common::seeded_env("observe-batch"),
            )
        })
        .expect("the kernel opens")
    };
    let roles: Vec<Value> = [("pm", "PM"), ("developer", "Developer")]
        .iter()
        .map(|(name, kind)| json!({"name": name, "kind": kind, "host": "claude", "configHash": "a".repeat(64)}))
        .collect();
    ledger(&kernel, "syncRoleDefinitions", vec![json!(roles)]);
    let mut agents = vec![("pm-1".to_string(), "pm", "PM")];
    agents.extend((1..=workers).map(|i| (format!("dev-{i}"), "developer", "Developer")));
    for (agent, role, kind) in agents {
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
        states: Mutex::new(
            (1..=workers)
                .map(|i| (format!("dev-{i}"), "working"))
                .collect(),
        ),
        observations: AtomicUsize::new(0),
        batches: AtomicUsize::new(0),
    });
    let clock = Arc::new(AtomicI64::new(EPOCH_MS));
    let now = {
        let clock = Arc::clone(&clock);
        Arc::new(move || clock.load(Ordering::SeqCst))
    };
    let runner = Arc::new(StubRunner::new());
    runner.set_default(Ok(HerdrOutput {
        code: 0,
        stdout: r#"{"result":{"process_info":{"shell_pid":10}}}"#.into(),
        stderr: String::new(),
    }));
    let reads = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&reads);
    let probe = Arc::new(CountingProbe {
        inner: HerdrProcessProbe::with_table_reader(
            runner,
            Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(Vec::new())
            }),
        ),
        begins: AtomicUsize::new(0),
    });
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
        kernel,
        adapter: adapter.clone(),
        timers: json!({
            "maxDeferralSeconds": hour, "maxBusyDeferralSeconds": hour, "pmAckTimeoutSeconds": hour,
            "pmNotifyAfterSeconds": hour, "notifyIntervalSeconds": hour, "stallAfterSeconds": hour,
            "workerAckTimeoutSeconds": hour, "pmWakeAfterSeconds": hour, "pmWakeIntervalSeconds": hour,
        }),
        notifier,
        credential: OWNER.into(),
        now,
        log: Arc::new(|_, _| {}),
        tick_ms: None,
        process_probe: Some(probe.clone()),
        pm_stale_seconds: 1200,
        pm_stuck_minutes: 0,
        liveness: None,
        stall_child_max_seconds: 3600,
        snapshot: None,
    });
    Rig {
        _dir: dir,
        adapter,
        driver,
        probe,
        reads,
        clock,
    }
}

#[test]
fn six_working_agents_read_the_process_table_once_per_tick() {
    let mut rig = rig(6);
    rig.driver.tick();
    assert_eq!(
        rig.adapter.batches.load(Ordering::SeqCst),
        1,
        "one listing per tick"
    );
    assert_eq!(
        rig.adapter.observations.load(Ordering::SeqCst),
        0,
        "no per-agent observation"
    );
    assert_eq!(rig.reads.load(Ordering::SeqCst), 1);
    // The samples are spaced; move past the spacing so the next tick samples again.
    rig.clock.fetch_add(3_600_000, Ordering::SeqCst);
    rig.driver.tick();
    assert_eq!(
        rig.reads.load(Ordering::SeqCst),
        2,
        "a new tick reads the table again"
    );
    assert_eq!(rig.probe.begins.load(Ordering::SeqCst), 2);
}

#[test]
fn a_tick_that_needs_no_table_clears_the_cache_and_reads_nothing() {
    let mut rig = rig(0);
    for tick in 1..=3 {
        rig.driver.tick();
        assert_eq!(rig.probe.begins.load(Ordering::SeqCst), tick);
    }
    assert_eq!(rig.reads.load(Ordering::SeqCst), 0);
}
