//! A ledger that lacks migration 0030 migrates, keeps its data and gets the PM's `prompt:relay` grant
//! (test/prompt-relay-ledger.test.ts). The ledger is rolled back the way the Node test rolled it back: raw SQL on the closed
//! file that undoes everything from version 30 on.

use crate::fixture::Fixture;

const ROLL_BACK: &str = "
  ALTER TABLE agent_panes DROP COLUMN terminal_id;
  ALTER TABLE orphan_panes DROP COLUMN terminal_id;
  ALTER TABLE agent_panes DROP COLUMN task_ref;
  ALTER TABLE agent_panes DROP COLUMN task_title;
  ALTER TABLE messages DROP COLUMN action_needed;
  DROP TABLE pauses;
  DELETE FROM transition_rules WHERE role = 'PM' AND entity_type = 'run_control';
  DELETE FROM capability_grants WHERE capability = 'run:control' AND actor_id IN (SELECT actor_id FROM actors WHERE role = 'PM');
  DELETE FROM role_capabilities WHERE role = 'PM' AND capability = 'run:control';
  DROP INDEX actors_by_seat_active;
  DROP INDEX assignments_by_seat_authority;
  DELETE FROM schema_migrations WHERE version >= 30;
  DROP TABLE prompt_relays;
  DELETE FROM role_capabilities WHERE capability = 'prompt:relay';
  DELETE FROM capability_grants WHERE capability = 'prompt:relay';
";

fn count(database: &rusqlite::Connection, sql: &str) -> i64 {
    database.query_row(sql, [], |row| row.get(0)).unwrap()
}

#[test]
fn an_existing_ledger_that_lacks_0030_migrates_keeps_its_data_and_backfills_the_pm_grant() {
    let mut f = Fixture::new();
    f.tell_pm("a message that must survive the migration");
    f.close();
    {
        let database = rusqlite::Connection::open(f.database()).unwrap();
        database.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
        database.execute_batch(ROLL_BACK).unwrap();
        assert_eq!(
            count(&database, "SELECT max(version) FROM schema_migrations"),
            29
        );
        assert_eq!(
            count(
                &database,
                "SELECT count(*) FROM capability_grants WHERE capability = 'prompt:relay'"
            ),
            0
        );
    }
    // Opening the ledger runs 0030 and the migrations after it.
    f.reopen();
    f.close();
    let database = rusqlite::Connection::open(f.database()).unwrap();
    let versions: Vec<i64> = database
        .prepare("SELECT version FROM schema_migrations ORDER BY version")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        versions,
        (1..=37).collect::<Vec<i64>>(),
        "every migration is recorded again"
    );
    let grants: Vec<String> = database
        .prepare("SELECT actor_id FROM capability_grants WHERE capability = 'prompt:relay' AND revoked_at IS NULL")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        grants,
        [f.pm_actor.clone()],
        "only the active PM actor is granted, and only once"
    );
    assert_eq!(count(&database, "SELECT count(*) FROM role_capabilities WHERE capability = 'prompt:relay' AND role = 'PM'"), 1);
    assert_eq!(
        count(&database, "SELECT count(*) FROM prompt_relays"),
        0,
        "the table exists and is empty"
    );
    // The data that was there is still there.
    assert_eq!(
        count(
            &database,
            "SELECT count(*) FROM messages WHERE body LIKE 'a message that must survive%'"
        ),
        1
    );
    assert_eq!(
        count(
            &database,
            "SELECT count(*) FROM agents WHERE state = 'active'"
        ),
        2
    );
    // The PM can use the grant it was given: the developer cannot.
    f.reopen();
    let core = f.core();
    core.configure_prompt_relay(&serde_json::json!({"enabled": true, "captureTtlSeconds": 600}))
        .unwrap();
    assert!(core.prompt_relay_enabled().unwrap());
    let _ = &f.developer_credential;
}
