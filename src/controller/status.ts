import type { ControllerKernel } from "./kernel.js";
import type { ControllerAreas } from "./areas.js";
import { ControllerError } from "./errors.js";
import {
  type PlanStatusEntry,
  type PlanRow,
  type ReadinessResult,
  type ControllerStatus,
} from "./records.js";

export class StatusArea {
  constructor(
    readonly kernel: ControllerKernel,
    readonly areas: ControllerAreas,
  ) {}

  readiness(workItemId: string): ReadinessResult {
    this.kernel.assertOpen();
    const item = this.kernel.database
      .prepare(
        `
      SELECT w.state, w.input_revision, w.required_role, p.current_input_revision, r.state AS run_state
      FROM work_items w JOIN projects p ON p.project_id = w.project_id
      JOIN run_controls r ON r.project_id = w.project_id
      WHERE w.project_id = ? AND w.work_item_id = ?
    `,
      )
      .get(this.kernel.projectId, workItemId) as
      | {
          state: string;
          input_revision: number;
          required_role: string;
          current_input_revision: number;
          run_state: string;
        }
      | undefined;
    if (!item) throw new ControllerError("work item does not exist");
    const reasons: string[] = [];
    if (
      item.state !== "pending" &&
      item.state !== "blocked" &&
      item.state !== "ready"
    )
      reasons.push(`work item state is ${item.state}`);
    if (item.input_revision !== item.current_input_revision)
      reasons.push("work item input revision is stale");
    if (item.run_state !== "active") reasons.push(`run is ${item.run_state}`);
    const supervision = this.kernel.database
      .prepare(
        `SELECT enabled, health, target_epoch, checkpoint_epoch, bootstrap_pm_allowed
         FROM supervision_control WHERE project_id = ?`,
      )
      .get(this.kernel.projectId) as
      | {
          enabled: number;
          health: string;
          target_epoch: number;
          checkpoint_epoch: number | null;
          bootstrap_pm_allowed: number;
        }
      | undefined;
    if (supervision?.enabled && item.required_role !== "Supervisor") {
      const bootstrapPm =
        item.required_role === "PM" &&
        supervision.checkpoint_epoch === null &&
        supervision.bootstrap_pm_allowed === 1 &&
        !this.kernel.database
          .prepare(
            `SELECT 1 FROM assignments a JOIN seats s
              ON s.project_id = a.project_id AND s.seat_id = a.seat_id
             WHERE a.project_id = ? AND s.role = 'PM'
               AND a.authority_state IN ('active', 'unknown') LIMIT 1`,
          )
          .get(this.kernel.projectId);
      const correction = this.kernel.database
        .prepare(
          `SELECT c.finding_id, c.target_assignment_id, c.target_generation,
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
           LIMIT 1`,
        )
        .get(
          this.kernel.projectId,
          workItemId,
          this.kernel.projectId,
          workItemId,
        ) as
        | {
            finding_id: string;
            target_assignment_id: string;
            target_generation: number;
            target_seat_id: string;
            target_role: string;
            state: string;
            intervention_count: number;
            authority_state: string;
            active_generation: number;
            verifier_child: number;
          }
        | undefined;
      if (!bootstrapPm) {
        if (
          supervision.health !== "healthy" ||
          supervision.checkpoint_epoch !== supervision.target_epoch
        )
          reasons.push("supervision is degraded or its checkpoint is stale");
        const openFinding = this.kernel.database
          .prepare(
            "SELECT 1 FROM findings WHERE project_id = ? AND state <> 'resolved' LIMIT 1",
          )
          .get(this.kernel.projectId);
        if (openFinding && !correction)
          reasons.push("an open finding blocks ordinary dispatch");
        if (correction) {
          const occupied = this.kernel.database
            .prepare(
              `SELECT 1 FROM assignments WHERE project_id = ? AND seat_id = ?
               AND assignment_id <> ? AND authority_state IN ('active', 'unknown') LIMIT 1`,
            )
            .get(
              this.kernel.projectId,
              correction.target_seat_id,
              correction.target_assignment_id,
            );
          if (
            (correction.verifier_child === 1
              ? item.required_role !== "Verifier" ||
                correction.target_role !== "Developer" ||
                correction.state !== "correcting"
              : correction.target_role !== item.required_role) ||
            correction.authority_state !== "contained" ||
            correction.active_generation !== correction.target_generation ||
            correction.intervention_count > 2 ||
            !["reported", "acknowledged", "correcting"].includes(
              correction.state,
            ) ||
            occupied
          )
            reasons.push("finding-bound correction is not ready for dispatch");
        }
      }
    }
    const seats = this.kernel.database
      .prepare(
        `
      SELECT 1 AS present FROM seats s
      WHERE s.project_id = ? AND s.role = ? AND s.state = 'active'
        AND EXISTS (
          SELECT 1 FROM actors a
          WHERE a.project_id = s.project_id AND a.seat_id = s.seat_id
            AND a.role = s.role AND a.active = 1 AND a.revoked_at IS NULL
        )
      LIMIT 1
    `,
      )
      .get(this.kernel.projectId, item.required_role);
    if (!seats)
      reasons.push(`no active ${item.required_role} seat with an active actor`);
    const activeAssignment = this.kernel.database
      .prepare(
        `
      SELECT 1 AS present FROM assignments WHERE project_id = ? AND work_item_id = ?
        AND authority_state IN ('active', 'unknown') LIMIT 1
    `,
      )
      .get(this.kernel.projectId, workItemId);
    if (activeAssignment)
      reasons.push("work item has active or unknown assignment authority");
    const dependencies = this.kernel.database
      .prepare(
        `
      SELECT d.depends_on_work_item_id, d.required_candidate_id, w.state, w.accepted_candidate_id,
        w.input_revision AS accepted_work_input_revision,
        c.input_revision AS candidate_input_revision
      FROM dependency_edges d JOIN work_items w
        ON w.project_id = d.project_id AND w.work_item_id = d.depends_on_work_item_id
      LEFT JOIN candidates c ON c.project_id = w.project_id AND c.candidate_id = w.accepted_candidate_id
      WHERE d.project_id = ? AND d.work_item_id = ? ORDER BY d.depends_on_work_item_id
    `,
      )
      .all(this.kernel.projectId, workItemId) as Array<{
      depends_on_work_item_id: string;
      required_candidate_id: string | null;
      state: string;
      accepted_candidate_id: string | null;
      accepted_work_input_revision: number;
      candidate_input_revision: number | null;
    }>;
    for (const dependency of dependencies) {
      if (dependency.state !== "accepted") {
        reasons.push(
          `dependency ${dependency.depends_on_work_item_id} is not accepted`,
        );
      } else if (
        dependency.required_candidate_id !== null &&
        dependency.required_candidate_id !== dependency.accepted_candidate_id
      ) {
        reasons.push(
          `dependency ${dependency.depends_on_work_item_id} has a different accepted candidate`,
        );
      } else if (
        (dependency.accepted_candidate_id
          ? dependency.candidate_input_revision
          : dependency.accepted_work_input_revision) !==
        item.current_input_revision
      ) {
        reasons.push(
          `dependency ${dependency.depends_on_work_item_id} is bound to a stale input revision`,
        );
      }
    }
    return {
      ready: reasons.length === 0,
      reasons,
      workItemId,
      inputRevision: item.input_revision,
    };
  }

