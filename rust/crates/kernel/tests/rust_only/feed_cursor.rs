//! The feed cursor (`areas::feed_cursor`): the ledger epoch, head and history anchor on real ledgers, and the cursor codec.

use crate::fixture::{initial, step, Fixture};
use capstan_kernel::areas::feed_cursor::{
    anchor, decode_cursor, encode_cursor, epoch, head, CursorEntry, CursorError,
};
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::{Core, SeededEnv};
use serde_json::json;
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn private_dir(path: &Path) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

fn open_seeded(state: &Path, seed: &str) -> Core {
    Core::open(
        state,
        &initial(),
        &KernelOptions::default(),
        Box::new(SeededEnv::new(seed)),
    )
    .expect("the ledger opens")
}

fn new_state() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    private_dir(dir.path());
    let state = dir.path().join("state");
    std::fs::create_dir(&state).unwrap();
    private_dir(&state);
    (dir, state)
}

/// Mutations (one event each) until the ledger head reaches `target`.
fn grow(core: &Core, target: i64) {
    while head(core).unwrap() < target {
        let n = head(core).unwrap();
        step(
            core,
            "createSeat",
            json!([{"seatId": format!("seat-{n}"), "name": format!("seat-{n}"), "role": "Developer"}]),
        );
    }
}

fn raw(state: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(state.join("controller.sqlite")).unwrap()
}

#[test]
fn a_bootstrapped_ledger_has_a_stable_epoch_of_its_own() {
    let mut f = Fixture::new();
    assert!(head(f.core()).unwrap() >= 1);
    let first = epoch(f.core()).unwrap();
    assert_eq!(first.len(), 16);
    f.close();
    f.reopen();
    assert_eq!(epoch(f.core()).unwrap(), first);
    let other = Fixture::new();
    assert_ne!(epoch(other.core()).unwrap(), first);
}

#[test]
fn reconcile_rows_on_restart_leave_the_epoch() {
    let mut f = Fixture::new();
    let before = epoch(f.core()).unwrap();
    let anchored = anchor(f.core(), 1).unwrap();
    f.tell_pm("hello");
    f.close();
    f.reopen();
    assert_eq!(epoch(f.core()).unwrap(), before);
    assert_eq!(anchor(f.core(), 1).unwrap(), anchored);
}

#[test]
fn a_ledger_without_sequence_one_has_the_no_genesis_epoch() {
    let mut f = Fixture::new();
    let genesis = epoch(f.core()).unwrap();
    let top = head(f.core()).unwrap();
    assert!(top >= 2);
    f.close();
    let (event_id, row) = {
        let db = raw(&f.state);
        let row: (String, String, String, String, i64, String, String, i64, String, String) = db
            .query_row(
                "SELECT event_id, entity_type, entity_id, to_state, state_version, actor_id, request_id, input_revision, payload_json, created_at
                 FROM controller_events WHERE sequence = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?)),
            )
            .unwrap();
        db.execute_batch("DROP TRIGGER immutable_controller_events_delete")
            .unwrap();
        db.execute("DELETE FROM controller_events WHERE sequence = 1", [])
            .unwrap();
        (row.0.clone(), row)
    };
    f.reopen();
    let none = epoch(f.core()).unwrap();
    assert_ne!(none, genesis);
    assert_eq!(head(f.core()).unwrap(), top);
    assert_eq!(anchor(f.core(), 1).unwrap(), None);
    f.close();
    {
        let db = raw(&f.state);
        let project: String = db
            .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
            .unwrap();
        db.execute(
            "INSERT INTO controller_events(project_id, sequence, event_id, entity_type, entity_id, from_state, to_state, state_version, actor_id, request_id, input_revision, payload_json, created_at)
             VALUES (?, 1, ?, ?, ?, NULL, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![project, event_id, row.1, row.2, row.3, row.4, row.5, row.6, row.7, row.8, row.9],
        )
        .unwrap();
    }
    f.reopen();
    assert_eq!(epoch(f.core()).unwrap(), genesis);
}

#[test]
fn an_empty_ledger_has_head_zero() {
    let mut f = Fixture::new();
    f.close();
    {
        let db = raw(&f.state);
        db.execute_batch("DROP TRIGGER immutable_controller_events_delete")
            .unwrap();
        db.execute("DELETE FROM controller_events", []).unwrap();
    }
    f.reopen();
    assert_eq!(head(f.core()).unwrap(), 0);
    assert_eq!(anchor(f.core(), 0).unwrap(), None);
    assert_eq!(anchor(f.core(), 1).unwrap(), None);
    assert_eq!(epoch(f.core()).unwrap().len(), 16);
}

#[test]
fn anchor_is_none_for_zero_and_a_missing_row() {
    let f = Fixture::new();
    let top = head(f.core()).unwrap();
    assert_eq!(anchor(f.core(), 0).unwrap(), None);
    assert_eq!(anchor(f.core(), -1).unwrap(), None);
    assert_eq!(anchor(f.core(), top + 1).unwrap(), None);
    assert_eq!(anchor(f.core(), top).unwrap().unwrap().len(), 16);
}

