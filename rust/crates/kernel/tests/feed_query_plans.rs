//! The hot statements of the feed keep an index that covers every equality column. Each statement is held here as a
//! constant and must occur verbatim in its source file, so a later edit of the statement fails this test instead of
//! escaping it. To guard another statement, add a `Guarded` to `GUARDED`.

use capstan_kernel::areas::event_feed::{feed_head, feed_page, Position};
use capstan_kernel::canonical::sha256;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::types::{InitialProject, ProjectInput};
use capstan_kernel::{Core, SeededEnv};
use capstan_ledger::{migrate, migrations, open_database, Database, OpenOptions};
use rusqlite::{types::Null, Connection, OptionalExtension};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const EVENT_FEED: &str = include_str!("../src/areas/event_feed.rs");
const FEED_CURSOR: &str = include_str!("../src/areas/feed_cursor.rs");

const CREATION_PAYLOAD: &str = "SELECT payload_json FROM controller_events INDEXED BY controller_events_by_entity WHERE project_id = ? AND entity_type = ? AND entity_id = ?
           AND from_state IS NULL AND to_state IS NOT NULL ORDER BY sequence LIMIT 1";
const HEAD: &str = "SELECT COALESCE(MAX(sequence), 0) FROM controller_events WHERE project_id = ?";
const EVENT_ID_AT: &str =
    "SELECT event_id FROM controller_events WHERE project_id = ? AND sequence = ?";
const READ_ROW: &str =
    "SELECT entity_type, entity_id, from_state, to_state, actor_id, payload_json, created_at
           FROM controller_events WHERE project_id = ? AND sequence = ?";

/// A hot statement: where it lives, and for each table it reads the equality columns an index must cover.
struct Guarded {
    name: &'static str,
    source: &'static str,
    sql: &'static str,
    tables: &'static [(&'static str, &'static [&'static str])],
}

const GUARDED: &[Guarded] = &[
    Guarded {
        name: "creation_payload",
        source: EVENT_FEED,
        sql: CREATION_PAYLOAD,
        tables: &[(
            "controller_events",
            &["project_id", "entity_type", "entity_id"],
        )],
    },
    Guarded {
        name: "feed_cursor head",
        source: FEED_CURSOR,
        sql: HEAD,
        tables: &[("controller_events", &["project_id"])],
    },
    Guarded {
        name: "feed_cursor event_id_at",
        source: FEED_CURSOR,
        sql: EVENT_ID_AT,
        tables: &[("controller_events", &["project_id", "sequence"])],
    },
    Guarded {
        name: "event_feed read_row",
        source: EVENT_FEED,
        sql: READ_ROW,
        tables: &[("controller_events", &["project_id", "sequence"])],
    },
];

/// The plan lines of the statement that break the rule: a line that reads a guarded table by SCAN, or by a SEARCH whose
/// index condition lacks one of the table's equality columns. Empty when the statement is covered.
fn violations(conn: &Connection, guarded: &Guarded) -> Vec<String> {
    // A statement that cannot be prepared (it names an index the ledger lacks) is flagged too.
    let Ok(plan) = try_plan_of(conn, guarded.sql) else {
        return vec![format!("{}: the statement does not prepare", guarded.name)];
    };
    let mut found = Vec::new();
    for (table, columns) in guarded.tables {
        let lines: Vec<&String> = plan
            .iter()
            .filter(|line| {
                line.starts_with(&format!("SCAN {table}"))
                    || line.starts_with(&format!("SEARCH {table}"))
            })
            .collect();
        if lines.is_empty() {
            found.push(format!(
                "{}: no plan line reads {table}: {plan:?}",
                guarded.name
            ));
        }
        for line in lines {
            let condition = line.split_once('(').map_or("", |(_, rest)| rest);
            let covered = line.starts_with("SEARCH")
                && columns.iter().all(|column| {
                    condition
                        .split(" AND ")
                        .any(|part| part.starts_with(&format!("{column}=")))
                });
            if !covered {
                found.push(format!("{}: {line}", guarded.name));
            }
        }
    }
    found
}

fn try_plan_of(conn: &Connection, sql: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
    let nulls = vec![Null; stmt.parameter_count()];
    let rows = stmt
        .query_map(rusqlite::params_from_iter(nulls), |r| r.get::<_, String>(3))?
        .collect();
    rows
}

fn plan_of(conn: &Connection, sql: &str) -> Vec<String> {
    try_plan_of(conn, sql).unwrap_or_else(|e| panic!("{sql}: {e}"))
}

fn migrated() -> (tempfile::TempDir, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("controller.sqlite");
    open_database(&file, &OpenOptions::default())
        .unwrap()
        .close()
        .unwrap();
    let conn = Connection::open(&file).unwrap();
    (dir, conn)
}

#[test]
fn each_statement_constant_is_found_verbatim_in_its_source() {
    for guarded in GUARDED {
        assert!(
            guarded.source.contains(guarded.sql),
            "{}: the statement in the source changed; update the constant and check its plan",
            guarded.name
        );
    }
}

