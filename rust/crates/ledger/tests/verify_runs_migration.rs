//! The verify-runs migration (the file whose name ends in `_verify_runs.sql`): a ledger at the previous version migrates
//! with every row intact, a fresh ledger has the table, and the triggers keep a run's identity and finished state fixed.
//! No version number is written here: it comes from the file name, so renumbering the file touches nothing else.
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
const SHA: &str = "1111111111111111111111111111111111111111";

/// The version of the verify-runs migration, from its file name.
fn version() -> usize {
    migrations()
        .iter()
        .find(|m| m.name.ends_with("_verify_runs.sql"))
        .expect("the verify-runs migration is embedded")
        .version as usize
}

/// The fixture one version before the verify-runs migration, with a report to run against.
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
    conn.execute(
        "INSERT INTO agent_reports(project_id, report_id, sequence, agent_id, generation, actor_id, commit_sha, summary, state, evidence_json, created_at)
         VALUES (?, 'rep', 900, 'a', 1, 'act', ?, 's', 'accepted', '{}', ?)",
        rusqlite::params![project, SHA, NOW],
    )
    .unwrap();
    project
}

fn insert_run(
    conn: &Connection,
    project: &str,
    run: &str,
    commands_sha: &str,
) -> rusqlite::Result<usize> {
    let sequence: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM verify_runs",
            [],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO verify_runs(project_id, run_id, sequence, subject_report_id, commit_sha, commands_json, commands_sha, trigger, requested_by_actor_id, state, queued_at)
         VALUES (?, ?, ?, 'rep', ?, '[\"make\"]', ?, 'manual', 'act', 'queued', ?)",
        rusqlite::params![project, run, sequence, SHA, commands_sha, NOW],
    )
}

fn refused(conn: &Connection, sql: &str, message: &str) {
    let error = conn.execute(sql, []).unwrap_err().to_string();
    assert!(error.contains(message), "{sql}: {error}");
}

fn migrated(file: &Path) -> Connection {
    open_database(file, &keep_all()).unwrap().close().unwrap();
    let conn = Connection::open(file).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    conn
}

