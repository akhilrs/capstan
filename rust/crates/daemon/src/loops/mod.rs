//! The background loops (src/driver.ts, src/supervision.ts, src/reports.ts relay, src/reviews.ts and src/integration.ts
//! recovery, src/notifier.ts, src/pm-mail.ts), implementing `deps::Loops`.
//!
//! Every loop runs on its own thread and takes the kernel once per ledger step, where Node awaits; a Herdr call or a
//! process sample never runs inside a kernel closure. A stop wakes every sleeping loop at once and waits for the step in
//! progress, as Node's `stop()` awaits its running tick.

pub mod activity;
pub mod driver;
pub mod notifier;
pub mod pm_mail;
pub mod recover;
pub mod relay;
pub mod supervision;

use crate::deps::{Deps, DriverSnapshot, DriverView, Loops};
use capstan_config::RoleConfig;
use capstan_herdr::api::{
    AdapterResult, ClearInput, ClearOutcome, DriverAdapter, GuardedSendInput, HerdrAdapter,
    HerdrState, NotifierAdapter, PaneEntry, PaneRegistry, ProcessActivityProbe, SendOutcome,
    WakeOutcome, WakePmInput,
};
use capstan_herdr::process_activity::HerdrProcessProbe;
use capstan_herdr::runner::{ProcessRunner, RunnerOptions};
use driver::{Driver, DriverLog, DriverOptions, Steps, DEFAULT_TICK_MS};
use notifier::{Clock, Notifier};
use relay::{Relay, RelayOptions};
use serde_json::{json, Value};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use supervision::{Supervision, SupervisionOptions};

/// How often the supervision tick looks, by default.
pub const DEFAULT_SUPERVISION_TICK_MS: u64 = 15_000;

/// A flag a sleeping loop waits on, so a stop does not wait for the end of a tick interval.
#[derive(Clone, Default)]
pub struct StopFlag {
    inner: Arc<(Mutex<bool>, Condvar)>,
}

impl StopFlag {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stop(&self) {
        let (flag, condvar) = &*self.inner;
        *flag.lock().unwrap_or_else(|p| p.into_inner()) = true;
        condvar.notify_all();
    }

