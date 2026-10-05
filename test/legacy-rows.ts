import path from "node:path";
import { openSqlite, type Database } from "../src/controller/sqlite.js";
import { canonicalJson, sha256 } from "../src/controller/canonical.js";

/**
 * SQL helpers that insert legacy-shaped rows (work items, assignments, commands, candidates,
 * final verification, findings, recovery attempts and the supervision control row) straight into
 * a test ledger. They are for test setup only: the controller no longer has a method that
 * creates these rows, but it still reads them, so tests seed them here.
 */

const CREATED_AT = "2026-01-01T00:00:00.000Z";

export function openLedger(stateDirectory: string): Database {
  return openSqlite(path.join(stateDirectory, "controller.sqlite"));
}

/** Runs `seed` against the ledger in `stateDirectory` and closes it again. */
export function seedLedger<T>(
  stateDirectory: string,
  seed: (db: Database) => T,
): T {
  const db = openLedger(stateDirectory);
  try {
    return seed(db);
  } finally {
    db.close();
  }
}

/** The actor that owns the project: legacy rows record it as their creator. */
export function ownerActorId(db: Database, projectId: string): string {
  const row = db
    .prepare(
      "SELECT actor_id FROM actors WHERE project_id = ? AND role = 'operator' ORDER BY created_at, actor_id LIMIT 1",
    )
    .get(projectId) as { actor_id: string } | undefined;
  if (!row) throw new Error("test ledger has no operator actor");
  return row.actor_id;
}

function bumpProjectVersion(db: Database, projectId: string): void {
  db.prepare(
    "UPDATE projects SET state_version = state_version + 1 WHERE project_id = ?",
  ).run(projectId);
}

export interface LegacyWorkItem {
  readonly workItemId: string;
  readonly title?: string;
  readonly description?: string;
  readonly requiredRole?: "PM" | "Developer" | "Verifier" | "Supervisor";
  readonly state?: string;
  readonly parentWorkItemId?: string | null;
  readonly acceptedCandidateId?: string | null;
  readonly acceptanceCriteria?: readonly string[] | null;
  readonly finalVerification?: boolean;
  readonly createdAt?: string;
  readonly createdBy?: string;
}

export function insertWorkItem(
  db: Database,
  projectId: string,
  item: LegacyWorkItem,
): void {
  const inputRevision = (
    db
      .prepare(
        "SELECT current_input_revision AS r FROM projects WHERE project_id = ?",
      )
      .get(projectId) as { r: number }
  ).r;
  db.prepare(
    `INSERT INTO work_items(project_id, work_item_id, parent_work_item_id, title, description,
       required_role, state, state_version, input_revision, accepted_candidate_id, created_by,
       created_at, acceptance_criteria_json, final_verification)
     VALUES (?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?, ?, ?, ?)`,
  ).run(
    projectId,
    item.workItemId,
    item.parentWorkItemId ?? null,
    item.title ?? `Work ${item.workItemId}`,
    item.description ?? "d",
    item.requiredRole ?? "Developer",
    item.state ?? "pending",
    inputRevision,
    item.acceptedCandidateId ?? null,
    item.createdBy ?? ownerActorId(db, projectId),
    item.createdAt ?? CREATED_AT,
    item.acceptanceCriteria ? canonicalJson(item.acceptanceCriteria) : null,
    item.finalVerification ? 1 : 0,
  );
  bumpProjectVersion(db, projectId);
}

export function insertDependency(
  db: Database,
  projectId: string,
  workItemId: string,
  dependsOnWorkItemId: string,
  requiredCandidateId: string | null = null,
): void {
  db.prepare(
    `INSERT INTO dependency_edges(project_id, work_item_id, depends_on_work_item_id,
       required_candidate_id, created_by, created_at)
     VALUES (?, ?, ?, ?, ?, ?)`,
  ).run(
    projectId,
    workItemId,
    dependsOnWorkItemId,
    requiredCandidateId,
    ownerActorId(db, projectId),
    CREATED_AT,
  );
}

