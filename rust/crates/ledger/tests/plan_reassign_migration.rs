//! The plan-reassign migration (the file whose name ends in `_plan_reassign.sql`): a ledger at the previous version
//! migrates, `plan_reassignments` is append-only with real foreign keys, and the plan architect trigger accepts a change
//! only against the newest reassignment of that plan, while keeping the replace rebind. No version number is written
//! here: it comes from the file name.
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
const FINAL: &str = "the plan architect is final";

fn version() -> usize {
    migrations()
        .iter()
        .find(|m| m.name.ends_with("_plan_reassign.sql"))
        .expect("the plan-reassign migration is embedded")
        .version as usize
}

/// The fixture one version before the migration with a PM actor, architects `a` and `b` (ended) and `e` (active) of role
/// architect, `c` and `d` (active) of role lead, `r` (active, role reviewer) and plans `p` and `q` owned by `a`. The
/// replace rebind moves a plan only between agents of one role, so the reassignment targets are of another role.
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
        "INSERT INTO actors(actor_id, project_id, display_name, role, seat_id, credential_hash, active, is_internal, created_at)
         VALUES ('a-pm', ?, 'pm', 'PM', NULL, ?, 1, 0, ?)",
        rusqlite::params![project, "1".repeat(64), NOW],
    )
    .unwrap();
    for (agent, role, state) in [
        ("a", "architect", "ended"),
        ("b", "architect", "ended"),
        ("c", "lead", "active"),
        ("d", "lead", "active"),
        ("e", "architect", "active"),
        ("r", "reviewer", "active"),
    ] {
        conn.execute(
            "INSERT INTO agents(project_id, agent_id, role_name, kind, seat_id, actor_id, generation, state, last_activity_at, created_at)
             VALUES (?, ?, ?, 'Developer', ?, ?, 1, 'active', ?, ?)",
            rusqlite::params![project, agent, role, format!("seat-{agent}"), format!("actor-{agent}"), NOW, NOW],
        )
        .unwrap();
        if state == "ended" {
            conn.execute(
                "UPDATE agents SET state = 'ended', ended_at = ? WHERE agent_id = ?",
                rusqlite::params![NOW, agent],
            )
            .unwrap();
        }
    }
    for (n, plan) in ["p", "q"].into_iter().enumerate() {
        conn.execute(
            "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, architect_agent_id, current_revision, approved_revision, created_at, updated_at)
             VALUES (?, ?, ?, 't', 'normal', 'draft', 'pm', 'a', 0, NULL, ?, ?)",
            rusqlite::params![project, plan, 100 + n as i64, NOW, NOW],
        )
        .unwrap();
    }
    project
}

fn migrated(file: &Path) -> Connection {
    open_database(file, &keep_all()).unwrap().close().unwrap();
    let conn = Connection::open(file).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    conn
}

fn reassign(
    conn: &Connection,
    project: &str,
    plan: &str,
    sequence: i64,
    from: &str,
    to: &str,
) -> rusqlite::Result<usize> {
    conn.execute(
        "INSERT INTO plan_reassignments(project_id, plan_id, sequence, from_agent_id, to_agent_id, actor_id, reason, created_at)
         VALUES (?, ?, ?, ?, ?, 'a-pm', NULL, ?)",
        rusqlite::params![project, plan, sequence, from, to, NOW],
    )
}

fn architect_of(conn: &Connection, plan: &str) -> String {
    conn.query_row(
        "SELECT architect_agent_id FROM plans WHERE plan_id = ?",
        [plan],
        |r| r.get(0),
    )
    .unwrap()
}

fn move_to(conn: &Connection, plan: &str, agent: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE plans SET architect_agent_id = ? WHERE plan_id = ?",
        [agent, plan],
    )
}

fn refused(result: rusqlite::Result<usize>, why: &str) {
    let error = result.expect_err(why).to_string();
    assert!(error.contains(FINAL), "{why}: {error}");
}

