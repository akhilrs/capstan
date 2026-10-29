//! The driver's PM wake bookkeeping reads the PM's mail through `pmMailDigest` (the states of its pending wakes and the
//! latest pull), never the whole history through `messagesFor`; an unanswered wake is logged after the backoff and a wake
//! cleared by a pull or an ack is not.

mod common;

use capstan_daemon::deps::KernelHandle;
use capstan_daemon::loops::driver::{Driver, DriverOptions};
use capstan_daemon::loops::notifier::Notifier;
use capstan_herdr::api::{PaneEntry, PanePhase, PaneRole, StubAdapter, WakeOutcome};
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::types::InitialProject;
use capstan_kernel::{Core, Env, SystemEnv};
use common::private_tempdir;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

const OWNER: &str = "owner-credential-0123456789-abcdefghijklmnopqrstuvwxyz";
const EPOCH_MS: i64 = 1_767_225_600_000;

/// A clock the test moves, shared by the ledger and the driver.
struct FakeEnv(Arc<AtomicI64>);

impl Env for FakeEnv {
    fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
    fn uuid(&self) -> String {
        SystemEnv.uuid()
    }
    fn random_bytes(&self, n: usize) -> Vec<u8> {
        SystemEnv.random_bytes(n)
    }
}

struct World {
    _dir: common::PrivateDir,
    kernel: KernelHandle,
    clock: Arc<AtomicI64>,
    log: Arc<Mutex<Vec<Value>>>,
    driver: Driver,
    pm_credential: String,
    calls: BTreeMap<String, u64>,
}

fn step(kernel: &KernelHandle, op: &'static str, args: Value) -> Value {
    kernel
        .run(move |core| {
            let context = serde_json::to_value(capstan_daemon::deps::new_context(core, OWNER)?)?;
            let mut all = vec![context];
            all.extend(args.as_array().cloned().unwrap());
            core.dispatch(op, &all)
        })
        .unwrap_or_else(|e| panic!("{op}: {e}"))
}