    pub fn stopped(&self) -> bool {
        *self.inner.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Sleeps for `timeout` or until stopped; true when stopped.
    pub fn sleep(&self, timeout: Duration) -> bool {
        let (flag, condvar) = &*self.inner;
        let guard = flag.lock().unwrap_or_else(|p| p.into_inner());
        let (guard, _) = condvar
            .wait_timeout_while(guard, timeout, |stopped| !*stopped)
            .unwrap_or_else(|p| p.into_inner());
        *guard
    }
}

/// The status view of the driver: what `status` reads.
struct SharedView(Arc<Mutex<DriverSnapshot>>);

impl DriverView for SharedView {
    fn snapshot(&self) -> DriverSnapshot {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

/// The driver's part of the whole Herdr adapter.
struct DriverPart(Arc<dyn HerdrAdapter>);

impl PaneRegistry for DriverPart {
    fn pane_for_agent(&self, agent_id: &str) -> Option<String> {
        self.0.pane_for_agent(agent_id)
    }
    fn pane_entry(&self, pane_id: &str) -> Option<PaneEntry> {
        self.0.pane_entry(pane_id)
    }
    fn agent_observation(&self, agent_id: &str) -> AdapterResult<HerdrState> {
        self.0.agent_observation(agent_id)
    }
}

impl DriverAdapter for DriverPart {
    fn guarded_send(&self, input: GuardedSendInput<'_>) -> AdapterResult<SendOutcome> {
        self.0.guarded_send(input)
    }
    fn wake_pm(&self, input: WakePmInput<'_>) -> AdapterResult<WakeOutcome> {
        self.0.wake_pm(input)
    }
    fn clear_after_deferral(&self, input: ClearInput<'_>) -> AdapterResult<ClearOutcome> {
        self.0.clear_after_deferral(input)
    }
}

/// The notifier's part of the whole Herdr adapter.
struct NotifierPart(Arc<dyn HerdrAdapter>);

impl NotifierAdapter for NotifierPart {
    fn notify(&self, title: &str, body: &str) -> AdapterResult<()> {
        self.0.notify(title, body)
    }
}

/// `MESSAGING_TIMER_NAMES` as the camelCase object the core takes.
pub fn timers_value(config: &RoleConfig) -> Value {
    let t = &config.timers;
    json!({
        "maxDeferralSeconds": t.max_deferral_seconds,
        "maxBusyDeferralSeconds": t.max_busy_deferral_seconds,
        "pmAckTimeoutSeconds": t.pm_ack_timeout_seconds,
        "pmNotifyAfterSeconds": t.pm_notify_after_seconds,
        "notifyIntervalSeconds": t.notify_interval_seconds,
        "stallAfterSeconds": t.stall_after_seconds,
        "workerAckTimeoutSeconds": t.worker_ack_timeout_seconds,
        "pmWakeAfterSeconds": t.pm_wake_after_seconds,
        "pmWakeIntervalSeconds": t.pm_wake_interval_seconds,
    })
}

/// Node's default probe: `createProcessProbe(createHerdrRunner({ session }))`.
fn default_probe(config: &RoleConfig) -> Option<Arc<dyn ProcessActivityProbe>> {
    let runner = ProcessRunner::new(RunnerOptions {
        session: config.herdr_session.clone(),
        binary: None,
        timeout_ms: None,
        env: None,
    })
    .ok()?;
    Some(Arc::new(HerdrProcessProbe::new(Arc::new(runner))))
}

fn system_clock() -> Clock {
    Arc::new(capstan_ledger::now_millis)
}

#[derive(Default)]
struct Running {
    ticks_stop: Option<StopFlag>,
    ticks: Vec<JoinHandle<()>>,
    relay_stop: Option<StopFlag>,
    relay: Vec<JoinHandle<()>>,
}

/// The loops of the Rust daemon. Build it before the deps (`driver_view` goes into `Deps::driver`), start it through the
/// `Loops` trait.
pub struct RustLoops {
    snapshot: Arc<Mutex<DriverSnapshot>>,
    probe: Option<Arc<dyn ProcessActivityProbe>>,
    now: Clock,
    running: Mutex<Running>,
}

impl RustLoops {
    pub fn new() -> Arc<Self> {
        Self::with(None, system_clock())
    }

    /// With a process probe (tells whether a tool process under a working agent uses CPU; without one the driver builds
    /// Node's default from the configured Herdr session) and a clock.
    pub fn with(probe: Option<Arc<dyn ProcessActivityProbe>>, now: Clock) -> Arc<Self> {
        Arc::new(Self {
            snapshot: Arc::new(Mutex::new(DriverSnapshot {
                lost_agent_ids: Some(Vec::new()),
                ..DriverSnapshot::default()
            })),
            probe,
            now,
            running: Mutex::new(Running::default()),
        })
    }

    /// What `status` reads of the driver.
    pub fn driver_view(&self) -> Arc<dyn DriverView> {
        Arc::new(SharedView(Arc::clone(&self.snapshot)))
    }

    fn running(&self) -> std::sync::MutexGuard<'_, Running> {
        self.running.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Runs one step of a loop; a panic in it is logged as `event` and the loop goes on with its next tick, as Node's
/// `start()` loop does when a tick throws.
fn guarded<F: FnOnce()>(log: &DriverLog, event: &str, step: F) {
    if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(step)) {
        let text = panic
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic".to_string());
        log(event, json!({"error": format!("Error: {text}")}));
    }
}

fn spawn<F: FnOnce() + Send + 'static>(name: &str, body: F) -> Option<JoinHandle<()>> {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(body)
        .ok()
}

fn detail_log(deps: &Deps) -> DriverLog {
    let deps = deps.clone();
    Arc::new(move |event, details| deps.detail_log(event, details))
}

impl Loops for RustLoops {
    fn start_ticks(&self, deps: &Deps) {
        let stop = StopFlag::new();
        let mut handles = Vec::new();
        let log = detail_log(deps);
        let credential = deps.credential().to_string();
        if let (Some(config), Some(adapter)) = (deps.options.capstan.clone(), deps.adapter.clone())
        {
            let notifier_log = {
                let deps = deps.clone();
                Arc::new(move |event: &str, details: Value| {
                    deps.detail_log(&format!("notifier:{event}"), details)
                }) as notifier::NotifierLog
            };
            let notifier = Arc::new(Notifier::new(
                Arc::new(NotifierPart(Arc::clone(&adapter))),
                config.notifications.herdr,
                config.notifications.fallback,
                deps.options.state_directory.join("notifications.jsonl"),
                Arc::clone(&self.now),
                notifier_log,
            ));
            let mut driver = Driver::new(DriverOptions {
                kernel: deps.kernel.clone(),
                adapter: Arc::new(DriverPart(adapter)),
                timers: timers_value(&config),
                notifier,
                credential: credential.clone(),
                now: Arc::clone(&self.now),
                log: Arc::clone(&log),
                tick_ms: deps.options.tick_ms,
                process_probe: self.probe.clone().or_else(|| default_probe(&config)),
                pm_stale_seconds: config.notifications.pm_stale_minutes * 60,
                snapshot: Some(Arc::clone(&self.snapshot)),
            });
            let (flag, tick_ms, tick_log) = (stop.clone(), driver.tick_ms(), Arc::clone(&log));
            handles.extend(spawn("driver", move || {
                // The first tick comes at once, the next ones a tick interval after the end of the one before.
                while !flag.stopped() {
                    guarded(&tick_log, "tick_failed", || driver.tick());
                    if flag.sleep(Duration::from_millis(tick_ms)) {
                        break;
                    }
                }
            }));
            if let Some(launcher) = deps.launcher.clone() {
                let mut supervision = Supervision::new(SupervisionOptions {
                    steps: Steps::new(deps.kernel.clone(), credential.clone()),
                    launcher,
                    enabled: config.supervision.enabled,
                    check_seconds: config.supervision.check_seconds as f64,
                    supervisor_role: config
                        .roles
                        .iter()
                        .find(|role| role.kind == "Supervisor")
                        .map(|role| role.name.clone()),
                    now: Arc::clone(&self.now),
                    log: Arc::clone(&log),
                });
                let interval = deps
                    .options
                    .supervision_tick_ms
                    .unwrap_or(DEFAULT_SUPERVISION_TICK_MS);
                let (flag, tick_log) = (stop.clone(), Arc::clone(&log));
                handles.extend(spawn("supervision", move || {
                    while !flag.sleep(Duration::from_millis(interval)) {
                        guarded(&tick_log, "supervision_failed", || supervision.tick());
                    }
                }));
            }
        }
        let mut running = self.running();
        running.ticks_stop = Some(stop);
        running.ticks = handles;
    }

