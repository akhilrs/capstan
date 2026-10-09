//! The status (src/controller/status.ts).

use crate::dispatch::{arg, call, Args};
use crate::errors::{KernelError, KernelResult};
use crate::helpers::{exists, query_all, query_opt};
use crate::kernel::Kernel;
use crate::plan_body::{number_value, package_of_body};
use crate::records::PlanRow;
use rusqlite::types::ValueRef;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

use super::{pauses, plan_packages};

struct ReadyItem {
    state: String,
    input_revision: i64,
    required_role: String,
    current_input_revision: i64,
    run_state: String,
}

struct Correction {
    target_assignment_id: String,
    target_generation: i64,
    target_seat_id: String,
    target_role: String,
    state: String,
    intervention_count: i64,
    authority_state: String,
    active_generation: i64,
    verifier_child: i64,
}

/// `readiness`: whether a work item can be dispatched now, and every reason it cannot.
pub fn readiness(kernel: &Kernel, work_item_id: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    let item = query_opt(
        &kernel.database,
        "SELECT w.state, w.input_revision, w.required_role, p.current_input_revision, r.state AS run_state
         FROM work_items w JOIN projects p ON p.project_id = w.project_id
         JOIN run_controls r ON r.project_id = w.project_id
         WHERE w.project_id = ? AND w.work_item_id = ?",
        [&kernel.project_id, work_item_id],
        |row| {
            Ok(ReadyItem {
                state: row.get(0)?,
                input_revision: row.get(1)?,
                required_role: row.get(2)?,
                current_input_revision: row.get(3)?,
                run_state: row.get(4)?,
            })
        },
    )?;
    let Some(item) = item else {
        return Err(KernelError::controller("work item does not exist"));
    };
    let mut reasons: Vec<String> = Vec::new();
    if !matches!(item.state.as_str(), "pending" | "blocked" | "ready") {
        reasons.push(format!("work item state is {}", item.state));
    }
    if item.input_revision != item.current_input_revision {
        reasons.push("work item input revision is stale".into());
    }
    if item.run_state != "active" {
        reasons.push(format!("run is {}", item.run_state));
    }
    let supervision = query_opt(
        &kernel.database,
        "SELECT enabled, health, target_epoch, checkpoint_epoch, bootstrap_pm_allowed
         FROM supervision_control WHERE project_id = ?",
        [&kernel.project_id],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, i64>(4)?,
            ))
        },
    )?;
    if let Some((enabled, health, target_epoch, checkpoint_epoch, bootstrap_pm_allowed)) =
        supervision
    {
        if enabled != 0 && item.required_role != "Supervisor" {
            let bootstrap_pm = item.required_role == "PM"
                && checkpoint_epoch.is_none()
                && bootstrap_pm_allowed == 1
                && !exists(
                    &kernel.database,
                    "SELECT 1 FROM assignments a JOIN seats s
                      ON s.project_id = a.project_id AND s.seat_id = a.seat_id
                     WHERE a.project_id = ? AND s.role = 'PM'
                       AND a.authority_state IN ('active', 'unknown') LIMIT 1",
                    [&kernel.project_id],
                )?;
            let correction = query_opt(
                &kernel.database,
                "SELECT c.finding_id, c.target_assignment_id, c.target_generation,
                  c.target_seat_id, c.target_role, f.state, f.intervention_count,
                  a.authority_state, a.active_generation, 0 AS verifier_child
                 FROM finding_correction_work c
                 JOIN findings f ON f.project_id = c.project_id AND f.finding_id = c.finding_id
                 JOIN assignments a ON a.project_id = c.project_id
                   AND a.assignment_id = c.target_assignment_id
                 WHERE c.project_id = ? AND c.work_item_id = ?
                 UNION ALL
                 SELECT c.finding_id, c.target_assignment_id, c.target_generation,
                  c.target_seat_id, c.target_role, f.state, f.intervention_count,
                  a.authority_state, a.active_generation, 1 AS verifier_child
                 FROM work_items child
                 JOIN finding_correction_work c ON c.project_id = child.project_id
                   AND c.work_item_id = child.parent_work_item_id
                 JOIN findings f ON f.project_id = c.project_id AND f.finding_id = c.finding_id
                 JOIN assignments a ON a.project_id = c.project_id
                   AND a.assignment_id = c.target_assignment_id
                 JOIN work_items correction_parent ON correction_parent.project_id = c.project_id
                   AND correction_parent.work_item_id = c.work_item_id
                   AND correction_parent.state = 'awaiting_verification'
                 JOIN assignments developer ON developer.project_id = c.project_id
                   AND developer.work_item_id = c.work_item_id
                 JOIN candidates candidate ON candidate.project_id = developer.project_id
                   AND candidate.assignment_id = developer.assignment_id
                   AND candidate.generation = developer.active_generation
                   AND candidate.input_revision = correction_parent.input_revision
                 WHERE child.project_id = ? AND child.work_item_id = ?
                   AND child.required_role = 'Verifier' AND c.target_role = 'Developer'
                   AND f.state = 'correcting'
                   AND child.acceptance_criteria_json IS NOT NULL
                 LIMIT 1",
                rusqlite::params![
                    kernel.project_id,
                    work_item_id,
                    kernel.project_id,
                    work_item_id
                ],
                |row| {
                    Ok(Correction {
                        target_assignment_id: row.get(1)?,
                        target_generation: row.get(2)?,
                        target_seat_id: row.get(3)?,
                        target_role: row.get(4)?,
                        state: row.get(5)?,
                        intervention_count: row.get(6)?,
                        authority_state: row.get(7)?,
                        active_generation: row.get(8)?,
                        verifier_child: row.get(9)?,
                    })
                },
            )?;
            if !bootstrap_pm {
                if health != "healthy" || checkpoint_epoch != Some(target_epoch) {
                    reasons.push("supervision is degraded or its checkpoint is stale".into());
                }
                let open_finding = exists(
                    &kernel.database,
                    "SELECT 1 FROM findings WHERE project_id = ? AND state <> 'resolved' LIMIT 1",
                    [&kernel.project_id],
                )?;
                if open_finding && correction.is_none() {
                    reasons.push("an open finding blocks ordinary dispatch".into());
                }
                if let Some(correction) = &correction {
                    let occupied = exists(
                        &kernel.database,
                        "SELECT 1 FROM assignments WHERE project_id = ? AND seat_id = ?
                         AND assignment_id <> ? AND authority_state IN ('active', 'unknown') LIMIT 1",
                        [
                            &kernel.project_id,
                            &correction.target_seat_id,
                            &correction.target_assignment_id,
                        ],
                    )?;
                    let role_mismatch = if correction.verifier_child == 1 {
                        item.required_role != "Verifier"
                            || correction.target_role != "Developer"
                            || correction.state != "correcting"
                    } else {
                        correction.target_role != item.required_role
                    };
                    if role_mismatch
                        || correction.authority_state != "contained"
                        || correction.active_generation != correction.target_generation
                        || correction.intervention_count > 2
                        || !["reported", "acknowledged", "correcting"]
                            .contains(&correction.state.as_str())
                        || occupied
                    {
                        reasons.push("finding-bound correction is not ready for dispatch".into());
                    }
                }
            }
        }
    }
    let seats = exists(
        &kernel.database,
        "SELECT 1 AS present FROM seats s
         WHERE s.project_id = ? AND s.role = ? AND s.state = 'active'
           AND EXISTS (
             SELECT 1 FROM actors a
             WHERE a.project_id = s.project_id AND a.seat_id = s.seat_id
               AND a.role = s.role AND a.active = 1 AND a.revoked_at IS NULL
           )
         LIMIT 1",
        [&kernel.project_id, &item.required_role],
    )?;
    if !seats {
        reasons.push(format!(
            "no active {} seat with an active actor",
            item.required_role
        ));
    }
    let active_assignment = exists(
        &kernel.database,
        "SELECT 1 AS present FROM assignments WHERE project_id = ? AND work_item_id = ?
           AND authority_state IN ('active', 'unknown') LIMIT 1",
        [&kernel.project_id, work_item_id],
    )?;
    if active_assignment {
        reasons.push("work item has active or unknown assignment authority".into());
    }
    type Dependency = (
        String,
        Option<String>,
        String,
        Option<String>,
        i64,
        Option<i64>,
    );
    let dependencies = query_all(
        &kernel.database,
        "SELECT d.depends_on_work_item_id, d.required_candidate_id, w.state, w.accepted_candidate_id,
           w.input_revision AS accepted_work_input_revision,
           c.input_revision AS candidate_input_revision
         FROM dependency_edges d JOIN work_items w
           ON w.project_id = d.project_id AND w.work_item_id = d.depends_on_work_item_id
         LEFT JOIN candidates c ON c.project_id = w.project_id AND c.candidate_id = w.accepted_candidate_id
         WHERE d.project_id = ? AND d.work_item_id = ? ORDER BY d.depends_on_work_item_id",
        [&kernel.project_id, work_item_id],
        |row| -> rusqlite::Result<Dependency> {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        },
    )?;
    for (depends_on, required, state, accepted, work_revision, candidate_revision) in dependencies {
        if state != "accepted" {
            reasons.push(format!("dependency {depends_on} is not accepted"));
        } else if required.is_some() && required != accepted {
            reasons.push(format!(
                "dependency {depends_on} has a different accepted candidate"
            ));
        } else {
            let bound = if accepted.as_deref().is_some_and(|id| !id.is_empty()) {
                candidate_revision
            } else {
                Some(work_revision)
            };
            if bound != Some(item.current_input_revision) {
                reasons.push(format!(
                    "dependency {depends_on} is bound to a stale input revision"
                ));
            }
        }
    }
    Ok(json!({
        "ready": reasons.is_empty(),
        "reasons": reasons,
        "workItemId": work_item_id,
        "inputRevision": item.input_revision,
    }))
}

