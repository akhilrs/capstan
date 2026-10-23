//! Retention of the mutation request ledger: what a prune deletes and what it leaves, how a replay behaves once its row is
//! gone, and (ignored, `--release`) what a batch costs on 150k rows. Rows are written by real `Kernel::mutate` calls under an
//! injected clock; raw SQL only counts rows or lays down the bulk rows of the cost figures.

mod common;
#[path = "rust_only/fixture.rs"]
mod fixture;

use capstan_kernel::areas::retention::prune_mutation_requests;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::{Core, Env, KernelError, SeededEnv};
use capstan_ledger::iso_from_millis;
use fixture::{context, initial, step, OWNER};
use serde_json::{json, Value};
use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

const DAY_MS: i64 = 86_400_000;
const T0: i64 = 1_800_000_000_000;

/// The clock is whatever the test set; uuids and random bytes are the seeded stream.
struct ClockEnv {
    clock: Rc<Cell<i64>>,
    random: SeededEnv,
}

impl Env for ClockEnv {
    fn now(&self) -> i64 {
        self.clock.get()
    }
    fn uuid(&self) -> String {
        self.random.uuid()
    }
    fn random_bytes(&self, n: usize) -> Vec<u8> {
        self.random.random_bytes(n)
    }
}

struct World {
    _dir: tempfile::TempDir,
    core: Core,
    clock: Rc<Cell<i64>>,
    database: std::path::PathBuf,
    developer: String,
}