    fn recover_and_relay(&self, deps: &Deps) {
        let stop = StopFlag::new();
        let mut handles = Vec::new();
        let credential = deps.credential().to_string();
        // Recovery talks to the launcher and to git, so it has a thread of its own and the relay does not wait for it.
        {
            let (deps, flag) = (deps.clone(), stop.clone());
            let credential = credential.clone();
            handles.extend(spawn("recovery", move || {
                if !flag.stopped() {
                    crate::reviews::recover_reviews(&deps, &credential);
                }
                recover::recover_integrations(&deps, &|| flag.stopped());
            }));
        }
        let mut relay = Relay::new(RelayOptions {
            steps: Steps::new(deps.kernel.clone(), credential.clone()),
            credential,
            now: Arc::clone(&self.now),
            finding_check_seconds: deps
                .options
                .capstan
                .as_ref()
                .map(|config| config.timers.finding_check_seconds as f64),
            log: detail_log(deps),
        });
        let interval = deps.options.tick_ms.unwrap_or(DEFAULT_TICK_MS);
        let (flag, tick_log) = (stop.clone(), detail_log(deps));
        handles.extend(spawn("report-relay", move || {
            while !flag.sleep(Duration::from_millis(interval)) {
                guarded(&tick_log, "report_relay_failed", || relay.tick());
            }
        }));
        let mut running = self.running();
        running.relay_stop = Some(stop);
        running.relay = handles;
    }

    fn stop_relay(&self) {
        let (stop, handles) = {
            let mut running = self.running();
            (
                running.relay_stop.take(),
                std::mem::take(&mut running.relay),
            )
        };
        if let Some(stop) = stop {
            stop.stop();
        }
        for handle in handles {
            let _ = handle.join();
        }
    }

    fn stop_ticks(&self) {
        let (stop, handles) = {
            let mut running = self.running();
            (
                running.ticks_stop.take(),
                std::mem::take(&mut running.ticks),
            )
        };
        if let Some(stop) = stop {
            stop.stop();
        }
        for handle in handles {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_in_a_step_is_logged_and_the_loop_goes_on() {
        let seen = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
        let log: DriverLog = {
            let seen = Arc::clone(&seen);
            Arc::new(move |event, details| seen.lock().unwrap().push((event.to_string(), details)))
        };
        guarded(&log, "tick_failed", || panic!("the adapter broke"));
        guarded(&log, "tick_failed", || {});
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, "tick_failed");
        assert_eq!(seen[0].1["error"], "Error: the adapter broke");
    }

    #[test]
    fn a_stop_wakes_a_sleeper_at_once() {
        let flag = StopFlag::new();
        let sleeper = {
            let flag = flag.clone();
            std::thread::spawn(move || flag.sleep(Duration::from_secs(30)))
        };
        std::thread::sleep(Duration::from_millis(50));
        let started = std::time::Instant::now();
        flag.stop();
        assert!(sleeper.join().unwrap());
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(flag.sleep(Duration::from_secs(30)));
    }
}
