//! Ledgers written by Node open in Rust and migrate to the same schema, with the same `schema_migrations` rows and
//! checksums and the same `pre-vN` backups (replaces test/ledger-compat.test.ts, test/migration-backups.test.ts,
//! test/ledger-rust-interop.test.ts and test/sqlite-adapter.test.ts for the Rust ledger).
//!
//! The Node-made input is the committed better-sqlite3 fixture (`test/fixtures/ledger-better-sqlite3.sqlite`, a v31
//! ledger with rows). The ledger of every other version is built the way Node's `openDatabase` builds one: each
//! migration file run as it is, then its `schema_migrations` row. That is plain SQLite work in this file; it does not go
//! through the migration runner under test. No Node runs and no new fixture is generated.
mod common;

use capstan_ledger::{
    checksum, max_embedded_migration, migrations, open_database, open_database_read_only,
    OpenOptions,
};
use common::scratch;
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn fixture() -> PathBuf {
    root().join("test/fixtures/ledger-better-sqlite3.sqlite")
}

fn migration_files() -> Vec<(String, Vec<u8>)> {
    let mut names: Vec<String> = std::fs::read_dir(root().join("migrations"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".sql"))
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let bytes = std::fs::read(root().join("migrations").join(&name)).unwrap();
            (name, bytes)
        })
        .collect()
}

/// A ledger at `version` the way Node builds one: WAL, each file, its row.
fn node_style_ledger(file: &Path, version: usize) {
    let conn = Connection::open(file).unwrap();
    conn.pragma_update(None, "journal_mode", "wal").unwrap();
    for (index, (name, bytes)) in migration_files().iter().take(version).enumerate() {
        conn.execute_batch(std::str::from_utf8(bytes).unwrap())
            .unwrap();
        conn.execute(
            "INSERT INTO schema_migrations(version, name, checksum, applied_at) VALUES (?, ?, ?, ?)",
            rusqlite::params![
                index as i64 + 1,
                name,
                checksum(bytes),
                "2026-01-01T00:00:00.000Z"
            ],
        )
        .unwrap();
    }
}

fn backups(dir: &Path) -> Vec<(i64, i64)> {
    let mut found: Vec<(i64, i64)> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| {
            let name = e.unwrap().file_name().into_string().unwrap();
            let rest = name.strip_prefix("controller.sqlite.pre-v")?;
            let rest = rest.strip_suffix(".sqlite")?;
            let (version, at) = rest.split_once('-')?;
            Some((version.parse().ok()?, at.parse().ok()?))
        })
        .collect();
    found.sort_by(|a, b| b.cmp(a));
    found
}

#[derive(Debug, PartialEq)]
struct Dump {
    master: Vec<(String, String, String, Option<String>)>,
    migrations: Vec<(i64, String, String)>,
    user_version: i64,
}