export interface LegacyAssignment {
  readonly assignmentId: string;
  readonly workItemId: string;
  readonly seatId: string;
  readonly workerActorId?: string | null;
  readonly state?: string;
  readonly authorityState?: "active" | "contained" | "unknown" | "revoked";
  readonly generation?: number;
  readonly createdAt?: string;
  /** An in-flight command for the attempt, when present. */
  readonly command?: {
    readonly commandId: string;
    readonly state?: string;
    readonly startRequested?: boolean;
  };
}

export function insertAssignment(
  db: Database,
  projectId: string,
  assignment: LegacyAssignment,
): void {
  const state = assignment.state ?? "dispatched";
  const authority = assignment.authorityState ?? "active";
  const generation = assignment.generation ?? 1;
  const createdBy = ownerActorId(db, projectId);
  const inputRevision = (
    db
      .prepare(
        "SELECT current_input_revision AS r FROM projects WHERE project_id = ?",
      )
      .get(projectId) as { r: number }
  ).r;
  const createdAt = assignment.createdAt ?? CREATED_AT;
  db.prepare(
    `INSERT INTO assignments(project_id, assignment_id, work_item_id, seat_id, state,
       state_version, input_revision, active_generation, authority_state, created_by,
       created_at, worker_actor_id)
     VALUES (?, ?, ?, ?, ?, 1, ?, ?, ?, ?, ?, ?)`,
  ).run(
    projectId,
    assignment.assignmentId,
    assignment.workItemId,
    assignment.seatId,
    state,
    inputRevision,
    generation,
    authority,
    createdBy,
    createdAt,
    assignment.workerActorId ?? null,
  );
  db.prepare(
    `INSERT INTO assignment_attempts(project_id, assignment_id, attempt, generation, state,
       state_version, authority_state, created_at)
     VALUES (?, ?, 1, ?, ?, 1, ?, ?)`,
  ).run(
    projectId,
    assignment.assignmentId,
    generation,
    state,
    authority,
    createdAt,
  );
  if (assignment.command) {
    const payload = canonicalJson({ assignmentId: assignment.assignmentId });
    db.prepare(
      `INSERT INTO commands(project_id, command_id, assignment_id, attempt, generation,
         command_type, payload_json, payload_hash, state, state_version, created_at,
         updated_at, start_requested)
       VALUES (?, ?, ?, 1, ?, 'dispatch', ?, ?, ?, 1, ?, ?, ?)`,
    ).run(
      projectId,
      assignment.command.commandId,
      assignment.assignmentId,
      generation,
      payload,
      sha256(payload),
      assignment.command.state ?? "queued",
      createdAt,
      createdAt,
      assignment.command.startRequested ? 1 : 0,
    );
  }
  bumpProjectVersion(db, projectId);
}

export interface LegacyCandidate {
  readonly candidateId: string;
  readonly assignmentId: string;
  readonly commitSha?: string;
  readonly generation?: number;
  readonly developerEvidence?: readonly string[] | null;
  readonly evidence?: ReadonlyArray<{
    readonly evidenceId: string;
    readonly verifierAssignmentId: string;
    readonly criterion: string;
    readonly artifactRef?: string;
    readonly passed?: boolean;
    readonly observation?: string | null;
    readonly exitStatus?: number | null;
  }>;
}

