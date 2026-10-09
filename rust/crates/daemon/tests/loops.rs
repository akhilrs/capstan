//! Replays the loops sequences Node exported (tests/loops-parity/*.json, found at run time, written by
//! test/loops-parity-export.ts from test/loops-sequences/*.json) through the Rust loops: the delivery driver, the
//! supervision tick and the report relay, one scripted tick at a time against a seeded kernel, `StubDriverAdapter` and a
//! scripted probe, notifier adapter and launcher. Everything the export records is compared: the adapter calls in order, the
//! log events, the notification lines, the calls made on the core by method (so the Rust loops make exactly the calls Node
//! makes, which carries the statement and row budgets of test/daemon-cost.test.ts over: the kernel's methods are proven
//! SQL-equivalent by the kernel replays), the driver's snapshot, the PM mail summary and the ledger's table changes.
//!
//! Strict mode (`CAPSTAN_DAEMON_PARITY_STRICT=1`) is moot here: a sequence has no pending steps.
//!
//! The other tests: the loops stop within the daemon's drain flush (1 s), and a driver tick makes the same kernel calls
//! however much mail the PM holds.

mod common;

use capstan_daemon::deps::{
    CommitInspection, DaemonOptions, Deps, DriverSnapshot, GitError, GitPort, GitResult,
    InspectCommitInput, KernelHandle, LogEntry, Logger, Loops, NewCommitMessages,
    NewCommitMessagesInput,
};
use capstan_daemon::loops::driver::{Driver, DriverOptions, Steps};
use capstan_daemon::loops::notifier::Notifier;
use capstan_daemon::loops::pm_mail::pm_mail_summary;
use capstan_daemon::loops::recover::recover_integrations;
use capstan_daemon::loops::relay::{Relay, RelayOptions};
use capstan_daemon::loops::supervision::{Supervision, SupervisionOptions};
use capstan_daemon::loops::RustLoops;
use capstan_herdr::api::{
    AdapterError, AdapterResult, ClearInput, ClearOutcome, DeferralReason, DriverAdapter,
    GuardedSendInput, HerdrError, HerdrState, HookError, InputBlocker, KeyLogEntry,
    NotifierAdapter, PaneEntry, PanePhase, PaneRegistry, PaneRole, ProcessActivityProbe,
    ProcessEntry, ProcessSample, SendOutcome, StubAdapter, WakeOutcome, WakePmInput,
};
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::types::InitialProject;
use capstan_kernel::Core;
use capstan_launcher::api::{LauncherError, StubLauncher};
use common::tables::{describe_tables_difference, diff_tables, dump_tables};
use common::{private_tempdir, seeded_env};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 2026-01-01T00:00:00.000Z, where the seeded clock starts.
const SEEDED_EPOCH_MS: i64 = 1_767_225_600_000;
const OWNER: &str = "owner-credential-0123456789-abcdefghijklmnopqrstuvwxyz";

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

// ------------------------------------------------------------------------------------------------ scripts

/// What the stubs answer; each section is merged key by key into the one before (null removes a key), as the exporter
/// does.
#[derive(Default)]
struct Script {
    panes: BTreeMap<String, Value>,
    entries: BTreeMap<String, Value>,
    observe: BTreeMap<String, Value>,
    send: BTreeMap<String, Value>,
    wake: BTreeMap<String, Value>,
    clear: BTreeMap<String, Value>,
    probe: BTreeMap<String, Value>,
    notify: Value,
    spawn: Value,
    release: Value,
    /// (recover) `tips`, `delete` and `covered`, as the exporter's `git` section.
    git: Map<String, Value>,
}

impl Script {
    fn new() -> Self {
        Self {
            notify: json!("ok"),
            spawn: json!("ok"),
            release: json!("ok"),
            ..Self::default()
        }
    }

    fn merge(&mut self, patch: &Value) {
        let sections: [(&str, &mut BTreeMap<String, Value>); 7] = [
            ("panes", &mut self.panes),
            ("entries", &mut self.entries),
            ("observe", &mut self.observe),
            ("send", &mut self.send),
            ("wake", &mut self.wake),
            ("clear", &mut self.clear),
            ("probe", &mut self.probe),
        ];
        for (name, into) in sections {
            for (key, value) in patch[name].as_object().into_iter().flatten() {
                if value.is_null() {
                    into.remove(key);
                } else {
                    into.insert(key.clone(), value.clone());
                }
            }
        }
        for (name, into) in [
            ("notify", &mut self.notify),
            ("spawn", &mut self.spawn),
            ("release", &mut self.release),
        ] {
            if let Some(value) = patch.get(name) {
                *into = value.clone();
            }
        }
        for (key, value) in patch["git"].as_object().into_iter().flatten() {
            self.git.insert(key.clone(), value.clone());
        }
    }

    /// The agent the pane's registry entry names: `entries` says so, else the agent that has the pane.
    fn entry_agent(&self, pane_id: &str) -> Option<String> {
        if let Some(entry) = self.entries.get(pane_id) {
            return entry.as_str().map(str::to_string);
        }
        self.panes
            .iter()
            .find(|(_, pane)| pane.as_str() == Some(pane_id))
            .map(|(agent, _)| agent.clone())
    }
}