fn is_ready(kernel: &Kernel, work_item_id: &str) -> KernelResult<bool> {
    Ok(readiness(kernel, work_item_id)?["ready"] == Value::Bool(true))
}

fn plan_status_entries(kernel: &Kernel) -> KernelResult<Vec<Value>> {
    let plans = query_all(
        &kernel.database,
        "SELECT * FROM plans WHERE project_id = ? ORDER BY sequence",
        [&kernel.project_id],
        PlanRow::from_row,
    )?;
    let mut out = Vec::new();
    for row in plans {
        let mut packages = Map::new();
        for entry in plan_packages::plan_packages(kernel, &row.plan_id)? {
            let key = if entry["cancelledAt"].is_null() {
                entry["progress"].as_str().unwrap_or("").to_string()
            } else {
                "cancelled".to_string()
            };
            let count = packages.get(&key).and_then(Value::as_i64).unwrap_or(0);
            packages.insert(key, json!(count + 1));
        }
        let signoffs = query_all(
            &kernel.database,
            "SELECT integration_id, created_at FROM plan_signoffs WHERE project_id = ? AND plan_id = ? ORDER BY created_at, integration_id",
            [&kernel.project_id, &row.plan_id],
            |r| {
                Ok(json!({
                    "integrationId": r.get::<_, String>(0)?,
                    "createdAt": r.get::<_, String>(1)?,
                }))
            },
        )?;
        out.push(json!({
            "planId": row.plan_id,
            "title": row.title,
            "tier": row.tier,
            "state": row.state,
            "cancelled": row.cancelled_at.is_some(),
            "architectAgentId": row.architect_agent_id,
            "createdAt": row.created_at,
            "updatedAt": row.updated_at,
            "packages": packages,
            "signoffs": signoffs,
        }));
    }
    Ok(out)
}