export function insertCandidate(
  db: Database,
  projectId: string,
  candidate: LegacyCandidate,
): void {
  const createdBy = ownerActorId(db, projectId);
  const generation = candidate.generation ?? 1;
  const inputRevision = (
    db
      .prepare(
        "SELECT current_input_revision AS r FROM projects WHERE project_id = ?",
      )
      .get(projectId) as { r: number }
  ).r;
  const sha = candidate.commitSha ?? "a".repeat(40);
  const reportHash = sha256(`report:${candidate.candidateId}`);
  db.prepare(
    `INSERT INTO candidates(project_id, candidate_id, assignment_id, attempt, generation,
       input_revision, commit_sha, base_sha, changed_scope_json, limitations_json, report_hash,
       created_by, created_at, evidence_json)
     VALUES (?, ?, ?, 1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
  ).run(
    projectId,
    candidate.candidateId,
    candidate.assignmentId,
    generation,
    inputRevision,
    sha,
    "b".repeat(40),
    canonicalJson(["src/"]),
    canonicalJson([]),
    reportHash,
    createdBy,
    CREATED_AT,
    candidate.developerEvidence
      ? canonicalJson(candidate.developerEvidence)
      : null,
  );
  for (const entry of candidate.evidence ?? []) {
    const artifactRef = entry.artifactRef ?? `artifact:${entry.evidenceId}`;
    db.prepare(
      `INSERT INTO candidate_evidence(project_id, evidence_id, candidate_id,
         verifier_assignment_id, input_revision, criterion, passed, artifact_ref,
         evidence_hash, created_by, created_at, observation, exit_status)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
    ).run(
      projectId,
      entry.evidenceId,
      candidate.candidateId,
      entry.verifierAssignmentId,
      inputRevision,
      entry.criterion,
      entry.passed === false ? 0 : 1,
      artifactRef,
      sha256(`${entry.evidenceId}:${artifactRef}`),
      createdBy,
      CREATED_AT,
      entry.observation ?? null,
      entry.exitStatus ?? null,
    );
  }
}

/** Marks a work item as accepted through `candidateId`; the candidate row must already exist. */
export function setAcceptedCandidate(
  db: Database,
  projectId: string,
  workItemId: string,
  candidateId: string,
): void {
  db.prepare(
    "UPDATE work_items SET state = 'accepted', accepted_candidate_id = ? WHERE project_id = ? AND work_item_id = ?",
  ).run(candidateId, projectId, workItemId);
}

export interface LegacyFinalVerification {
  readonly workItemId: string;
  readonly assignmentId: string;
  readonly commitSha?: string;
  readonly evidence: ReadonlyArray<{
    readonly evidenceId: string;
    readonly criterion: string;
    readonly artifactRef?: string;
  }>;
}

export function insertFinalVerification(
  db: Database,
  projectId: string,
  verification: LegacyFinalVerification,
): void {
  const createdBy = ownerActorId(db, projectId);
  const inputRevision = (
    db
      .prepare(
        "SELECT current_input_revision AS r FROM projects WHERE project_id = ?",
      )
      .get(projectId) as { r: number }
  ).r;
  const sha = verification.commitSha ?? "c".repeat(40);
  db.prepare(
    `INSERT INTO final_verification_commits(project_id, work_item_id, assignment_id,
       input_revision, commit_sha, created_by, created_at)
     VALUES (?, ?, ?, ?, ?, ?, ?)`,
  ).run(
    projectId,
    verification.workItemId,
    verification.assignmentId,
    inputRevision,
    sha,
    createdBy,
    CREATED_AT,
  );
  for (const entry of verification.evidence) {
    const artifactRef = entry.artifactRef ?? `artifact:${entry.evidenceId}`;
    db.prepare(
      `INSERT INTO final_verification_evidence(project_id, work_item_id, assignment_id,
         evidence_id, input_revision, commit_sha, criterion, passed, artifact_ref,
         evidence_hash, created_by, created_at)
       VALUES (?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?, ?)`,
    ).run(
      projectId,
      verification.workItemId,
      verification.assignmentId,
      entry.evidenceId,
      inputRevision,
      sha,
      entry.criterion,
      artifactRef,
      sha256(`${entry.evidenceId}:${artifactRef}`),
      createdBy,
      CREATED_AT,
    );
  }
}

export interface LegacyFinding {
  readonly findingId: string;
  readonly workItemId: string;
  readonly assignmentId?: string | null;
  readonly seatId?: string | null;
  readonly generation?: number | null;
  readonly state?: string;
  readonly severity?: string;
}