/// `Class`, `Class:message`, `HerdrError:code[:message]`, `LauncherError:code[:message]` or
/// `InputUnreadable:blocker[:message]`, as the exporter's `errorFromSpec`.
fn split_spec(spec: &str) -> (&str, Option<&str>, String, String) {
    let mut parts = spec.splitn(3, ':');
    let name = parts.next().unwrap_or("");
    let arg = parts.next();
    let tail = parts.next().unwrap_or("").to_string();
    let plain = spec
        .split_once(':')
        .map(|(_, rest)| rest.to_string())
        .unwrap_or_default();
    (name, arg, tail, plain)
}

fn or_default(text: String, name: &str) -> String {
    if text.is_empty() {
        format!("{name} (scripted)")
    } else {
        text
    }
}

fn adapter_error(spec: &str) -> AdapterError {
    let (name, arg, tail, plain) = split_spec(spec);
    let message = or_default(plain, name);
    match name {
        "HerdrError" => AdapterError::Herdr(HerdrError::new(
            arg.unwrap_or("failed"),
            or_default(tail, name),
        )),
        "InputUnreadable" => AdapterError::InputUnreadable {
            message: or_default(tail, name),
            blocker: match arg {
                Some("dialog") => InputBlocker::Dialog,
                Some("permission_prompt") => InputBlocker::PermissionPrompt,
                _ => InputBlocker::Unknown,
            },
        },
        "AgentPaneMismatch" => AdapterError::AgentPaneMismatch(message),
        "PhaseError" => AdapterError::Phase(message),
        "UnknownPaneError" => AdapterError::UnknownPane(message),
        "SendAfterRecordError" => AdapterError::SendAfterRecord(message),
        "InvalidArgumentError" => AdapterError::InvalidArgument(message),
        "DeferralNotElapsed" => AdapterError::DeferralNotElapsed(message),
        "NotIdle" => AdapterError::NotIdle(message),
        "ClearFailed" => AdapterError::ClearFailed(message),
        // A plain `Error` of Node: nothing the adapter's classes name.
        _ => AdapterError::Hook(HookError::new("Error", message)),
    }
}

/// The error a script asks for: `{ "error": "<spec>" }`.
fn scripted_error(script: &Value) -> Option<AdapterError> {
    script["error"].as_str().map(adapter_error)
}

// ------------------------------------------------------------------------------------------------ the stubs

type Calls = Arc<Mutex<Vec<String>>>;

/// The recording stub of the Herdr adapter the driver uses: every call is logged as the exporter's stub logs it, and the
/// answers come from the tick's script.
struct StubDriverAdapter {
    script: Arc<Mutex<Script>>,
    calls: Calls,
}

impl PaneRegistry for StubDriverAdapter {
    fn pane_for_agent(&self, agent_id: &str) -> Option<String> {
        lock(&self.calls).push(format!("paneForAgent {agent_id}"));
        lock(&self.script)
            .panes
            .get(agent_id)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    fn pane_entry(&self, pane_id: &str) -> Option<PaneEntry> {
        lock(&self.calls).push(format!("paneEntry {pane_id}"));
        lock(&self.script)
            .entry_agent(pane_id)
            .map(|agent| PaneEntry {
                role: PaneRole::Worker,
                phase: PanePhase::Started,
                kind: "claude".into(),
                agent: Some(agent),
                worktree_path: None,
                workspace_id: None,
            })
    }

    fn agent_observation(&self, agent_id: &str) -> AdapterResult<HerdrState> {
        lock(&self.calls).push(format!("agentObservation {agent_id}"));
        let answer = lock(&self.script)
            .observe
            .get(agent_id)
            .cloned()
            .unwrap_or_else(|| json!("idle"));
        match scripted_error(&answer) {
            Some(error) => Err(error),
            None => Ok(HerdrState::of(answer.as_str().unwrap_or("unknown"))),
        }
    }
}

impl DriverAdapter for StubDriverAdapter {
    fn guarded_send(&self, input: GuardedSendInput<'_>) -> AdapterResult<SendOutcome> {
        lock(&self.calls).push(format!(
            "guardedSend {} {}",
            input.pane_id,
            serde_json::to_string(input.text).unwrap()
        ));
        let script = lock(&self.script)
            .send
            .get(input.pane_id)
            .cloned()
            .unwrap_or_else(|| json!("sent"));
        if let Some(error) = scripted_error(&script) {
            return Err(error);
        }
        if let Some(spec) = script["hookThenError"].as_str() {
            (input.before_send)()?;
            return Err(adapter_error(spec));
        }
        if let Some(reason) = script["deferred"].as_str() {
            return Ok(SendOutcome::Deferred {
                reason: match reason {
                    "agent_blocked" => DeferralReason::AgentBlocked,
                    "input_not_empty" => DeferralReason::InputNotEmpty,
                    _ => DeferralReason::AgentBusy,
                },
                detail: script["detail"].as_str().map(str::to_string),
                blocker: script["blocker"].as_str().map(|b| match b {
                    "dialog" => InputBlocker::Dialog,
                    "permission_prompt" => InputBlocker::PermissionPrompt,
                    _ => InputBlocker::Unknown,
                }),
            });
        }
        (input.before_send)()?;
        Ok(SendOutcome::Sent)
    }