impl World {
    /// A project with an owner, a PM and a developer, all registered at `T0`.
    fn new() -> World {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let state = dir.path().join("state");
        std::fs::create_dir(&state).unwrap();
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
        let clock = Rc::new(Cell::new(T0));
        let core = Core::open(
            &state,
            &initial(),
            &KernelOptions::default(),
            Box::new(ClockEnv {
                clock: Rc::clone(&clock),
                random: SeededEnv::new("retention"),
            }),
        )
        .expect("the ledger opens");
        step(
            &core,
            "syncRoleDefinitions",
            json!([[
                {"name": "pm", "kind": "PM", "host": "claude", "configHash": "b".repeat(64)},
                {"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)},
            ]]),
        );
        let mut developer = String::new();
        for (agent, role_name, kind) in [("pm-1", "pm", "PM"), ("dev-1", "developer", "Developer")]
        {
            let seat = format!("seat-{agent}");
            step(
                &core,
                "createSeat",
                json!([{"seatId": seat, "name": seat, "role": kind}]),
            );
            let actor = step(
                &core,
                "createActor",
                json!([{"displayName": agent, "role": kind, "seatId": seat}]),
            );
            step(
                &core,
                "registerAgent",
                json!([{"agentId": agent, "roleName": role_name, "seatId": seat, "actorId": actor["actorId"]}]),
            );
            developer = actor["credential"].as_str().unwrap().to_string();
        }
        World {
            database: state.join("controller.sqlite"),
            _dir: dir,
            core,
            clock,
            developer,
        }
    }

    fn at(&self, millis: i64) {
        self.clock.set(millis);
    }

    fn sql(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(&self.database).unwrap()
    }

    fn count(&self, table: &str) -> i64 {
        self.sql()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    /// Every table with its row count, `mutation_requests` left out.
    fn other_counts(&self) -> Vec<(String, i64)> {
        let sql = self.sql();
        let mut names: Vec<String> = sql
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|n| n.unwrap())
            .collect();
        names.retain(|n| n != "mutation_requests");
        names
            .into_iter()
            .map(|n| (n.clone(), self.count(&n)))
            .collect()
    }

    /// A message from the owner to the PM: one real mutation, one `mutation_requests` row stamped with the clock.
    fn send(&self, body: &str) -> Value {
        let ctx = context(&self.core, OWNER);
        self.core
            .enqueue_message(&ctx, &json!({"recipientAgentId": "pm-1", "body": body}))
            .unwrap()
    }

    fn prune(&self, credential: &str, cutoff: i64, batch: i64) -> Result<i64, KernelError> {
        prune_mutation_requests(
            self.core.kernel(),
            credential,
            &iso_from_millis(cutoff),
            batch,
        )
    }

    fn version(&self) -> i64 {
        self.core.state_version().unwrap()
    }
}

#[test]
fn prune_deletes_exactly_the_rows_before_the_cutoff_in_batches() {
    let w = World::new();
    for n in 0..5 {
        w.send(&format!("old {n}"));
    }
    let cutoff = T0 + 5 * DAY_MS;
    w.at(cutoff);
    w.send("at the cutoff");
    w.at(cutoff + DAY_MS);
    for n in 0..3 {
        w.send(&format!("new {n}"));
    }
    // A second project's rows are not this project's to delete.
    let sql = w.sql();
    sql.execute(
        "INSERT INTO projects(project_id, name, current_input_revision, state_version, created_at) VALUES ('pother', 'other', 1, 1, '2000-01-01T00:00:00.000Z')",
        [],
    )
    .unwrap();
    let actor: String = sql
        .query_row("SELECT actor_id FROM actors LIMIT 1", [], |r| r.get(0))
        .unwrap();
    for n in 0..4 {
        sql.execute(
            "INSERT INTO mutation_requests VALUES ('pother', ?1, ?1, ?2, ?3, '{}', '2000-01-01T00:00:00.000Z')",
            rusqlite::params![format!("k{n}"), actor, "0".repeat(64)],
        )
        .unwrap();
    }
    drop(sql);

    let old = {
        let boundary = iso_from_millis(cutoff);
        w.sql()
            .query_row(
                "SELECT COUNT(*) FROM mutation_requests WHERE project_id = 'prustonly' AND created_at < ?1",
                [boundary],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
    };
    assert!(old > 5, "the setup rows are old too: {old}");
    let (total, others, version) = (w.count("mutation_requests"), w.other_counts(), w.version());

    let mut deleted = Vec::new();
    loop {
        let n = w.prune(OWNER, cutoff, 3).unwrap();
        assert!(n <= 3);
        deleted.push(n);
        if n < 3 {
            break;
        }
    }
    assert_eq!(deleted.iter().sum::<i64>(), old);
    assert!(deleted[..deleted.len() - 1].iter().all(|n| *n == 3));
    assert_eq!(w.count("mutation_requests"), total - old);
    let newest_old: i64 = w
        .sql()
        .query_row(
            "SELECT COUNT(*) FROM mutation_requests WHERE project_id = 'prustonly' AND created_at < ?1",
            [iso_from_millis(cutoff)],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(newest_old, 0);
    let kept_at_cutoff: i64 = w
        .sql()
        .query_row(
            "SELECT COUNT(*) FROM mutation_requests WHERE created_at = ?1",
            [iso_from_millis(cutoff)],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(kept_at_cutoff, 1, "a row stamped at the cutoff is kept");
    let other_project: i64 = w
        .sql()
        .query_row(
            "SELECT COUNT(*) FROM mutation_requests WHERE project_id = 'pother'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(other_project, 4);
    assert_eq!(w.other_counts(), others, "no other table changes");
    assert_eq!(w.version(), version, "the prune is not a mutation");
    assert_eq!(w.prune(OWNER, cutoff, 3).unwrap(), 0);
}

#[test]
fn a_credential_without_the_capability_deletes_nothing() {
    let w = World::new();
    w.send("old");
    let total = w.count("mutation_requests");
    for credential in ["not-a-credential", w.developer.as_str()] {
        assert!(w.prune(credential, T0 + DAY_MS, 100).is_err());
    }
    assert_eq!(w.count("mutation_requests"), total);
}

#[test]
fn replay_inside_the_window_returns_the_stored_result_and_after_a_prune_conflicts() {
    let w = World::new();
    // Message send.
    let ctx = context(&w.core, OWNER);
    let input = json!({"recipientAgentId": "pm-1", "body": "hello"});
    let first = w.core.enqueue_message(&ctx, &input).unwrap();
    assert_eq!(w.core.enqueue_message(&ctx, &input).unwrap(), first);
    // Actor registration: the credential comes back from the stored, encrypted result.
    let reg = context(&w.core, OWNER);
    let reg_input = json!({"displayName": "late", "role": "operator"});
    let registered = w.core.create_actor(&reg, &reg_input).unwrap();
    assert_eq!(w.core.create_actor(&reg, &reg_input).unwrap(), registered);

    let (events, version, total) = (
        w.count("controller_events"),
        w.version(),
        w.count("mutation_requests"),
    );
    w.at(T0 + 40 * DAY_MS);
    let pruned = w.prune(OWNER, T0 + 30 * DAY_MS, 1000).unwrap();
    assert_eq!(pruned, total);
    for replay in [
        w.core.enqueue_message(&ctx, &input).unwrap_err(),
        w.core.create_actor(&reg, &reg_input).unwrap_err(),
    ] {
        assert!(
            matches!(replay, KernelError::StateVersionConflict(_)),
            "{replay:?}"
        );
    }
    assert_eq!(w.count("controller_events"), events);
    assert_eq!(w.count("mutation_requests"), 0);
    assert_eq!(w.version(), version);
}

#[test]
fn a_pruned_request_id_can_be_used_again_with_the_current_version() {
    let w = World::new();
    let ctx = context(&w.core, OWNER);
    w.core
        .enqueue_message(&ctx, &json!({"recipientAgentId": "pm-1", "body": "first"}))
        .unwrap();
    let mut again = context(&w.core, OWNER);
    again.request_id = ctx.request_id.clone();
    again.idempotency_key = "idem-again".into();
    let input = json!({"recipientAgentId": "pm-1", "body": "second"});
    let refused = w.core.enqueue_message(&again, &input).unwrap_err();
    assert!(
        matches!(refused, KernelError::IdempotencyConflict(_)),
        "{refused:?}"
    );
    w.at(T0 + 40 * DAY_MS);
    w.prune(OWNER, T0 + 30 * DAY_MS, 1000).unwrap();
    w.core.enqueue_message(&again, &input).unwrap();
}

#[test]
fn fast_paths_take_their_no_op_branch_for_a_pruned_key() {
    let w = World::new();
    // record_agent_observation: the first call records, a replay returns the stored result.
    let observe = context(&w.core, OWNER);
    let recorded = w
        .core
        .record_agent_observation(&observe, "dev-1", &json!("working"))
        .unwrap();
    assert_eq!(recorded, json!({"recorded": true}));
    assert_eq!(
        w.core
            .record_agent_observation(&observe, "dev-1", &json!("working"))
            .unwrap(),
        recorded
    );
    // advance_messaging: a message the PM has not answered makes the first call transition; a later call finds nothing due.
    w.send("wake up");
    let timers = json!({
        "maxDeferralSeconds": 60, "maxBusyDeferralSeconds": 600, "pmAckTimeoutSeconds": 600,
        "pmNotifyAfterSeconds": 100, "notifyIntervalSeconds": 1_000_000, "stallAfterSeconds": 100_000_000,
        "workerAckTimeoutSeconds": 600, "pmWakeAfterSeconds": 10, "pmWakeIntervalSeconds": 1_000_000,
    });
    w.at(T0 + 1_000_000);
    let advance = context(&w.core, OWNER);
    let advanced = w.core.advance_messaging(&advance, &timers).unwrap();
    assert_eq!(
        w.core.advance_messaging(&advance, &timers).unwrap(),
        advanced
    );
    // end_wait_as_controller: a wait the developer opened and the controller closed.
    let began = w.core.begin_wait(&context(&w.core, &w.developer)).unwrap();
    let wait_id = began["waitId"].as_str().unwrap().to_string();
    let end = context(&w.core, OWNER);
    let ended = w.core.end_wait_as_controller(&end, &wait_id).unwrap();
    assert_eq!(ended, json!({"ended": true}));
    assert_eq!(
        w.core.end_wait_as_controller(&end, &wait_id).unwrap(),
        ended
    );

    w.at(T0 + 100 * DAY_MS);
    w.prune(OWNER, T0 + 50 * DAY_MS, 1000).unwrap();
    assert_eq!(w.count("mutation_requests"), 0);
    let (events, version) = (w.count("controller_events"), w.version());
    assert_eq!(
        w.core
            .record_agent_observation(&observe, "dev-1", &json!("working"))
            .unwrap(),
        json!({"recorded": false})
    );
    w.core.advance_messaging(&advance, &timers).unwrap();
    assert_eq!(
        w.core.end_wait_as_controller(&end, &wait_id).unwrap(),
        json!({"ended": false})
    );
    assert_eq!(
        w.count("mutation_requests"),
        0,
        "the fast paths write no request row"
    );
    assert_eq!(w.count("controller_events"), events);
    assert_eq!(w.version(), version);
}

#[test]
fn controller_events_stay_immutable_after_a_prune() {
    let w = World::new();
    w.send("old");
    w.at(T0 + 40 * DAY_MS);
    assert!(w.prune(OWNER, T0 + 30 * DAY_MS, 1000).unwrap() > 0);
    let error = w
        .sql()
        .execute("DELETE FROM controller_events", [])
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("controller events are immutable"),
        "{error}"
    );
}

// ---------------------------------------------------------------------------------------------------- cost figures

struct Figures {
    max_batch_ms: f64,
    mean_batch_ms: f64,
    full_ms: f64,
    noop_ms: f64,
}

fn figures(layout: &str, interleaved: bool) -> Figures {
    const ROWS: usize = 150_000;
    let w = World::new();
    let cutoff = T0 + 30 * DAY_MS;
    let old_stamp = iso_from_millis(T0);
    let new_stamp = iso_from_millis(cutoff + DAY_MS);
    let mut sql = w.sql();
    let actor: String = sql
        .query_row("SELECT actor_id FROM actors LIMIT 1", [], |r| r.get(0))
        .unwrap();
    let hash = "0".repeat(64);
    let seed = SeededEnv::new(layout);
    let transaction = sql.transaction().unwrap();
    {
        let mut insert = transaction
            .prepare("INSERT INTO mutation_requests VALUES ('prustonly', ?1, ?1, ?2, ?3, '{\"ok\":true}', ?4)")
            .unwrap();
        for n in 0..ROWS {
            // Interleaved: random uuid keys, half old; contiguous: the old rows are the first half of the key order.
            let (key, stamp) = if interleaved {
                (
                    format!("idem-{}", seed.uuid()),
                    if n % 2 == 0 { &old_stamp } else { &new_stamp },
                )
            } else {
                (
                    format!("idem-{n:08}"),
                    if n < ROWS / 2 { &old_stamp } else { &new_stamp },
                )
            };
            insert
                .execute(rusqlite::params![key, actor, hash, stamp])
                .unwrap();
        }
    }
    transaction.commit().unwrap();
    drop(sql);

    // A pass with nothing old, measured first on the rows all newer than a cutoff before them.
    let started = Instant::now();
    assert_eq!(w.prune(OWNER, T0 - DAY_MS, 1000).unwrap(), 0);
    let noop_ms = started.elapsed().as_secs_f64() * 1000.0;

    let started = Instant::now();
    let mut batches = Vec::new();
    loop {
        let begun = Instant::now();
        let n = w.prune(OWNER, cutoff, 1000).unwrap();
        batches.push(begun.elapsed().as_secs_f64() * 1000.0);
        if n < 1000 {
            break;
        }
    }
    let full_ms = started.elapsed().as_secs_f64() * 1000.0;
    Figures {
        max_batch_ms: batches.iter().cloned().fold(0.0, f64::max),
        mean_batch_ms: batches.iter().sum::<f64>() / batches.len() as f64,
        full_ms,
        noop_ms,
    }
}

/// `cargo test --release -p capstan-kernel --test retention -- --ignored --nocapture`
#[test]
#[ignore = "cost figures on 150k rows; run with --release"]
fn cost_of_a_prune_on_150k_rows() {
    for (layout, interleaved) in [("contiguous", false), ("interleaved", true)] {
        let f = figures(layout, interleaved);
        println!(
            "{layout:12} max batch {:8.2} ms | mean batch {:8.2} ms | full prune {:9.2} ms | no-op pass {:8.2} ms",
            f.max_batch_ms, f.mean_batch_ms, f.full_ms, f.noop_ms
        );
        assert!(Path::new(".").exists());
    }
}
