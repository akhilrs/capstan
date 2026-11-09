//! The event, review, report and notice lookups read through the index meant for them. Each statement is the constant the
//! code runs (`areas::*_SQL`), so an edit of the text is checked here. A ledger of thousands of rows is seeded and every
//! plan is read before and after ANALYZE: each must name exactly its expected index and never scan the table.
//!
//! The plans are those of the SQLite bundled into this build (see `capstan_ledger::sqlite_version`); a SQLite bump reruns
//! this test as a gate. `INDEXED BY` is used only where the planner chose wrongly on that version.

use capstan_kernel::areas::{
    ACCEPTED_REPORT_FOR_SQL, BRANCH_AT_END_SQL, COMPLETE_REVIEW_SQL, IS_AGENT_REPLACED_SQL,
    LAST_VERDICT_AT_SQL, LOST_RECORDED_SQL, NOTICE_PACKAGE_REVIEWED_SQL, NOTICE_SENT_SQL,
    PLAN_ROUNDS_SQL, RECORDED_SUCCESSOR_SQL, RECORD_AGENT_REPORT_SQL,
    REPLACEMENT_FAMILY_ENTITY_SQL, REPLACEMENT_FAMILY_SUCCESSOR_SQL,
};
use capstan_ledger::{
    migrate, migrations, open_database, sqlite_version, Database, OpenOptions,
    PLANNER_STATS_MIGRATION,
};
use rusqlite::types::Value;
use rusqlite::Connection;