    fn wake_pm(&self, input: WakePmInput<'_>) -> AdapterResult<WakeOutcome> {
        lock(&self.calls).push(format!(
            "wakePm {} {}",
            input.pane_id,
            serde_json::to_string(input.text).unwrap()
        ));
        let script = lock(&self.script)
            .wake
            .get(input.pane_id)
            .cloned()
            .unwrap_or_else(|| json!("sent"));
        if let Some(error) = scripted_error(&script) {
            return Err(error);
        }
        match script.as_str() {
            Some("pm_not_idle") => return Ok(WakeOutcome::PmNotIdle),
            Some("input_not_empty") => return Ok(WakeOutcome::InputNotEmpty),
            _ => {}
        }
        (input.before_send)()?;
        if let Some(spec) = script["hookThenError"].as_str() {
            return Err(adapter_error(spec));
        }
        Ok(WakeOutcome::Sent)
    }

    fn clear_after_deferral(&self, input: ClearInput<'_>) -> AdapterResult<ClearOutcome> {
        lock(&self.calls).push(format!(
            "clearAfterDeferral {} deferredForMs={} maxDeferralMs={}",
            input.pane_id, input.deferred_for_ms, input.max_deferral_ms
        ));
        let script = lock(&self.script)
            .clear
            .get(input.pane_id)
            .cloned()
            .unwrap_or_else(|| json!({"cleared": false}));
        if let Some(error) = scripted_error(&script) {
            return Err(error);
        }
        for key in script["keys"].as_array().into_iter().flatten() {
            (input.log)(&KeyLogEntry {
                pane: input.pane_id.to_string(),
                key: key[0].as_str().unwrap_or("").to_string(),
                reason: key[1].as_str().unwrap_or("").to_string(),
            });
        }
        let text = script["text"].as_str().unwrap_or("").to_string();
        let cleared = script["cleared"] == json!(true);
        if cleared {
            (input.discard)(&text)?;
        }
        Ok(ClearOutcome { cleared, text })
    }
}

struct StubNotifierAdapter {
    script: Arc<Mutex<Script>>,
    calls: Calls,
}

impl NotifierAdapter for StubNotifierAdapter {
    fn notify(&self, title: &str, body: &str) -> AdapterResult<()> {
        lock(&self.calls).push(format!(
            "notify {} {}",
            serde_json::to_string(title).unwrap(),
            serde_json::to_string(body).unwrap()
        ));
        let script = lock(&self.script).notify.clone();
        scripted_error(&script).map_or(Ok(()), Err)
    }
}

struct StubProbe {
    script: Arc<Mutex<Script>>,
    calls: Calls,
}

impl ProcessActivityProbe for StubProbe {
    fn sample(&self, pane_id: &str) -> Result<ProcessSample, HerdrError> {
        lock(&self.calls).push(format!("sample {pane_id}"));
        let script = lock(&self.script)
            .probe
            .get(pane_id)
            .cloned()
            .unwrap_or_else(|| json!({"processes": []}));
        if let Some(AdapterError::Herdr(error)) = scripted_error(&script) {
            return Err(error);
        }
        Ok(ProcessSample {
            shell_pid: None,
            processes: script["processes"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|p| ProcessEntry {
                    pid: p["pid"].as_i64().unwrap_or(0),
                    ppid: p["ppid"].as_i64().unwrap_or(0),
                    comm: p["comm"].as_str().unwrap_or("").to_string(),
                    cpu_seconds: p["cpuMs"].as_f64().unwrap_or(0.0) / 1000.0,
                })
                .collect(),
        })
    }
}

// ------------------------------------------------------------------------------------------------ comparing

fn clip(text: &str) -> String {
    let mut out: String = text.chars().take(600).collect();
    if out.len() < text.len() {
        out.push('…');
    }
    out
}

fn first_difference(expected: &Value, actual: &Value, path: &str) -> Option<String> {
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            for key in e.keys().chain(a.keys()) {
                let left = e.get(key).unwrap_or(&Value::Null);
                let right = a.get(key).unwrap_or(&Value::Null);
                if e.contains_key(key) != a.contains_key(key) {
                    return Some(format!(
                        "{path}.{key}: expected {} but got {}",
                        clip(&left.to_string()),
                        clip(&right.to_string())
                    ));
                }
                if let Some(found) = first_difference(left, right, &format!("{path}.{key}")) {
                    return Some(found);
                }
            }
            None
        }
        (Value::Array(e), Value::Array(a)) => {
            for index in 0..e.len().max(a.len()) {
                let left = e.get(index);
                let right = a.get(index);
                match (left, right) {
                    (Some(l), Some(r)) => {
                        if let Some(found) = first_difference(l, r, &format!("{path}[{index}]")) {
                            return Some(found);
                        }
                    }
                    _ => {
                        return Some(format!(
                            "{path}[{index}]: expected {} but got {} (lengths {} and {})",
                            clip(&left.map_or("nothing".into(), Value::to_string)),
                            clip(&right.map_or("nothing".into(), Value::to_string)),
                            e.len(),
                            a.len()
                        ))
                    }
                }
            }
            None
        }
        _ if expected == actual => None,
        _ => Some(format!(
            "{path}: expected {} but got {}",
            clip(&expected.to_string()),
            clip(&actual.to_string())
        )),
    }
}

// ------------------------------------------------------------------------------------------------ one sequence

struct Scratch {
    _dir: tempfile::TempDir,
    state: PathBuf,
    database: PathBuf,
}

fn project() -> InitialProject {
    InitialProject {
        project_id: "proj1".into(),
        name: "Loops Project".into(),
        owner_credential: OWNER.into(),
        initial_inputs: capstan_daemon::run::placeholder_inputs(),
    }
}

