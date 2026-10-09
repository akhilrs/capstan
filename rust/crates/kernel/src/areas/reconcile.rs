//! Reconciliation of assignments a restart left uncertain (src/controller/reconcile.ts).

use crate::canonical::canonical_json;
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{execute, query_all, query_opt};
use crate::kernel::Kernel;
use serde_json::json;

struct Uncertain {
    command_id: String,
    command_state: String,
    start_requested: i64,
    assignment_id: String,
    work_item_id: String,
    assignment_state: String,
    attempt: i64,
    attempt_state: String,
    attempt_version: i64,
    work_state: String,
}

/// `reconcileUncertainAssignments`: returns how many assignments it reconciled.
pub fn reconcile_uncertain_assignments(kernel: &Kernel) -> KernelResult<usize> {
    kernel.database.exec("BEGIN IMMEDIATE")?;
    match reconcile_rows(kernel) {
        Ok(reconciled) => {
            kernel.database.exec("COMMIT")?;
            Ok(reconciled)
        }
        Err(error) => {
            let _ = kernel.database.exec("ROLLBACK");
            Err(error)
        }
    }
}

fn reconcile_rows(kernel: &Kernel) -> KernelResult<usize> {
    let rows = query_all(
        &kernel.database,
        "SELECT c.command_id, c.state AS command_state, c.start_requested,
           a.assignment_id, a.work_item_id, a.state AS assignment_state,
           at.attempt, at.state AS attempt_state, at.state_version AS attempt_version,
           w.state AS work_state
         FROM commands c
         JOIN assignments a ON a.project_id = c.project_id AND a.assignment_id = c.assignment_id
         JOIN assignment_attempts at ON at.project_id = c.project_id
           AND at.assignment_id = c.assignment_id AND at.attempt = c.attempt
         JOIN work_items w ON w.project_id = a.project_id AND w.work_item_id = a.work_item_id
         WHERE c.project_id = ? AND c.state IN ('queued', 'attempting', 'acknowledged', 'started', 'completed')
           AND a.authority_state = 'active'
           AND at.state IN ('created', 'dispatched', 'acknowledged', 'running', 'reported')
         ORDER BY c.command_id",
        [&kernel.project_id],
        |row| {
            Ok(Uncertain {
                command_id: row.get("command_id")?,
                command_state: row.get("command_state")?,
                start_requested: row.get("start_requested")?,
                assignment_id: row.get("assignment_id")?,
                work_item_id: row.get("work_item_id")?,
                assignment_state: row.get("assignment_state")?,
                attempt: row.get("attempt")?,
                attempt_state: row.get("attempt_state")?,
                attempt_version: row.get("attempt_version")?,
                work_state: row.get("work_state")?,
            })
        },
    )?;
    let controller = kernel.internal_principal()?;
    let mut reconciled = 0;
    for row in rows {
        reconciled += 1;
        let preserve_reported_completion = row.command_state == "completed"
            && row.assignment_state == "reported"
            && row.attempt_state == "reported";
        if row.command_state != "completed"
            && !kernel.is_transition_allowed(
                "command",
                &row.command_state,
                "unknown",
                &controller,
            )?
        {
            return Err(KernelError::TransitionAuthorization(format!(
                "transition table rejects restart reconciliation of {} command",
                row.command_state
            )));
        }
        if !preserve_reported_completion
            && !kernel.is_transition_allowed(
                "assignment_attempt",
                &row.attempt_state,
                "revoked",
                &controller,
            )?
        {
            return Err(KernelError::TransitionAuthorization(format!(
                "transition table rejects restart reconciliation of {} attempt",
                row.attempt_state
            )));
        }
        let blocks_work = row.work_state == "running" || row.work_state == "awaiting_verification";
        if blocks_work
            && !kernel.is_transition_allowed(
                "work_item",
                &row.work_state,
                "blocked",
                &controller,
            )?
        {
            return Err(KernelError::TransitionAuthorization(format!(
                "transition table rejects restart blocking of {} work",
                row.work_state
            )));
        }
        let now = kernel.now();
        if row.command_state != "completed" {
            execute(
                &kernel.database,
                "UPDATE commands SET state = 'unknown', state_version = state_version + 1, updated_at = ? WHERE project_id = ? AND command_id = ?",
                [&now, &kernel.project_id, &row.command_id],
            )?;
            append_outbox_outcome(
                kernel,
                &row.command_id,
                "unknown",
                &canonical_json(&json!({
                    "reason": "controller restarted before worker authority was contained"
                })),
                &now,
            )?;
        }
        if preserve_reported_completion {
            execute(
                &kernel.database,
                "UPDATE assignments SET authority_state = 'unknown', state_version = state_version + 1 WHERE project_id = ? AND assignment_id = ?",
                [&kernel.project_id, &row.assignment_id],
            )?;
            execute(
                &kernel.database,
                "UPDATE assignment_attempts SET authority_state = 'unknown', state_version = state_version + 1 WHERE project_id = ? AND assignment_id = ? AND attempt = ?",
                rusqlite::params![kernel.project_id, row.assignment_id, row.attempt],
            )?;
        } else {
            execute(
                &kernel.database,
                "UPDATE assignments SET state = 'revoked', state_version = state_version + 1,
                   authority_state = 'unknown', ended_at = COALESCE(ended_at, ?)
                 WHERE project_id = ? AND assignment_id = ?",
                [&now, &kernel.project_id, &row.assignment_id],
            )?;
            execute(
                &kernel.database,
                "UPDATE assignment_attempts SET state = 'revoked', state_version = state_version + 1,
                   authority_state = 'unknown', ended_at = COALESCE(ended_at, ?)
                 WHERE project_id = ? AND assignment_id = ? AND attempt = ?",
                rusqlite::params![now, kernel.project_id, row.assignment_id, row.attempt],
            )?;
        }
        if blocks_work {
            execute(
                &kernel.database,
                "UPDATE work_items SET state = 'blocked', state_version = state_version + 1 WHERE project_id = ? AND work_item_id = ?",
                [&kernel.project_id, &row.work_item_id],
            )?;
        }
        let project_version = query_opt(
            &kernel.database,
            "SELECT state_version FROM projects WHERE project_id = ?",
            [&kernel.project_id],
            |r| r.get::<_, i64>(0),
        )?
        .unwrap_or(0)
            + 1;
        let sequence = query_opt(
            &kernel.database,
            "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM controller_events WHERE project_id = ?",
            [&kernel.project_id],
            |r| r.get::<_, i64>(0),
        )?
        .unwrap_or(1);
        execute(
            &kernel.database,
            "UPDATE projects SET state_version = ? WHERE project_id = ?",
            rusqlite::params![project_version, kernel.project_id],
        )?;
        execute(
            &kernel.database,
            "INSERT INTO controller_events(project_id, sequence, event_id, entity_type, entity_id, from_state, to_state,
               state_version, actor_id, request_id, input_revision, payload_json, created_at)
             SELECT ?, ?, ?, 'assignment_attempt', ?, ?, ?, ?, ?, ?, current_input_revision, ?, ?
             FROM projects WHERE project_id = ?",
            rusqlite::params![
                kernel.project_id,
                sequence,
                kernel.env.uuid(),
                row.assignment_id,
                row.attempt_state,
                if preserve_reported_completion { "reported" } else { "revoked" },
                row.attempt_version + 1,
                kernel.internal_actor_id,
                format!("restart-reconcile:{}", row.command_id),
                canonical_json(&json!({
                    "commandState": row.command_state,
                    "authorityState": "unknown",
                    "preservedReport": preserve_reported_completion,
                    "startRequested": row.start_requested == 1,
                })),
                now,
                kernel.project_id
            ],
        )?;
    }
    Ok(reconciled)
}