fn world() -> World {
    let dir = private_tempdir();
    let state = dir.path().to_path_buf();
    let database = capstan_ledger::resolve_database_path(&state).unwrap();
    capstan_ledger::open_database(&database, &capstan_ledger::OpenOptions::default()).unwrap();
    let clock = Arc::new(AtomicI64::new(EPOCH_MS));
    let kernel = {
        let (clock, state) = (Arc::clone(&clock), state.clone());
        KernelHandle::spawn(move || {
            Core::open(
                &state,
                &InitialProject {
                    project_id: "proj1".into(),
                    name: "Digest Project".into(),
                    owner_credential: OWNER.into(),
                    initial_inputs: capstan_daemon::run::placeholder_inputs(),
                },
                &KernelOptions {
                    workspace_root: Some(state.clone()),
                    ..KernelOptions::default()
                },
                Box::new(FakeEnv(clock)),
            )
        })
        .unwrap()
    };
    step(
        &kernel,
        "syncRoleDefinitions",
        json!([[
            {"name": "pm", "kind": "PM", "host": "claude", "configHash": "a".repeat(64)},
            {"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)},
        ]]),
    );
    let adapter = Arc::new(StubAdapter::new());
    adapter.set_wake(WakeOutcome::Sent);
    let mut pm_credential = String::new();
    for (agent, role, kind) in [("pm-1", "pm", "PM"), ("dev-1", "developer", "Developer")] {
        let seat = format!("seat-{agent}");
        step(
            &kernel,
            "createSeat",
            json!([{"seatId": seat, "name": seat, "role": kind}]),
        );
        let actor = step(
            &kernel,
            "createActor",
            json!([{"displayName": agent, "role": kind, "seatId": seat}]),
        );
        step(
            &kernel,
            "registerAgent",
            json!([{"agentId": agent, "roleName": role, "seatId": seat, "actorId": actor["actorId"]}]),
        );
        if kind == "PM" {
            pm_credential = actor["credential"].as_str().unwrap().to_string();
        }
        adapter.register(
            &format!("pane-{agent}"),
            PaneEntry {
                role: PaneRole::Worker,
                phase: PanePhase::Started,
                kind: "claude".into(),
                agent: Some(agent.to_string()),
                worktree_path: None,
                workspace_id: None,
            },
        );
    }
    let timers = json!({
        "maxDeferralSeconds": 3600, "maxBusyDeferralSeconds": 3600, "pmAckTimeoutSeconds": 3600,
        "pmNotifyAfterSeconds": 3600, "notifyIntervalSeconds": 3600, "stallAfterSeconds": 3600,
        "workerAckTimeoutSeconds": 3600, "pmWakeAfterSeconds": 30, "pmWakeIntervalSeconds": 60,
        "pmWakeClass": "action", "infoWakeAfterSeconds": 600,
    });
    kernel
        .run({
            let policy = timers.clone();
            move |core| {
                core.set_message_policy(&policy);
                Ok(())
            }
        })
        .unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    let now = Arc::clone(&clock);
    let driver = Driver::new(DriverOptions {
        kernel: kernel.clone(),
        adapter: adapter.clone(),
        timers,
        notifier: Arc::new(Notifier::new(
            adapter,
            true,
            true,
            state.join("notifications.jsonl"),
            Arc::new(|| EPOCH_MS),
            Arc::new(|_, _| {}),
        )),
        credential: OWNER.into(),
        now: Arc::new(move || now.load(Ordering::SeqCst)),
        log: {
            let log = Arc::clone(&log);
            Arc::new(move |event, details| {
                log.lock()
                    .unwrap()
                    .push(json!({"event": event, "details": details}));
            })
        },
        tick_ms: None,
        process_probe: None,
        pm_stale_seconds: 1200,
        liveness: None,
        stall_child_max_seconds: 3600,
        snapshot: None,
    });
    World {
        _dir: dir,
        kernel,
        clock,
        log,
        driver,
        pm_credential,
        calls: BTreeMap::new(),
    }
}

impl World {
    fn tell_pm(&self, body: &'static str) -> String {
        self.kernel
            .run(move |core| {
                let context = capstan_daemon::deps::new_context(core, OWNER)?;
                Ok(core.enqueue_message(
                    &context,
                    &json!({"recipientAgentId": "pm-1", "body": body}),
                )?["messageId"]
                    .as_str()
                    .unwrap()
                    .to_string())
            })
            .unwrap()
    }

    fn pass(&self, seconds: i64) {
        self.clock.fetch_add(seconds * 1000, Ordering::SeqCst);
    }

    fn tick(&mut self) {
        self.driver.tick();
        for (method, count) in self.driver.take_kernel_calls() {
            *self.calls.entry(method).or_default() += count;
        }
    }

    fn events(&self, name: &str) -> usize {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e["event"] == name)
            .count()
    }

    fn wake(&mut self) {
        self.pass(31);
        self.tick();
        assert_eq!(
            self.events("pm_wake_typed"),
            1,
            "{:?}",
            self.log.lock().unwrap()
        );
    }
}

#[test]
fn an_unanswered_wake_is_logged_after_the_backoff_from_the_digest_alone() {
    let mut w = world();
    let id = w.tell_pm("act");
    w.wake();
    w.pass(5);
    w.tick();
    assert_eq!(w.events("pm_wake_unanswered"), 0, "before the backoff");
    w.pass(100);
    w.tick();
    assert_eq!(w.events("pm_wake_unanswered"), 1);
    assert_eq!(
        w.log
            .lock()
            .unwrap()
            .iter()
            .find(|e| e["event"] == "pm_wake_unanswered")
            .unwrap()["details"]["messageId"],
        json!(id)
    );
    assert!(
        w.calls.get("pmMailDigest").copied().unwrap_or(0) > 0,
        "{:?}",
        w.calls
    );
    assert_eq!(w.calls.get("messagesFor"), None, "{:?}", w.calls);
    w.kernel.close();
}

#[test]
fn a_wake_cleared_by_a_pull_or_an_ack_is_not_logged_as_unanswered() {
    for ack in [false, true] {
        let mut w = world();
        w.tell_pm("act");
        w.wake();
        w.pass(1);
        let credential = w.pm_credential.clone();
        w.kernel
            .run(move |core| {
                let context = capstan_daemon::deps::new_context(core, &credential)?;
                core.pull_message(&context)?;
                if ack {
                    let context = capstan_daemon::deps::new_context(core, &credential)?;
                    core.ack_all_read(&context)?;
                }
                Ok(())
            })
            .unwrap();
        w.pass(1000);
        w.tick();
        assert_eq!(w.events("pm_wake_unanswered"), 0, "ack: {ack}");
        assert_eq!(w.calls.get("messagesFor"), None, "{:?}", w.calls);
        w.kernel.close();
    }
}