fn open_kernel(seed: &str) -> (Scratch, KernelHandle) {
    let dir = private_tempdir();
    let state = dir.path().to_path_buf();
    // The ledger's migration timestamps come from the system clock in both implementations; migrating first keeps them out
    // of the seeded clock.
    let database = capstan_ledger::resolve_database_path(&state).expect("the ledger path");
    capstan_ledger::open_database(&database, &capstan_ledger::OpenOptions::default())
        .expect("the ledger migrates");
    let (opened, seed) = (state.clone(), seed.to_string());
    let kernel = KernelHandle::spawn(move || {
        Core::open(
            &opened,
            &project(),
            &KernelOptions {
                workspace_root: Some(opened.clone()),
                ..KernelOptions::default()
            },
            seeded_env(&seed),
        )
    })
    .expect("the kernel opens");
    (
        Scratch {
            _dir: dir,
            state,
            database,
        },
        kernel,
    )
}

/// Runs a recorded step on the kernel and checks its result or error against the record.
fn replay_step(kernel: &KernelHandle, record: &Value, label: &str) -> Result<(), String> {
    let op = record["op"].as_str().ok_or("a step has no op")?.to_string();
    let mut args: Vec<Value> = Vec::new();
    if record.get("context").is_some() {
        args.push(record["context"].clone());
    }
    args.extend(record["args"].as_array().cloned().unwrap_or_default());
    let call = op.clone();
    let got = kernel
        .call(move |core| core.dispatch(&call, &args))
        .map_err(|_| "the kernel is closed".to_string())?;
    match (&got, record.get("error")) {
        (Ok(value), None) => {
            let expected = record.get("result").cloned().unwrap_or(Value::Null);
            let actual: Value = serde_json::from_str(&value.to_string()).unwrap();
            match first_difference(&expected, &actual, "result") {
                None => Ok(()),
                Some(diff) => Err(format!("{label} step {op}: {diff}")),
            }
        }
        (Err(error), Some(name)) => {
            let message = record["message"].as_str().unwrap_or("");
            if error.name() == name.as_str().unwrap_or("") && error.message() == message {
                Ok(())
            } else {
                Err(format!(
                    "{label} step {op}: expected {name}: {message} but got {}: {}",
                    error.name(),
                    error.message()
                ))
            }
        }
        (Ok(_), Some(name)) => Err(format!("{label} step {op}: expected the error {name}")),
        (Err(error), None) => Err(format!("{label} step {op}: unexpected {error}")),
    }
}

fn new_lines(path: &Path, seen: &mut usize) -> Vec<Value> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let all: Vec<Value> = text
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let fresh = all[(*seen).min(all.len())..].to_vec();
    *seen = all.len();
    fresh
}

fn counts_json(counts: BTreeMap<String, u64>) -> Value {
    Value::Object(
        counts
            .into_iter()
            .map(|(k, v)| (k, json!(v)))
            .collect::<Map<_, _>>(),
    )
}

fn snapshot_json(snapshot: &DriverSnapshot) -> Value {
    json!({
        "stalledAgentIds": snapshot.stalled_agent_ids,
        "stuck": snapshot.stuck.iter().map(|(id, reason)| json!({"messageId": id, "reason": reason})).collect::<Vec<_>>(),
        "lostAgentIds": snapshot.lost_agent_ids.clone().unwrap_or_default(),
        "pmStale": snapshot.pm_stale,
    })
}

/// The tick's table changes against the tables before it, compared with the recorded ones.
struct Tables {
    previous: Value,
}

impl Tables {
    fn check(&mut self, scratch: &Scratch, tick: &Value, label: &str) -> Result<(), String> {
        if tick.get("tablesDiff").is_none() && tick.get("calls").is_none() {
            return Ok(());
        }
        let current = dump_tables(&scratch.database);
        let diff = diff_tables(&self.previous, &current);
        let expected = tick.get("tablesDiff").cloned().unwrap_or_else(|| json!({}));
        self.previous = current;
        match describe_tables_difference(&expected, &diff) {
            None => Ok(()),
            Some(why) => Err(format!("{label}: the ledger differs: {why}")),
        }
    }
}

fn compare(label: &str, field: &str, tick: &Value, actual: Value) -> Result<(), String> {
    let expected = tick.get(field).cloned().unwrap_or(Value::Null);
    match first_difference(&expected, &actual, field) {
        None => Ok(()),
        Some(diff) => Err(format!("{label}: {diff}")),
    }
}

struct Run {
    kernel: KernelHandle,
    scratch: Scratch,
    clock: Arc<AtomicI64>,
    script: Arc<Mutex<Script>>,
    calls: Calls,
    log: Arc<Mutex<Vec<Value>>>,
}

fn replay_sequence(group: &str, sequence: &Value) -> Result<(), String> {
    let name = sequence["name"].as_str().unwrap_or("?");
    let label = format!("{group}/{name}");
    let seed = sequence["seed"].as_str().unwrap_or(name);
    let (scratch, kernel) = open_kernel(seed);
    let run = Run {
        kernel: kernel.clone(),
        scratch,
        clock: Arc::new(AtomicI64::new(SEEDED_EPOCH_MS)),
        script: Arc::new(Mutex::new(Script::new())),
        calls: Arc::new(Mutex::new(Vec::new())),
        log: Arc::new(Mutex::new(Vec::new())),
    };
    let outcome = (|| {
        for record in sequence["setup"].as_array().into_iter().flatten() {
            replay_step(&run.kernel, record, &format!("{label} setup"))?;
        }
        match sequence["loop"].as_str() {
            Some("driver") => replay_driver(&label, sequence, &run),
            Some("supervision") => replay_supervision(&label, sequence, &run),
            Some("relay") => replay_relay(&label, sequence, &run),
            Some("recover") => replay_recover(&label, sequence, &run),
            other => Err(format!("{label}: unknown loop {other:?}")),
        }
    })();
    kernel.close();
    outcome
}

