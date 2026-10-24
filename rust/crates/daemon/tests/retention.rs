//! The ledger retention loop: `run_pass` on a real ledger under an injected clock, and `start` with `retention_days` at its
//! default and at 0. The kernel's clock is a shared number the test moves; its randomness is the seeded stream.

mod common;

use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::loops::retention::{run_pass, start, BATCH_ROWS};
use capstan_daemon::loops::StopFlag;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::{Core, Env, SeededEnv};
use common::project::Identity;
use common::{private_tempdir, PrivateDir};
use serde_json::json;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

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

struct World {
    dir: PrivateDir,
    deps: Deps,
    clock: Arc<AtomicI64>,
    serial: AtomicI64,
    logs: Arc<Mutex<Vec<LogEntry>>>,
}

impl World {
    /// `extra` is appended to the configuration; `None` opens the daemon without one.
    fn new(extra: Option<&str>) -> World {
        let dir = private_tempdir();
        let identity = Identity {
            project_id: "pretention".into(),
            name: "Retention".into(),
            credential: format!("operator-{}", "e".repeat(40)),
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
                        random: SeededEnv::new("retention"),
                    }),
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
        if let Some(extra) = extra {
            let text = format!("{CONFIG}{extra}");
            options.capstan = Some(Arc::new(
                capstan_config::parse_config(text.as_bytes(), dir.path())
                    .expect("the config loads"),
            ));
        }
        World {
            deps: Deps::new(kernel, options, sink),
            dir,
            clock,
            serial: AtomicI64::new(1),
            logs,
        }
    }

    fn at(&self, millis: i64) {
        self.clock.store(millis, Ordering::SeqCst);
    }

    /// `n` real mutations by the owner, each one `mutation_requests` row stamped with the clock.
    fn mutations(&self, n: usize) {
        let owner = self.deps.credential().to_string();
        for _ in 0..n {
            let hash = format!("{:064x}", self.serial.fetch_add(1, Ordering::SeqCst));
            let owner = owner.clone();
            self.deps
                .kernel
                .run(move |core| {
                    let hash = hash.clone();
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

    fn count(&self, table: &str) -> i64 {
        let database = self.dir.path().join(".capstan/state/controller.sqlite");
        count(&database, table)
    }

    fn logged(&self, code: &str) -> usize {
        self.logs
            .lock()
            .unwrap()
            .iter()
            .filter(|entry| entry.code == code || entry.command == code)
            .count()
    }
}

fn count(database: &Path, table: &str) -> i64 {
    rusqlite::Connection::open(database)
        .unwrap()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn a_pass_deletes_old_rows_in_batches_and_a_stop_ends_it_after_one() {
    let w = World::new(None);
    w.mutations(2300);
    w.at(T0 + 40 * DAY_MS);
    w.mutations(5);
    let (total, events) = (w.count("mutation_requests"), w.count("controller_events"));
    assert!(total >= 2305);
    let credential = w.deps.credential().to_string();

    let stopped = StopFlag::new();
    stopped.stop();
    assert_eq!(
        run_pass(&w.deps.kernel, &credential, 30, &stopped).deleted,
        BATCH_ROWS,
        "a stop ends the pass after one batch"
    );
    assert_eq!(w.count("mutation_requests"), total - BATCH_ROWS);

    let rest = run_pass(&w.deps.kernel, &credential, 30, &StopFlag::new()).deleted;
    assert_eq!(
        rest,
        total - 5 - BATCH_ROWS,
        "only the five new rows remain"
    );
    assert_eq!(w.count("mutation_requests"), 5);
    assert_eq!(w.count("controller_events"), events);
}

#[test]
fn a_pass_with_nothing_old_writes_nothing() {
    let w = World::new(None);
    w.mutations(20);
    let (requests, events) = (w.count("mutation_requests"), w.count("controller_events"));
    let credential = w.deps.credential().to_string();
    let pass = run_pass(&w.deps.kernel, &credential, 30, &StopFlag::new());
    assert_eq!((pass.deleted, pass.error), (0, None));
    assert_eq!(w.count("mutation_requests"), requests);
    assert_eq!(w.count("controller_events"), events);
}

#[test]
fn start_returns_none_without_retention_and_a_handle_with_the_default() {
    let off = World::new(Some("[ledger]\nretention_days = 0\n"));
    let stop = StopFlag::new();
    assert!(start(&off.deps, &stop).is_none());
    assert_eq!(off.logged("daemon:ledger_retention_start"), 0);

    let without_config = World::new(None);
    assert!(start(&without_config.deps, &stop).is_none());
    assert_eq!(without_config.logged("daemon:ledger_retention_start"), 0);

    let on = World::new(Some(""));
    let handle = start(&on.deps, &stop).expect("the default keeps thirty days");
    assert_eq!(on.logged("daemon:ledger_retention_start"), 1);
    stop.stop();
    handle.join().unwrap();
}

#[test]
fn a_failed_batch_ends_the_pass_with_its_error_and_deletes_nothing() {
    let w = World::new(None);
    w.mutations(3);
    w.at(T0 + 40 * DAY_MS);
    let requests = w.count("mutation_requests");
    let pass = run_pass(&w.deps.kernel, "not-a-credential", 30, &StopFlag::new());
    assert_eq!(pass.deleted, 0);
    assert!(pass.error.is_some(), "{pass:?}");
    assert_eq!(w.count("mutation_requests"), requests);
}