/// How a statement binds its parameters.
enum Bind {
    Positional(Vec<Value>),
    Named(Vec<(&'static str, Value)>),
}

struct Stmt {
    name: &'static str,
    sql: &'static str,
    table: &'static str,
    /// The index the plan must name; `None` is the autoindex of `UNIQUE (project_id, subject_plan_id, round)`.
    index: Option<&'static str>,
    bind: fn() -> Bind,
}

fn text(s: &str) -> Value {
    Value::Text(s.to_string())
}

fn long_text() -> String {
    format!("Plan plan7 needs attention: {}", "x".repeat(200))
}

fn statements() -> Vec<Stmt> {
    let by_entity = Some("controller_events_by_entity");
    vec![
        Stmt {
            name: "lost_recorded",
            sql: LOST_RECORDED_SQL,
            table: "controller_events",
            index: by_entity,
            bind: || Bind::Positional(vec![text("p"), text("agent4"), Value::Integer(1)]),
        },
        Stmt {
            name: "recorded_successor",
            sql: RECORDED_SUCCESSOR_SQL,
            table: "controller_events",
            index: by_entity,
            bind: || Bind::Positional(vec![text("p"), text("agent1")]),
        },
        Stmt {
            name: "is_agent_replaced",
            sql: IS_AGENT_REPLACED_SQL,
            table: "controller_events",
            index: by_entity,
            bind: || Bind::Positional(vec![text("p"), text("agent1")]),
        },
        Stmt {
            name: "branch_at_end",
            sql: BRANCH_AT_END_SQL,
            table: "controller_events",
            index: by_entity,
            bind: || Bind::Positional(vec![text("p"), text("agent2")]),
        },
        Stmt {
            name: "replacement_family entity",
            sql: REPLACEMENT_FAMILY_ENTITY_SQL,
            table: "controller_events",
            index: by_entity,
            bind: || Bind::Positional(vec![text("p"), text("agent1")]),
        },
        Stmt {
            name: "replacement_family successor",
            sql: REPLACEMENT_FAMILY_SUCCESSOR_SQL,
            table: "controller_events",
            index: Some("controller_events_by_successor"),
            bind: || Bind::Positional(vec![text("p"), text("agent2")]),
        },
        Stmt {
            name: "last_verdict_at",
            sql: LAST_VERDICT_AT_SQL,
            table: "reviews",
            index: Some("reviews_by_reviewer"),
            bind: || Bind::Positional(vec![text("p"), text("agent4")]),
        },
        Stmt {
            name: "complete_review",
            sql: COMPLETE_REVIEW_SQL,
            table: "reviews",
            index: Some("reviews_by_reviewer"),
            bind: || Bind::Positional(vec![text("p"), text("agent4")]),
        },
        Stmt {
            name: "plan_rounds",
            sql: PLAN_ROUNDS_SQL,
            table: "reviews",
            index: None,
            bind: || Bind::Positional(vec![text("p"), text("plan7")]),
        },
        Stmt {
            name: "notice_sent short",
            sql: NOTICE_SENT_SQL,
            table: "messages",
            index: Some("messages_by_body_head"),
            bind: || notice_bind("Plan plan7 approved"),
        },
        Stmt {
            name: "notice_sent long",
            sql: NOTICE_SENT_SQL,
            table: "messages",
            index: Some("messages_by_body_head"),
            bind: || notice_bind(&long_text()),
        },
        Stmt {
            name: "notice_package_reviewed",
            sql: NOTICE_PACKAGE_REVIEWED_SQL,
            table: "messages",
            index: Some("messages_by_body_head"),
            bind: || Bind::Positional(vec![text("p"), text("Plan plan7 package a reviewed")]),
        },
        Stmt {
            name: "record_agent_report",
            sql: RECORD_AGENT_REPORT_SQL,
            table: "agent_reports",
            index: Some("agent_reports_by_agent"),
            bind: || Bind::Positional(vec![text("p"), text("agent4"), Value::Integer(1)]),
        },
        Stmt {
            name: "accepted_report_for",
            sql: ACCEPTED_REPORT_FOR_SQL,
            table: "agent_reports",
            index: Some("agent_reports_by_agent"),
            bind: || {
                Bind::Positional(vec![
                    text("p"),
                    text("agent4"),
                    Value::Integer(1),
                    text(&"a".repeat(40)),
                ])
            },
        },
    ]
}

fn notice_bind(text_: &str) -> Bind {
    let head: String = text_.chars().take(120).collect();
    Bind::Named(vec![
        (":project", text("p")),
        (":head", text(&head)),
        (
            ":length",
            Value::Integer(text_.encode_utf16().count() as i64),
        ),
        (":text", text(text_)),
    ])
}

/// The value for each parameter of the prepared statement, in order.
fn values(stmt: &rusqlite::Statement<'_>, bind: Bind) -> Vec<Value> {
    match bind {
        Bind::Positional(values) => {
            assert_eq!(values.len(), stmt.parameter_count());
            values
        }
        Bind::Named(named) => (1..=stmt.parameter_count())
            .map(|i| {
                let name = stmt.parameter_name(i).expect("a named parameter");
                named
                    .iter()
                    .find(|(n, _)| *n == name)
                    .unwrap_or_else(|| panic!("no value for {name}"))
                    .1
                    .clone()
            })
            .collect(),
    }
}

fn plan_of(conn: &Connection, stmt: &Stmt) -> Vec<String> {
    let mut prepared = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {}", stmt.sql))
        .unwrap_or_else(|e| panic!("{}: {e}", stmt.name));
    let bound = values(&prepared, (stmt.bind)());
    let rows = prepared
        .query_map(rusqlite::params_from_iter(bound), |r| r.get::<_, String>(3))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    rows
}

/// Prepares and runs the statement, returning the number of rows.
fn run(conn: &Connection, stmt: &Stmt) -> usize {
    let mut prepared = conn
        .prepare(stmt.sql)
        .unwrap_or_else(|e| panic!("{}: {e}", stmt.name));
    let bound = values(&prepared, (stmt.bind)());
    let mut rows = prepared
        .query(rusqlite::params_from_iter(bound))
        .unwrap_or_else(|e| panic!("{}: {e}", stmt.name));
    let mut n = 0;
    while rows.next().unwrap().is_some() {
        n += 1;
    }
    n
}

/// The name of the autoindex of `UNIQUE (project_id, subject_plan_id, round)` on `reviews`.
fn plan_round_autoindex(conn: &Connection) -> String {
    let names: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'reviews' AND sql IS NULL")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for name in names {
        let columns: Vec<String> = conn
            .prepare(&format!("PRAGMA index_info('{name}')"))
            .unwrap()
            .query_map([], |r| r.get(2))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        if columns[..] == ["project_id", "subject_plan_id", "round"] {
            return name;
        }
    }
    panic!("the autoindex of (project_id, subject_plan_id, round) is missing");
}

/// The plan lines that read `table`, with the index each names.
fn table_lines<'a>(plan: &'a [String], table: &str) -> Vec<&'a String> {
    plan.iter()
        .filter(|l| {
            l.starts_with(&format!("SCAN {table}")) || l.starts_with(&format!("SEARCH {table}"))
        })
        .collect()
}