fn log_sink(log: &Arc<Mutex<Vec<Value>>>) -> capstan_daemon::loops::driver::DriverLog {
    let log = Arc::clone(log);
    Arc::new(move |event, details| {
        lock(&log).push(json!({"event": event, "details": details}));
    })
}

/// Applies a tick's recorded steps and adapter patch and sets its clock; then the loop's own tick runs.
fn begin_tick(run: &Run, tick: &Value, label: &str) -> Result<(), String> {
    run.clock.store(
        SEEDED_EPOCH_MS + tick["at"].as_i64().unwrap_or(0),
        Ordering::SeqCst,
    );
    for record in tick["steps"].as_array().into_iter().flatten() {
        replay_step(&run.kernel, record, &format!("{label} step"))?;
    }
    if let Some(patch) = tick.get("adapter") {
        lock(&run.script).merge(patch);
    }
    lock(&run.calls).clear();
    lock(&run.log).clear();
    Ok(())
}

fn replay_driver(label: &str, sequence: &Value, run: &Run) -> Result<(), String> {
    let config = &sequence["config"];
    let notifications = run.scratch.state.join("notifications.jsonl");
    let notifier_log = {
        let log = Arc::clone(&run.log);
        Arc::new(move |event: &str, details: Value| {
            lock(&log).push(json!({"event": format!("notifier:{event}"), "details": details}));
        })
    };
    let clock = Arc::clone(&run.clock);
    let notifier_clock = Arc::clone(&run.clock);
    let notifier = Arc::new(Notifier::new(
        Arc::new(StubNotifierAdapter {
            script: Arc::clone(&run.script),
            calls: Arc::clone(&run.calls),
        }),
        config["channels"]["herdr"].as_bool().unwrap_or(true),
        config["channels"]["fallback"].as_bool().unwrap_or(true),
        notifications.clone(),
        Arc::new(move || notifier_clock.load(Ordering::SeqCst)),
        notifier_log,
    ));
    let mut driver = Driver::new(DriverOptions {
        kernel: run.kernel.clone(),
        adapter: Arc::new(StubDriverAdapter {
            script: Arc::clone(&run.script),
            calls: Arc::clone(&run.calls),
        }),
        timers: config["timers"].clone(),
        notifier,
        credential: OWNER.into(),
        now: Arc::new(move || clock.load(Ordering::SeqCst)),
        log: log_sink(&run.log),
        tick_ms: config["tickMs"].as_u64(),
        process_probe: Some(Arc::new(StubProbe {
            script: Arc::clone(&run.script),
            calls: Arc::clone(&run.calls),
        })),
        pm_stale_seconds: config["pmStaleSeconds"].as_i64().unwrap_or(1200),
        snapshot: None,
    });
    let mut tables = Tables {
        previous: dump_tables(&run.scratch.database),
    };
    let mut seen_notifications = 0usize;
    for (index, tick) in sequence["ticks"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let here = format!("{label} tick {index}");
        begin_tick(run, tick, &here)?;
        // The steps are not the driver's calls.
        driver.take_kernel_calls();
        driver.tick();
        compare(&here, "calls", tick, json!(lock(&run.calls).clone()))?;
        compare(&here, "log", tick, json!(lock(&run.log).clone()))?;
        compare(
            &here,
            "coreCalls",
            tick,
            counts_json(driver.take_kernel_calls()),
        )?;
        compare(&here, "snapshot", tick, snapshot_json(&driver.snapshot()))?;
        compare(
            &here,
            "notifications",
            tick,
            json!(new_lines(&notifications, &mut seen_notifications)),
        )?;
        if let Some(agent) = config["pmMail"].as_str() {
            let agent = agent.to_string();
            let mail = run
                .kernel
                .run(move |core| core.open_messages_for(&agent))
                .map_err(|e| e.to_string())?;
            let summary = pm_mail_summary(
                mail.as_array().map_or(&[][..], |a| a.as_slice()),
                run.clock.load(Ordering::SeqCst),
                config["pmStaleSeconds"].as_i64().unwrap_or(1200),
            );
            compare(
                &here,
                "pmMail",
                tick,
                json!({
                    "oldestAgeSeconds": summary.oldest_age_seconds,
                    "oldestMessageId": summary.oldest_message_id,
                    "oldestQueuedAt": summary.oldest_queued_at,
                    "pending": summary.pending,
                    "stale": summary.stale,
                }),
            )?;
        }
        tables.check(&run.scratch, tick, &here)?;
    }
    Ok(())
}

