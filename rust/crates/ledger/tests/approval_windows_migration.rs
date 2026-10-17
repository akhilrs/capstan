//! The approval-windows migration (the file whose name ends in `_approval_windows.sql`): a ledger at the previous version
//! migrates with its grants and relays intact, its open timed grant ends, the controller holds `prompt:relay`, and the
//! triggers hold the controller to an open window. No version number is written here: it comes from the file name.
mod common;

use capstan_ledger::{
    max_embedded_migration, migrate, migrations, open_database, Database, OpenOptions,
};
use common::scratch;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/ledger/ledger-better-sqlite3.sqlite")
}

fn keep_all() -> OpenOptions {
    OpenOptions {
        keep_migration_backups: 100,
        ..OpenOptions::default()
    }
}

const NOW: &str = "2026-01-01T00:00:00.000Z";
const SHA: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const OPTIONS: &str = r#"[{"number":1,"text":"Yes","acceptsText":false,"widensPermissions":false},{"number":2,"text":"Always","acceptsText":false,"widensPermissions":true},{"number":3,"text":"No, say why","acceptsText":true,"widensPermissions":false}]"#;

fn version() -> usize {
    migrations()
        .iter()
        .find(|m| m.name.ends_with("_approval_windows.sql"))
        .expect("the approval-windows migration is embedded")
        .version as usize
}

fn sha(n: i64) -> String {
    format!("{n:064x}")
}

