//! `recent_page`: the newest outcome events of a ledger, read backwards within a budget. The ledgers here are built by
//! writing rows of two shapes after the project's own: an outcome (an `integration.finish`, which the feed shows as
//! `integration.finished` and which costs its row and one entity lookup) and a filler (an `integration.begin`, an
//! invalidate that costs its row alone and is no outcome).

use capstan_kernel::areas::event_feed::{
    feed_head, feed_page, recent_page, Position, RecentEvents, RecentPage, OUTCOME_TYPES,
};
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::types::{InitialProject, ProjectInput};
use capstan_kernel::{Core, SeededEnv};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn private(path: &Path) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

fn initial() -> InitialProject {
    InitialProject {
        project_id: "precentfeed".into(),
        name: "recent-feed".into(),
        owner_credential: "operator-0123456789abcdef0123456789abcdef01234567".into(),
        initial_inputs: [
            "project_config",
            "task_brief",
            "acceptance_criteria",
            "policy",
            "plan",
        ]
        .into_iter()
        .map(|kind| ProjectInput {
            kind: kind.into(),
            content: if kind == "acceptance_criteria" {
                json!(["(none yet)"])
            } else {
                json!({"placeholder": true})
            },
        })
        .collect(),
    }
}

fn open_core(state: &Path) -> Core {
    Core::open(
        state,
        &initial(),
        &KernelOptions::default(),
        Box::new(SeededEnv::new("recent-feed")),
    )
    .expect("the ledger opens")
}

/// A ledger of exactly `total` rows (at least the project's own): after those, the rows whose sequence `outcome(seq)`
/// accepts are outcomes, the others fillers.
fn ledger(total: i64, outcome: impl Fn(i64) -> bool) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    private(dir.path());
    let state = dir.path().join("state");
    std::fs::create_dir(&state).unwrap();
    private(&state);
    drop(open_core(&state));
    let conn = Connection::open(state.join("controller.sqlite")).unwrap();
    let (project, actor, head): (String, String, i64) = conn
        .query_row(
            "SELECT project_id, actor_id, sequence FROM controller_events ORDER BY sequence DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert!(total > head, "the project's own rows are {head}");
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    conn.execute_batch("BEGIN").unwrap();
    {
        let mut insert = conn
            .prepare(
                "INSERT INTO controller_events(project_id, sequence, event_id, entity_type, entity_id, from_state, to_state,
                   state_version, actor_id, request_id, input_revision, payload_json, created_at)
                 VALUES (?, ?, ?, 'integration', ?, ?, ?, 0, ?, ?, 0, ?, '2026-01-01T00:00:00.000Z')",
            )
            .unwrap();
        for seq in head + 1..=total {
            let (action, from, to) = if outcome(seq) {
                ("integration.finish", "running", "merged")
            } else {
                ("integration.begin", "queued", "running")
            };
            insert
                .execute(rusqlite::params![
                    project,
                    seq,
                    format!("event-{seq}"),
                    format!("i{seq}"),
                    from,
                    to,
                    actor,
                    format!("req-{seq}"),
                    json!({"action": action, "payload": {}}).to_string(),
                ])
                .unwrap();
        }
    }
    conn.execute_batch("COMMIT").unwrap();
    drop(conn);
    (dir, state)
}

fn epoch_of(core: &Core) -> String {
    feed_head(core).unwrap().epoch
}

fn recent(core: &Core, head: u64, limit: u32) -> RecentEvents {
    match recent_page(core, &epoch_of(core), head, limit).unwrap() {
        RecentPage::Page(page) => page,
        RecentPage::Resync(resync) => panic!("a resync answer: {resync:?}"),
    }
}

fn seqs(page: &RecentEvents) -> Vec<u64> {
    page.events
        .iter()
        .map(|e| e["seq"].as_u64().unwrap())
        .collect()
}

fn is_outcome(base: i64) -> impl Fn(i64) -> bool {
    move |seq| seq > base
}

#[test]
fn the_twenty_newest_outcomes_come_newest_first_on_a_large_ledger() {
    // 30 outcomes, all within the newest 400 rows of 2,000.
    let (_dir, state) = ledger(2_000, |seq| seq > 1_600 && (seq - 1_600) % 13 == 0);
    let core = open_core(&state);
    let head = feed_head(&core).unwrap().head;
    let page = recent(&core, head, 20);
    assert_eq!(page.events.len(), 20);
    let found = seqs(&page);
    assert!(found.windows(2).all(|w| w[0] > w[1]), "{found:?}");
    assert_eq!(found[0], 1_600 + 13 * 30);
    assert!(page.truncated);
    assert!(page.rows_read <= 500);
}

#[test]
fn a_budget_stop_before_the_first_row_is_truncated() {
    // 3 outcomes in the newest 500 rows, none needed beyond.
    let (_dir, state) = ledger(2_000, |seq| matches!(seq, 1_999 | 1_700 | 1_600));
    let core = open_core(&state);
    let head = feed_head(&core).unwrap().head;
    let page = recent(&core, head, 20);
    assert_eq!(seqs(&page), vec![1_999, 1_700, 1_600]);
    // The read stops on the budget (3 rows of cost 2, the rest of cost 1), well before the first row.
    assert!(page.truncated);
    assert_eq!(page.rows_read, 500);
    assert!(page.rows_read <= 500);
    assert_eq!(page.rows_scanned as u64, head - page.oldest_scanned + 1);
}

