//! The workspace-repos migration (the file whose name ends in `_workspace_repos.sql`): a populated ledger at the previous
//! version migrates with its rows intact and its older schema objects unchanged except the verify run identity (the repo).
//! No version number is written here: it comes from the file name.
mod common;

use capstan_ledger::{
    max_embedded_migration, migrate, migrations, open_database, Database, OpenOptions,
};
use common::scratch;
use rusqlite::Connection;
use std::collections::BTreeSet;
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
const OTHER: &str = "2222222222222222222222222222222222222222";

fn version() -> usize {
    migrations()
        .iter()
        .find(|m| m.name.ends_with("_workspace_repos.sql"))
        .expect("the workspace-repos migration is embedded")
        .version as usize
}

/// The names this migration replaces: the verify runs' table, its active-run index and its identity trigger.
const CHANGED: [&str; 3] = [
    "verify_runs",
    "one_active_verify_run",
    "immutable_verify_runs_identity",
];

fn schema(conn: &Connection) -> BTreeSet<(String, String, String)> {
    let mut statement = conn
        .prepare("SELECT type, name, COALESCE(sql, '') FROM sqlite_master WHERE name NOT LIKE 'sqlite_stat%'")
        .unwrap();
    let rows = statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    rows
}

/// A populated ledger one version before the migration: a queued run of a report, a finished run and a review of an
/// integration. Returns the project id.
fn populated_before(file: &Path) -> String {
    std::fs::copy(fixture(), file).unwrap();
    let db = Database::new(Connection::open(file).unwrap());
    migrate(&db, file, &migrations()[..version() - 1]).unwrap();
    db.close().unwrap();
    let conn = Connection::open(file).unwrap();
    let project: String = conn
        .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
        .unwrap();
    let (report, actor): (String, String) = conn
        .query_row(
            "SELECT report_id, actor_id FROM agent_reports LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let pm_actor: String = conn
        .query_row(
            "SELECT actor_id FROM agents WHERE agent_id = 'pm-agent'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, head_sha, created_at, completed_at)
         VALUES (?, 'int-1', 1, ?, 'int/one', 'x', 'merged', ?, ?, ?)",
        rusqlite::params![project, OTHER, SHA, NOW, NOW],
    )
    .unwrap();
    for (run, sequence, report_id, integration, state, started, finished) in [
        (
            "run-1",
            1,
            Some(report.as_str()),
            None,
            "queued",
            None,
            None,
        ),
        (
            "run-2",
            2,
            None,
            Some("int-1"),
            "passed",
            Some(NOW),
            Some(NOW),
        ),
    ] {
        conn.execute(
            "INSERT INTO verify_runs(project_id, run_id, sequence, subject_report_id, subject_integration_id, commit_sha, commands_json, commands_sha, trigger, requested_by_actor_id, state, queued_at, started_at, finished_at, exit_code)
             VALUES (?, ?, ?, ?, ?, ?, '[\"make test\"]', 'cmd-a', 'manual', ?, ?, ?, ?, ?, ?)",
            rusqlite::params![project, run, sequence, report_id, integration, SHA, actor, state, NOW, started, finished, finished.map(|_| 0)],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO reviews(project_id, review_id, sequence, round, subject_integration_id, commit_sha, base_sha, requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, state, created_at)
         VALUES (?, 'rv-1', 1, 1, 'int-1', ?, ?, ?, 'pm', 'pm-agent', ?, 'started', ?)",
        rusqlite::params![project, SHA, OTHER, actor, pm_actor, NOW],
    )
    .unwrap();
    project
}

fn migrated(file: &Path) -> Connection {
    open_database(file, &keep_all()).unwrap().close().unwrap();
    let conn = Connection::open(file).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
    conn
}

fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn a_populated_ledger_migrates_with_its_rows_and_older_objects_intact() {
    let (_dir, file) = scratch();
    populated_before(&file);
    let before = Connection::open(&file).unwrap();
    let old_schema = schema(&before);
    let counts: Vec<(String, i64)> = [
        "agent_reports",
        "agents",
        "integrations",
        "verify_runs",
        "reviews",
        "actors",
    ]
    .iter()
    .map(|t| (t.to_string(), count(&before, t)))
    .collect();
    drop(before);

    let conn = migrated(&file);
    let versions: i64 = conn
        .query_row("SELECT count(*) FROM schema_migrations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(versions, max_embedded_migration());
    for (table, rows) in counts {
        assert_eq!(count(&conn, &table), rows, "{table}");
    }
    // Every older schema object keeps its SQL, except the three this migration replaces.
    let new_schema = schema(&conn);
    for object in old_schema
        .iter()
        .filter(|(_, name, _)| !CHANGED.contains(&name.as_str()))
    {
        assert!(new_schema.contains(object), "{} changed", object.1);
    }
    let sql = |name: &str| -> String {
        conn.query_row(
            "SELECT sql FROM sqlite_master WHERE name = ?",
            [name],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert!(sql("verify_runs").contains("repo TEXT NOT NULL DEFAULT ('')"));
    assert!(sql("one_active_verify_run").contains("(project_id, repo, commit_sha, commands_sha)"));
    assert!(sql("immutable_verify_runs_identity").contains(" repo,"));
    let violations: i64 = conn
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(violations, 0);
    for table in [
        "agent_worktrees",
        "report_commits",
        "integration_sets",
        "integration_set_members",
        "cleanup_repos",
    ] {
        assert_eq!(count(&conn, table), 0, "{table} starts empty");
        let (strict, without_rowid): (i64, i64) = conn
            .query_row(
                "SELECT strict, wr FROM pragma_table_list WHERE name = ?",
                [table],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((strict, without_rowid), (1, 1), "{table}");
    }
    // Old runs belong to the root repo.
    let repos: i64 = conn
        .query_row(
            "SELECT count(*) FROM verify_runs WHERE repo = ''",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(repos, 2);
}

fn queue(
    conn: &Connection,
    run: &str,
    sequence: i64,
    repo: Option<&str>,
) -> rusqlite::Result<usize> {
    let (project, report, actor): (String, String, String) = conn
        .query_row(
            "SELECT project_id, report_id, actor_id FROM agent_reports LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    match repo {
        Some(repo) => conn.execute(
            "INSERT INTO verify_runs(project_id, run_id, sequence, subject_report_id, commit_sha, commands_json, commands_sha, trigger, requested_by_actor_id, state, queued_at, repo)
             VALUES (?, ?, ?, ?, ?, '[\"x\"]', 'cmd-a', 'manual', ?, 'queued', ?, ?)",
            rusqlite::params![project, run, sequence, report, SHA, actor, NOW, repo],
        ),
        None => conn.execute(
            "INSERT INTO verify_runs(project_id, run_id, sequence, subject_report_id, commit_sha, commands_json, commands_sha, trigger, requested_by_actor_id, state, queued_at)
             VALUES (?, ?, ?, ?, ?, '[\"x\"]', 'cmd-a', 'manual', ?, 'queued', ?)",
            rusqlite::params![project, run, sequence, report, SHA, actor, NOW],
        ),
    }
}

#[test]
fn the_active_run_is_unique_per_repo_and_the_repo_is_part_of_the_identity() {
    let (_dir, file) = scratch();
    populated_before(&file);
    let conn = migrated(&file);
    // The old queued run (repo '') is still the one active run of its commit and commands.
    assert!(queue(&conn, "run-3", 3, None).is_err());
    // Two other repos may queue the same commit and commands, one repo twice may not.
    queue(&conn, "run-4", 4, Some("api")).unwrap();
    queue(&conn, "run-5", 5, Some("web")).unwrap();
    assert!(queue(&conn, "run-6", 6, Some("api")).is_err());
    // The repo and the older identity columns are fixed.
    assert!(conn
        .execute(
            "UPDATE verify_runs SET repo = 'x' WHERE run_id = 'run-4'",
            []
        )
        .is_err());
    assert!(conn
        .execute(
            "UPDATE verify_runs SET commit_sha = ? WHERE run_id = 'run-4'",
            [OTHER]
        )
        .is_err());
    assert!(conn
        .execute(
            "UPDATE verify_runs SET commands_sha = 'z' WHERE run_id = 'run-4'",
            []
        )
        .is_err());
    // A run still moves forward as before.
    conn.execute(
        "UPDATE verify_runs SET state = 'cancelled', finished_at = ? WHERE run_id = 'run-4'",
        [NOW],
    )
    .unwrap();
    queue(&conn, "run-7", 7, Some("api")).unwrap();
}

#[test]
fn the_new_tables_keep_their_rules() {
    let (_dir, file) = scratch();
    let project = populated_before(&file);
    let conn = migrated(&file);
    let (agent, report): (String, String) = conn
        .query_row(
            "SELECT agent_id, report_id FROM agent_reports LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    // Report commits: one per repo, immutable, tied to a report.
    let insert = |repo: &str, report_id: &str, sha: &str| {
        conn.execute(
            "INSERT INTO report_commits(project_id, report_id, repo, commit_sha, branch, base_sha) VALUES (?, ?, ?, ?, 'b', ?)",
            rusqlite::params![project, report_id, repo, sha, OTHER],
        )
    };
    insert("api", &report, SHA).unwrap();
    assert!(insert("api", &report, SHA).is_err());
    assert!(insert("web", "no-such-report", SHA).is_err());
    assert!(insert("web", &report, "short").is_err());
    assert!(conn
        .execute("UPDATE report_commits SET branch = 'c'", [])
        .is_err());
    assert!(conn.execute("DELETE FROM report_commits", []).is_err());
    // Worktrees: one row per agent, generation and repo.
    let worktree = |repo: &str, sha: &str| {
        conn.execute(
            "INSERT INTO agent_worktrees(project_id, agent_id, generation, repo, worktree_path, branch, base_sha, created_at) VALUES (?, ?, 1, ?, '/w', 'b', ?, ?)",
            rusqlite::params![project, agent, repo, sha, NOW],
        )
    };
    worktree("api", SHA).unwrap();
    assert!(worktree("api", SHA).is_err());
    assert!(worktree("web", "bad").is_err());
    // Sets: members belong to a set and to an integration that exists.
    conn.execute(
        "INSERT INTO integration_sets(project_id, set_id, sequence, branch, state, created_by, created_at) VALUES (?, 's-1', 1, 'int/s', 'running', 'x', ?)",
        rusqlite::params![project, NOW],
    )
    .unwrap();
    let member = |set: &str, integration: Option<&str>| {
        conn.execute(
            "INSERT INTO integration_set_members(project_id, set_id, repo, position, integration_id) VALUES (?, ?, 'api', 0, ?)",
            rusqlite::params![project, set, integration],
        )
    };
    assert!(member("no-set", None).is_err());
    assert!(member("s-1", Some("no-integration")).is_err());
    member("s-1", Some("int-1")).unwrap();
    assert!(member("s-1", None).is_err());
}