#[test]
fn a_ledger_at_the_previous_version_migrates_with_its_rows_intact() {
    let (_dir, file) = scratch();
    ledger_before(&file);
    let before = Connection::open(&file).unwrap();
    let reports: i64 = before
        .query_row("SELECT count(*) FROM agent_reports", [], |r| r.get(0))
        .unwrap();
    drop(before);
    let conn = migrated(&file);
    let versions: i64 = conn
        .query_row("SELECT count(*) FROM schema_migrations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(versions, max_embedded_migration());
    let after: i64 = conn
        .query_row("SELECT count(*) FROM agent_reports", [], |r| r.get(0))
        .unwrap();
    assert_eq!(reports, after);
    let runs: i64 = conn
        .query_row("SELECT count(*) FROM verify_runs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(runs, 0);
    let strict: i64 = conn
        .query_row(
            "SELECT strict FROM pragma_table_list WHERE name = 'verify_runs'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(strict, 1);
}

#[test]
fn a_fresh_ledger_has_the_table_and_its_indexes() {
    let (_dir, file) = scratch();
    let conn = {
        open_database(&file, &keep_all()).unwrap().close().unwrap();
        Connection::open(&file).unwrap()
    };
    let indexes: Vec<String> = conn
        .prepare("SELECT name FROM pragma_index_list('verify_runs') ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(
        indexes.contains(&"verify_runs_by_commit".to_string()),
        "{indexes:?}"
    );
    assert!(
        indexes.contains(&"one_active_verify_run".to_string()),
        "{indexes:?}"
    );
}

#[test]
fn only_one_run_per_commit_and_commands_is_queued_or_running() {
    let (_dir, file) = scratch();
    let project = ledger_before(&file);
    let conn = migrated(&file);
    insert_run(&conn, &project, "r1", "c1").unwrap();
    assert!(insert_run(&conn, &project, "r2", "c1").is_err());
    insert_run(&conn, &project, "r3", "c2").unwrap();
    conn.execute(
        "UPDATE verify_runs SET state = 'cancelled', finished_at = ? WHERE run_id = 'r1'",
        [NOW],
    )
    .unwrap();
    insert_run(&conn, &project, "r4", "c1").unwrap();
}

#[test]
fn a_run_moves_forward_and_a_finished_run_is_final() {
    let (_dir, file) = scratch();
    let project = ledger_before(&file);
    let conn = migrated(&file);
    insert_run(&conn, &project, "r1", "c1").unwrap();
    refused(&conn, &format!("UPDATE verify_runs SET state = 'passed', exit_code = 0, started_at = '{NOW}', finished_at = '{NOW}' WHERE run_id = 'r1'"), "passes only from running");
    refused(
        &conn,
        "UPDATE verify_runs SET commit_sha = 'x' WHERE run_id = 'r1'",
        "identity is immutable",
    );
    refused(
        &conn,
        "UPDATE verify_runs SET commands_sha = 'x' WHERE run_id = 'r1'",
        "identity is immutable",
    );
    conn.execute(
        &format!(
            "UPDATE verify_runs SET state = 'running', started_at = '{NOW}' WHERE run_id = 'r1'"
        ),
        [],
    )
    .unwrap();
    refused(&conn, &format!("UPDATE verify_runs SET state = 'passed', exit_code = 1, finished_at = '{NOW}' WHERE run_id = 'r1'"), "exit code 0");
    refused(
        &conn,
        "UPDATE verify_runs SET state = 'queued' WHERE run_id = 'r1'",
        "only moves forward",
    );
    conn.execute(
        "UPDATE verify_runs SET pgid = 55, leader_start = 'a' WHERE run_id = 'r1'",
        [],
    )
    .unwrap();
    refused(
        &conn,
        "UPDATE verify_runs SET pgid = 56 WHERE run_id = 'r1'",
        "process is final",
    );
    refused(
        &conn,
        "UPDATE verify_runs SET pgid = 1 WHERE run_id = 'r1'",
        "process is final",
    );
    conn.execute(&format!("UPDATE verify_runs SET state = 'passed', exit_code = 0, finished_at = '{NOW}' WHERE run_id = 'r1'"), []).unwrap();
    for sql in [
        "UPDATE verify_runs SET state = 'failed' WHERE run_id = 'r1'",
        "UPDATE verify_runs SET exit_code = 3 WHERE run_id = 'r1'",
        "UPDATE verify_runs SET output_tail = 'x' WHERE run_id = 'r1'",
    ] {
        let error = conn.execute(sql, []).unwrap_err().to_string();
        assert!(
            error.contains("final") || error.contains("moves forward"),
            "{sql}: {error}"
        );
    }
    refused(
        &conn,
        "DELETE FROM verify_runs WHERE run_id = 'r1'",
        "immutable",
    );
}

#[test]
fn a_queued_run_cannot_fail_or_interrupt_and_an_interrupted_run_is_final() {
    let (_dir, file) = scratch();
    let project = ledger_before(&file);
    let conn = migrated(&file);
    insert_run(&conn, &project, "r1", "c1").unwrap();
    refused(
        &conn,
        "UPDATE verify_runs SET state = 'interrupted' WHERE run_id = 'r1'",
        "only moves forward",
    );
    conn.execute(
        &format!(
            "UPDATE verify_runs SET state = 'running', started_at = '{NOW}' WHERE run_id = 'r1'"
        ),
        [],
    )
    .unwrap();
    conn.execute(&format!("UPDATE verify_runs SET state = 'interrupted', finished_at = '{NOW}' WHERE run_id = 'r1'"), []).unwrap();
    refused(
        &conn,
        "UPDATE verify_runs SET state = 'passed', exit_code = 0 WHERE run_id = 'r1'",
        "",
    );
    // The output tail is bounded.
    insert_run(&conn, &project, "r2", "c2").unwrap();
    refused(
        &conn,
        "UPDATE verify_runs SET pgid = 1 WHERE run_id = 'r2'",
        "CHECK",
    );
    let long = "x".repeat(12289);
    let error = conn
        .execute(
            "UPDATE verify_runs SET output_tail = ? WHERE run_id = 'r2'",
            [&long],
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("CHECK"), "{error}");
}