/// The fixture one version before the migration, with an open timed grant, an open exact grant, a PM and a developer
/// agent and a captured relay of the PM; returns the project id.
fn ledger_before(file: &Path) -> String {
    std::fs::copy(fixture(), file).unwrap();
    let db = Database::new(Connection::open(file).unwrap());
    migrate(&db, file, &migrations()[..version() - 1]).unwrap();
    db.close().unwrap();
    let conn = Connection::open(file).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    let project: String = conn
        .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
        .unwrap();
    for (actor, role, n) in [("a-pm", "PM", 1), ("a-dev", "Developer", 2)] {
        conn.execute(
            "INSERT INTO actors(actor_id, project_id, display_name, role, seat_id, credential_hash, active, is_internal, created_at)
             VALUES (?, ?, ?, ?, NULL, ?, 1, 0, ?)",
            rusqlite::params![actor, project, actor, role, sha(n), NOW],
        )
        .unwrap();
    }
    for (agent, kind, actor) in [("pm-x", "PM", "a-pm"), ("dev-x", "Developer", "a-dev")] {
        conn.execute(
            "INSERT INTO agents(project_id, agent_id, role_name, kind, seat_id, actor_id, generation, state, last_activity_at, created_at)
             VALUES (?, ?, 'r', ?, ?, ?, 1, 'active', ?, ?)",
            rusqlite::params![project, agent, kind, format!("seat-{agent}"), actor, NOW, NOW],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO operator_proposals(project_id, proposal_id, sequence, kind, command, command_sha, reason, force_restart, proposer_agent_id, proposer_actor_id, state, created_at, updated_at)
         VALUES (?, 'op-1', 1, 'command', 'ls', ?, 'look', 0, 'dev-x', 'a-dev', 'proposed', ?, ?)",
        rusqlite::params![project, SHA, NOW, NOW],
    )
    .unwrap();
    for (n, kind) in [(1, "timed"), (2, "exact")] {
        conn.execute(
            "INSERT INTO operator_grants(project_id, grant_id, sequence, kind, text, command_sha, created_by, source_proposal_id, created_at, expires_at)
             VALUES (?, ?, ?, ?, 'ls', ?, 'a-pm', 'op-1', ?, '2099-01-01T00:00:00.000Z')",
            rusqlite::params![project, format!("grant-{n}"), n, kind, SHA, NOW],
        )
        .unwrap();
    }
    insert_relay(&conn, &project, 1, "dev-x", "a-pm", None, NOW).unwrap();
    project
}

fn insert_relay(
    conn: &Connection,
    project: &str,
    n: i64,
    agent: &str,
    actor: &str,
    window: Option<&str>,
    at: &str,
) -> rusqlite::Result<usize> {
    let columns = if window.is_some() {
        ", auto_window_id"
    } else {
        ""
    };
    let values = if window.is_some() { ", ?" } else { "" };
    let mut params: Vec<rusqlite::types::Value> = vec![
        project.to_string().into(),
        format!("relay-{n}").into(),
        n.into(),
        agent.to_string().into(),
        OPTIONS.to_string().into(),
        sha(100 + n).into(),
        actor.to_string().into(),
        at.to_string().into(),
    ];
    if let Some(window) = window {
        params.push(window.to_string().into());
    }
    conn.execute(
        &format!(
            "INSERT INTO prompt_relays(project_id, relay_id, sequence, agent_id, pane_id, host_kind, prompt_text, options_json, prompt_sha, captured_by_actor_id, captured_at, expires_at, state{columns})
             VALUES (?, ?, ?, ?, 'p', 'herdr', 'Allow?', ?, ?, ?, ?, '2099-01-01T00:00:00.000Z', 'captured'{values})"
        ),
        rusqlite::params_from_iter(params),
    )
}

/// The write is refused by the relay trigger (not by some other constraint).
fn refused(result: rusqlite::Result<usize>, why: &str) {
    let error = result.expect_err(why).to_string();
    assert!(error.contains("inside an open window"), "{why}: {error}");
}

fn migrated(file: &Path) -> Connection {
    open_database(file, &keep_all()).unwrap().close().unwrap();
    let conn = Connection::open(file).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    conn
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

fn controller(conn: &Connection) -> String {
    conn.query_row(
        "SELECT actor_id FROM actors WHERE role = 'controller' AND is_internal = 1",
        [],
        |r| r.get(0),
    )
    .unwrap()
}

fn window(
    conn: &Connection,
    project: &str,
    n: i64,
    proposal: Option<&str>,
    relay: Option<&str>,
) -> rusqlite::Result<usize> {
    conn.execute(
        "INSERT INTO approval_windows(project_id, window_id, sequence, kind, minutes, created_by, source_proposal_id, source_relay_id, created_at, expires_at)
         VALUES (?, ?, ?, 'minutes', 5, 'a-pm', ?, ?, ?, '2026-01-01T00:05:00.000Z')",
        rusqlite::params![project, format!("win-{n}"), n, proposal, relay, NOW],
    )
}

#[test]
fn a_previous_ledger_migrates_ends_its_timed_grant_and_keeps_its_rows() {
    let (_dir, file) = scratch();
    ledger_before(&file);
    let conn = migrated(&file);
    assert_eq!(
        count(&conn, "SELECT count(*) FROM schema_migrations"),
        max_embedded_migration()
    );
    let grants: Vec<(String, Option<String>)> = conn
        .prepare("SELECT kind, ended_reason FROM operator_grants ORDER BY sequence")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        grants,
        vec![
            ("timed".into(), Some("restart".into())),
            ("exact".into(), None)
        ]
    );
    assert_eq!(count(&conn, "SELECT count(*) FROM prompt_relays WHERE relay_id = 'relay-1' AND auto_window_id IS NULL AND state = 'captured'"), 1);
    let (strict, without_rowid): (i64, i64) = conn
        .query_row(
            "SELECT strict, wr FROM pragma_table_list WHERE name = 'approval_windows'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((strict, without_rowid), (1, 1));
}

#[test]
fn the_controller_holds_prompt_relay_after_the_migration() {
    let (_dir, file) = scratch();
    ledger_before(&file);
    let conn = migrated(&file);
    assert_eq!(count(&conn, "SELECT count(*) FROM role_capabilities WHERE role = 'controller' AND capability = 'prompt:relay'"), 1);
    assert_eq!(
        count(
            &conn,
            &format!(
                "SELECT count(*) FROM capability_grants WHERE actor_id = '{}' AND capability = 'prompt:relay' AND revoked_at IS NULL",
                controller(&conn)
            )
        ),
        1
    );
}

#[test]
fn one_open_window_and_exactly_one_source() {
    let (_dir, file) = scratch();
    let project = ledger_before(&file);
    let conn = migrated(&file);
    window(&conn, &project, 1, Some("op-1"), None).unwrap();
    assert!(
        window(&conn, &project, 2, Some("op-1"), None).is_err(),
        "a second open window"
    );
    conn.execute(
        "UPDATE approval_windows SET ended_at = ?, ended_reason = 'revoked'",
        [NOW],
    )
    .unwrap();
    assert!(
        window(&conn, &project, 3, Some("op-1"), Some("relay-1")).is_err(),
        "both sources"
    );
    assert!(window(&conn, &project, 3, None, None).is_err(), "no source");
    window(&conn, &project, 3, None, Some("relay-1")).unwrap();
    assert!(conn
        .execute("UPDATE approval_windows SET minutes = 9", [])
        .is_err());
    assert!(conn.execute("DELETE FROM approval_windows", []).is_err());
}

#[test]
fn the_relay_triggers_hold_the_controller_to_an_open_window_and_a_non_widening_option() {
    let (_dir, file) = scratch();
    let project = ledger_before(&file);
    let conn = migrated(&file);
    let controller = controller(&conn);
    conn.execute("UPDATE prompt_relays SET state = 'expired', outcome_reason = 'superseded' WHERE relay_id = 'relay-1'", []).unwrap();
    window(&conn, &project, 1, Some("op-1"), None).unwrap();
    let at = "2026-01-01T00:01:00.000Z";
    refused(
        insert_relay(&conn, &project, 2, "dev-x", &controller, None, at),
        "no window",
    );
    refused(
        insert_relay(&conn, &project, 2, "pm-x", &controller, Some("win-1"), at),
        "a PM agent",
    );
    refused(
        insert_relay(&conn, &project, 2, "dev-x", &controller, Some("win-9"), at),
        "an unknown window",
    );
    refused(
        insert_relay(&conn, &project, 2, "dev-x", "a-dev", Some("win-1"), at),
        "not the controller",
    );
    // The boundary: a capture at the expiry is refused, one millisecond before is accepted.
    refused(
        insert_relay(
            &conn,
            &project,
            2,
            "dev-x",
            &controller,
            Some("win-1"),
            "2026-01-01T00:05:00.000Z",
        ),
        "a capture at the expiry",
    );
    insert_relay(
        &conn,
        &project,
        2,
        "dev-x",
        &controller,
        Some("win-1"),
        "2026-01-01T00:04:59.999Z",
    )
    .unwrap();
    // A PM capture is as before and carries no window.
    refused(
        insert_relay(&conn, &project, 3, "dev-x", "a-pm", Some("win-1"), at),
        "a PM capture with a window",
    );
    insert_relay(&conn, &project, 3, "pm-x", "a-pm", None, at).unwrap();

    let answer = |kind: &str, option: Option<i64>, begin: &str, relay: &str| {
        conn.execute(
            "UPDATE prompt_relays SET state = 'typing', answer_kind = ?, answer_option = ?, answer_widens_permissions = 0,
               answered_by_actor_id = ?, auto_begin_at = ? WHERE relay_id = ?",
            rusqlite::params![kind, option, controller, begin, relay],
        )
    };
    refused(
        answer("option", Some(2), at, "relay-2"),
        "a widening option",
    );
    refused(
        answer("option", Some(3), at, "relay-2"),
        "an option that takes text",
    );
    refused(answer("esc", None, at, "relay-2"), "esc");
    refused(answer("text", Some(1), at, "relay-2"), "text");
    refused(
        answer("option", Some(1), "2026-01-01T00:05:00.000Z", "relay-2"),
        "at the expiry",
    );
    refused(
        answer("option", Some(1), at, "relay-3"),
        "a relay of the PM with no window",
    );
    answer("option", Some(1), "2026-01-01T00:04:59.999Z", "relay-2").unwrap();
}