fn named_index(line: &str) -> Option<&str> {
    let rest = line.split_once(" USING ")?.1;
    let rest = rest
        .strip_prefix("COVERING INDEX ")
        .or_else(|| rest.strip_prefix("INDEX "))?;
    rest.split(' ').next()
}

fn assert_plan(conn: &Connection, stmt: &Stmt, autoindex: &str, when: &str) {
    let plan = plan_of(conn, stmt);
    let lines = table_lines(&plan, stmt.table);
    assert!(
        !lines.is_empty(),
        "{} {when}: no line reads {}: {plan:?}",
        stmt.name,
        stmt.table
    );
    let expected = stmt.index.unwrap_or(autoindex);
    for line in lines {
        assert!(!line.starts_with("SCAN"), "{} {when}: {plan:?}", stmt.name);
        assert!(
            !line.contains("USING PRIMARY KEY (project_id=?)"),
            "{} {when}: {plan:?}",
            stmt.name
        );
        assert_eq!(
            named_index(line),
            Some(expected),
            "{} {when}: {plan:?}",
            stmt.name
        );
    }
    for scanned in [
        "controller_events",
        "messages",
        "reviews",
        "agent_reports",
        "plan_revisions",
    ] {
        for line in table_lines(&plan, scanned) {
            assert!(!line.starts_with("SCAN"), "{} {when}: {plan:?}", stmt.name);
        }
    }
}

const SEED: &str = "
DROP TRIGGER IF EXISTS plan_review_needs_a_plan_in_review;
DROP TRIGGER IF EXISTS integration_reviewer_is_no_author;
WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 6000)
INSERT INTO controller_events(project_id, sequence, event_id, entity_type, entity_id, from_state, to_state,
    state_version, actor_id, request_id, input_revision, payload_json, created_at)
  SELECT 'p', i, 'event-' || i, CASE WHEN i % 5 = 0 THEN 'message' ELSE 'agent' END, 'agent' || (i % 300), NULL,
    CASE i % 4 WHEN 0 THEN 'lost' WHEN 1 THEN 'replaced' WHEN 2 THEN 'ended' ELSE 'active' END, 0, 'actor',
    'request-' || i, 0,
    json_object('details', json_object('generation', i % 3, 'successorId', 'agent' || ((i + 1) % 300), 'branch', 'b' || i)),
    '2026-01-01T00:00:00.000Z' FROM n;
WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 4000)
INSERT INTO messages(project_id, message_id, sequence, recipient_agent_id, recipient_generation, sender_actor_id, body,
    body_hash, state, state_version, queued_at, deferral_count, send_attempts, created_at, updated_at, action_needed, class)
  SELECT 'p', 'm' || i, i, 'agent' || (i % 50), 1, 'actor',
    CASE i % 3 WHEN 0 THEN 'Plan plan' || (i % 200) || ' approved: ' || i
               WHEN 1 THEN 'Plan plan' || (i % 200) || ' package a reviewed ' || i
               ELSE 'Plan plan' || (i % 200) || ' needs attention: ' || i END,
    printf('%064d', i), 'sent', 0, '2026-01-01T00:00:00.000Z', 0, 0, '2026-01-01T00:00:00.000Z',
    '2026-01-01T00:00:00.000Z', 0, 'info' FROM n;