#[test]
fn a_small_project_reaches_its_first_row_and_is_not_truncated() {
    let (_dir, state) = ledger(40, |seq| seq % 7 == 0);
    let core = open_core(&state);
    let head = feed_head(&core).unwrap().head;
    let page = recent(&core, head, 20);
    assert_eq!(seqs(&page), vec![35, 28, 21, 14, 7]);
    assert!(!page.truncated);
    assert_eq!(page.oldest_scanned, 1);
    assert_eq!(page.rows_scanned, 40);
}

#[test]
fn a_stop_on_the_event_limit_counts_as_truncated() {
    let (_dir, state) = ledger(40, |seq| seq > 15);
    let core = open_core(&state);
    let head = feed_head(&core).unwrap().head;
    let page = recent(&core, head, 20);
    assert_eq!(page.events.len(), 20);
    assert_eq!(seqs(&page)[0], 40);
    assert!(page.truncated);
    assert_eq!(page.oldest_scanned, 21);
    // Exactly the limit and then the end of the ledger is not a stop.
    let (_dir2, exact) = ledger(40, |seq| seq > 20);
    let core = open_core(&exact);
    let page = recent(&core, 40, 20);
    assert_eq!(page.events.len(), 20);
    assert!(!page.truncated);
}

#[test]
fn a_wrong_epoch_or_a_head_beyond_the_ledger_gives_the_ledger_position() {
    let (_dir, state) = ledger(30, is_outcome(25));
    let core = open_core(&state);
    let ledger_head = feed_head(&core).unwrap();
    for (epoch, head) in [
        ("0000000000000000".to_string(), 30),
        (ledger_head.epoch.clone(), 31),
    ] {
        match recent_page(&core, &epoch, head, 20).unwrap() {
            RecentPage::Resync(answer) => {
                let value = serde_json::to_value(&answer).unwrap();
                assert_eq!(
                    value,
                    json!({"projectId": ledger_head.project_id, "epoch": ledger_head.epoch, "head": 30})
                );
            }
            RecentPage::Page(_) => panic!("expected a resync"),
        }
    }
}

#[test]
fn an_out_of_range_limit_is_refused() {
    let (_dir, state) = ledger(30, is_outcome(25));
    let core = open_core(&state);
    let epoch = epoch_of(&core);
    for limit in [0, 21] {
        assert!(recent_page(&core, &epoch, 30, limit).is_err());
    }
}

#[test]
fn the_events_are_those_of_feed_page() {
    let (_dir, state) = ledger(40, |seq| seq % 3 == 0);
    let core = open_core(&state);
    let head = feed_head(&core).unwrap().head;
    let page = recent(&core, head, 20);
    assert!(!page.truncated);
    let all = feed_page(&core, Position { seq: 0, k: 0 }, 500, None).unwrap();
    let mut expected: Vec<Value> = all
        .events
        .into_iter()
        .filter(|e| OUTCOME_TYPES.contains(&e["type"].as_str().unwrap()))
        .collect();
    expected.reverse();
    assert!(!expected.is_empty());
    assert_eq!(page.events, expected);
}

#[test]
fn a_stale_head_reads_back_from_that_head() {
    let (_dir, state) = ledger(60, |seq| seq % 5 == 0);
    let core = open_core(&state);
    let page = recent(&core, 33, 20);
    assert_eq!(page.head, 33);
    assert_eq!(seqs(&page), vec![30, 25, 20, 15, 10, 5]);
    assert!(page.events.iter().all(|e| e["seq"].as_u64().unwrap() <= 33));
}

#[test]
fn the_call_writes_nothing() {
    let (_dir, state) = ledger(60, |seq| seq % 5 == 0);
    let core = open_core(&state);
    let conn = Connection::open(state.join("controller.sqlite")).unwrap();
    let summary = |conn: &Connection| -> (i64, i64, i64) {
        conn.query_row(
            "SELECT COUNT(*), MAX(sequence), SUM(state_version) FROM controller_events",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
    };
    let before = (summary(&conn), feed_head(&core).unwrap());
    let _ = recent(&core, 60, 20);
    assert_eq!((summary(&conn), feed_head(&core).unwrap()), before);
}

#[test]
fn a_large_ledger_is_read_within_five_hundred_charges() {
    let (_dir, state) = ledger(5_000, |seq| seq % 40 == 0);
    let core = open_core(&state);
    let head = feed_head(&core).unwrap().head;
    let page = recent(&core, head, 20);
    assert!(page.rows_read <= 500, "rowsRead {}", page.rows_read);
    assert_eq!(page.rows_scanned as u64, head - page.oldest_scanned + 1);
    assert!(page.truncated);
    let value = serde_json::to_value(&page).unwrap();
    assert!(value["rowsRead"].is_number() && value["rowsScanned"].is_number());
}

/// The cost on a ledger of 170,000 rows is the same bound; run once with `--ignored`, it also prints the time.
#[test]
#[ignore = "builds a 170,000-row ledger"]
fn a_170k_row_ledger_is_read_within_five_hundred_charges() {
    let (_dir, state) = ledger(170_000, |seq| seq % 1_000 == 0);
    let core = open_core(&state);
    let head = feed_head(&core).unwrap().head;
    let started = std::time::Instant::now();
    let page = recent(&core, head, 20);
    eprintln!(
        "recent_page on {head} rows: {:?}, rowsRead {}, rowsScanned {}",
        started.elapsed(),
        page.rows_read,
        page.rows_scanned
    );
    assert!(page.rows_read <= 500);
    assert!(page.truncated);
}