fn append_outbox_outcome(
    kernel: &Kernel,
    command_id: &str,
    outcome: &str,
    response_json: &str,
    now: &str,
) -> KernelResult<()> {
    let ordinal = query_opt(
        &kernel.database,
        "SELECT COALESCE(MAX(ordinal), 0) + 1 AS next FROM outbox_delivery_attempts WHERE project_id = ? AND command_id = ?",
        [&kernel.project_id, command_id],
        |r| r.get::<_, i64>(0),
    )?
    .unwrap_or(1);
    execute(
        &kernel.database,
        "INSERT INTO outbox_delivery_attempts(project_id, command_id, ordinal, outcome, started_at, completed_at, response_json)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        rusqlite::params![kernel.project_id, command_id, ordinal, outcome, now, now, response_json],
    )?;
    Ok(())
}

/// Reconciliation has no `ControllerCore` operation of its own, so nothing is dispatched by name.
pub(crate) fn dispatch(
    _kernel: &Kernel,
    _op: &str,
    _args: &[serde_json::Value],
) -> Option<KernelResult<serde_json::Value>> {
    None
}

#[cfg(test)]
mod tests {
    use crate::areas::status::legacy_rows::*;
    use serde_json::json;

    /// test/controller-project.test.ts "opening the controller reconciles an active assignment with an in-flight
    /// command fail-closed".
    #[test]
    fn opening_the_controller_reconciles_an_active_assignment_with_an_in_flight_command_fail_closed(
    ) {
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(
            directory.path(),
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .unwrap();
        let info = project("preconcilerows", "owner-reconcile-credential-0123456789");
        let id = info.project_id.clone();
        let kernel = open_kernel(directory.path(), &info);
        let (developer_seat, developer_actor) = add_seat_and_actor(
            &kernel,
            &info.owner_credential,
            "Developer",
            "open-reconcile",
        );
        let (reporter_seat, reporter_actor) = add_seat_and_actor(
            &kernel,
            &info.owner_credential,
            "Developer",
            "open-reported",
        );
        kernel.close();
        drop(kernel);
        let (version_before, events_before) = {
            let db = open_ledger(directory.path());
            insert_work_item(
                &db,
                &id,
                "in-flight-work",
                None,
                "running",
                CREATED_AT,
                None,
            );
            insert_assignment(
                &db,
                &id,
                "in-flight-assignment",
                "in-flight-work",
                &developer_seat,
                &developer_actor,
                "running",
                "active",
                Some(("in-flight-command", "attempting", true)),
            );
            insert_work_item(
                &db,
                &id,
                "reported-work",
                None,
                "awaiting_verification",
                CREATED_AT,
                None,
            );
            insert_assignment(
                &db,
                &id,
                "reported-assignment",
                "reported-work",
                &reporter_seat,
                &reporter_actor,
                "reported",
                "active",
                Some(("reported-command", "completed", false)),
            );
            let version: i64 = db
                .query_row(
                    "SELECT state_version FROM projects WHERE project_id = ?",
                    [&id],
                    |r| r.get(0),
                )
                .unwrap();
            let events: i64 = db
                .query_row(
                    "SELECT COUNT(*) FROM controller_events WHERE project_id = ?",
                    [&id],
                    |r| r.get(0),
                )
                .unwrap();
            (version, events)
        };
        // Opening reconciles; the second open finds nothing left to do.
        open_kernel(directory.path(), &info).close();
        let db = open_ledger(directory.path());
        let one = |sql: &str, arg: &str| -> serde_json::Value {
            db.query_row(sql, [arg], |row| {
                Ok(json!([
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?
                ]))
            })
            .unwrap()
        };
        assert_eq!(
            one(
                "SELECT state, 'x', state_version FROM commands WHERE command_id = ?",
                "in-flight-command"
            ),
            json!(["unknown", "x", 2])
        );
        assert_eq!(
            one(
                "SELECT state, 'x', state_version FROM commands WHERE command_id = ?",
                "reported-command"
            ),
            json!(["completed", "x", 1])
        );
        let outbox: Vec<(String, String)> = db
            .prepare("SELECT command_id, outcome FROM outbox_delivery_attempts ORDER BY command_id, ordinal")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            outbox,
            [("in-flight-command".to_string(), "unknown".to_string())]
        );
        assert_eq!(
            one("SELECT state, authority_state, state_version FROM assignments WHERE assignment_id = ?", "in-flight-assignment"),
            json!(["revoked", "unknown", 2])
        );
        let ended: i64 = db
            .query_row("SELECT ended_at IS NOT NULL FROM assignments WHERE assignment_id = 'in-flight-assignment'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ended, 1);
        assert_eq!(
            one("SELECT state, authority_state, state_version FROM assignment_attempts WHERE assignment_id = ?", "in-flight-assignment"),
            json!(["revoked", "unknown", 2])
        );
        assert_eq!(
            one("SELECT state, authority_state, state_version FROM assignments WHERE assignment_id = ?", "reported-assignment"),
            json!(["reported", "unknown", 2])
        );
        assert_eq!(
            one("SELECT state, authority_state, state_version FROM assignment_attempts WHERE assignment_id = ?", "reported-assignment"),
            json!(["reported", "unknown", 2])
        );
        let works: Vec<(String, String, i64)> = db
            .prepare(
                "SELECT work_item_id, state, state_version FROM work_items ORDER BY work_item_id",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            works,
            [
                ("in-flight-work".to_string(), "blocked".to_string(), 2),
                ("reported-work".to_string(), "blocked".to_string(), 2)
            ]
        );
        let events: Vec<serde_json::Value> = db
            .prepare(
                "SELECT entity_type, entity_id, from_state, to_state, request_id, payload_json
                 FROM controller_events WHERE request_id LIKE 'restart-reconcile:%' ORDER BY request_id",
            )
            .unwrap()
            .query_map([], |row| {
                Ok(json!([
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    serde_json::from_str::<serde_json::Value>(&row.get::<_, String>(5)?).unwrap(),
                ]))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            events,
            [
                json!(["assignment_attempt", "in-flight-assignment", "running", "revoked", "restart-reconcile:in-flight-command",
                    {"commandState": "attempting", "authorityState": "unknown", "preservedReport": false, "startRequested": true}]),
                json!(["assignment_attempt", "reported-assignment", "reported", "reported", "restart-reconcile:reported-command",
                    {"commandState": "completed", "authorityState": "unknown", "preservedReport": true, "startRequested": false}]),
            ]
        );
        let total: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM controller_events WHERE project_id = ?",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        let version: i64 = db
            .query_row(
                "SELECT state_version FROM projects WHERE project_id = ?",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(total, events_before + 2);
        assert_eq!(version, version_before + 2);
        drop(db);
        let again = open_kernel(directory.path(), &info);
        assert_eq!(again.state_version().unwrap(), version);
        again.close();
    }
}