  #planStatusEntries(): PlanStatusEntry[] {
    return (
      this.kernel.database
        .prepare("SELECT * FROM plans WHERE project_id = ? ORDER BY sequence")
        .all(this.kernel.projectId) as PlanRow[]
    ).map((row) => {
      const packages: Record<string, number> = {};
      for (const entry of this.areas.planPackages.planPackages(row.plan_id)) {
        const key = entry.cancelledAt === null ? entry.progress : "cancelled";
        packages[key] = (packages[key] ?? 0) + 1;
      }
      return {
        planId: row.plan_id,
        title: row.title,
        tier: row.tier,
        state: row.state,
        cancelled: row.cancelled_at !== null,
        architectAgentId: row.architect_agent_id,
        createdAt: row.created_at,
        updatedAt: row.updated_at,
        packages,
        signoffs: (
          this.kernel.database
            .prepare(
              "SELECT integration_id, created_at FROM plan_signoffs WHERE project_id = ? AND plan_id = ? ORDER BY created_at, integration_id",
            )
            .all(this.kernel.projectId, row.plan_id) as {
            integration_id: string;
            created_at: string;
          }[]
        ).map((signoff) => ({
          integrationId: signoff.integration_id,
          createdAt: signoff.created_at,
        })),
      };
    });
  }

  statusSnapshot(): ControllerStatus {
    this.kernel.assertOpen();
    const run = this.kernel.database
      .prepare(
        "SELECT state, state_version FROM run_controls WHERE project_id = ?",
      )
      .get(this.kernel.projectId) as { state: string; state_version: number };
    const supervision = this.kernel.database
      .prepare(
        `SELECT enabled, health, target_epoch, checkpoint_epoch,
          checkpoint_assignment_id, replacement_attempts
         FROM supervision_control WHERE project_id = ?`,
      )
      .get(this.kernel.projectId) as
      | {
          enabled: number;
          health: "healthy" | "evaluating" | "degraded";
          target_epoch: number;
          checkpoint_epoch: number | null;
          checkpoint_assignment_id: string | null;
          replacement_attempts: number;
        }
      | undefined;
    if (!supervision)
      throw new ControllerError("supervision control record is missing");
    const roles = this.kernel.database
      .prepare(
        `
      SELECT s.role, s.seat_id, s.state AS seat_state,
        EXISTS(SELECT 1 FROM actors a WHERE a.project_id = s.project_id
          AND a.seat_id = s.seat_id AND a.active = 1 AND a.revoked_at IS NULL) AS actor_active,
        (SELECT a.assignment_id FROM assignments a WHERE a.project_id = s.project_id
          AND a.seat_id = s.seat_id AND a.authority_state IN ('active', 'unknown')
          ORDER BY a.created_at DESC LIMIT 1) AS assignment_id
      FROM seats s WHERE s.project_id = ? ORDER BY s.role, s.seat_id
    `,
      )
      .all(this.kernel.projectId) as Array<{
      role: string;
      seat_id: string;
      seat_state: string;
      actor_active: number;
      assignment_id: string | null;
    }>;
    const work = this.kernel.database
      .prepare(
        `
      SELECT w.work_item_id, w.title, w.required_role, w.state, s.name AS owner,
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
      WHERE w.project_id = ? ORDER BY w.created_at, w.work_item_id
    `,
      )
      .all(this.kernel.projectId) as Array<{
      work_item_id: string;
      title: string;
      required_role: string;
      state: string;
      owner: string | null;
      blockers: string;
    }>;
    const findings = this.kernel.database
      .prepare(
        `
      SELECT finding_id, affected_work_item_id, affected_seat_id, affected_assignment_id,
        affected_generation, fingerprint, severity, state, evidence_json, requested_correction,
        acknowledgement_deadline, resolution_condition, escalation_route, intervention_count,
        reopened_from_finding_id
      FROM findings WHERE project_id = ? ORDER BY created_at, finding_id
    `,
      )
      .all(this.kernel.projectId) as Array<{
      finding_id: string;
      affected_work_item_id: string | null;
      affected_seat_id: string | null;
      affected_assignment_id: string | null;
      affected_generation: number | null;
      fingerprint: string;
      severity: string;
      state: string;
      evidence_json: string;
      requested_correction: string;
      acknowledgement_deadline: string | null;
      resolution_condition: string;
      escalation_route: string;
      intervention_count: number;
      reopened_from_finding_id: string | null;
    }>;
    const evidence = this.kernel.database
      .prepare(
        `
      SELECT c.candidate_id, c.commit_sha, c.report_hash, c.evidence_json,
        (SELECT e.artifact_ref FROM candidate_evidence e WHERE e.project_id = c.project_id
          AND e.candidate_id = c.candidate_id ORDER BY e.created_at DESC LIMIT 1) AS evidence_ref
      FROM candidates c WHERE c.project_id = ? ORDER BY c.created_at, c.candidate_id
    `,
      )
      .all(this.kernel.projectId) as Array<{
      candidate_id: string;
      commit_sha: string;
      report_hash: string;
      evidence_json: string | null;
      evidence_ref: string | null;
    }>;
    const verifierEvidenceRows = this.kernel.database
      .prepare(
        `SELECT evidence_id, candidate_id, criterion, passed, artifact_ref,
          observation, exit_status, evidence_hash
         FROM candidate_evidence WHERE project_id = ?
         ORDER BY candidate_id, criterion, evidence_id`,
      )
      .all(this.kernel.projectId) as Array<{
      evidence_id: string;
      candidate_id: string;
      criterion: string;
      passed: number;
      artifact_ref: string;
      observation: string | null;
      exit_status: number | null;
      evidence_hash: string;
    }>;
    const verifierEvidenceByCandidate = new Map<
      string,
      ControllerStatus["evidence"][number]["verifierEvidence"][number][]
    >();
    for (const row of verifierEvidenceRows) {
      const entries = verifierEvidenceByCandidate.get(row.candidate_id) ?? [];
      entries.push({
        evidenceId: row.evidence_id,
        criterion: row.criterion,
        passed: row.passed === 1,
        artifactRef: row.artifact_ref,
        observation: row.observation,
        exitStatus: row.exit_status,
        evidenceHash: row.evidence_hash,
      });
      verifierEvidenceByCandidate.set(row.candidate_id, entries);
    }
    const finalVerificationRows = this.kernel.database
      .prepare(
        `
      SELECT c.work_item_id, c.assignment_id, c.commit_sha, e.evidence_id, e.criterion,
        e.passed, e.artifact_ref, e.observation, e.exit_status, e.evidence_hash
      FROM final_verification_commits c
      JOIN final_verification_evidence e ON e.project_id = c.project_id
        AND e.work_item_id = c.work_item_id AND e.commit_sha = c.commit_sha
        AND e.input_revision = c.input_revision
      WHERE c.project_id = ?
      ORDER BY c.work_item_id, e.criterion
    `,
      )
      .all(this.kernel.projectId) as Array<{
      work_item_id: string;
      assignment_id: string;
      commit_sha: string;
      evidence_id: string;
      criterion: string;
      passed: number;
      artifact_ref: string;
      observation: string | null;
      exit_status: number | null;
      evidence_hash: string;
    }>;
    const finalVerificationByWork = new Map<
      string,
      {
        workItemId: string;
        assignmentId: string;
        commitSha: string;
        evidence: Array<{
          evidenceId: string;
          criterion: string;
          passed: boolean;
          artifactRef: string;
          observation: string | null;
          exitStatus: number | null;
          evidenceHash: string;
        }>;
      }
    >();
    for (const row of finalVerificationRows) {
      let verification = finalVerificationByWork.get(row.work_item_id);
      if (!verification) {
        verification = {
          workItemId: row.work_item_id,
          assignmentId: row.assignment_id,
          commitSha: row.commit_sha,
          evidence: [],
        };
        finalVerificationByWork.set(row.work_item_id, verification);
      }
      verification.evidence.push({
        evidenceId: row.evidence_id,
        criterion: row.criterion,
        passed: row.passed === 1,
        artifactRef: row.artifact_ref,
        observation: row.observation,
        exitStatus: row.exit_status,
        evidenceHash: row.evidence_hash,
      });
    }
    return {
      projectId: this.kernel.projectId,
      run: { state: run.state, stateVersion: run.state_version },
      pause: this.areas.pauses.pauseState(),
      stateVersion: this.kernel.stateVersion,
      inputRevision: this.kernel.inputRevision,
      supervision: {
        enabled: supervision.enabled === 1,
        health: supervision.health,
        targetEpoch: supervision.target_epoch,
        checkpointEpoch: supervision.checkpoint_epoch,
        checkpointAssignmentId: supervision.checkpoint_assignment_id,
        replacementAttempts: supervision.replacement_attempts,
      },
      roles: roles.map((row) => ({
        role: row.role,
        seatId: row.seat_id,
        seatState: row.seat_state,
        actorActive: row.actor_active === 1,
        sessionState: null,
        assignmentId: row.assignment_id,
      })),
      plans: this.#planStatusEntries(),
      work: work.map((row) => {
        const blockers = JSON.parse(row.blockers) as string[];
        const actionable =
          (row.state === "pending" || row.state === "ready") &&
          this.readiness(row.work_item_id).ready;
        const nextLegalActions =
          row.state === "ready" && actionable
            ? ["assign"]
            : row.state === "pending" && actionable
              ? ["mark_ready"]
              : row.state === "blocked"
                ? ["reconcile", "contain"]
                : row.state === "awaiting_verification"
                  ? ["verify"]
                  : row.state === "accepted"
                    ? []
                    : ["wait"];
        return {
          workItemId: row.work_item_id,
          title: row.title,
          role: row.required_role,
          state: row.state,
          owner: row.owner,
          blockers,
          nextLegalActions,
        };
      }),
      findings: findings.map((row) => ({
        findingId: row.finding_id,
        affectedWorkItemId: row.affected_work_item_id ?? "",
        affectedSeatId: row.affected_seat_id ?? "",
        affectedAssignmentId: row.affected_assignment_id ?? "",
        affectedGeneration: row.affected_generation ?? 0,
        fingerprint: row.fingerprint,
        severity: row.severity,
        state: row.state,
        evidence: JSON.parse(row.evidence_json) as unknown,
        requestedCorrection: row.requested_correction,
        acknowledgementDeadline: row.acknowledgement_deadline ?? "",
        resolutionCondition: row.resolution_condition,
        escalationRoute: row.escalation_route,
        interventionCount: row.intervention_count,
        reopenedFromFindingId: row.reopened_from_finding_id,
      })),
      evidence: evidence.map((row) => ({
        candidateId: row.candidate_id,
        commitSha: row.commit_sha,
        reportHash: row.report_hash,
        developerEvidence: row.evidence_json
          ? (JSON.parse(row.evidence_json) as string[])
          : null,
        evidenceRef: row.evidence_ref,
        verifierEvidence:
          verifierEvidenceByCandidate.get(row.candidate_id) ?? [],
      })),
      finalVerification: [...finalVerificationByWork.values()],
    };
  }

  inspect(id: string): unknown {
    this.kernel.assertOpen();
    for (const [kind, table, column] of [
      ["work_item", "work_items", "work_item_id"],
      ["assignment", "assignments", "assignment_id"],
      ["candidate", "candidates", "candidate_id"],
      ["finding", "findings", "finding_id"],
      ["recovery", "recovery_attempts", "recovery_id"],
      ["report", "agent_reports", "report_id"],
      ["review", "reviews", "review_id"],
      ["integration", "integrations", "integration_id"],
      ["agent_finding", "agent_findings", "finding_id"],
      ["prompt_relay", "prompt_relays", "relay_id"],
    ] as const) {
      const row = this.kernel.database
        .prepare(
          `SELECT * FROM ${table} WHERE project_id = ? AND ${column} = ?`,
        )
        .get(this.kernel.projectId, id) as Record<string, unknown> | undefined;
      if (!row) continue;
      return { kind, id, record: row };
    }
    throw new ControllerError("inspect target does not exist");
  }

  /** What an Operator restart waits for. Read-only. */
  busyIndicators(): {
    readonly startedReviews: number;
    readonly nonTerminalIntegrations: number;
    readonly unackedDeliveries: number;
  } {
    this.kernel.assertOpen();
    const count = (sql: string): number =>
      (
        this.kernel.database.prepare(sql).get(this.kernel.projectId) as {
          n: number;
        }
      ).n;
    return {
      startedReviews: count(
        "SELECT COUNT(*) AS n FROM reviews WHERE project_id = ? AND state = 'started'",
      ),
      nonTerminalIntegrations: count(
        "SELECT COUNT(*) AS n FROM integrations WHERE project_id = ? AND state IN ('running', 'merged')",
      ),
      unackedDeliveries: count(
        "SELECT COUNT(*) AS n FROM messages WHERE project_id = ? AND state IN ('sent', 'unacked')",
      ),
    };
  }

  /** Row counts per state of reports, reviews and integrations: one GROUP BY each. Operator-only. */
  pipelineCounts(credential: string): {
    readonly reports: Record<string, number>;
    readonly reviews: Record<string, number>;
    readonly integrations: Record<string, number>;
  } {
    this.kernel.authorize(credential, "controller:reconcile");
    const group = (table: string): Record<string, number> => {
      const counts: Record<string, number> = {};
      for (const row of this.kernel.database
        .prepare(
          `SELECT state, COUNT(*) AS n FROM ${table} WHERE project_id = ? GROUP BY state`,
        )
        .all(this.kernel.projectId) as { state: string; n: number }[])
        counts[row.state] = row.n;
      return counts;
    };
    return {
      reports: group("agent_reports"),
      reviews: group("reviews"),
      integrations: group("integrations"),
    };
  }

  /** Integrations in state merged (waiting for confirmation), newest first, with their latest review state. */
  awaitingConfirm(
    credential: string,
    limit = 20,
  ): {
    readonly integrationId: string;
    readonly branch: string;
    readonly createdAt: string;
    readonly reviewState: string | null;
  }[] {
    this.kernel.authorize(credential, "controller:reconcile");
    return (
      this.kernel.database
        .prepare(
          `SELECT i.integration_id, i.branch, i.created_at,
             (SELECT r.state FROM reviews r WHERE r.project_id = i.project_id AND r.subject_integration_id = i.integration_id
              ORDER BY r.sequence DESC LIMIT 1) AS review_state
           FROM integrations i WHERE i.project_id = ? AND i.state = 'merged' ORDER BY i.sequence DESC LIMIT ?`,
        )
        .all(this.kernel.projectId, limit) as {
        integration_id: string;
        branch: string;
        created_at: string;
        review_state: string | null;
      }[]
    ).map((r) => ({
      integrationId: r.integration_id,
      branch: r.branch,
      createdAt: r.created_at,
      reviewState: r.review_state,
    }));
  }
}