#[test]
fn a_restored_and_regrown_ledger_has_another_anchor() {
    let (_original_dir, original) = new_state();
    let core = open_seeded(&original, "run-a1");
    grow(&core, 50);
    core.close();
    // The copy taken at sequence 50 (the write-ahead log folded into the file first).
    raw(&original)
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .unwrap();
    let (_restored_dir, restored) = new_state();
    std::fs::copy(
        original.join("controller.sqlite"),
        restored.join("controller.sqlite"),
    )
    .unwrap();
    let core = open_seeded(&original, "run-a2");
    grow(&core, 100);
    let at_50 = anchor(&core, 50).unwrap();
    let at_100 = anchor(&core, 100).unwrap().unwrap();
    let genesis = epoch(&core).unwrap();
    core.close();

    let core = open_seeded(&restored, "run-b");
    assert_eq!(head(&core).unwrap(), 50);
    grow(&core, 110);
    assert_eq!(epoch(&core).unwrap(), genesis);
    assert_eq!(anchor(&core, 50).unwrap(), at_50);
    assert_ne!(anchor(&core, 100).unwrap().unwrap(), at_100);
    core.close();

    let core = open_seeded(&original, "run-a3");
    assert_eq!(anchor(&core, 100).unwrap().unwrap(), at_100);
    core.close();
}

fn entry(k: u32, d: Option<i64>) -> CursorEntry {
    CursorEntry {
        e: "0123456789abcdef".into(),
        s: 42,
        k,
        h: "fedcba9876543210".into(),
        r: "/work/a".into(),
        d,
    }
}

fn pack(json: &str) -> String {
    use base64::Engine;
    format!(
        "c1.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
    )
}

#[test]
fn cursors_round_trip() {
    let mut map = BTreeMap::new();
    map.insert("pb".to_string(), entry(3, Some(1_700_000_000)));
    map.insert("pa".to_string(), entry(0, None));
    let text = encode_cursor(&map);
    assert!(text.starts_with("c1."));
    assert!(!text.contains('='));
    assert_eq!(decode_cursor(&text).unwrap(), map);
    assert_eq!(
        decode_cursor(&encode_cursor(&BTreeMap::new())).unwrap(),
        BTreeMap::new()
    );
}

#[test]
fn a_cursor_is_the_sorted_key_bytes() {
    let mut map = BTreeMap::new();
    map.insert("pb".to_string(), entry(3, Some(1_700_000_000)));
    map.insert("pa".to_string(), entry(0, None));
    let json = r#"{"p":{"pa":{"e":"0123456789abcdef","h":"fedcba9876543210","r":"/work/a","s":42},"pb":{"d":1700000000,"e":"0123456789abcdef","h":"fedcba9876543210","k":3,"r":"/work/a","s":42}}}"#;
    let golden = pack(json);
    assert_eq!(encode_cursor(&map), golden);
    assert_eq!(decode_cursor(&golden).unwrap(), map);
}

#[test]
fn bad_cursors_are_refused() {
    let ok = r#"{"p":{"pa":{"e":"0123456789abcdef","h":"fedcba9876543210","r":"/w","s":1}}}"#;
    assert!(decode_cursor(&pack(ok)).is_ok());
    assert_eq!(
        decode_cursor(&pack(ok).replace("c1.", "c2.")),
        Err(CursorError::Prefix)
    );
    assert_eq!(decode_cursor(""), Err(CursorError::Prefix));
    assert_eq!(decode_cursor("c1.@@@"), Err(CursorError::Base64));
    assert_eq!(
        decode_cursor(&format!("{}=", pack(ok))),
        Err(CursorError::Base64)
    );
    assert_eq!(decode_cursor(&pack("{not json")), Err(CursorError::Json));
    for bad in [
        r#"[]"#,
        r#"{"p":[]}"#,
        r#"{"p":{"pa":1}}"#,
        r#"{"p":{"pa":{"e":"xyz","h":"fedcba9876543210","r":"/w","s":1}}}"#,
        r#"{"p":{"pa":{"e":"0123456789ABCDEF","h":"fedcba9876543210","r":"/w","s":1}}}"#,
        r#"{"p":{"pa":{"e":"0123456789abcdef","h":"short","r":"/w","s":1}}}"#,
        r#"{"p":{"pa":{"e":"0123456789abcdef","h":"fedcba9876543210","s":1}}}"#,
        r#"{"p":{"pa":{"e":"0123456789abcdef","h":"fedcba9876543210","r":"/w","s":-1}}}"#,
        r#"{"p":{"pa":{"e":"0123456789abcdef","h":"fedcba9876543210","r":"/w","s":"1"}}}"#,
        r#"{"p":{"pa":{"e":"0123456789abcdef","h":"fedcba9876543210","r":"/w","s":1,"k":-1}}}"#,
        r#"{"p":{"pa":{"e":"0123456789abcdef","h":"fedcba9876543210","r":"/w","s":1,"k":4294967296}}}"#,
        r#"{"p":{"pa":{"e":"0123456789abcdef","h":"fedcba9876543210","r":"/w","s":1,"d":"x"}}}"#,
        r#"{"p":{"pa":{"e":"0123456789abcdef","h":"fedcba9876543210","r":"/w","s":1,"z":1}}}"#,
    ] {
        assert!(
            matches!(decode_cursor(&pack(bad)), Err(CursorError::Field(_))),
            "{bad}"
        );
    }
}
