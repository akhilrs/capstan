//! The idle-release loop against a real kernel on a test clock, with a stub launcher and a stub process sampler: the
//! warning ten minutes before the release, the release and its ledger row, everything that cancels it, and the
//! Supervisor rule that follows the last worker.

mod common;

use capstan_daemon::deps::{new_context, KernelHandle};
use capstan_daemon::loops::driver::{DriverLog, Steps};
use capstan_daemon::loops::idle_release::{
    IdleRelease, IdleReleaseOptions, ToolProcessSampler, RETRY_AFTER_FAILURE_MS,
};
use capstan_daemon::loops::supervision::{Supervision, SupervisionOptions};
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::types::InitialProject;
use capstan_kernel::Core;
use capstan_launcher::api::{LauncherError, StubLauncher};
use common::{private_tempdir, PrivateDir};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

const OWNER: &str = "owner-credential-0123456789-abcdefghijklmnopqrstuvwxyz";
const MINUTE: i64 = 60_000;
const IDLE: i64 = 120;
const WARN: i64 = 10;
/// A start far from zero, so no time is a default.
const BASE: i64 = 1_800_000_000_000;
const SHA: &str = "1111111111111111111111111111111111111111";

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

/// The kernel clock is the test clock.
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

/// Which agents have a tool process alive.
#[derive(Default)]
struct StubSampler {
    alive: Mutex<HashSet<String>>,
    asked: Mutex<Vec<String>>,
}

impl ToolProcessSampler for StubSampler {
    fn tool_process_alive(&self, agent_id: &str) -> Result<bool, String> {
        lock(&self.asked).push(agent_id.to_string());
        Ok(lock(&self.alive).contains(agent_id))
    }
}

struct World {
    _dir: PrivateDir,
    database: PathBuf,
    kernel: KernelHandle,
    clock: Arc<AtomicI64>,
    launcher: Arc<StubLauncher>,
    sampler: Arc<StubSampler>,
    child: Arc<Mutex<HashMap<String, i64>>>,
    log: Arc<Mutex<Vec<Value>>>,
    idle: IdleRelease,
}

fn log_sink(log: &Arc<Mutex<Vec<Value>>>) -> DriverLog {
    let log = Arc::clone(log);
    Arc::new(move |event, details| lock(&log).push(json!({"event": event, "details": details})))
}

fn call(kernel: &KernelHandle, op: &'static str, args: Vec<Value>) -> Value {
    kernel
        .run(move |core| {
            let context = serde_json::to_value(new_context(core, OWNER)?).unwrap();
            let mut all = vec![context];
            all.extend(args);
            core.dispatch(op, &all)
        })
        .unwrap_or_else(|e| panic!("{op}: {e}"))
}

fn register(kernel: &KernelHandle, agent: &str, role: &str, kind: &str) {
    let seat = format!("seat-{agent}");
    call(
        kernel,
        "createSeat",
        vec![json!({"seatId": seat, "name": seat, "role": kind})],
    );
    let actor = call(
        kernel,
        "createActor",
        vec![json!({"displayName": agent, "role": kind, "seatId": seat})],
    );
    call(
        kernel,
        "registerAgent",
        vec![
            json!({"agentId": agent, "roleName": role, "seatId": seat, "actorId": actor["actorId"]}),
        ],
    );
}

#[allow(clippy::too_many_arguments)]
fn make_idle(
    clock: &Arc<AtomicI64>,
    kernel: &KernelHandle,
    steps: &Steps,
    launcher: Arc<StubLauncher>,
    sampler: Arc<StubSampler>,
    child: Arc<Mutex<HashMap<String, i64>>>,
    log: &Arc<Mutex<Vec<Value>>>,
    idle: i64,
) -> IdleRelease {
    let _ = kernel;
    let now = Arc::clone(clock);
    IdleRelease::new(IdleReleaseOptions {
        steps: steps.clone(),
        launcher,
        idle_minutes: idle,
        warn_minutes: WARN,
        now: Arc::new(move || now.load(Ordering::SeqCst)),
        log: log_sink(log),
        child_activity: child,
        sampler,
    })
}

