//! Migration 0040 (message classes): a ledger at schema 39 with a PM and messages in every state migrates with every row
//! intact and every message an action message; an info message never needs an action.
mod common;

use capstan_ledger::{max_embedded_migration, open_database, OpenOptions};
use common::scratch;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/ledger/schema-39-messages.sqlite")
}

fn rows(file: &Path, columns: &str) -> Vec<Vec<String>> {
    let conn = Connection::open(file).unwrap();
    let mut statement = conn
        .prepare(&format!("SELECT {columns} FROM messages ORDER BY sequence"))
        .unwrap();
    let count = statement.column_count();
    statement
        .query_map([], |row| {
            (0..count)
                .map(|i| {
                    row.get::<_, rusqlite::types::Value>(i)
                        .map(|v| format!("{v:?}"))
                })
                .collect()
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

const OLD_COLUMNS: &str =
    "message_id, sequence, state, state_version, action_needed, body, recipient_agent_id";

#[test]
fn migration_0040_gives_every_existing_message_class_action_and_keeps_states_and_versions() {
    let (_dir, file) = scratch();
    std::fs::copy(fixture(), &file).unwrap();
    let before = rows(&file, OLD_COLUMNS);
    let states: Vec<String> = rows(&file, "state")
        .into_iter()
        .map(|r| r[0].clone())
        .collect();
    for state in [
        "queued",
        "deferred",
        "sent",
        "acked",
        "unacked",
        "acked_late",
        "expired",
        "failed",
        "cancelled",
    ] {
        assert!(
            states.contains(&format!("Text({state:?})")),
            "the fixture lacks a {state} message"
        );
    }
    let conn = Connection::open(&file).unwrap();
    let version: i64 = conn
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
            r.get(0)
        })
        .unwrap();
    let has_class = conn.prepare("SELECT class FROM messages").is_ok();
    drop(conn);
    assert!(!has_class, "the fixture is already past the migration");
    assert!(version < max_embedded_migration());

    open_database(
        &file,
        &OpenOptions {
            keep_migration_backups: 100,
            ..OpenOptions::default()
        },
    )
    .unwrap()
    .close()
    .unwrap();

    assert_eq!(rows(&file, OLD_COLUMNS), before);
    let classes = rows(&file, "class");
    assert_eq!(classes.len(), before.len());
    assert!(
        classes.iter().all(|c| c[0] == "Text(\"action\")"),
        "{classes:?}"
    );
    let conn = Connection::open(&file).unwrap();
    let latest: i64 = conn
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(latest, max_embedded_migration());
}

#[test]
fn an_info_message_that_needs_an_action_is_refused_by_the_ledger() {
    let (_dir, file) = scratch();
    std::fs::copy(fixture(), &file).unwrap();
    open_database(&file, &OpenOptions::default())
        .unwrap()
        .close()
        .unwrap();
    let conn = Connection::open(&file).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    let copy = |class: &str, action_needed: i64, id: &str| {
        conn.execute(
            "INSERT INTO messages(project_id, message_id, sequence, recipient_agent_id, recipient_generation, sender_actor_id,
               body, body_hash, state, state_version, queued_at, deferral_count, send_attempts, created_at, updated_at, action_needed, class)
             SELECT project_id, ?, 1000 + ?, recipient_agent_id, recipient_generation, sender_actor_id,
               body, body_hash, 'queued', 0, queued_at, 0, 0, created_at, updated_at, ?, ? FROM messages LIMIT 1",
            rusqlite::params![id, action_needed + id.len() as i64 * 10, action_needed, class],
        )
    };
    assert!(copy("info", 0, "i-ok").is_ok());
    assert!(copy("action", 1, "a-ok").is_ok());
    let refused = copy("info", 1, "i-bad").unwrap_err().to_string();
    assert!(
        refused.contains("an info message cannot need an action"),
        "{refused}"
    );
    let bad_class = copy("loud", 0, "l-bad").unwrap_err().to_string();
    assert!(bad_class.contains("CHECK"), "{bad_class}");
    // Updating an info message into one that needs an action is refused as well; so is the other way round.
    let update = conn
        .execute(
            "UPDATE messages SET action_needed = 1 WHERE message_id = 'i-ok'",
            [],
        )
        .unwrap_err()
        .to_string();
    assert!(
        update.contains("an info message cannot need an action"),
        "{update}"
    );
    let update = conn
        .execute(
            "UPDATE messages SET class = 'info' WHERE message_id = 'a-ok'",
            [],
        )
        .unwrap_err()
        .to_string();
    assert!(
        update.contains("an info message cannot need an action"),
        "{update}"
    );
}