fn replay_supervision(label: &str, sequence: &Value, run: &Run) -> Result<(), String> {
    let config = &sequence["config"];
    let launcher = Arc::new(StubLauncher::new());
    let clock = Arc::clone(&run.clock);
    let mut supervision = Supervision::new(SupervisionOptions {
        steps: Steps::new(run.kernel.clone(), OWNER.into()),
        launcher: launcher.clone(),
        enabled: config["supervision"]["enabled"].as_bool().unwrap_or(true),
        check_seconds: config["supervision"]["checkSeconds"]
            .as_f64()
            .unwrap_or(60.0),
        supervisor_role: config["supervisorRole"].as_str().map(str::to_string),
        now: Arc::new(move || clock.load(Ordering::SeqCst)),
        log: log_sink(&run.log),
    });
    let mut tables = Tables {
        previous: dump_tables(&run.scratch.database),
    };
    let mut handled_calls = 0usize;
    for (index, tick) in sequence["ticks"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let here = format!("{label} tick {index}");
        begin_tick(run, tick, &here)?;
        // A call the tick makes fails as the script says (queued only when the recorded tick makes it).
        let expected_calls = tick["calls"].as_array().cloned().unwrap_or_default();
        for method in ["spawn", "release"] {
            if !expected_calls.iter().any(|c| {
                c.as_str()
                    .is_some_and(|c| c.starts_with(&format!("{method} ")))
            }) {
                continue;
            }
            let wanted = lock(&run.script).clone_field(method);
            if let Some(spec) = wanted["error"].as_str() {
                let (_, code, tail, _) = split_spec(spec);
                launcher.fail_next(
                    method,
                    LauncherError::new(code.unwrap_or("failed"), or_default(tail, "LauncherError")),
                );
            }
        }
        supervision.tick();
        let all = launcher.calls();
        let fresh: Vec<String> = all[handled_calls..]
            .iter()
            .map(|line| line.split(' ').take(2).collect::<Vec<_>>().join(" "))
            .collect();
        handled_calls = all.len();
        compare(&here, "calls", tick, json!(fresh))?;
        compare(&here, "log", tick, json!(lock(&run.log).clone()))?;
        // The coreCalls of the supervision tick are the Steps' own, counted by a Steps the test cannot reach; the ledger
        // below is what they did.
        tables.check(&run.scratch, tick, &here)?;
    }
    Ok(())
}

impl Script {
    fn clone_field(&self, name: &str) -> Value {
        match name {
            "spawn" => self.spawn.clone(),
            _ => self.release.clone(),
        }
    }
}

fn replay_relay(label: &str, sequence: &Value, run: &Run) -> Result<(), String> {
    let config = &sequence["config"];
    let steps = Steps::new(run.kernel.clone(), OWNER.into());
    let kernel = run.kernel.clone();
    let mut relay = Relay::new(RelayOptions {
        steps: steps.clone(),
        credential: OWNER.into(),
        // The relay has no clock option in Node: it reads Date.now(), which the seeded clock answers, one reading per use.
        now: Arc::new(move || kernel.now_ms()),
        finding_check_seconds: config["findingCheckSeconds"].as_f64(),
        log: log_sink(&run.log),
    });
    let mut tables = Tables {
        previous: dump_tables(&run.scratch.database),
    };
    for (index, tick) in sequence["ticks"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let here = format!("{label} tick {index}");
        begin_tick(run, tick, &here)?;
        steps.take_calls();
        relay.tick();
        compare(&here, "log", tick, json!(lock(&run.log).clone()))?;
        compare(&here, "coreCalls", tick, counts_json(steps.take_calls()))?;
        tables.check(&run.scratch, tick, &here)?;
    }
    Ok(())
}

/// The git the integration recovery asks, answering from the tick's `git` script and logging each call as the exporter's
/// stub does.
struct StubGit {
    script: Arc<Mutex<Script>>,
    calls: Calls,
}

impl StubGit {
    fn unavailable<T>() -> GitResult<T> {
        Err(GitError {
            message: "not used by the recovery".into(),
        })
    }

    /// `answer(table, key, fallback)` of the exporter.
    fn answer(&self, table: &str, key: &str, fallback: Value) -> GitResult<Value> {
        let value = lock(&self.script)
            .git
            .get(table)
            .and_then(|t| t.get(key))
            .cloned()
            .unwrap_or(fallback);
        match value["error"].as_str() {
            Some(spec) => Err(GitError {
                message: or_default(split_spec(spec).3, "Error"),
            }),
            None => Ok(value),
        }
    }
}

impl GitPort for StubGit {
    fn commit_exists(&self, _sha: &str) -> GitResult<bool> {
        Self::unavailable()
    }
    fn inspect_commit(&self, _input: &InspectCommitInput) -> GitResult<CommitInspection> {
        Self::unavailable()
    }
    fn new_commit_messages(&self, _input: &NewCommitMessagesInput) -> GitResult<NewCommitMessages> {
        Self::unavailable()
    }
    fn head_commit(&self) -> GitResult<String> {
        Self::unavailable()
    }
    fn commit_subject(&self, _sha: &str) -> GitResult<Option<String>> {
        Self::unavailable()
    }
    fn merge(&self, _input: &Value) -> GitResult<Value> {
        Self::unavailable()
    }
    fn branch_tip(&self, branch: &str) -> GitResult<Option<String>> {
        lock(&self.calls).push(format!("branchTip {branch}"));
        Ok(self
            .answer("tips", branch, Value::Null)?
            .as_str()
            .map(str::to_string))
    }
    fn is_in_head(&self, _sha: &str) -> GitResult<bool> {
        Self::unavailable()
    }
    fn delete_branch(&self, branch: &str, sha: &str) -> GitResult<bool> {
        lock(&self.calls).push(format!("deleteBranch {branch} {sha}"));
        Ok(self
            .answer("delete", branch, json!(true))?
            .as_bool()
            .unwrap_or(true))
    }
    fn covered_reports(&self, head: &str, reports: &Value, options: &Value) -> GitResult<Value> {
        let ids: Vec<Value> = reports
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| r["reportId"].clone())
            .collect();
        lock(&self.calls).push(format!(
            "coveredReports {head} {} {}",
            Value::Array(ids),
            if options["memberCommits"].is_null() {
                Value::Null
            } else {
                options["memberCommits"].clone()
            }
        ));
        let covered = lock(&self.script)
            .git
            .get("covered")
            .cloned()
            .unwrap_or_else(|| json!([]));
        match covered["error"].as_str() {
            Some(spec) => Err(GitError {
                message: or_default(split_spec(spec).3, "Error"),
            }),
            None => Ok(covered),
        }
    }
}