export function insertFinding(
  db: Database,
  projectId: string,
  finding: LegacyFinding,
): void {
  db.prepare(
    `INSERT INTO findings(project_id, finding_id, work_item_id, assignment_id, generation,
       fingerprint, severity, evidence_json, requested_correction, resolution_condition, state,
       state_version, created_by, created_at, affected_seat_id, affected_work_item_id,
       affected_assignment_id, affected_generation, acknowledgement_deadline)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?, ?, ?, ?, ?)`,
  ).run(
    projectId,
    finding.findingId,
    finding.workItemId,
    finding.assignmentId ?? null,
    finding.generation ?? null,
    sha256(`fingerprint:${finding.findingId}`),
    finding.severity ?? "medium",
    canonicalJson({ observation: `observed ${finding.findingId}` }),
    "fix it",
    "evidence of the fix",
    finding.state ?? "reported",
    ownerActorId(db, projectId),
    CREATED_AT,
    finding.seatId ?? null,
    finding.workItemId,
    finding.assignmentId ?? null,
    finding.generation ?? null,
    "2026-01-02T00:00:00.000Z",
  );
  bumpProjectVersion(db, projectId);
}

export interface LegacyRecovery {
  readonly recoveryId: string;
  readonly workItemId: string;
  readonly assignmentId: string;
  readonly recoveryType?:
    "worker_replacement" | "finding_correction" | "implementation_remediation";
  readonly generation?: number;
  readonly reason?: string;
  readonly containmentState?: "unknown" | "contained";
  readonly outcome?: "pending" | "replacement_created" | "blocked" | "failed";
}

export function insertRecovery(
  db: Database,
  projectId: string,
  recovery: LegacyRecovery,
): void {
  db.prepare(
    `INSERT INTO recovery_attempts(project_id, recovery_id, work_item_id, assignment_id,
       recovery_type, generation, reason, containment_state, containment_proof_ref, outcome,
       created_by, created_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
  ).run(
    projectId,
    recovery.recoveryId,
    recovery.workItemId,
    recovery.assignmentId,
    recovery.recoveryType ?? "worker_replacement",
    recovery.generation ?? 1,
    recovery.reason ?? "worker lost",
    recovery.containmentState ?? "contained",
    recovery.containmentState === "unknown" ? null : "proof:test",
    recovery.outcome ?? "pending",
    ownerActorId(db, projectId),
    CREATED_AT,
  );
}

/**
 * Leaves supervision enabled and degraded, as the retired Supervisor loop did, and records the
 * run_control event that `supervisionReason` reads the reason from.
 */
export function setSupervisionDegraded(
  db: Database,
  projectId: string,
  reason: string,
): void {
  db.prepare(
    "UPDATE supervision_control SET enabled = 1, health = 'degraded', bootstrap_pm_allowed = 0, updated_at = ? WHERE project_id = ?",
  ).run(CREATED_AT, projectId);
  const sequence = (
    db
      .prepare(
        "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM controller_events WHERE project_id = ?",
      )
      .get(projectId) as { next: number }
  ).next;
  db.prepare(
    `INSERT INTO controller_events(project_id, sequence, event_id, entity_type, entity_id,
       from_state, to_state, state_version, actor_id, request_id, input_revision, payload_json,
       created_at)
     SELECT ?, ?, ?, 'run_control', ?, NULL, 'degraded', 0, ?, ?, current_input_revision, ?, ?
     FROM projects WHERE project_id = ?`,
  ).run(
    projectId,
    sequence,
    `legacy-degraded-${sequence}`,
    projectId,
    ownerActorId(db, projectId),
    `legacy-degraded-${sequence}`,
    canonicalJson({ details: { reason } }),
    CREATED_AT,
    projectId,
  );
  bumpProjectVersion(db, projectId);
}

/** The stored state of a command row, which only the startup reconcile still changes. */
export function commandStateOf(db: Database, commandId: string): string {
  const row = db
    .prepare("SELECT state FROM commands WHERE command_id = ?")
    .get(commandId) as { state: string } | undefined;
  if (!row) throw new Error(`command ${commandId} does not exist`);
  return row.state;
}