#[test]
fn the_hot_statements_read_through_an_index_covering_every_equality_column() {
    let (_dir, conn) = migrated();
    for guarded in GUARDED {
        assert_eq!(violations(&conn, guarded), Vec::<String>::new());
    }
    let creation = plan_of(&conn, CREATION_PAYLOAD).join("\n");
    assert!(
        creation.contains("controller_events_by_entity"),
        "{creation}"
    );
    for sql in [HEAD, EVENT_ID_AT, READ_ROW] {
        let plan = plan_of(&conn, sql).join("\n");
        assert!(plan.contains("USING PRIMARY KEY"), "{plan}");
    }
    for sql in [EVENT_ID_AT, READ_ROW] {
        let plan = plan_of(&conn, sql).join("\n");
        assert!(plan.contains("project_id=? AND sequence=?"), "{plan}");
    }
}

#[test]
fn the_guard_flags_a_ledger_without_the_entity_index() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("controller.sqlite");
    let at = migrations()
        .iter()
        .position(|m| m.name.ends_with("_controller_events_by_entity.sql"))
        .expect("the migration is embedded");
    let db = Database::new(Connection::open(&file).unwrap());
    migrate(&db, &file, &migrations()[..at]).unwrap();
    db.close().unwrap();
    let conn = Connection::open(&file).unwrap();
    let creation = &GUARDED[0];
    assert_eq!(creation.name, "creation_payload");
    assert!(!violations(&conn, creation).is_empty());
    // The unpinned text on that ledger reads by the primary key prefix alone, which the rule flags as well.
    let unpinned = Guarded {
        sql: &*Box::leak(
            CREATION_PAYLOAD
                .replace(" INDEXED BY controller_events_by_entity", "")
                .into_boxed_str(),
        ),
        ..*creation
    };
    assert!(!violations(&conn, &unpinned).is_empty());
}

#[test]
fn the_pinned_statement_returns_the_same_row_as_the_unpinned_one() {
    let (_dir, state) = ledger(20_000, true);
    let conn = Connection::open(state.join("controller.sqlite")).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    let (project, actor, head): (String, String, i64) = conn
        .query_row(
            "SELECT project_id, actor_id, MAX(sequence) FROM controller_events",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    // Later rows of an entity whose first creation row is far earlier: the first one must win.
    for (n, from) in [(1, None), (2, Some("queued")), (3, None)] {
        conn.execute(
            "INSERT INTO controller_events(project_id, sequence, event_id, entity_type, entity_id, from_state, to_state,
               state_version, actor_id, request_id, input_revision, payload_json, created_at)
             VALUES (?, ?, ?, 'message', 'm7', ?, 'queued', 0, ?, ?, 0, ?, '2026-01-01T00:00:00.000Z')",
            rusqlite::params![
                project,
                head + n,
                format!("extra-{n}"),
                from,
                actor,
                format!("extra-req-{n}"),
                format!("{{\"extra\":{n}}}")
            ],
        )
        .unwrap();
    }
    let unpinned = CREATION_PAYLOAD.replace(" INDEXED BY controller_events_by_entity", "");
    let read = |sql: &str, kind: &str, id: &str| -> Option<String> {
        conn.query_row(sql, rusqlite::params![project, kind, id], |r| r.get(0))
            .optional()
            .unwrap()
    };
    let ids = [
        ("message", "m1"),
        ("message", "m2"),
        ("message", "m7"),
        ("message", "m5000"),
        ("message", "m10000"),
        ("message", "m10001"),
        ("message", "m19999"),
        ("message", "m20000"),
        ("prompt_relay", "r1"),
        ("message", "missing"),
    ];
    for (kind, id) in ids {
        assert_eq!(
            read(CREATION_PAYLOAD, kind, id),
            read(&unpinned, kind, id),
            "{kind} {id}"
        );
    }
    let original = |n: i64| {
        let sequence: i64 = conn
            .query_row(
                "SELECT sequence FROM controller_events WHERE project_id = ? AND event_id = ?",
                rusqlite::params![project, format!("event-{n}")],
                |r| r.get(0),
            )
            .unwrap();
        sequence
    };
    assert!(
        original(7) < head,
        "the entity's first creation row is the earliest"
    );
    let first: String = conn
        .query_row(
            "SELECT payload_json FROM controller_events WHERE project_id = ? AND sequence = ?",
            rusqlite::params![project, original(7)],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(read(CREATION_PAYLOAD, "message", "m7"), Some(first));
    assert_eq!(read(CREATION_PAYLOAD, "message", "missing"), None);
    assert_eq!(read(&unpinned, "message", "missing"), None);
}

// ------------------------------------------------------------------------------------------------ same results

fn private(path: &Path) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

fn initial() -> InitialProject {
    InitialProject {
        project_id: "pfeedplans".into(),
        name: "feed-plans".into(),
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
        Box::new(SeededEnv::new("feed-query-plans")),
    )
    .expect("the ledger opens")
}

/// A ledger with `messages` message events, one prompt_relay capture and the relay's begin event after the project's own
/// rows; the begin event is the row whose body reads the relay's creation row (`creation_payload`). With `index` false the
/// entity index is dropped again.
fn ledger(messages: i64, index: bool) -> (tempfile::TempDir, PathBuf) {
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
    // The relay's agent is not a row of this ledger.
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    let agent = "relay-agent";
    // The hash the relay's capture carries: the feed shows the prompt text only when it matches the relay row.
    let prompt_sha = sha256(
        &json!({"agentId": agent, "paneId": "pane", "hostKind": "herdr", "text": "Allow?", "options": []}).to_string(),
    );
    conn.execute_batch("BEGIN").unwrap();
    {
        let mut insert = conn
            .prepare(
                "INSERT INTO controller_events(project_id, sequence, event_id, entity_type, entity_id, from_state, to_state,
                   state_version, actor_id, request_id, input_revision, payload_json, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?, ?, 0, ?, '2026-01-01T00:00:00.000Z')",
            )
            .unwrap();
        for n in 1..=messages + 2 {
            let (kind, id, action, from, to) = if n <= messages {
                (
                    "message",
                    format!("m{n}"),
                    "message.enqueue",
                    None,
                    "queued",
                )
            } else if n == messages + 1 {
                (
                    "prompt_relay",
                    "r1".to_string(),
                    "prompt_relay.capture",
                    None,
                    "captured",
                )
            } else {
                (
                    "prompt_relay",
                    "r1".to_string(),
                    "prompt_relay.begin",
                    Some("captured"),
                    "typing",
                )
            };
            insert
                .execute(rusqlite::params![
                    project,
                    head + n,
                    format!("event-{n}"),
                    kind,
                    id,
                    from,
                    to,
                    actor,
                    format!("req-{n}"),
                    json!({"action": action, "payload": {"promptSha": prompt_sha}}).to_string(),
                ])
                .unwrap();
        }
    }
    // The row `load_relay` reads for the begin event; the capture trigger is dropped because this actor is no PM.
    conn.execute_batch("DROP TRIGGER prompt_relays_capture_by_a_pm_or_in_a_window")
        .unwrap();
    conn.execute(
        "INSERT INTO prompt_relays(project_id, relay_id, sequence, agent_id, pane_id, host_kind, prompt_text, options_json,
           prompt_sha, captured_by_actor_id, captured_at, expires_at, state)
         VALUES (?, 'r1', ?, ?, 'pane', 'herdr', 'Allow?', '[]', ?, ?, '2026-01-01T00:00:00.000Z',
           '2099-01-01T00:00:00.000Z', 'captured')",
        rusqlite::params![project, head + messages + 1, agent, prompt_sha, actor],
    )
    .unwrap();
    conn.execute_batch("COMMIT").unwrap();
    if !index {
        conn.execute_batch("DROP INDEX controller_events_by_entity")
            .unwrap();
    }
    drop(conn);
    (dir, state)
}