fn replay_recover(label: &str, sequence: &Value, run: &Run) -> Result<(), String> {
    let entries = Arc::clone(&run.log);
    let sink: Logger = Arc::new(move |entry: &LogEntry| {
        lock(&entries).push(json!({
            "event": entry.command.strip_prefix("daemon:").unwrap_or(&entry.command),
            "details": entry.detail.clone().unwrap_or(Value::Null),
        }));
    });
    let options = DaemonOptions::new(
        run.scratch.state.clone(),
        project(),
        run.scratch.state.clone(),
    );
    let mut deps = Deps::new(run.kernel.clone(), options, sink);
    deps.git = Arc::new(StubGit {
        script: Arc::clone(&run.script),
        calls: Arc::clone(&run.calls),
    });
    let mut tables = Tables {
        previous: dump_tables(&run.scratch.database),
    };
    for (index, tick) in sequence["ticks"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let here = format!("{label} tick {index}");
        begin_tick(run, tick, &here)?;
        recover_integrations(&deps, &|| false);
        compare(&here, "calls", tick, json!(lock(&run.calls).clone()))?;
        compare(&here, "log", tick, json!(lock(&run.log).clone()))?;
        tables.check(&run.scratch, tick, &here)?;
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------------ the tests

fn parity_directory() -> PathBuf {
    common::crate_dir().join("tests").join("loops-parity")
}

fn groups() -> Vec<(String, Value)> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(parity_directory())
        .expect("tests/loops-parity exists")
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path).unwrap();
            let value: Value =
                serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            (
                path.file_stem().unwrap().to_string_lossy().to_string(),
                value,
            )
        })
        .collect()
}

