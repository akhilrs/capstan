//! Settling an integration: a conflicted one can be discarded (it holds no branch), a merged one still can, and one
//! that is running or conflicted-and-confirmed cannot.

use crate::fixture::{context, Fixture, OWNER};
use serde_json::json;

fn insert(f: &Fixture, id: &str, sequence: i64, state: &str) {
    let db = rusqlite::Connection::open(f.database()).unwrap();
    let project: String = db
        .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
        .unwrap();
    let head = if state == "merged" {
        Some("3".repeat(40))
    } else {
        None
    };
    let done = (state != "running").then_some("2026-01-01T00:00:00.000Z");
    let conflict = (state == "conflicted").then_some(("r-x", "[\"a.txt\"]", 0));
    db.execute(
        "INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, head_sha, created_at, completed_at,
                                  conflict_report_id, conflict_files_json, conflict_files_omitted)
         VALUES (?, ?, ?, ?, ?, 'pm-1', ?, ?, '2026-01-01T00:00:00.000Z', ?, ?, ?, ?)",
        rusqlite::params![
            project, id, sequence, "4".repeat(40), format!("capstan/integration/{id}"), state, head, done,
            conflict.map(|c| c.0), conflict.map(|c| c.1), conflict.map(|c| c.2),
        ],
    )
    .unwrap();
}

fn settle(f: &Fixture, id: &str, outcome: &str) -> Result<serde_json::Value, String> {
    let core = f.core();
    core.settle_integration(
        &context(core, OWNER),
        &json!({"integrationId": id, "outcome": outcome}),
    )
    .map_err(|e| e.message().to_string())
}

#[test]
fn a_conflicted_integration_can_be_discarded_but_not_confirmed() {
    let f = Fixture::new();
    insert(&f, "int-conflicted", 1, "conflicted");
    assert!(settle(&f, "int-conflicted", "confirmed")
        .unwrap_err()
        .contains("only a merged integration"));
    let record = settle(&f, "int-conflicted", "discarded").unwrap();
    assert_eq!(record["state"], "discarded");
    // Settled once: a second discard is refused.
    assert!(settle(&f, "int-conflicted", "discarded").is_err());
}

#[test]
fn a_merged_integration_still_settles_and_a_running_one_is_refused() {
    let f = Fixture::new();
    insert(&f, "int-merged", 1, "merged");
    insert(&f, "int-running", 2, "running");
    assert_eq!(
        settle(&f, "int-merged", "discarded").unwrap()["state"],
        "discarded"
    );
    assert!(settle(&f, "int-running", "discarded")
        .unwrap_err()
        .contains("only a merged integration"));
}
