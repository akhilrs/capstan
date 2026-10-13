//! The timed-grants migration (the file whose name ends in `_operator_timed_grants.sql`): a ledger at the previous version
//! migrates with its grant rows, triggers and index intact, and the kind check admits 'timed' and nothing else new. No version
//! number is written here: it comes from the file name.
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

fn version() -> usize {
    migrations()
        .iter()
        .find(|m| m.name.ends_with("_operator_timed_grants.sql"))
        .expect("the timed-grants migration is embedded")
        .version as usize
}

/// The fixture one version before the migration, with an exact and a prefix grant (and the proposal and actor they need);
/// returns the project id.
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
    let (agent, actor): (String, String) = conn
        .query_row("SELECT agent_id, actor_id FROM agents LIMIT 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    conn.execute(
        "INSERT INTO operator_proposals(project_id, proposal_id, sequence, kind, command, command_sha, reason, force_restart, proposer_agent_id, proposer_actor_id, state, created_at, updated_at)
         VALUES (?, 'op-1', 1, 'command', 'ls', ?, 'look', 0, ?, ?, 'proposed', ?, ?)",
        rusqlite::params![project, SHA, agent, actor, NOW, NOW],
    )
    .unwrap();
    for (n, kind) in [(1, "exact"), (2, "prefix")] {
        insert(&conn, &project, n, kind).unwrap();
    }
    conn.execute(
        "UPDATE operator_grants SET ended_reason = 'revoked', revoked_at = ? WHERE sequence = 2",
        [NOW],
    )
    .unwrap();
    project
}

fn insert(conn: &Connection, project: &str, n: i64, kind: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "INSERT INTO operator_grants(project_id, grant_id, sequence, kind, text, command_sha, created_by, source_proposal_id, created_at, expires_at)
         VALUES (?, ?, ?, ?, 'ls', ?, (SELECT actor_id FROM actors LIMIT 1), 'op-1', ?, ?)",
        rusqlite::params![project, format!("grant-{n}"), n, kind, SHA, NOW, NOW],
    )
}

fn migrated(file: &Path) -> Connection {
    open_database(file, &keep_all()).unwrap().close().unwrap();
    let conn = Connection::open(file).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    conn
}

#[test]
fn a_ledger_at_the_previous_version_migrates_with_its_grants_intact() {
    let (_dir, file) = scratch();
    ledger_before(&file);
    let conn = migrated(&file);
    let versions: i64 = conn
        .query_row("SELECT count(*) FROM schema_migrations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(versions, max_embedded_migration());
    let rows: Vec<(String, String, Option<String>)> = conn
        .prepare("SELECT grant_id, kind, ended_reason FROM operator_grants ORDER BY sequence")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        rows,
        vec![
            ("grant-1".into(), "exact".into(), None),
            ("grant-2".into(), "prefix".into(), Some("revoked".into())),
        ]
    );
    let (strict, without_rowid): (i64, i64) = conn
        .query_row(
            "SELECT strict, wr FROM pragma_table_list WHERE name = 'operator_grants'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((strict, without_rowid), (1, 1));
    let index: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = 'operator_grants_open' AND tbl_name = 'operator_grants'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(index, 1);
    let triggers: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'trigger' AND tbl_name = 'operator_grants'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(triggers, 3);
}

#[test]
fn the_kind_check_admits_timed_and_nothing_else() {
    let (_dir, file) = scratch();
    let project = ledger_before(&file);
    let conn = migrated(&file);
    insert(&conn, &project, 3, "timed").unwrap();
    let error = insert(&conn, &project, 4, "other").unwrap_err().to_string();
    assert!(error.contains("CHECK"), "{error}");
}

#[test]
fn the_triggers_survive_the_rebuild() {
    let (_dir, file) = scratch();
    let project = ledger_before(&file);
    let conn = migrated(&file);
    insert(&conn, &project, 3, "timed").unwrap();
    for sql in [
        "UPDATE operator_grants SET text = 'rm' WHERE sequence = 3",
        "UPDATE operator_grants SET kind = 'exact' WHERE sequence = 3",
        "UPDATE operator_grants SET expires_at = 'x' WHERE sequence = 3",
    ] {
        let error = conn.execute(sql, []).unwrap_err().to_string();
        assert!(error.contains("immutable"), "{sql}: {error}");
    }
    let error = conn
        .execute(
            "UPDATE operator_grants SET ended_reason = 'expired' WHERE sequence = 2",
            [],
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("ends once"), "{error}");
    conn.execute(
        "UPDATE operator_grants SET ended_reason = 'expired' WHERE sequence = 3",
        [],
    )
    .unwrap();
    let error = conn
        .execute("DELETE FROM operator_grants", [])
        .unwrap_err()
        .to_string();
    assert!(error.contains("immutable"), "{error}");
}