#[test]
fn every_loops_sequence_replays() {
    let groups = groups();
    assert!(!groups.is_empty(), "no loops sequences found");
    let mut failures = Vec::new();
    let mut replayed = 0;
    for (group, file) in &groups {
        for sequence in file["sequences"].as_array().into_iter().flatten() {
            replayed += 1;
            if let Err(why) = replay_sequence(group, sequence) {
                failures.push(why);
            }
        }
    }
    assert!(replayed >= 20, "only {replayed} sequences");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn every_sequence_file_has_a_parity_file() {
    let root = common::repo_root();
    let sources: Vec<String> = std::fs::read_dir(root.join("test").join("loops-sequences"))
        .expect("test/loops-sequences exists")
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .filter(|name| name.ends_with(".json"))
        .map(|name| name.trim_end_matches(".json").to_string())
        .collect();
    let exported: Vec<String> = groups().into_iter().map(|(group, _)| group).collect();
    for source in &sources {
        assert!(
            exported.contains(source),
            "test/loops-sequences/{source}.json has no tests/loops-parity/{source}.json"
        );
    }
    for group in &exported {
        assert!(
            sources.contains(group),
            "tests/loops-parity/{group}.json has no source"
        );
    }
}

// ------------------------------------------------------------------------------------------------ shutdown

fn role_config(dir: &Path) -> capstan_config::RoleConfig {
    use std::os::unix::fs::PermissionsExt;
    let file = dir.join("capstan.toml");
    std::fs::write(
        &file,
        "schema_version = 1\n\n[hosts.claude]\nkind = \"claude\"\n\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n\n[roles.developer]\nkind = \"Developer\"\nhost = \"claude\"\n\n[roles.supervisor]\nkind = \"Supervisor\"\nhost = \"claude\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    capstan_config::load_role_config(dir).expect("the configuration loads")
}

fn deps_with_loops(dir: &Path, kernel: KernelHandle) -> (Deps, Arc<Mutex<Vec<LogEntry>>>) {
    let entries: Arc<Mutex<Vec<LogEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let sink: Logger = {
        let entries = Arc::clone(&entries);
        Arc::new(move |entry: &LogEntry| lock(&entries).push(entry.clone()))
    };
    let mut options = DaemonOptions::new(dir.to_path_buf(), project(), dir.to_path_buf());
    options.capstan = Some(Arc::new(role_config(dir)));
    options.tick_ms = Some(500);
    options.supervision_tick_ms = Some(50);
    let mut deps = Deps::new(kernel, options, sink);
    deps.adapter = Some(Arc::new(StubAdapter::new()));
    deps.launcher = Some(Arc::new(StubLauncher::new()));
    (deps, entries)
}

#[test]
fn the_loops_stop_within_the_drain_flush() {
    let (scratch, kernel) = open_kernel("loops-stop");
    let (mut deps, entries) = deps_with_loops(&scratch.state, kernel.clone());
    let loops = RustLoops::new();
    deps.driver = loops.driver_view();
    loops.start_ticks(&deps);
    loops.recover_and_relay(&deps);
    // Let every loop run a few ticks: the driver ticks at once, the supervision every 50 ms, the relay every 500 ms.
    std::thread::sleep(Duration::from_millis(1300));
    assert_eq!(deps.driver.snapshot().lost_agent_ids, Some(vec![]));
    let started = Instant::now();
    loops.stop_relay();
    loops.stop_ticks();
    let took = started.elapsed();
    // Node's drain flush is 1 s (DRAIN_FLUSH_MS of src/daemon.ts); a stop that waits for no tick interval is far inside it.
    assert!(
        took < Duration::from_millis(1000),
        "the loops took {took:?} to stop"
    );
    // Stopping twice is harmless, and nothing runs afterwards.
    loops.stop_relay();
    loops.stop_ticks();
    let before = lock(&entries).len();
    std::thread::sleep(Duration::from_millis(700));
    assert_eq!(
        lock(&entries).len(),
        before,
        "a loop kept running after the stop"
    );
    kernel.close();
}

// ------------------------------------------------------------------------------------------------ cost

#[test]
fn a_driver_tick_makes_the_calls_of_the_node_budget_whatever_the_ledger_holds() {
    // test/daemon-cost.test.ts holds the Node driver pass to at most 80 statements and 400 rows over a ledger with 500 agents
    // and a PM with 1046 messages. The parity replay holds the Rust driver to Node's exact per-method call counts; this adds
    // that those counts do not grow with the PM's mail: an idle tick over a PM with many messages reads its open mail once.
    let seed;
    let setup: Vec<Value> = {
        let groups = groups();
        let driver = groups.iter().find(|(g, _)| g == "driver").unwrap();
        seed = driver.1["sequences"][0]["seed"]
            .as_str()
            .unwrap()
            .to_string();
        driver.1["sequences"][0]["setup"]
            .as_array()
            .unwrap()
            .clone()
    };
    let (scratch, kernel) = open_kernel(&seed);
    for record in &setup {
        replay_step(&kernel, record, "cost setup").unwrap();
    }
    let script = Arc::new(Mutex::new(Script::new()));
    let calls: Calls = Arc::new(Mutex::new(Vec::new()));
    lock(&script).merge(&json!({
        "panes": {"pm-1": "pane-pm", "dev-1": "pane-dev", "ver-1": "pane-ver", "sup-1": "pane-sup"},
        "observe": {"pm-1": "idle", "dev-1": "idle", "ver-1": "idle", "sup-1": "idle"},
    }));
    let timers = json!({
        "maxDeferralSeconds": 3600, "maxBusyDeferralSeconds": 3600, "pmAckTimeoutSeconds": 3600,
        "pmNotifyAfterSeconds": 3600, "notifyIntervalSeconds": 3600, "stallAfterSeconds": 3600,
        "workerAckTimeoutSeconds": 3600, "pmWakeAfterSeconds": 3600, "pmWakeIntervalSeconds": 3600,
    });
    let make = |kernel: &KernelHandle| {
        Driver::new(DriverOptions {
            kernel: kernel.clone(),
            adapter: Arc::new(StubDriverAdapter {
                script: Arc::clone(&script),
                calls: Arc::clone(&calls),
            }),
            timers: timers.clone(),
            notifier: Arc::new(Notifier::new(
                Arc::new(StubNotifierAdapter {
                    script: Arc::clone(&script),
                    calls: Arc::clone(&calls),
                }),
                true,
                true,
                scratch.state.join("notifications.jsonl"),
                Arc::new(|| SEEDED_EPOCH_MS),
                Arc::new(|_, _| {}),
            )),
            credential: OWNER.into(),
            now: Arc::new(|| SEEDED_EPOCH_MS),
            log: Arc::new(|_, _| {}),
            tick_ms: None,
            process_probe: None,
            pm_stale_seconds: 1200,
            snapshot: None,
        })
    };
    let mut driver = make(&kernel);
    driver.tick();
    let quiet = driver.take_kernel_calls();
    // Fifty messages to the PM, none of them wakeable or notifiable yet: the tick reads the PM's open mail once more,
    // not once per message.
    for n in 0..50 {
        let body = format!("note {n}");
        kernel
            .run(move |core| {
                let context = json!({
                    "credential": OWNER,
                    "requestId": format!("bulk-{n}"),
                    "idempotencyKey": format!("bulk-{n}"),
                    "expectedVersion": core.state_version()?,
                    "inputRevision": core.input_revision()?,
                });
                core.dispatch(
                    "enqueueMessage",
                    &[context, json!({"recipientAgentId": "pm-1", "body": body})],
                )
            })
            .expect("a message is queued");
    }
    driver.tick();
    let busy = driver.take_kernel_calls();
    for method in [
        "openMessagesFor",
        "messagesFor",
        "advanceMessaging",
        "activeAgents",
    ] {
        assert_eq!(
            busy.get(method).copied().unwrap_or(0),
            quiet.get(method).copied().unwrap_or(0),
            "{method} grew with the PM's mail: {quiet:?} then {busy:?}"
        );
    }
    let total: u64 = busy.values().sum();
    assert!(total <= 80, "{total} kernel calls in one idle tick");
    kernel.close();
}