fn dump(file: &Path) -> Dump {
    let conn = Connection::open_with_flags(file, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let master = conn
        .prepare("SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let migrations = conn
        .prepare("SELECT version, name, checksum FROM schema_migrations ORDER BY version")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let user_version = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    Dump {
        master,
        migrations,
        user_version,
    }
}

fn header(file: &Path) -> Vec<u8> {
    std::fs::read(file).unwrap()[..100].to_vec()
}

fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn columns(conn: &Connection, table: &str) -> Vec<String> {
    conn.prepare(&format!("PRAGMA table_info({table})"))
        .unwrap()
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn keep_all() -> OpenOptions {
    OpenOptions {
        keep_migration_backups: 100,
        ..OpenOptions::default()
    }
}

#[test]
fn every_migration_file_is_embedded_with_the_checksum_of_its_bytes() {
    let files = migration_files();
    assert_eq!(files.len() as i64, max_embedded_migration());
    for (index, (name, bytes)) in files.iter().enumerate() {
        let embedded = &migrations()[index];
        assert_eq!(
            (embedded.version, embedded.name),
            (index as i64 + 1, name.as_str())
        );
        assert_eq!(embedded.checksum, checksum(bytes));
    }
    // No migration number is used twice.
    let mut numbers: Vec<&str> = files.iter().map(|(n, _)| &n[..4]).collect();
    numbers.dedup();
    assert_eq!(numbers.len(), files.len());
}

#[test]
fn the_better_sqlite3_fixture_opens_verifies_and_reads_back_field_for_field() {
    let (dir, file) = scratch();
    std::fs::copy(fixture(), &file).unwrap();
    let before = header(&file);
    let before_rows = dump(&file).migrations;
    assert_eq!(before_rows.len(), 31, "the fixture is a v31 ledger");
    let behind = before_rows.len() as i64;
    let pending = max_embedded_migration() - behind;
    assert!(pending > 0, "the fixture is behind this build");

    open_database(&file, &keep_all()).unwrap().close().unwrap();

    // Every row is the embedded migration's; the rows the fixture already had were not touched.
    let after = dump(&file);
    assert_eq!(after.migrations.len() as i64, max_embedded_migration());
    for (index, (version, name, sum)) in after.migrations.iter().enumerate() {
        let (file_name, bytes) = &migration_files()[index];
        assert_eq!((*version, name), (index as i64 + 1, file_name));
        assert_eq!(sum, &checksum(bytes), "{name} checksum verifies");
    }
    assert_eq!(after.migrations[..before_rows.len()], before_rows[..]);
    // Only the pending migrations wrote a backup.
    let versions: Vec<i64> = backups(dir.path()).iter().map(|b| b.0).collect();
    assert_eq!(
        versions,
        (behind + 1..=max_embedded_migration())
            .rev()
            .collect::<Vec<_>>()
    );
    // The header keeps its page size, format versions and user_version; only the change counter moves.
    let after_header = header(&file);
    assert_eq!(after_header[16..24], before[16..24]);
    assert_eq!(after_header[60..64], before[60..64]);

    let conn = Connection::open(&file).unwrap();
    let check: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(check, "ok");
    let violations: i64 = conn
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(violations, 0);
    let journal: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(journal, "wal");

    assert_eq!(count(&conn, "projects"), 1);
    assert_eq!(count(&conn, "actors"), 4);
    assert_eq!(count(&conn, "messages"), 1);
    assert_eq!(count(&conn, "plans"), 1);
    assert_eq!(count(&conn, "agent_reports"), 1);
    let name: String = conn
        .query_row("SELECT name FROM projects", [], |r| r.get(0))
        .unwrap();
    assert_eq!(name, "Ledger fixture project");
    let plan: (String, i64, String, String, String) = conn
        .query_row(
            "SELECT plan_id, sequence, title, tier, state FROM plans",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(
        plan,
        (
            "plan-1".into(),
            1,
            "Fixture plan".into(),
            "normal".into(),
            "draft".into()
        )
    );
    let report: (String, String, String) = conn
        .query_row(
            "SELECT commit_sha, summary, state FROM agent_reports",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        report,
        ("b".repeat(40), "fixture report".into(), "accepted".into())
    );
    let mut roles: Vec<String> = conn
        .prepare("SELECT role FROM actors")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    roles.sort();
    assert_eq!(roles, ["Developer", "PM", "controller", "operator"]);
}

#[test]
fn the_fixture_opens_read_only_without_migrating() {
    let (dir, file) = scratch();
    std::fs::copy(fixture(), &file).unwrap();
    let database = open_database_read_only(&file).unwrap();
    let rows = database
        .connection()
        .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap();
    assert_eq!(rows, 31);
    assert!(database.exec("CREATE TABLE t (x)").is_err());
    database.close().unwrap();
    assert!(backups(dir.path()).is_empty());
}

#[test]
fn a_ledger_written_by_node_at_every_version_migrates_to_the_schema_of_a_straight_run() {
    let latest = migration_files().len();
    // The reference: every file in order on an empty database, no runner involved.
    let (_reference_dir, reference_file) = scratch();
    node_style_ledger(&reference_file, latest);
    let reference = dump(&reference_file);
    assert_eq!(reference.migrations.len(), latest);

    for version in 0..=latest {
        let (dir, file) = scratch();
        if version > 0 {
            node_style_ledger(&file, version);
        }
        open_database(&file, &OpenOptions::default())
            .unwrap()
            .close()
            .unwrap();
        let migrated = dump(&file);
        assert_eq!(migrated.master, reference.master, "v{version}: schema");
        assert_eq!(
            migrated.migrations, reference.migrations,
            "v{version}: schema_migrations rows"
        );
        assert_eq!(migrated.user_version, reference.user_version, "v{version}");
        // One backup before every step that has a ledger before it (not the first), and the newest three are kept.
        let kept = backups(dir.path());
        let expected = std::cmp::min(
            3,
            if version == 0 {
                latest - 1
            } else {
                latest - version
            },
        );
        assert_eq!(kept.len(), expected, "v{version}: kept backups {kept:?}");
        let newest = latest as i64;
        for (index, (backed_up, at)) in kept.iter().enumerate() {
            assert_eq!(*backed_up, newest - index as i64, "v{version}");
            assert!(*at > 1_700_000_000_000, "v{version}: {at}");
        }
    }
}

#[test]
fn a_ledger_created_by_rust_is_what_node_would_find_current() {
    // Node opens a ledger when every row matches its migration file and the newest version is its own; a second open of a
    // Rust-made ledger changes no byte of the schema and writes no backup.
    let (dir, file) = scratch();
    open_database(&file, &OpenOptions::default())
        .unwrap()
        .close()
        .unwrap();
    let before = dump(&file);
    let backups_before = backups(dir.path());
    for row in &before.migrations {
        let (_, bytes) = &migration_files()[row.0 as usize - 1];
        assert_eq!(row.2, checksum(bytes));
    }
    let applied: Vec<String> = Connection::open(&file)
        .unwrap()
        .prepare("SELECT applied_at FROM schema_migrations ORDER BY version")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    for at in &applied {
        let b = at.as_bytes();
        assert!(
            at.len() == 24 && b[10] == b'T' && b[19] == b'.' && at.ends_with('Z'),
            "{at}"
        );
    }
    open_database(&file, &OpenOptions::default())
        .unwrap()
        .close()
        .unwrap();
    assert_eq!(dump(&file), before);
    assert_eq!(backups(dir.path()), backups_before);
}

#[test]
fn migrating_the_fixture_prunes_the_backups_it_took_to_the_configured_count() {
    let (dir, file) = scratch();
    std::fs::copy(fixture(), &file).unwrap();
    // Older backups of other versions are pruned together with the new ones, newest versions first.
    for (version, at) in [(1, 1), (2, 2), (3, 3)] {
        std::fs::write(
            dir.path()
                .join(format!("controller.sqlite.pre-v{version}-{at}.sqlite")),
            "x",
        )
        .unwrap();
    }
    let options = OpenOptions {
        keep_migration_backups: 2,
        ..OpenOptions::default()
    };
    open_database(&file, &options).unwrap().close().unwrap();
    let kept: Vec<i64> = backups(dir.path()).iter().map(|b| b.0).collect();
    let last = max_embedded_migration();
    assert_eq!(kept, [last, last - 1]);
}

#[test]
fn migration_0033_gives_every_existing_message_action_needed_0_and_is_backed_up_first() {
    let (dir, file) = scratch();
    std::fs::copy(fixture(), &file).unwrap();
    let before = Connection::open(&file).unwrap();
    let stored = count(&before, "messages");
    assert!(stored > 0, "the fixture has messages");
    assert!(!columns(&before, "messages").contains(&"action_needed".to_string()));
    drop(before);
    open_database(&file, &keep_all()).unwrap().close().unwrap();
    let conn = Connection::open(&file).unwrap();
    let values: Vec<i64> = conn
        .prepare("SELECT action_needed FROM messages")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(values.len() as i64, stored);
    assert!(values.iter().all(|v| *v == 0));
    assert!(conn
        .execute("UPDATE messages SET action_needed = 2", [])
        .is_err());
    assert!(backups(dir.path()).iter().any(|b| b.0 == 33));
}

#[test]
fn migration_0034_adds_task_columns_as_null_and_its_checks_reject_bad_values() {
    let (dir, file) = scratch();
    std::fs::copy(fixture(), &file).unwrap();
    let select = "SELECT project_id, agent_id, workspace_id, pane_id, worktree_path, branch, base_sha, generation, created_at, updated_at FROM agent_panes ORDER BY agent_id";
    let rows = |conn: &Connection| -> Vec<String> {
        conn.prepare(select)
            .unwrap()
            .query_map([], |r| {
                (0..10)
                    .map(|i| {
                        Ok(match r.get_ref(i)? {
                            rusqlite::types::ValueRef::Null => "NULL".to_string(),
                            rusqlite::types::ValueRef::Integer(n) => n.to_string(),
                            rusqlite::types::ValueRef::Text(t) => {
                                String::from_utf8_lossy(t).into_owned()
                            }
                            other => format!("{other:?}"),
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(|cells| cells.join("|"))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let before = Connection::open(&file).unwrap();
    assert!(!columns(&before, "agent_panes").contains(&"task_ref".to_string()));
    assert!(!columns(&before, "agent_panes").contains(&"task_title".to_string()));
    let old_rows = rows(&before);
    assert!(!old_rows.is_empty(), "the fixture has agent_panes rows");
    drop(before);
    open_database(&file, &keep_all()).unwrap().close().unwrap();
    let conn = Connection::open(&file).unwrap();
    assert!(columns(&conn, "agent_panes").contains(&"task_ref".to_string()));
    assert!(columns(&conn, "agent_panes").contains(&"task_title".to_string()));
    assert_eq!(rows(&conn), old_rows);
    let set: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM agent_panes WHERE task_ref IS NOT NULL OR task_title IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(set, 0);
    for (column, value) in [
        ("task_ref", String::new()),
        ("task_ref", "  ".into()),
        ("task_ref", "a".repeat(258)),
        ("task_title", String::new()),
        ("task_title", "   ".into()),
        ("task_title", "a".repeat(201)),
    ] {
        assert!(
            conn.execute(&format!("UPDATE agent_panes SET {column} = ?"), [&value])
                .is_err(),
            "{column} {}",
            value.len()
        );
    }
    conn.execute(
        "UPDATE agent_panes SET task_ref = ?, task_title = ?",
        ["a".repeat(257), "b".repeat(200)],
    )
    .unwrap();
    assert!(backups(dir.path()).iter().any(|b| b.0 == 34));
}

#[test]
fn migration_0037_adds_terminal_ids_as_null_and_its_checks_reject_bad_values() {
    let (dir, file) = scratch();
    std::fs::copy(fixture(), &file).unwrap();
    let before = Connection::open(&file).unwrap();
    for table in ["agent_panes", "orphan_panes"] {
        assert!(!columns(&before, table).contains(&"terminal_id".to_string()));
    }
    assert!(count(&before, "agent_panes") > 0);
    drop(before);
    open_database(&file, &keep_all()).unwrap().close().unwrap();
    let conn = Connection::open(&file).unwrap();
    for table in ["agent_panes", "orphan_panes"] {
        assert!(columns(&conn, table).contains(&"terminal_id".to_string()));
        let set: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE terminal_id IS NOT NULL"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(set, 0);
    }
    for value in [String::new(), "a b".into(), "a;b".into(), "a".repeat(129)] {
        assert!(
            conn.execute("UPDATE agent_panes SET terminal_id = ?", [&value])
                .is_err(),
            "terminal_id {value:?}"
        );
    }
    conn.execute(
        "UPDATE agent_panes SET terminal_id = ?",
        ["term_65d5f7a45f83b5a"],
    )
    .unwrap();
    assert!(backups(dir.path()).iter().any(|b| b.0 == 37));
}

#[test]
fn a_ledger_with_integration_branches_keeps_every_row_trigger_and_index_through_the_migrations() {
    let (_dir, file) = scratch();
    std::fs::copy(fixture(), &file).unwrap();
    let sha = "a".repeat(40);
    let body = r#"{"summary":"s","packages":[{"id":"wp1","title":"One","owns":["src/a"],"estimate_hours":1,"acceptance":["works"]}]}"#;
    {
        let old = Connection::open(&file).unwrap();
        let project: String = old
            .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
            .unwrap();
        let report: String = old
            .query_row("SELECT report_id FROM agent_reports", [], |r| r.get(0))
            .unwrap();
        let (agent, actor): (String, String) = old
            .query_row("SELECT agent_id, actor_id FROM agents LIMIT 1", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        old.execute(
            "INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
             VALUES (?, 'plan-1', 1, ?, ?, ?, ?, ?, '2026-01-01T00:00:00Z')",
            rusqlite::params![
                project,
                sha,
                body,
                capstan_ledger::checksum(body.as_bytes()),
                agent,
                actor
            ],
        )
        .unwrap();
        old.execute(
            "INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, head_sha, created_at, completed_at)
             VALUES (?, 'int-old', 1, ?, 'capstan/integration/int-old', 'operator', 'running', NULL, '2026-01-01T00:00:00Z', NULL)",
            rusqlite::params![project, sha],
        )
        .unwrap();
        old.execute(
            "INSERT INTO integration_reports(project_id, integration_id, position, report_id) VALUES (?, 'int-old', 1, ?)",
            rusqlite::params![project, report],
        )
        .unwrap();
        old.execute(
            "UPDATE integrations SET state = 'merged', head_sha = ?, completed_at = '2026-01-01T00:00:01Z' WHERE integration_id = 'int-old'",
            ["c".repeat(40)],
        )
        .unwrap();
    }
    open_database(&file, &keep_all()).unwrap().close().unwrap();
    let conn = Connection::open(&file).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
    let violations: i64 = conn
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(violations, 0);
    // A stored plan body is never rewritten.
    let (stored, stored_sha): (String, String) = conn
        .query_row("SELECT body_json, body_sha FROM plan_revisions", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(stored, body);
    assert_eq!(stored_sha, checksum(stored.as_bytes()));
    let integration: (String, String, String, String) = conn
        .query_row(
            "SELECT integration_id, branch, state, head_sha FROM integrations",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        integration,
        (
            "int-old".into(),
            "capstan/integration/int-old".into(),
            "merged".into(),
            "c".repeat(40)
        )
    );
    assert_eq!(count(&conn, "integration_reports"), 1);
    let names: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE tbl_name = 'integrations' AND type IN ('trigger', 'index') AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        names,
        [
            "immutable_integrations_delete",
            "immutable_integrations_identity",
            "integrations_conflict_report_is_a_member",
            "integrations_move_forward",
            "one_running_integration"
        ]
    );
    let refused = |sql: &str| conn.execute(sql, []).unwrap_err().to_string();
    assert!(refused("DELETE FROM integrations").contains("immutable"));
    assert!(refused("UPDATE integrations SET branch = 'integration/x'").contains("immutable"));
    // A name that is not capstan/integration/<id> is allowed; the branch stays unique.
    let insert = |id: &str, sequence: i64, branch: &str| {
        conn.execute(
            "INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, created_at)
             SELECT project_id, ?, ?, ?, ?, 'operator', 'running', '2026-01-02T00:00:00Z' FROM projects",
            rusqlite::params![id, sequence, "a".repeat(40), branch],
        )
    };
    insert("int-new", 2, "integration/plan-1-x").unwrap();
    let duplicate = insert("int-dup", 3, "integration/plan-1-x")
        .unwrap_err()
        .to_string();
    assert!(
        duplicate.contains("UNIQUE") || duplicate.contains("constraint"),
        "{duplicate}"
    );
    conn.execute(
        "UPDATE integrations SET state = 'confirmed' WHERE integration_id = 'int-old'",
        [],
    )
    .unwrap();
}

#[test]
fn a_ledger_whose_writer_keeps_uncheckpointed_wal_frames_copies_and_opens_with_an_unchanged_header()
{
    let (_dir, file) = scratch();
    std::fs::copy(fixture(), &file).unwrap();
    open_database(&file, &OpenOptions::default())
        .unwrap()
        .close()
        .unwrap();
    // A writer that never checkpoints, as the live daemon can.
    let writer = Connection::open(&file).unwrap();
    writer.pragma_update(None, "journal_mode", "wal").unwrap();
    writer.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
    writer
        .execute_batch("CREATE TABLE IF NOT EXISTS scratch_wal(n INTEGER)")
        .unwrap();
    for i in 0..50 {
        writer
            .execute("INSERT INTO scratch_wal(n) VALUES (?)", [i])
            .unwrap();
    }
    let wal = PathBuf::from(format!("{}-wal", file.display()));
    assert!(
        std::fs::metadata(&wal).unwrap().len() > 0,
        "WAL frames are left behind"
    );

    // A consistent snapshot with the WAL folded in, from a read-only source connection.
    let (copy_dir, copy) = scratch();
    let source = open_database_read_only(&file).unwrap();
    source.backup(&copy).unwrap();
    source.close().unwrap();
    let before = header(&copy);
    let rows = dump(&copy).migrations;
    open_database(&copy, &OpenOptions::default())
        .unwrap()
        .close()
        .unwrap();
    let after = header(&copy);
    assert_eq!(
        after[16..28],
        before[16..28],
        "nothing is pending, so nothing changes"
    );
    assert_eq!(after[60..64], before[60..64]);
    assert_eq!(dump(&copy).migrations, rows);
    assert!(backups(copy_dir.path()).is_empty());
    let scratch_rows: i64 = Connection::open(&copy)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM scratch_wal", [], |r| r.get(0))
        .unwrap();
    assert_eq!(scratch_rows, 50);
    drop(writer);
}