INSERT INTO messages(project_id, message_id, sequence, recipient_agent_id, recipient_generation, sender_actor_id, body,
    body_hash, state, state_version, queued_at, deferral_count, send_attempts, created_at, updated_at, action_needed, class)
  VALUES ('p', 'mshort', 5001, 'agent1', 1, 'actor', 'Plan plan7 approved and a longer tail follows', printf('%064d', 5001),
    'sent', 0, 't', 0, 0, 't', 't', 0, 'info'),
  ('p', 'mlong', 5002, 'agent1', 1, 'actor', 'Plan plan7 needs attention: ' || printf('%.200c', 'x') || ' tail', printf('%064d', 5002),
    'sent', 0, 't', 0, 0, 't', 't', 0, 'info');
WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 1500)
INSERT INTO reviews(project_id, review_id, sequence, round, subject_report_id, subject_plan_id, subject_plan_revision,
    commit_sha, base_sha, author_agent_id, author_actor_id, requested_by_actor_id, reviewer_role, reviewer_agent_id,
    reviewer_actor_id, state, verdict_text, created_at, completed_at)
  SELECT 'p', 'review-' || i, i, CASE WHEN i % 2 = 0 THEN i / 100 + 1 ELSE 1 END,
    CASE WHEN i % 2 = 1 THEN 'report' || i END, CASE WHEN i % 2 = 0 THEN 'plan' || (i % 100) END,
    CASE WHEN i % 2 = 0 THEN 1 END, printf('%040d', i), printf('%040d', i), 'author' || i, 'author-actor' || i, 'pm',
    'reviewer', 'agent' || (i % 100), 'reviewer-actor' || (i % 100), 'passed', 'ok', '2026-01-01T00:00:00.000Z',
    '2026-01-01T00:00:01.000Z' FROM n;
WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 2000)
INSERT INTO agent_reports(project_id, report_id, sequence, agent_id, generation, actor_id, commit_sha, summary, state,
    reason, evidence_json, created_at)
  SELECT 'p', 'report-' || i, i, 'agent' || (i % 100), i % 3 + 1, 'actor', printf('%040d', i), 'done',
    CASE WHEN i % 4 = 0 THEN 'rejected' ELSE 'accepted' END, CASE WHEN i % 4 = 0 THEN 'bad' END, '{}',
    '2026-01-01T00:00:00.000Z' FROM n;
";

/// A seeded ledger: all migrations (or all but 0059), foreign keys off, thousands of rows.
fn ledger(with_0059: bool) -> (tempfile::TempDir, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("controller.sqlite");
    if with_0059 {
        open_database(&file, &OpenOptions::default())
            .unwrap()
            .close()
            .unwrap();
    } else {
        let list: Vec<_> = migrations()
            .iter()
            .filter(|m| m.version < PLANNER_STATS_MIGRATION)
            .cloned()
            .collect();
        let db = Database::new(Connection::open(&file).unwrap());
        migrate(&db, &file, &list).unwrap();
        db.close().unwrap();
    }
    let conn = Connection::open(&file).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    conn.execute_batch(SEED).unwrap();
    (dir, conn)
}

#[test]
fn every_statement_reads_through_its_index_before_and_after_analyze() {
    let (_dir, conn) = ledger(true);
    let autoindex = plan_round_autoindex(&conn);
    assert!(
        PLAN_ROUNDS_SQL.contains(&autoindex),
        "the statement names {autoindex}"
    );
    let all = statements();
    let mut failures = Vec::new();
    let mut check = |when: &str| {
        for stmt in &all {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                assert_plan(&conn, stmt, &autoindex, when)
            }));
            if outcome.is_err() {
                failures.push(format!("{} {when}", stmt.name));
            }
            run(&conn, stmt);
        }
    };
    check(&format!("before ANALYZE (SQLite {})", sqlite_version()));
    conn.execute_batch("ANALYZE").unwrap();
    check("after ANALYZE");
    assert!(failures.is_empty(), "{failures:?}");
}