/// The feed page over the last events of a ledger built by `ledger`, and its head.
fn page_of(state: &Path) -> Result<(u64, serde_json::Value), String> {
    let core = open_core(state);
    let head = feed_head(&core).map_err(|e| format!("{e:?}"))?.head;
    let page = feed_page(
        &core,
        Position {
            seq: head - 10,
            k: 0,
        },
        10,
        None,
    )
    .map_err(|e| format!("{e:?}"))?;
    Ok((head, serde_json::to_value(&page).unwrap()))
}

/// The page holds the relay's begin event, whose body is built from the relay's creation row (`creation_payload`): the text
/// shows only when the hash found there matches the relay, so the lookup returned the right row.
#[test]
fn the_feed_reads_the_creation_row_through_the_entity_index() {
    let (_dir, state) = ledger(20_000, true);
    let (head, page) = page_of(&state).expect("the feed reads with the index");
    assert!(head > 20_000);
    let shown = page.to_string();
    assert!(shown.contains("\"relayId\":\"r1\""), "{shown}");
    assert!(shown.contains("\"text\":\"Allow?\""), "{shown}");
}

/// Without the index the pinned statement does not prepare, so the feed cannot return the page it returns with the index:
/// the path through `creation_payload` is taken by this page. That the pinned and the unpinned text read the same row is
/// held by `the_pinned_statement_returns_the_same_row_as_the_unpinned_one`.
#[test]
fn the_feed_page_depends_on_the_entity_index() {
    let (_dir, with) = ledger(20_000, true);
    let (_dir2, without) = ledger(20_000, false);
    let conn = Connection::open(without.join("controller.sqlite")).unwrap();
    let prepared = conn
        .prepare(CREATION_PAYLOAD)
        .expect_err("the statement does not prepare");
    assert!(prepared.to_string().contains("no such index"), "{prepared}");
    drop(conn);
    page_of(&with).expect("the feed reads with the index");
    let error = page_of(&without).expect_err("the feed cannot read without the index");
    assert!(error.contains("controller_events_by_entity"), "{error}");
    assert!(error.contains("no such index"), "{error}");
}