#[test]
fn a_ledger_at_the_previous_version_migrates_with_the_new_table() {
    let (_dir, file) = scratch();
    ledger_before(&file);
    let conn = migrated(&file);
    assert_eq!(
        conn.query_row("SELECT count(*) FROM schema_migrations", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        max_embedded_migration()
    );
    assert_eq!(architect_of(&conn, "p"), "a");
    let (strict, without_rowid): (i64, i64) = conn
        .query_row(
            "SELECT strict, wr FROM pragma_table_list WHERE name = 'plan_reassignments'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((strict, without_rowid), (1, 1));
}

#[test]
fn reassignments_are_append_only_and_checked() {
    let (_dir, file) = scratch();
    let project = ledger_before(&file);
    let conn = migrated(&file);
    reassign(&conn, &project, "p", 1, "a", "c").unwrap();
    assert!(conn
        .execute("UPDATE plan_reassignments SET reason = 'x'", [])
        .is_err());
    assert!(conn.execute("DELETE FROM plan_reassignments", []).is_err());
    assert!(
        reassign(&conn, &project, "p", 0, "a", "c").is_err(),
        "sequence must be positive"
    );
    assert!(
        reassign(&conn, &project, "p", 2, "c", "c").is_err(),
        "to differs from from"
    );
    assert!(
        reassign(&conn, &project, "p", 1, "a", "d").is_err(),
        "sequence is the key"
    );
}

#[test]
fn foreign_keys_refuse_unknown_plans_agents_and_actors() {
    let (_dir, file) = scratch();
    let project = ledger_before(&file);
    let conn = migrated(&file);
    conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
    reassign(&conn, &project, "p", 1, "a", "c").unwrap();
    assert!(
        reassign(&conn, &project, "p", 2, "a", "nobody").is_err(),
        "unknown target"
    );
    assert!(
        reassign(&conn, &project, "p", 2, "nobody", "c").is_err(),
        "unknown source"
    );
    assert!(
        reassign(&conn, &project, "nope", 1, "a", "c").is_err(),
        "unknown plan"
    );
    assert!(
        conn.execute(
            "INSERT INTO plan_reassignments(project_id, plan_id, sequence, from_agent_id, to_agent_id, actor_id, created_at)
             VALUES (?, 'p', 2, 'a', 'd', 'a-ghost', ?)",
            rusqlite::params![project, NOW],
        )
        .is_err(),
        "unknown actor"
    );
}

#[test]
fn the_architect_moves_only_against_the_newest_row_of_its_plan() {
    let (_dir, file) = scratch();
    let project = ledger_before(&file);
    let conn = migrated(&file);
    refused(move_to(&conn, "p", "c"), "no row at all");
    reassign(&conn, &project, "p", 1, "a", "c").unwrap();
    refused(move_to(&conn, "p", "d"), "another target than the row's");
    refused(move_to(&conn, "q", "c"), "a row of another plan");
    move_to(&conn, "p", "c").unwrap();
    assert_eq!(architect_of(&conn, "p"), "c");
    assert_eq!(architect_of(&conn, "q"), "a");
}

#[test]
fn the_old_owner_must_have_stopped_the_target_be_active_and_the_plan_live() {
    let (_dir, file) = scratch();
    let project = ledger_before(&file);
    let conn = migrated(&file);
    // The target `b` is ended.
    reassign(&conn, &project, "p", 1, "a", "b").unwrap();
    refused(move_to(&conn, "p", "b"), "an ended target");
    // The old owner `c` is active.
    reassign(&conn, &project, "q", 1, "a", "c").unwrap();
    conn.execute(
        "UPDATE agents SET state = 'active', ended_at = NULL WHERE agent_id = 'a'",
        [],
    )
    .unwrap();
    refused(move_to(&conn, "q", "c"), "an owner that is still active");
    conn.execute(
        "UPDATE agents SET state = 'ended', ended_at = ? WHERE agent_id = 'a'",
        [NOW],
    )
    .unwrap();
    conn.execute(
        "UPDATE plans SET cancelled_at = ? WHERE plan_id = 'q'",
        [NOW],
    )
    .unwrap();
    refused(move_to(&conn, "q", "c"), "a cancelled plan");
}

#[test]
fn a_chain_moves_on_and_never_back() {
    let (_dir, file) = scratch();
    let project = ledger_before(&file);
    let conn = migrated(&file);
    reassign(&conn, &project, "p", 1, "a", "c").unwrap();
    move_to(&conn, "p", "c").unwrap();
    conn.execute(
        "UPDATE agents SET state = 'ended', ended_at = ? WHERE agent_id = 'c'",
        [NOW],
    )
    .unwrap();
    reassign(&conn, &project, "p", 2, "c", "d").unwrap();
    move_to(&conn, "p", "d").unwrap();
    conn.execute(
        "UPDATE agents SET state = 'ended', ended_at = ? WHERE agent_id = 'd'",
        [NOW],
    )
    .unwrap();
    // Both earlier owners are active again, so only the newest row (c -> d) stands in the way.
    conn.execute(
        "UPDATE agents SET state = 'active', ended_at = NULL WHERE agent_id IN ('a', 'c')",
        [],
    )
    .unwrap();
    refused(move_to(&conn, "p", "a"), "back to the first owner");
}

#[test]
fn the_replace_rebind_still_works_and_other_changes_are_still_refused() {
    let (_dir, file) = scratch();
    ledger_before(&file);
    let conn = migrated(&file);
    refused(move_to(&conn, "p", "r"), "another role");
    refused(
        move_to(&conn, "p", "c"),
        "another role without a reassignment",
    );
    refused(move_to(&conn, "p", "b"), "an ended successor");
    move_to(&conn, "p", "e").unwrap();
    assert_eq!(architect_of(&conn, "p"), "e");
}