/// Many messages share one 120-character head: the statistics must not push the planner off the index.
#[test]
fn skewed_message_heads_keep_the_notice_statements_on_the_index() {
    let (_dir, conn) = ledger(true);
    conn.execute_batch(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 6000)
         INSERT INTO messages(project_id, message_id, sequence, recipient_agent_id, recipient_generation, sender_actor_id,
             body, body_hash, state, state_version, queued_at, deferral_count, send_attempts, created_at, updated_at,
             action_needed, class)
           SELECT 'p', 'skew' || i, 10000 + i, 'agent1', 1, 'actor', 'Plan plan7 approved ' || printf('%.101c', 'y') || i,
             printf('%064d', i), 'sent', 0, 't', 0, 0, 't', 't', 0, 'info' FROM n;
         ANALYZE;",
    )
    .unwrap();
    let autoindex = plan_round_autoindex(&conn);
    for stmt in statements().iter().filter(|s| s.table == "messages") {
        assert_plan(&conn, stmt, &autoindex, "skewed");
    }
}

#[test]
fn replacement_family_keeps_ledger_order_and_returns_an_event_once() {
    let (_dir, conn) = ledger(true);
    // One event that names the agent as both the predecessor and the successor matches both statements.
    conn.execute(
        "INSERT INTO controller_events(project_id, sequence, event_id, entity_type, entity_id, from_state, to_state,
           state_version, actor_id, request_id, input_revision, payload_json, created_at)
         VALUES ('p', 7000, 'both', 'agent', 'loop1', 'active', 'replaced', 0, 'actor', 'r-both', 0,
           '{\"details\":{\"successorId\":\"loop1\"}}', 't')",
        [],
    )
    .unwrap();
    let rows = |sql: &str| -> Vec<(i64, String, Option<String>)> {
        conn.prepare(sql)
            .unwrap()
            .query_map(rusqlite::params!["p", "loop1"], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    let entity = rows(REPLACEMENT_FAMILY_ENTITY_SQL);
    let successor = rows(REPLACEMENT_FAMILY_SUCCESSOR_SQL);
    assert_eq!(entity.len(), 1);
    assert_eq!(successor.len(), 1);
    assert_eq!(entity, successor);
    // The union the code builds: sorted by sequence, deduplicated by sequence.
    let mut merged = [entity, successor].concat();
    merged.sort_by_key(|(sequence, _, _)| *sequence);
    merged.dedup_by_key(|(sequence, _, _)| *sequence);
    assert_eq!(
        merged,
        vec![(7000, "loop1".to_string(), Some("loop1".to_string()))]
    );
    // Order: the events of another agent come back in ledger order.
    let ordered = conn
        .prepare(REPLACEMENT_FAMILY_ENTITY_SQL)
        .unwrap()
        .query_map(rusqlite::params!["p", "agent1"], |r| r.get::<_, i64>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(!ordered.is_empty());
    assert!(ordered.windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn notice_sent_matches_a_short_text_a_long_text_and_nothing_else() {
    let (_dir, conn) = ledger(true);
    let sent = |t: &str| {
        let stmt = Stmt {
            name: "notice_sent",
            sql: NOTICE_SENT_SQL,
            table: "messages",
            index: Some("messages_by_body_head"),
            bind: || Bind::Positional(vec![]),
        };
        let mut prepared = conn.prepare(stmt.sql).unwrap();
        let bound = values(&prepared, notice_bind(t));
        prepared.exists(rusqlite::params_from_iter(bound)).unwrap()
    };
    // A short text, with a stored body that starts with it and is longer.
    assert!(sent("Plan plan7 approved and a longer"));
    assert!(sent("Plan plan7 approved and a longer tail follows"));
    // A long text (over 120 characters).
    let long = format!("Plan plan7 needs attention: {}", "x".repeat(200));
    assert!(sent(&long));
    assert!(sent(&long[..150]));
    // A different text, and a text that is longer than the stored body.
    assert!(!sent("Plan plan7 rejected"));
    assert!(!sent(
        "Plan plan7 approved and a longer tail follows and more"
    ));
    assert!(!sent(&format!("{long}y-different")));
}

/// A ledger without 0059 prepares and runs every statement: every `INDEXED BY` names an index older than 0059. Only the
/// two statements on `messages_by_body_head` may scan there.
#[test]
fn a_ledger_without_0059_prepares_and_runs_every_statement() {
    let (_dir, conn) = ledger(false);
    let missing: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'messages_by_body_head'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(missing, 0, "0059 must not be applied");
    for stmt in statements() {
        run(&conn, &stmt);
    }
}