fn evidence_entry(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<Value> {
    Ok(json!({
        "evidenceId": row.get::<_, String>(offset)?,
        "criterion": row.get::<_, String>(offset + 1)?,
        "passed": row.get::<_, i64>(offset + 2)? == 1,
        "artifactRef": row.get::<_, String>(offset + 3)?,
        "observation": row.get::<_, Option<String>>(offset + 4)?,
        "exitStatus": row.get::<_, Option<i64>>(offset + 5)?,
        "evidenceHash": row.get::<_, String>(offset + 6)?,
    }))
}

/// `statusSnapshot`.
pub fn status_snapshot(kernel: &Kernel) -> KernelResult<Value> {
    kernel.assert_open()?;
    let (run_state, run_version) = query_opt(
        &kernel.database,
        "SELECT state, state_version FROM run_controls WHERE project_id = ?",
        [&kernel.project_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
    )?
    .ok_or_else(|| {
        KernelError::type_error("Cannot read properties of undefined (reading 'state')")
    })?;
    let supervision = query_opt(
        &kernel.database,
        "SELECT enabled, health, target_epoch, checkpoint_epoch,
          checkpoint_assignment_id, replacement_attempts
         FROM supervision_control WHERE project_id = ?",
        [&kernel.project_id],
        |row| {
            Ok(json!({
                "enabled": row.get::<_, i64>(0)? == 1,
                "health": row.get::<_, String>(1)?,
                "targetEpoch": row.get::<_, i64>(2)?,
                "checkpointEpoch": row.get::<_, Option<i64>>(3)?,
                "checkpointAssignmentId": row.get::<_, Option<String>>(4)?,
                "replacementAttempts": row.get::<_, i64>(5)?,
            }))
        },
    )?
    .ok_or_else(|| KernelError::controller("supervision control record is missing"))?;
    let roles = query_all(
        &kernel.database,
        "SELECT s.role, s.seat_id, s.state AS seat_state,
           EXISTS(SELECT 1 FROM actors a WHERE a.project_id = s.project_id
             AND a.seat_id = s.seat_id AND a.active = 1 AND a.revoked_at IS NULL) AS actor_active,
           (SELECT a.assignment_id FROM assignments a WHERE a.project_id = s.project_id
             AND a.seat_id = s.seat_id AND a.authority_state IN ('active', 'unknown')
             ORDER BY a.created_at DESC LIMIT 1) AS assignment_id
         FROM seats s WHERE s.project_id = ? ORDER BY s.role, s.seat_id",
        [&kernel.project_id],
        |row| {
            Ok(json!({
                "role": row.get::<_, String>(0)?,
                "seatId": row.get::<_, String>(1)?,
                "seatState": row.get::<_, String>(2)?,
                "actorActive": row.get::<_, i64>(3)? == 1,
                "sessionState": Value::Null,
                "assignmentId": row.get::<_, Option<String>>(4)?,
            }))
        },
    )?;
    type WorkRow = (String, String, String, String, Option<String>, String);
    let work_rows = query_all(
        &kernel.database,
        "SELECT w.work_item_id, w.title, w.required_role, w.state, s.name AS owner,
           (SELECT json_group_array(dependency.depends_on_work_item_id)
             FROM dependency_edges dependency JOIN work_items prerequisite
               ON prerequisite.project_id = dependency.project_id
               AND prerequisite.work_item_id = dependency.depends_on_work_item_id
             WHERE dependency.project_id = w.project_id AND dependency.work_item_id = w.work_item_id
               AND prerequisite.state <> 'accepted') AS blockers
         FROM work_items w
         LEFT JOIN assignments a ON a.project_id = w.project_id AND a.work_item_id = w.work_item_id
           AND a.authority_state IN ('active', 'unknown')
         LEFT JOIN seats s ON s.project_id = a.project_id AND s.seat_id = a.seat_id
         WHERE w.project_id = ? ORDER BY w.created_at, w.work_item_id",
        [&kernel.project_id],
        |row| -> rusqlite::Result<WorkRow> {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        },
    )?;
    let findings = query_all(
        &kernel.database,
        "SELECT finding_id, affected_work_item_id, affected_seat_id, affected_assignment_id,
           affected_generation, fingerprint, severity, state, evidence_json, requested_correction,
           acknowledgement_deadline, resolution_condition, escalation_route, intervention_count,
           reopened_from_finding_id
         FROM findings WHERE project_id = ? ORDER BY created_at, finding_id",
        [&kernel.project_id],
        |row| {
            let evidence: String = row.get(8)?;
            Ok(json!({
                "findingId": row.get::<_, String>(0)?,
                "affectedWorkItemId": row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                "affectedSeatId": row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                "affectedAssignmentId": row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                "affectedGeneration": row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                "fingerprint": row.get::<_, String>(5)?,
                "severity": row.get::<_, String>(6)?,
                "state": row.get::<_, String>(7)?,
                "evidence": serde_json::from_str::<Value>(&evidence).unwrap_or(Value::Null),
                "requestedCorrection": row.get::<_, String>(9)?,
                "acknowledgementDeadline": row.get::<_, Option<String>>(10)?.unwrap_or_default(),
                "resolutionCondition": row.get::<_, String>(11)?,
                "escalationRoute": row.get::<_, String>(12)?,
                "interventionCount": row.get::<_, i64>(13)?,
                "reopenedFromFindingId": row.get::<_, Option<String>>(14)?,
            }))
        },
    )?;
    type EvidenceRow = (String, String, String, Option<String>, Option<String>);
    let evidence_rows = query_all(
        &kernel.database,
        "SELECT c.candidate_id, c.commit_sha, c.report_hash, c.evidence_json,
           (SELECT e.artifact_ref FROM candidate_evidence e WHERE e.project_id = c.project_id
             AND e.candidate_id = c.candidate_id ORDER BY e.created_at DESC LIMIT 1) AS evidence_ref
         FROM candidates c WHERE c.project_id = ? ORDER BY c.created_at, c.candidate_id",
        [&kernel.project_id],
        |row| -> rusqlite::Result<EvidenceRow> {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    let verifier_rows = query_all(
        &kernel.database,
        "SELECT candidate_id, evidence_id, criterion, passed, artifact_ref,
           observation, exit_status, evidence_hash
         FROM candidate_evidence WHERE project_id = ?
         ORDER BY candidate_id, criterion, evidence_id",
        [&kernel.project_id],
        |row| Ok((row.get::<_, String>(0)?, evidence_entry(row, 1)?)),
    )?;
    let mut verifier_by_candidate: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for (candidate_id, entry) in verifier_rows {
        verifier_by_candidate
            .entry(candidate_id)
            .or_default()
            .push(entry);
    }
    let final_rows = query_all(
        &kernel.database,
        "SELECT c.work_item_id, c.assignment_id, c.commit_sha, e.evidence_id, e.criterion,
           e.passed, e.artifact_ref, e.observation, e.exit_status, e.evidence_hash
         FROM final_verification_commits c
         JOIN final_verification_evidence e ON e.project_id = c.project_id
           AND e.work_item_id = c.work_item_id AND e.commit_sha = c.commit_sha
           AND e.input_revision = c.input_revision
         WHERE c.project_id = ?
         ORDER BY c.work_item_id, e.criterion",
        [&kernel.project_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                evidence_entry(row, 3)?,
            ))
        },
    )?;
    let mut final_order: Vec<String> = Vec::new();
    let mut final_by_work: BTreeMap<String, (String, String, Vec<Value>)> = BTreeMap::new();
    for (work_item_id, assignment_id, commit_sha, entry) in final_rows {
        let slot = final_by_work
            .entry(work_item_id.clone())
            .or_insert_with(|| {
                final_order.push(work_item_id.clone());
                (assignment_id, commit_sha, Vec::new())
            });
        slot.2.push(entry);
    }
    let pause = pauses::pause_state(kernel)?;
    let state_version = kernel.state_version()?;
    let input_revision = kernel.input_revision()?;
    let plans = plan_status_entries(kernel)?;
    let mut work = Vec::new();
    for (work_item_id, title, role, state, owner, blockers) in work_rows {
        let blockers: Value = serde_json::from_str(&blockers)?;
        let actionable =
            (state == "pending" || state == "ready") && is_ready(kernel, &work_item_id)?;
        let next: &[&str] = if state == "ready" && actionable {
            &["assign"]
        } else if state == "pending" && actionable {
            &["mark_ready"]
        } else if state == "blocked" {
            &["reconcile", "contain"]
        } else if state == "awaiting_verification" {
            &["verify"]
        } else if state == "accepted" {
            &[]
        } else {
            &["wait"]
        };
        work.push(json!({
            "workItemId": work_item_id,
            "title": title,
            "role": role,
            "state": state,
            "owner": owner,
            "blockers": blockers,
            "nextLegalActions": next,
        }));
    }
    let evidence: Vec<Value> = evidence_rows
        .into_iter()
        .map(
            |(candidate_id, commit_sha, report_hash, evidence_json, evidence_ref)| {
                let developer = match evidence_json.as_deref() {
                    Some(text) if !text.is_empty() => {
                        serde_json::from_str(text).unwrap_or(Value::Null)
                    }
                    _ => Value::Null,
                };
                let verifier = verifier_by_candidate
                    .remove(&candidate_id)
                    .unwrap_or_default();
                json!({
                    "candidateId": candidate_id,
                    "commitSha": commit_sha,
                    "reportHash": report_hash,
                    "developerEvidence": developer,
                    "evidenceRef": evidence_ref,
                    "verifierEvidence": verifier,
                })
            },
        )
        .collect();
    let final_verification: Vec<Value> = final_order
        .iter()
        .map(|work_item_id| {
            let (assignment_id, commit_sha, evidence) = &final_by_work[work_item_id];
            json!({
                "workItemId": work_item_id,
                "assignmentId": assignment_id,
                "commitSha": commit_sha,
                "evidence": evidence,
            })
        })
        .collect();
    Ok(json!({
        "projectId": kernel.project_id,
        "run": {"state": run_state, "stateVersion": run_version},
        "pause": pause,
        "stateVersion": state_version,
        "inputRevision": input_revision,
        "supervision": supervision,
        "roles": roles,
        "plans": plans,
        "work": work,
        "findings": findings,
        "evidence": evidence,
        "finalVerification": final_verification,
    }))
}

/// `inspect`: the first row any of the ledger's record tables holds under this id.
pub fn inspect(kernel: &Kernel, id: &str) -> KernelResult<Value> {
    kernel.assert_open()?;
    for (kind, table, column) in [
        ("work_item", "work_items", "work_item_id"),
        ("assignment", "assignments", "assignment_id"),
        ("candidate", "candidates", "candidate_id"),
        ("finding", "findings", "finding_id"),
        ("recovery", "recovery_attempts", "recovery_id"),
        ("report", "agent_reports", "report_id"),
        ("review", "reviews", "review_id"),
        ("integration", "integrations", "integration_id"),
        ("agent_finding", "agent_findings", "finding_id"),
        ("prompt_relay", "prompt_relays", "relay_id"),
    ] {
        let record = query_opt(
            &kernel.database,
            &format!("SELECT * FROM {table} WHERE project_id = ? AND {column} = ?"),
            [&kernel.project_id, id],
            |row| {
                let names: Vec<String> = row
                    .as_ref()
                    .column_names()
                    .iter()
                    .map(|name| (*name).to_string())
                    .collect();
                let mut object = Map::new();
                for (index, name) in names.into_iter().enumerate() {
                    object.insert(
                        name,
                        match row.get_ref(index)? {
                            ValueRef::Null => Value::Null,
                            ValueRef::Integer(n) => Value::from(n),
                            ValueRef::Real(f) => number_value(f),
                            ValueRef::Text(t) => {
                                Value::String(String::from_utf8_lossy(t).into_owned())
                            }
                            ValueRef::Blob(b) => json!({
                                "type": "Buffer",
                                "data": b.iter().map(|byte| Value::from(*byte)).collect::<Vec<_>>(),
                            }),
                        },
                    );
                }
                Ok(Value::Object(object))
            },
        )?;
        if let Some(record) = record {
            return Ok(json!({"kind": kind, "id": id, "record": record}));
        }
    }
    Err(KernelError::controller("inspect target does not exist"))
}

fn count_of(kernel: &Kernel, sql: &str) -> KernelResult<i64> {
    Ok(
        query_opt(&kernel.database, sql, [&kernel.project_id], |row| {
            row.get::<_, i64>(0)
        })?
        .unwrap_or(0),
    )
}

/// `busyIndicators`: what an Operator restart waits for. Read-only.
pub fn busy_indicators(kernel: &Kernel) -> KernelResult<Value> {
    kernel.assert_open()?;
    Ok(json!({
        "startedReviews": count_of(kernel, "SELECT COUNT(*) AS n FROM reviews WHERE project_id = ? AND state = 'started'")?,
        "nonTerminalIntegrations": count_of(kernel, "SELECT COUNT(*) AS n FROM integrations WHERE project_id = ? AND state IN ('running', 'merged')")?,
        "unackedDeliveries": count_of(kernel, "SELECT COUNT(*) AS n FROM messages WHERE project_id = ? AND state IN ('sent', 'unacked')")?,
    }))
}

/// `pipelineCounts`: row counts per state of reports, reviews and integrations. Operator-only.
pub fn pipeline_counts(kernel: &Kernel, credential: &str) -> KernelResult<Value> {
    kernel.authorize(credential, "controller:reconcile")?;
    let group = |table: &str| -> KernelResult<Value> {
        let rows = query_all(
            &kernel.database,
            &format!(
                "SELECT state, COUNT(*) AS n FROM {table} WHERE project_id = ? GROUP BY state"
            ),
            [&kernel.project_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )?;
        let mut counts = Map::new();
        for (state, n) in rows {
            counts.insert(state, json!(n));
        }
        Ok(Value::Object(counts))
    };
    Ok(json!({
        "reports": group("agent_reports")?,
        "reviews": group("reviews")?,
        "integrations": group("integrations")?,
    }))
}

/// `awaitingConfirm`: integrations in state merged, newest first, with their latest review state.
pub fn awaiting_confirm(
    kernel: &Kernel,
    credential: &str,
    limit: Option<i64>,
) -> KernelResult<Value> {
    kernel.authorize(credential, "controller:reconcile")?;
    let rows = query_all(
        &kernel.database,
        "SELECT i.integration_id, i.branch, i.created_at,
           (SELECT r.state FROM reviews r WHERE r.project_id = i.project_id AND r.subject_integration_id = i.integration_id
            ORDER BY r.sequence DESC LIMIT 1) AS review_state
         FROM integrations i WHERE i.project_id = ? AND i.state = 'merged' ORDER BY i.sequence DESC LIMIT ?",
        rusqlite::params![kernel.project_id, limit.unwrap_or(20)],
        |row| {
            Ok(json!({
                "integrationId": row.get::<_, String>(0)?,
                "branch": row.get::<_, String>(1)?,
                "createdAt": row.get::<_, String>(2)?,
                "reviewState": row.get::<_, Option<String>>(3)?,
            }))
        },
    )?;
    Ok(Value::Array(rows))
}

fn nexora_of(kernel: &Kernel, ref_kind: &str, ref_id: &str) -> KernelResult<Option<String>> {
    query_opt(
        &kernel.database,
        "SELECT external_id FROM external_links WHERE project_id = ? AND ref_kind = ? AND ref_id = ? AND system = 'nexora'",
        [&kernel.project_id, ref_kind, ref_id],
        |row| row.get::<_, String>(0),
    )
}

/// The package ids a stored plan body names; an unreadable body names none.
fn body_package_ids(body_json: &str) -> Vec<String> {
    let Ok(Value::Object(body)) = serde_json::from_str::<Value>(body_json) else {
        return Vec::new();
    };
    body.get("packages")
        .and_then(Value::as_array)
        .map(|packages| {
            packages
                .iter()
                .filter_map(|p| p.get("id").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// `activeTasks`: the plans still in play with their packages, and the requirements an active agent works on. Capped
/// so a poll stays small; `truncated` says when a cap cut rows. Operator-only.
pub fn active_tasks(kernel: &Kernel, credential: &str, caps: &Value) -> KernelResult<Value> {
    kernel.authorize(credential, "controller:reconcile")?;
    let cap = |name: &str| caps.get(name).and_then(Value::as_i64).unwrap_or(0);
    let (plan_cap, package_cap, requirement_cap) =
        (cap("plans"), cap("packages"), cap("requirements"));
    let mut truncated = false;
    let mut plan_rows = query_all(
        &kernel.database,
        "SELECT * FROM plans WHERE project_id = ? AND cancelled_at IS NULL AND state IN ('draft', 'in_review', 'approved')
           AND NOT EXISTS (SELECT 1 FROM plan_signoffs s WHERE s.project_id = plans.project_id AND s.plan_id = plans.plan_id)
         ORDER BY sequence",
        [&kernel.project_id],
        PlanRow::from_row,
    )?;
    // `slice(-n)`: the last n rows; `slice(-0)` is the whole list.
    let start = if plan_cap <= 0 {
        0
    } else {
        plan_rows.len().saturating_sub(plan_cap as usize)
    };
    plan_rows.drain(..start);
    let active_plan_count = count_of(
        kernel,
        "SELECT COUNT(*) AS n FROM plans WHERE project_id = ? AND cancelled_at IS NULL AND state IN ('draft', 'in_review', 'approved')
           AND NOT EXISTS (SELECT 1 FROM plan_signoffs s WHERE s.project_id = plans.project_id AND s.plan_id = plans.plan_id)",
    )?;
    if active_plan_count > plan_rows.len() as i64 {
        truncated = true;
    }
    let mut plans = Vec::new();
    for row in &plan_rows {
        let revision = query_opt(
            &kernel.database,
            "SELECT body_json FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
            rusqlite::params![
                kernel.project_id,
                row.plan_id,
                row.approved_revision.unwrap_or(row.current_revision)
            ],
            |r| r.get::<_, String>(0),
        )?;
        let body_ids = revision
            .as_deref()
            .map(body_package_ids)
            .unwrap_or_default();
        let package_rows = plan_packages::plan_packages(kernel, &row.plan_id)?;
        let mut ids: BTreeSet<String> = body_ids.into_iter().collect();
        for entry in &package_rows {
            ids.insert(entry["packageId"].as_str().unwrap_or("").to_string());
        }
        let mut entries: Vec<Value> = Vec::new();
        for package_id in &ids {
            let entry = package_rows
                .iter()
                .find(|entry| entry["packageId"].as_str() == Some(package_id));
            let title = revision
                .as_deref()
                .and_then(|body| package_of_body(body, package_id))
                .map(|view| view.title)
                .unwrap_or_else(|| package_id.clone());
            let progress = match entry {
                None => "unassigned".to_string(),
                Some(entry) if entry["cancelledAt"].is_null() => {
                    entry["progress"].as_str().unwrap_or("").to_string()
                }
                Some(_) => "cancelled".to_string(),
            };
            entries.push(json!({
                "packageId": package_id,
                "title": title,
                "nexoraId": nexora_of(kernel, "package", &format!("{}/{package_id}", row.plan_id))?,
                "assigneeAgentId": entry.map_or(Value::Null, |e| e["assigneeAgentId"].clone()),
                "progress": progress,
            }));
        }
        if entries.len() as i64 > package_cap {
            truncated = true;
        }
        let counted: Vec<&Value> = entries
            .iter()
            .filter(|e| e["progress"] != "cancelled")
            .collect();
        let done = counted
            .iter()
            .filter(|e| e["progress"] == "reviewed" || e["progress"] == "integrated")
            .count();
        let total = counted.len();
        entries.truncate(package_cap.max(0) as usize);
        plans.push(json!({
            "planId": row.plan_id,
            "title": row.title,
            "state": row.state,
            "architectAgentId": row.architect_agent_id,
            "nexoraId": nexora_of(kernel, "plan", &row.plan_id)?,
            "total": total,
            "done": done,
            "packages": entries,
        }));
    }
    let mut by_ref: BTreeMap<String, (BTreeSet<String>, Option<String>)> = BTreeMap::new();
    let mut add = |ref_id: String, agent_id: String, title: Option<String>| {
        let entry = by_ref.entry(ref_id).or_default();
        entry.0.insert(agent_id);
        if entry.1.is_none() {
            entry.1 = title;
        }
    };
    let panes = query_all(
        &kernel.database,
        "SELECT p.agent_id, p.task_ref, p.task_title FROM agent_panes p JOIN agents a ON a.project_id = p.project_id AND a.agent_id = p.agent_id
         WHERE p.project_id = ? AND a.state = 'active' AND p.task_ref IS NOT NULL AND instr(p.task_ref, '/') = 0 ORDER BY p.agent_id",
        [&kernel.project_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?)),
    )?;
    for (agent_id, task_ref, task_title) in panes {
        add(task_ref, agent_id, task_title);
    }
    let links = query_all(
        &kernel.database,
        "SELECT l.ref_id, l.bound_agent_id AS agent_id, p.task_title FROM external_links l
         JOIN agents a ON a.project_id = l.project_id AND a.agent_id = l.bound_agent_id
         LEFT JOIN agent_panes p ON p.project_id = a.project_id AND p.agent_id = a.agent_id
         WHERE l.project_id = ? AND l.ref_kind = 'requirement' AND l.system = 'nexora' AND a.state = 'active'
         ORDER BY l.ref_id, l.bound_agent_id",
        [&kernel.project_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?)),
    )?;
    for (ref_id, agent_id, task_title) in links {
        add(ref_id, agent_id, task_title);
    }
    if by_ref.len() as i64 > requirement_cap {
        truncated = true;
    }
    let mut requirements = Vec::new();
    for (ref_id, (agent_ids, title)) in by_ref.into_iter().take(requirement_cap.max(0) as usize) {
        requirements.push(json!({
            "refId": ref_id,
            "nexoraId": nexora_of(kernel, "requirement", &ref_id)?,
            "title": title,
            "agentIds": agent_ids.into_iter().collect::<Vec<_>>(),
        }));
    }
    Ok(json!({"plans": plans, "requirements": requirements, "truncated": truncated}))
}

/// The operations of this area, by the `ControllerCore` method name; `None` for a name that is not this area's.
pub(crate) fn dispatch(kernel: &Kernel, op: &str, args: &[Value]) -> Option<KernelResult<Value>> {
    let a = Args::new(args);
    match op {
        "pipelineCounts" => {
            let credential = arg!(a.str(0, "credential"));
            Some(call(pipeline_counts(kernel, credential)))
        }
        "awaitingConfirm" => {
            let credential = arg!(a.str(0, "credential"));
            let limit = arg!(a.opt_i64(1, "limit"));
            Some(call(awaiting_confirm(kernel, credential, limit)))
        }
        "activeTasks" => {
            let credential = arg!(a.str(0, "credential"));
            let caps = a.value(1);
            Some(call(active_tasks(kernel, credential, caps)))
        }
        "readiness" => {
            let work_item_id = arg!(a.str(0, "workItemId"));
            Some(call(readiness(kernel, work_item_id)))
        }
        "statusSnapshot" => Some(call(status_snapshot(kernel))),
        "inspect" => {
            let id = arg!(a.str(0, "id"));
            Some(call(inspect(kernel, id)))
        }
        "busyIndicators" => Some(call(busy_indicators(kernel))),
        _ => None,
    }
}

/// Rows of the legacy kinds (work items, assignments, commands, candidates, final verification, findings, recovery
/// attempts, supervision), seeded straight into a ledger as test/legacy-rows.ts does: the controller no longer creates
/// them but still reads them.
#[cfg(test)]
pub(crate) mod legacy_rows {
    use crate::canonical::{canonical_json, sha256};
    use crate::env::SystemEnv;
    use crate::kernel::{Kernel, KernelOptions};
    use crate::types::{InitialProject, MutationContext, ProjectInput};
    use rusqlite::{params, Connection};
    use serde_json::{json, Value};
    use std::path::Path;

    pub const CREATED_AT: &str = "2026-01-01T00:00:00.000Z";

    pub fn open_ledger(dir: &Path) -> Connection {
        Connection::open(dir.join("controller.sqlite")).expect("the ledger opens")
    }

    pub fn open_kernel(dir: &Path, project: &InitialProject) -> Kernel {
        Kernel::open(dir, project, &KernelOptions::default(), Box::new(SystemEnv))
            .expect("the kernel opens")
    }

    pub fn project(project_id: &str, owner_credential: &str) -> InitialProject {
        let kinds = [
            "project_config",
            "task_brief",
            "acceptance_criteria",
            "policy",
            "plan",
        ];
        InitialProject {
            project_id: project_id.into(),
            name: "Legacy golden project".into(),
            owner_credential: owner_credential.into(),
            initial_inputs: kinds
                .iter()
                .map(|kind| ProjectInput {
                    kind: (*kind).into(),
                    content: if *kind == "acceptance_criteria" {
                        json!(["criterion-one"])
                    } else {
                        json!({"kind": kind, "revision": 1})
                    },
                })
                .collect(),
        }
    }

    pub fn context(kernel: &Kernel, credential: &str, label: &str) -> MutationContext {
        MutationContext {
            credential: credential.into(),
            request_id: format!("req-{label}"),
            idempotency_key: format!("idem-{label}"),
            expected_version: kernel.state_version().unwrap(),
            input_revision: kernel.input_revision().unwrap(),
        }
    }

    /// `addSeatAndActor`: returns the seat id and the actor id.
    pub fn add_seat_and_actor(
        kernel: &Kernel,
        owner: &str,
        role: &str,
        suffix: &str,
    ) -> (String, String) {
        let seat_id = format!("{suffix}-seat");
        crate::areas::actors::create_seat(
            kernel,
            &context(kernel, owner, &format!("seat-{suffix}")),
            &json!({"seatId": seat_id, "name": format!("{role} {suffix}"), "role": role}),
        )
        .expect("the seat is created");
        let actor = crate::areas::actors::create_actor(
            kernel,
            &context(kernel, owner, &format!("actor-{suffix}")),
            &json!({"displayName": format!("{role} {suffix}"), "role": role, "seatId": seat_id}),
        )
        .expect("the actor is created");
        (seat_id, actor["actorId"].as_str().unwrap().to_string())
    }

    pub fn owner_actor_id(db: &Connection, project_id: &str) -> String {
        db.query_row(
            "SELECT actor_id FROM actors WHERE project_id = ? AND role = 'operator' ORDER BY created_at, actor_id LIMIT 1",
            [project_id],
            |row| row.get(0),
        )
        .expect("the ledger has an operator actor")
    }

    fn input_revision(db: &Connection, project_id: &str) -> i64 {
        db.query_row(
            "SELECT current_input_revision FROM projects WHERE project_id = ?",
            [project_id],
            |row| row.get(0),
        )
        .unwrap()
    }

    pub fn bump(db: &Connection, project_id: &str) {
        db.execute(
            "UPDATE projects SET state_version = state_version + 1 WHERE project_id = ?",
            [project_id],
        )
        .unwrap();
    }

    pub fn insert_work_item(
        db: &Connection,
        project_id: &str,
        work_item_id: &str,
        title: Option<&str>,
        state: &str,
        created_at: &str,
        acceptance_criteria: Option<&Value>,
    ) {
        db.execute(
            "INSERT INTO work_items(project_id, work_item_id, parent_work_item_id, title, description,
               required_role, state, state_version, input_revision, accepted_candidate_id, created_by,
               created_at, acceptance_criteria_json, final_verification)
             VALUES (?, ?, NULL, ?, 'd', 'Developer', ?, 1, ?, NULL, ?, ?, ?, 0)",
            params![
                project_id,
                work_item_id,
                title.map_or_else(|| format!("Work {work_item_id}"), str::to_string),
                state,
                input_revision(db, project_id),
                owner_actor_id(db, project_id),
                created_at,
                acceptance_criteria.map(canonical_json)
            ],
        )
        .unwrap();
        bump(db, project_id);
    }

    pub fn insert_dependency(db: &Connection, project_id: &str, work_item: &str, depends_on: &str) {
        db.execute(
            "INSERT INTO dependency_edges(project_id, work_item_id, depends_on_work_item_id,
               required_candidate_id, created_by, created_at)
             VALUES (?, ?, ?, NULL, ?, ?)",
            params![
                project_id,
                work_item,
                depends_on,
                owner_actor_id(db, project_id),
                CREATED_AT
            ],
        )
        .unwrap();
    }

    /// An assignment with its first attempt and, when given, an in-flight command `(id, state, start_requested)`.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_assignment(
        db: &Connection,
        project_id: &str,
        assignment_id: &str,
        work_item_id: &str,
        seat_id: &str,
        worker_actor_id: &str,
        state: &str,
        authority: &str,
        command: Option<(&str, &str, bool)>,
    ) {
        db.execute(
            "INSERT INTO assignments(project_id, assignment_id, work_item_id, seat_id, state,
               state_version, input_revision, active_generation, authority_state, created_by,
               created_at, worker_actor_id)
             VALUES (?, ?, ?, ?, ?, 1, ?, 1, ?, ?, ?, ?)",
            params![
                project_id,
                assignment_id,
                work_item_id,
                seat_id,
                state,
                input_revision(db, project_id),
                authority,
                owner_actor_id(db, project_id),
                CREATED_AT,
                worker_actor_id
            ],
        )
        .unwrap();
        db.execute(
            "INSERT INTO assignment_attempts(project_id, assignment_id, attempt, generation, state,
               state_version, authority_state, created_at)
             VALUES (?, ?, 1, 1, ?, 1, ?, ?)",
            params![project_id, assignment_id, state, authority, CREATED_AT],
        )
        .unwrap();
        if let Some((command_id, command_state, start_requested)) = command {
            let payload = canonical_json(&json!({"assignmentId": assignment_id}));
            db.execute(
                "INSERT INTO commands(project_id, command_id, assignment_id, attempt, generation,
                   command_type, payload_json, payload_hash, state, state_version, created_at,
                   updated_at, start_requested)
                 VALUES (?, ?, ?, 1, 1, 'dispatch', ?, ?, ?, 1, ?, ?, ?)",
                params![
                    project_id,
                    command_id,
                    assignment_id,
                    payload,
                    sha256(&payload),
                    command_state,
                    CREATED_AT,
                    CREATED_AT,
                    i64::from(start_requested)
                ],
            )
            .unwrap();
        }
        bump(db, project_id);
    }

    pub fn insert_candidate(
        db: &Connection,
        project_id: &str,
        candidate_id: &str,
        assignment_id: &str,
        developer_evidence: &Value,
        evidence: &[(&str, &str, &str)],
    ) {
        let created_by = owner_actor_id(db, project_id);
        let revision = input_revision(db, project_id);
        db.execute(
            "INSERT INTO candidates(project_id, candidate_id, assignment_id, attempt, generation,
               input_revision, commit_sha, base_sha, changed_scope_json, limitations_json, report_hash,
               created_by, created_at, evidence_json)
             VALUES (?, ?, ?, 1, 1, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                project_id,
                candidate_id,
                assignment_id,
                revision,
                "a".repeat(40),
                "b".repeat(40),
                canonical_json(&json!(["src/"])),
                canonical_json(&json!([])),
                sha256(&format!("report:{candidate_id}")),
                created_by,
                CREATED_AT,
                canonical_json(developer_evidence)
            ],
        )
        .unwrap();
        for (evidence_id, verifier_assignment_id, criterion) in evidence {
            let artifact_ref = format!("artifact:{evidence_id}");
            db.execute(
                "INSERT INTO candidate_evidence(project_id, evidence_id, candidate_id,
                   verifier_assignment_id, input_revision, criterion, passed, artifact_ref,
                   evidence_hash, created_by, created_at, observation, exit_status)
                 VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?, ?, ?, NULL, NULL)",
                params![
                    project_id,
                    evidence_id,
                    candidate_id,
                    verifier_assignment_id,
                    revision,
                    criterion,
                    artifact_ref,
                    sha256(&format!("{evidence_id}:{artifact_ref}")),
                    created_by,
                    CREATED_AT
                ],
            )
            .unwrap();
        }
    }

    pub fn insert_final_verification(
        db: &Connection,
        project_id: &str,
        work_item_id: &str,
        assignment_id: &str,
        evidence: &[(&str, &str)],
    ) {
        let created_by = owner_actor_id(db, project_id);
        let revision = input_revision(db, project_id);
        let sha = "c".repeat(40);
        db.execute(
            "INSERT INTO final_verification_commits(project_id, work_item_id, assignment_id,
               input_revision, commit_sha, created_by, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                project_id,
                work_item_id,
                assignment_id,
                revision,
                sha,
                created_by,
                CREATED_AT
            ],
        )
        .unwrap();
        for (evidence_id, criterion) in evidence {
            let artifact_ref = format!("artifact:{evidence_id}");
            db.execute(
                "INSERT INTO final_verification_evidence(project_id, work_item_id, assignment_id,
                   evidence_id, input_revision, commit_sha, criterion, passed, artifact_ref,
                   evidence_hash, created_by, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?, ?)",
                params![
                    project_id,
                    work_item_id,
                    assignment_id,
                    evidence_id,
                    revision,
                    sha,
                    criterion,
                    artifact_ref,
                    sha256(&format!("{evidence_id}:{artifact_ref}")),
                    created_by,
                    CREATED_AT
                ],
            )
            .unwrap();
        }
    }

    pub fn insert_finding(
        db: &Connection,
        project_id: &str,
        finding_id: &str,
        work_item_id: &str,
        assignment_id: &str,
        seat_id: &str,
    ) {
        db.execute(
            "INSERT INTO findings(project_id, finding_id, work_item_id, assignment_id, generation,
               fingerprint, severity, evidence_json, requested_correction, resolution_condition, state,
               state_version, created_by, created_at, affected_seat_id, affected_work_item_id,
               affected_assignment_id, affected_generation, acknowledgement_deadline)
             VALUES (?, ?, ?, ?, 1, ?, 'medium', ?, 'fix it', 'evidence of the fix', 'reported', 1, ?, ?, ?, ?, ?, 1, ?)",
            params![
                project_id,
                finding_id,
                work_item_id,
                assignment_id,
                sha256(&format!("fingerprint:{finding_id}")),
                canonical_json(&json!({"observation": format!("observed {finding_id}")})),
                owner_actor_id(db, project_id),
                CREATED_AT,
                seat_id,
                work_item_id,
                assignment_id,
                "2026-01-02T00:00:00.000Z"
            ],
        )
        .unwrap();
        bump(db, project_id);
    }

    pub fn insert_recovery(
        db: &Connection,
        project_id: &str,
        recovery_id: &str,
        work_item_id: &str,
        assignment_id: &str,
    ) {
        db.execute(
            "INSERT INTO recovery_attempts(project_id, recovery_id, work_item_id, assignment_id,
               recovery_type, generation, reason, containment_state, containment_proof_ref, outcome,
               created_by, created_at)
             VALUES (?, ?, ?, ?, 'worker_replacement', 1, 'worker lost', 'contained', 'proof:test', 'pending', ?, ?)",
            params![
                project_id,
                recovery_id,
                work_item_id,
                assignment_id,
                owner_actor_id(db, project_id),
                CREATED_AT
            ],
        )
        .unwrap();
    }

    pub fn set_supervision_degraded(db: &Connection, project_id: &str, reason: &str) {
        db.execute(
            "UPDATE supervision_control SET enabled = 1, health = 'degraded', bootstrap_pm_allowed = 0, updated_at = ? WHERE project_id = ?",
            params![CREATED_AT, project_id],
        )
        .unwrap();
        let sequence: i64 = db
            .query_row(
                "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM controller_events WHERE project_id = ?",
                [project_id],
                |row| row.get(0),
            )
            .unwrap();
        db.execute(
            "INSERT INTO controller_events(project_id, sequence, event_id, entity_type, entity_id,
               from_state, to_state, state_version, actor_id, request_id, input_revision, payload_json,
               created_at)
             SELECT ?, ?, ?, 'run_control', ?, NULL, 'degraded', 0, ?, ?, current_input_revision, ?, ?
             FROM projects WHERE project_id = ?",
            params![
                project_id,
                sequence,
                format!("legacy-degraded-{sequence}"),
                project_id,
                owner_actor_id(db, project_id),
                format!("legacy-degraded-{sequence}"),
                canonical_json(&json!({"details": {"reason": reason}})),
                CREATED_AT,
                project_id
            ],
        )
        .unwrap();
        bump(db, project_id);
    }
}