fn world() -> World {
    let dir = private_tempdir();
    let state = dir.path().to_path_buf();
    let database = capstan_ledger::resolve_database_path(&state).expect("the ledger path");
    capstan_ledger::open_database(&database, &capstan_ledger::OpenOptions::default())
        .expect("the ledger migrates");
    let clock = Arc::new(AtomicI64::new(BASE));
    let (opened, env_clock) = (state.clone(), Arc::clone(&clock));
    let kernel = KernelHandle::spawn(move || {
        Core::open(
            &opened,
            &InitialProject {
                project_id: "proj1".into(),
                name: "Idle Project".into(),
                owner_credential: OWNER.into(),
                initial_inputs: capstan_daemon::run::placeholder_inputs(),
            },
            &KernelOptions {
                workspace_root: Some(opened.clone()),
                ..KernelOptions::default()
            },
            Box::new(TestEnv {
                clock: env_clock,
                inner: capstan_kernel::SeededEnv::new("idle-release"),
            }),
        )
    })
    .expect("the kernel opens");
    call(
        &kernel,
        "syncRoleDefinitions",
        vec![json!([
            {"name": "pm", "kind": "PM", "host": "claude", "configHash": "b".repeat(64)},
            {"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)},
            {"name": "architect", "kind": "Developer", "host": "claude", "configHash": "e".repeat(64)},
            {"name": "supervisor", "kind": "Supervisor", "host": "claude", "configHash": "c".repeat(64)},
        ])],
    );
    register(&kernel, "pm-1", "pm", "PM");
    register(&kernel, "dev-1", "developer", "Developer");
    let launcher = Arc::new(StubLauncher::new());
    let sampler = Arc::new(StubSampler::default());
    let child = Arc::new(Mutex::new(HashMap::new()));
    let log = Arc::new(Mutex::new(Vec::new()));
    let steps = Steps::new(kernel.clone(), OWNER.into());
    let idle = make_idle(
        &clock,
        &kernel,
        &steps,
        launcher.clone(),
        sampler.clone(),
        child.clone(),
        &log,
        IDLE,
    );
    World {
        _dir: dir,
        database,
        kernel,
        clock,
        launcher,
        sampler,
        child,
        log,
        idle,
    }
}

impl World {
    fn now(&self) -> i64 {
        self.clock.load(Ordering::SeqCst)
    }

    /// Advances the clock by `minutes`, one minute at a time, ticking the loop after each minute.
    fn run_minutes(&mut self, minutes: i64) {
        for _ in 0..minutes {
            self.clock.fetch_add(MINUTE, Ordering::SeqCst);
            self.idle.tick();
        }
    }

    fn advance(&self, minutes: i64) {
        self.clock.fetch_add(minutes * MINUTE, Ordering::SeqCst);
    }

    /// The agent shows work and goes idle again now: what activity looks like in the ledger.
    fn activity(&self, agent: &str) {
        for state in ["working", "idle"] {
            let agent = agent.to_string();
            self.kernel
                .run(move |core| {
                    let context = new_context(core, OWNER)?;
                    core.record_agent_observation(&context, &agent, &json!(state))
                })
                .unwrap();
        }
    }

    fn pm_notes(&self) -> Vec<String> {
        let messages = self.kernel.run(|core| core.messages_for("pm-1")).unwrap();
        messages
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["body"].as_str().unwrap().to_string())
            .collect()
    }

    fn warnings(&self) -> Vec<String> {
        self.pm_notes()
            .into_iter()
            .filter(|n| n.contains("has been idle since"))
            .collect()
    }

    fn releases(&self) -> usize {
        self.launcher
            .calls()
            .iter()
            .filter(|c| c.starts_with("release dev-1"))
            .count()
    }

    fn events(&self, entity_type: &str) -> Vec<(String, Option<String>, Option<String>, Value)> {
        let db = rusqlite::Connection::open(&self.database).unwrap();
        let mut statement = db
            .prepare("SELECT entity_id, from_state, to_state, payload_json FROM controller_events WHERE entity_type = ? ORDER BY sequence")
            .unwrap();
        statement
            .query_map([entity_type], |r| {
                let text: String = r.get(3)?;
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    serde_json::from_str(&text).unwrap(),
                ))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn sql(&self, statements: &str) {
        let project: String = rusqlite::Connection::open(&self.database)
            .unwrap()
            .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
            .unwrap();
        let db = rusqlite::Connection::open(&self.database).unwrap();
        db.execute_batch(&statements.replace("@P@", &format!("'{project}'")))
            .unwrap_or_else(|e| panic!("{statements}: {e}"));
    }

    fn actor_of(&self, agent: &str) -> String {
        let agent = agent.to_string();
        self.kernel
            .run(move |core| core.agent_record(&agent))
            .unwrap()["actorId"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn active(&self) -> Vec<String> {
        let agents = self.kernel.run(|core| core.active_agents()).unwrap();
        agents
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["agentId"].as_str().unwrap().to_string())
            .collect()
    }

    fn end(&self, agent: &str) {
        let agent = agent.to_string();
        self.kernel
            .run(move |core| {
                let context = new_context(core, OWNER)?;
                core.end_agent(&context, &agent, None)
            })
            .unwrap();
    }
}

/// A world whose developer went idle two hours after the daemon started (long after the grace), at the returned time.
fn idle_developer() -> (World, i64) {
    let mut w = world();
    w.advance(3 * 60);
    w.activity("dev-1");
    let since = w.now();
    w.run_minutes(0);
    (w, since)
}

#[test]
fn an_idle_developer_is_warned_ten_minutes_early_and_released_on_time() {
    let (mut w, since) = idle_developer();
    w.run_minutes(IDLE - WARN - 1);
    assert!(w.warnings().is_empty(), "{:?}", w.pm_notes());
    assert_eq!(w.releases(), 0);
    w.run_minutes(1);
    let warnings = w.warnings();
    assert_eq!(warnings.len(), 1, "{:?}", w.pm_notes());
    let since_iso = capstan_ledger::iso_from_millis(since);
    let release_iso = capstan_ledger::iso_from_millis(since + IDLE * MINUTE);
    assert_eq!(
        warnings[0],
        format!("dev-1 (developer) has been idle since {since_iso} and has no open work; it will be released at {release_iso}. To keep it: cstan keep dev-1 [hours], or give it work.")
    );
    // The warning is one ledger row, and nothing is released before the time.
    assert_eq!(w.events("agent_idle_warning").len(), 1);
    w.run_minutes(WARN - 1);
    assert_eq!(w.releases(), 0);
    assert_eq!(w.warnings().len(), 1);
    w.run_minutes(1);
    assert_eq!(w.releases(), 1, "{:?}", lock(&w.log));
    // The launcher ended the pane; the ledger row and the PM note follow.
    let rows = w.events("agent_auto_release");
    assert_eq!(rows.len(), 1, "{rows:?}");
    let release = &rows[0];
    assert_eq!(release.0.as_str(), "dev-1");
    assert_eq!(release.3["details"]["reason"], "idle_auto_release");
    let details = &release.3["details"];
    assert_eq!(details["idleSince"], since_iso);
    assert_eq!(details["idleMinutes"], IDLE);
    assert_eq!(
        details["warnedAt"],
        capstan_ledger::iso_from_millis(since + (IDLE - WARN) * MINUTE)
    );
    assert_eq!(details["keptBranches"], json!([]));
    let notes = w.pm_notes();
    assert!(
        notes.contains(
            &"dev-1 was released after 120 minutes idle; branches kept: none".to_string()
        ),
        "{notes:?}"
    );
    // Both notes are info messages.
    let messages = w.kernel.run(|core| core.messages_for("pm-1")).unwrap();
    assert!(messages
        .as_array()
        .unwrap()
        .iter()
        .all(|m| m["class"] == "info"));
    // The release ends the episode: no second release or warning follows.
    w.end("dev-1");
    w.run_minutes(60);
    assert_eq!(w.releases(), 1);
    assert_eq!(w.warnings().len(), 1);
}

#[test]
fn activity_between_the_warning_and_the_release_needs_a_new_warning() {
    let (mut w, _) = idle_developer();
    w.run_minutes(IDLE - WARN);
    assert_eq!(w.warnings().len(), 1);
    w.run_minutes(5);
    w.activity("dev-1");
    let second_since = w.now();
    // The old release time passes without a release.
    w.run_minutes(WARN);
    assert_eq!(w.releases(), 0);
    assert_eq!(w.warnings().len(), 1);
    // The new episode is warned at its own T-10 and released at its own T.
    w.run_minutes(IDLE - WARN - WARN);
    assert_eq!(w.warnings().len(), 2, "{:?}", w.pm_notes());
    assert_eq!(w.now(), second_since + (IDLE - WARN) * MINUTE);
    w.run_minutes(WARN);
    assert_eq!(w.releases(), 1);
}

#[test]
fn work_that_appears_and_resolves_after_a_warning_needs_a_new_warning() {
    let (mut w, _) = idle_developer();
    w.run_minutes(IDLE - WARN);
    assert_eq!(w.warnings().len(), 1);
    // A queued message is open work and does not move the idle-since time.
    w.kernel
        .run(|core| {
            let context = new_context(core, OWNER)?;
            core.enqueue_message(
                &context,
                &json!({"recipientAgentId": "dev-1", "body": "look at this"}),
            )
        })
        .unwrap();
    w.run_minutes(1);
    assert_eq!(w.releases(), 0);
    // The work resolves; idle-since is the same, the old warning must not count.
    w.sql("DELETE FROM messages WHERE recipient_agent_id = 'dev-1'");
    w.run_minutes(1);
    assert_eq!(w.releases(), 0, "{:?}", w.pm_notes());
    assert_eq!(w.warnings().len(), 2, "{:?}", w.pm_notes());
    w.run_minutes(WARN - 1);
    assert_eq!(w.releases(), 0);
    w.run_minutes(1);
    assert_eq!(w.releases(), 1);
}

#[test]
fn an_incoming_message_between_the_warning_and_the_release_cancels_it() {
    let (mut w, _) = idle_developer();
    w.run_minutes(IDLE - WARN);
    assert_eq!(w.warnings().len(), 1);
    w.kernel
        .run(|core| {
            let context = new_context(core, OWNER)?;
            core.enqueue_message(
                &context,
                &json!({"recipientAgentId": "dev-1", "body": "new task"}),
            )
        })
        .unwrap();
    w.run_minutes(WARN + 5);
    assert_eq!(w.releases(), 0);
    // Delivered and acknowledged: the idle clock restarts and the warning is given again.
    let sent = w.now();
    w.sql(&format!(
        "UPDATE messages SET state = 'acked', sent_at = '{}', acked_at = '{}' WHERE recipient_agent_id = 'dev-1'",
        capstan_ledger::iso_from_millis(sent),
        capstan_ledger::iso_from_millis(sent)
    ));
    w.run_minutes(IDLE - WARN);
    assert_eq!(w.warnings().len(), 2, "{:?}", w.pm_notes());
    assert_eq!(w.releases(), 0);
    w.run_minutes(WARN);
    assert_eq!(w.releases(), 1);
}

#[test]
fn a_live_tool_process_between_the_warning_and_the_release_cancels_it() {
    let (mut w, _) = idle_developer();
    w.run_minutes(IDLE - WARN);
    assert_eq!(w.warnings().len(), 1);
    w.run_minutes(WARN - 1);
    lock(&w.sampler.alive).insert("dev-1".into());
    w.run_minutes(1);
    assert_eq!(w.releases(), 0);
    lock(&w.sampler.alive).clear();
    // The process left now; the next release needs a warning of its own.
    w.run_minutes(IDLE - WARN - 1);
    assert_eq!(w.warnings().len(), 1);
    w.run_minutes(1);
    assert_eq!(w.warnings().len(), 2, "{:?}", w.pm_notes());
    assert_eq!(w.releases(), 0);
    w.run_minutes(WARN);
    assert_eq!(w.releases(), 1);
}

#[test]
fn a_build_that_runs_past_the_deadline_is_never_warned() {
    let (mut w, _) = idle_developer();
    // The driver sees a tool process alive every minute for five hours; the sampler agrees.
    lock(&w.sampler.alive).insert("dev-1".into());
    for _ in 0..5 * 60 {
        let now = w.now();
        lock(&w.child).insert("dev-1".into(), now);
        w.run_minutes(1);
    }
    assert!(w.warnings().is_empty(), "{:?}", w.pm_notes());
    assert_eq!(w.releases(), 0);
    // Only the driver's map (no live process at the fresh look): the map keeps the agent busy while it is fresh.
    lock(&w.sampler.alive).clear();
    let last = w.now();
    lock(&w.child).insert("dev-1".into(), last);
    w.run_minutes(IDLE - WARN - 1);
    assert!(w.warnings().is_empty());
    // And a live process found only by the fresh sample just before the warning stops it.
    lock(&w.sampler.alive).insert("dev-1".into());
    w.run_minutes(5);
    assert!(w.warnings().is_empty());
}

#[test]
fn an_architect_with_a_plan_in_review_and_a_developer_with_a_report_are_never_warned() {
    let (mut w, _) = idle_developer();
    // dev-1 now holds an unmerged report.
    let actor = w.actor_of("dev-1");
    w.sql(&format!(
        "INSERT INTO agent_reports(project_id, report_id, sequence, agent_id, generation, actor_id, commit_sha, branch, summary,
           state, reason, evidence_json, created_at)
         VALUES (@P@, 'rep-1', 1, 'dev-1', 1, '{actor}', '{SHA}', 'b', 's', 'accepted', NULL, '{{}}', '2026-01-01T00:00:00.000Z')"
    ));
    // arch-1 is the architect of a plan in review.
    register(&w.kernel, "arch-1", "architect", "Developer");
    let arch_actor = w.actor_of("arch-1");
    w.sql(&format!(
        "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, architect_agent_id, current_revision,
           created_at, updated_at)
         VALUES (@P@, 'plan-1', 1, 't', 'normal', 'draft', 'pm-1', 'arch-1', 0, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z');
         INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
         VALUES (@P@, 'plan-1', 1, '{SHA}', '{{\"packages\":[]}}', '{}', 'arch-1', '{arch_actor}', '2026-01-01T00:00:00.000Z');
         UPDATE plans SET current_revision = 1, state = 'in_review' WHERE plan_id = 'plan-1';",
        "c".repeat(64)
    ));
    w.activity("arch-1");
    w.run_minutes(IDLE + 60);
    assert!(w.warnings().is_empty(), "{:?}", w.pm_notes());
    assert!(w.launcher.calls().iter().all(|c| !c.starts_with("release")));
}

#[test]
fn a_failed_release_is_logged_noted_once_and_retried_after_thirty_minutes() {
    let (mut w, _) = idle_developer();
    w.launcher.fail_next(
        "release",
        LauncherError::new("pane_busy", "the pane would not close"),
    );
    w.run_minutes(IDLE);
    assert_eq!(w.releases(), 1);
    assert!(lock(&w.log)
        .iter()
        .any(|e| e["event"] == "idle_release_failed" && e["details"]["agentId"] == "dev-1"));
    let errors: Vec<String> = w
        .pm_notes()
        .into_iter()
        .filter(|n| n.contains("the pane would not close"))
        .collect();
    assert_eq!(errors.len(), 1, "{:?}", w.pm_notes());
    assert!(w.events("agent_auto_release").is_empty());
    // Nothing is tried for thirty minutes, then the release goes through without a new warning.
    w.run_minutes(RETRY_AFTER_FAILURE_MS / MINUTE - 1);
    assert_eq!(w.releases(), 1);
    w.run_minutes(1);
    assert_eq!(w.releases(), 2);
    assert_eq!(w.warnings().len(), 1);
    assert_eq!(
        w.pm_notes()
            .iter()
            .filter(|n| n.contains("the pane would not close"))
            .count(),
        1
    );
}

#[test]
fn zero_minutes_sends_nothing() {
    let mut w = world();
    let steps = Steps::new(w.kernel.clone(), OWNER.into());
    w.idle = make_idle(
        &w.clock,
        &w.kernel,
        &steps,
        w.launcher.clone(),
        w.sampler.clone(),
        w.child.clone(),
        &w.log,
        0,
    );
    w.advance(3 * 60);
    w.activity("dev-1");
    w.run_minutes(6 * 60);
    assert!(w.pm_notes().is_empty());
    assert!(w.launcher.calls().is_empty());
}

#[test]
fn after_a_daemon_restart_nothing_is_released_within_the_grace() {
    let mut w = world();
    w.advance(5 * 60);
    w.activity("dev-1");
    w.advance(5 * 60);
    // The daemon starts now: its loop is new, the agent has been idle for five hours.
    let steps = Steps::new(w.kernel.clone(), OWNER.into());
    w.idle = make_idle(
        &w.clock,
        &w.kernel,
        &steps,
        w.launcher.clone(),
        w.sampler.clone(),
        w.child.clone(),
        &w.log,
        IDLE,
    );
    w.run_minutes(IDLE - 1);
    assert!(w.warnings().is_empty(), "{:?}", w.pm_notes());
    assert_eq!(w.releases(), 0);
    w.run_minutes(1);
    assert_eq!(w.warnings().len(), 1);
    w.run_minutes(WARN - 1);
    assert_eq!(w.releases(), 0);
    w.run_minutes(1);
    assert_eq!(w.releases(), 1);
}

#[test]
fn the_last_released_worker_leaves_only_the_pm_once_the_supervisor_rule_runs() {
    let mut w = world();
    register(&w.kernel, "sup-1", "supervisor", "Supervisor");
    let supervisor_launcher = Arc::new(StubLauncher::new());
    let now = Arc::clone(&w.clock);
    let mut supervision = Supervision::new(SupervisionOptions {
        steps: Steps::new(w.kernel.clone(), OWNER.into()),
        launcher: supervisor_launcher.clone(),
        enabled: true,
        check_seconds: 60.0,
        supervisor_role: Some("supervisor".into()),
        now: Arc::new(move || now.load(Ordering::SeqCst)),
        log: log_sink(&w.log),
    });
    supervision.tick();
    w.advance(3 * 60);
    w.activity("dev-1");
    for _ in 0..IDLE {
        w.run_minutes(1);
        supervision.tick();
    }
    assert_eq!(w.releases(), 1);
    // The launcher closes the pane and the agent ends.
    w.end("dev-1");
    assert_eq!(w.active(), vec!["pm-1".to_string(), "sup-1".to_string()]);
    // Nine minutes without a worker: the Supervisor stays; the tenth releases it.
    w.advance(9);
    supervision.tick();
    assert!(supervisor_launcher
        .calls()
        .iter()
        .all(|c| !c.starts_with("release")));
    w.advance(1);
    supervision.tick();
    assert!(
        supervisor_launcher
            .calls()
            .iter()
            .any(|c| c.starts_with("release sup-1")),
        "{:?}",
        supervisor_launcher.calls()
    );
    w.end("sup-1");
    assert_eq!(w.active(), vec!["pm-1".to_string()]);
    // A new worker brings a Supervisor back.
    register(&w.kernel, "dev-2", "developer", "Developer");
    supervision.tick();
    assert!(
        supervisor_launcher
            .calls()
            .iter()
            .any(|c| c.starts_with("spawn supervisor")),
        "{:?}",
        supervisor_launcher.calls()
    );
}