#[cfg(test)]
mod tests {
    use super::legacy_rows::*;
    use super::*;
    use serde_json::json;

    /// test/controller-project.test.ts "status and inspect over one row of each legacy kind match the pre-removal golden".
    #[test]
    fn status_and_inspect_over_one_row_of_each_legacy_kind_match_the_golden() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(
            directory.path(),
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .unwrap();
        let info = project(
            "pgoldenlegacyrows",
            "owner-golden-legacy-credential-0123456789",
        );
        let id = info.project_id.clone();
        let kernel = open_kernel(directory.path(), &info);
        let (developer_seat, developer_actor) =
            add_seat_and_actor(&kernel, &info.owner_credential, "Developer", "golden-dev");
        let (verifier_seat, verifier_actor) = add_seat_and_actor(
            &kernel,
            &info.owner_credential,
            "Verifier",
            "golden-verifier",
        );
        kernel.close();
        drop(kernel);
        {
            let db = open_ledger(directory.path());
            insert_work_item(
                &db,
                &id,
                "golden-prerequisite",
                Some("Prerequisite"),
                "accepted",
                "2026-01-01T00:00:01.000Z",
                None,
            );
            insert_work_item(
                &db,
                &id,
                "golden-work",
                Some("Golden work"),
                "running",
                "2026-01-01T00:00:02.000Z",
                Some(&json!(["criterion-one"])),
            );
            insert_work_item(
                &db,
                &id,
                "golden-blocked-by",
                Some("Waits on work"),
                "pending",
                "2026-01-01T00:00:03.000Z",
                None,
            );
            for (work_item_id, state) in [
                ("golden-ready", "ready"),
                ("golden-pending", "pending"),
                ("golden-awaiting", "awaiting_verification"),
                ("golden-failed", "blocked"),
            ] {
                insert_work_item(
                    &db,
                    &id,
                    work_item_id,
                    None,
                    state,
                    "2026-01-01T00:00:04.000Z",
                    None,
                );
            }
            insert_dependency(&db, &id, "golden-blocked-by", "golden-work");
            insert_dependency(&db, &id, "golden-work", "golden-prerequisite");
            insert_assignment(
                &db,
                &id,
                "golden-assignment",
                "golden-work",
                &developer_seat,
                &developer_actor,
                "running",
                "active",
                None,
            );
            insert_assignment(
                &db,
                &id,
                "golden-verifier-assignment",
                "golden-prerequisite",
                &verifier_seat,
                &verifier_actor,
                "completed",
                "contained",
                None,
            );
            insert_candidate(
                &db,
                &id,
                "golden-candidate",
                "golden-assignment",
                &json!(["npm test"]),
                &[(
                    "golden-evidence",
                    "golden-verifier-assignment",
                    "criterion-one",
                )],
            );
            insert_final_verification(
                &db,
                &id,
                "golden-prerequisite",
                "golden-verifier-assignment",
                &[("golden-final-evidence", "criterion-one")],
            );
            insert_finding(
                &db,
                &id,
                "golden-finding",
                "golden-work",
                "golden-assignment",
                &developer_seat,
            );
            insert_recovery(
                &db,
                &id,
                "golden-recovery",
                "golden-work",
                "golden-assignment",
            );
            set_supervision_degraded(&db, &id, "golden supervision failure");
        }
        let owner_actor = owner_actor_id(&open_ledger(directory.path()), &id);
        let kernel = open_kernel(directory.path(), &info);
        let mut inspect = serde_json::Map::new();
        for target in [
            "golden-work",
            "golden-assignment",
            "golden-candidate",
            "golden-finding",
            "golden-recovery",
        ] {
            inspect.insert(target.into(), inspect_record(&kernel, target));
        }
        let mut text = serde_json::to_string(&json!({
            "status": status_snapshot(&kernel).unwrap(),
            "inspect": inspect,
        }))
        .unwrap();
        for (actor_id, name) in [
            (&developer_actor, "developer-actor"),
            (&verifier_actor, "verifier-actor"),
            (&owner_actor, "owner-actor"),
        ] {
            text = text.replace(actor_id.as_str(), name);
        }
        let actual: Value = serde_json::from_str(&text).unwrap();
        let golden: Value = serde_json::from_str(
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../../test/golden/legacy-status-inspect.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(actual, golden);
        // Key order is part of the contract: the dash and the CLI read this JSON.
        assert_eq!(
            serde_json::to_string(&actual).unwrap(),
            serde_json::to_string(&golden).unwrap()
        );
        // The supervision reason of the degraded row is read from the run_control event.
        let reason =
            crate::areas::findings::supervision_reason(&kernel, &info.owner_credential).unwrap();
        assert_eq!(reason, json!("golden supervision failure"));
    }

    fn inspect_record(kernel: &Kernel, target: &str) -> Value {
        inspect(kernel, target).unwrap()
    }
}
