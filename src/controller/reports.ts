import type { ControllerKernel } from "./kernel.js";
import type { ControllerAreas } from "./areas.js";
import { randomUUID } from "node:crypto";
import { sha256 } from "./canonical.js";
import type { MutationContext } from "./types.js";
import { ControllerError } from "./errors.js";
import {
  UNSAFE_TEXT,
  MAX_REPORT_SUMMARY_BYTES,
  MAX_REJECTED_REPORTS,
  type ReportReason,
  type ReportEvidence,
  type AgentReportRecord,
  type ReportResult,
  type AgentReportRow,
  type AgentRow,
} from "./records.js";
import { safeId, reportRecord, reportNotice } from "./helpers.js";

export class ReportsArea {
  constructor(
    readonly kernel: ControllerKernel,
    readonly areas: ControllerAreas,
  ) {}

  /**
   * Records a worker's report of a commit. The controller supplies the git
   * evidence; the ledger row (not the agent) says which branch and base the
   * agent has, and the verdict follows from both. Accepted and rejected
   * reports are both kept, with the claimed identity. An accepted report also
   * queues a notice to the active PM in the same transaction.
   */
  recordAgentReport(
    context: MutationContext,
    input: {
      readonly commitSha: string;
      readonly summary: string;
      readonly evidence: ReportEvidence;
    },
  ): ReportResult {
    if (!/^[0-9a-f]{40}$/.test(input.commitSha))
      throw new TypeError("commit sha must be 40 lowercase hex characters");
    if (
      typeof input.summary !== "string" ||
      !input.summary.isWellFormed() ||
      input.summary.trim() === "" ||
      UNSAFE_TEXT.test(input.summary) ||
      Buffer.byteLength(input.summary, "utf8") > MAX_REPORT_SUMMARY_BYTES
    )
      throw new TypeError(
        `report summary must be one line of printable text of at most ${MAX_REPORT_SUMMARY_BYTES} bytes`,
      );
    const evidence = input.evidence;
    if (
      !Number.isInteger(evidence.generation) ||
      typeof evidence.branch !== "string" ||
      typeof evidence.commitExists !== "boolean" ||
      typeof evidence.isAncestorOfTip !== "boolean" ||
      typeof evidence.isAncestorOfBase !== "boolean" ||
      typeof evidence.checkedAt !== "string" ||
      (evidence.baseSha !== null && !/^[0-9a-f]{40}$/.test(evidence.baseSha)) ||
      (evidence.branchTip !== null &&
        !/^[0-9a-f]{40}$/.test(evidence.branchTip))
    )
      throw new TypeError("report evidence is not well formed");
    return this.kernel.mutate<ReportResult>(
      context,
      "report.record",
      "report:submit",
      {
        commitSha: input.commitSha,
        summaryHash: sha256(input.summary),
        evidence,
      },
      (actor) => {
        const agent = this.kernel.agentByActor(actor.actorId);
        if (agent === undefined)
          throw new ControllerError("only an active agent can report");
        const existing = this.kernel.database
          .prepare(
            "SELECT * FROM agent_reports WHERE project_id = ? AND agent_id = ? AND generation = ? AND commit_sha = ? AND state = 'accepted'",
          )
          .get(
            this.kernel.projectId,
            agent.agent_id,
            agent.generation,
            input.commitSha,
          ) as AgentReportRow | undefined;
        if (existing !== undefined)
          return {
            value: { record: reportRecord(existing), duplicate: true },
            event: {
              entityType: "agent_report",
              entityId: existing.report_id,
              stateVersion: 0,
              details: { duplicate: true },
            },
          };
        const pane = this.kernel.database
          .prepare(
            "SELECT branch, base_sha FROM agent_panes WHERE project_id = ? AND agent_id = ?",
          )
          .get(this.kernel.projectId, agent.agent_id) as
          { branch: string | null; base_sha: string | null } | undefined;
        const baseSha = pane?.base_sha?.toLowerCase() ?? null;
        let reason: ReportReason | null = null;
        if (pane?.branch == null) reason = "no_branch";
        else if (baseSha === null) reason = "no_base";
        else if (
          evidence.generation !== agent.generation ||
          evidence.branch !== pane.branch ||
          evidence.baseSha !== baseSha
        )
          reason = "agent_changed";
        else if (!evidence.commitExists) reason = "commit_missing";
        else if (input.commitSha === baseSha || evidence.isAncestorOfBase)
          reason = "not_new_on_branch";
        else if (!evidence.isAncestorOfTip) reason = "not_on_branch";
        if (reason !== null) {
          const rejected = (
            this.kernel.database
              .prepare(
                "SELECT COUNT(*) AS n FROM agent_reports WHERE project_id = ? AND agent_id = ? AND generation = ? AND state = 'rejected'",
              )
              .get(this.kernel.projectId, agent.agent_id, agent.generation) as {
              n: number;
            }
          ).n;
          // Only a report that would be rejected is refused at the cap: a correct report always gets through.
          if (rejected >= MAX_REJECTED_REPORTS)
            throw new ControllerError(
              "report limit reached for this agent generation",
            );
        }
        const now = this.kernel.now();
        const reportId = randomUUID();
        const sequence = (
          this.kernel.database
            .prepare(
              "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM agent_reports WHERE project_id = ?",
            )
            .get(this.kernel.projectId) as { next: number }
        ).next;
        this.kernel.database
          .prepare(
            `INSERT INTO agent_reports(project_id, report_id, sequence, agent_id, generation, actor_id, commit_sha, branch, summary, state, reason, evidence_json, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
          )
          .run(
            this.kernel.projectId,
            reportId,
            sequence,
            agent.agent_id,
            agent.generation,
            actor.actorId,
            input.commitSha,
            pane?.branch ?? null,
            input.summary,
            reason === null ? "accepted" : "rejected",
            reason,
            JSON.stringify(evidence),
            now,
          );
        this.kernel.touchAgent(agent.agent_id, now);
        if (reason === null)
          this.#announceReport(reportId, agent.role_name, now);
        const row = this.kernel.database
          .prepare(
            "SELECT * FROM agent_reports WHERE project_id = ? AND report_id = ?",
          )
          .get(this.kernel.projectId, reportId) as AgentReportRow;
        return {
          value: { record: reportRecord(row), duplicate: false },
          event: {
            entityType: "agent_report",
            entityId: reportId,
            stateVersion: 0,
            toState: reason === null ? "accepted" : "rejected",
            details: {
              agentId: agent.agent_id,
              generation: agent.generation,
              reason,
            },
          },
        };
      },
    );
  }

  /**
   * The package a worker's reports belong to (its latest assignment in an approved plan) and that plan's architect while the
   * architect is active; undefined when there is none, so the notice falls back to the PM.
   */
  #packageRoute(
    agentId: string,
  ): { architect: AgentRow; planId: string; packageId: string } | undefined {
    const row = this.kernel.database
      .prepare(
        `SELECT p.plan_id, p.package_id, pl.architect_agent_id FROM plan_packages p
         JOIN plans pl ON pl.project_id = p.project_id AND pl.plan_id = p.plan_id
         WHERE p.project_id = ? AND p.assignee_agent_id = ? AND pl.state = 'approved' AND pl.architect_agent_id IS NOT NULL
         ORDER BY p.assigned_at DESC, pl.sequence DESC LIMIT 1`,
      )
      .get(this.kernel.projectId, agentId) as
      | { plan_id: string; package_id: string; architect_agent_id: string }
      | undefined;
    if (row === undefined) return undefined;
    const architect = this.kernel.agentRow(row.architect_agent_id);
    return architect?.state === "active"
      ? { architect, planId: row.plan_id, packageId: row.package_id }
      : undefined;
  }

  /** Queues the notice for an accepted report that has none yet, to the plan's architect for a package report and to the PM otherwise; false when no recipient is active or there is no controller actor. The caller owns the transaction. */
  #announceReport(reportId: string, roleName: string, now: string): boolean {
    const row = this.kernel.database
      .prepare(
        "SELECT * FROM agent_reports WHERE project_id = ? AND report_id = ? AND state = 'accepted' AND notified_message_id IS NULL",
      )
      .get(this.kernel.projectId, reportId) as AgentReportRow | undefined;
    if (row === undefined) return false;
    const controller = this.kernel.database
      .prepare(
        "SELECT actor_id FROM actors WHERE project_id = ? AND is_internal = 1 AND role = 'controller' AND active = 1 AND revoked_at IS NULL",
      )
      .get(this.kernel.projectId) as { actor_id: string } | undefined;
    if (controller === undefined) return false;
    const route = this.#packageRoute(row.agent_id);
    let recipient: AgentRow | undefined = route?.architect;
    if (recipient === undefined) {
      const pms = this.kernel.database
        .prepare(
          "SELECT * FROM agents WHERE project_id = ? AND kind = 'PM' AND state = 'active'",
        )
        .all(this.kernel.projectId) as AgentRow[];
      if (pms.length !== 1) return false;
      recipient = pms[0]!;
    }
    const body =
      route === undefined || recipient !== route.architect
        ? reportNotice(row, roleName)
        : `${reportNotice(row, roleName)}\nWork package: ${route.planId}/${route.packageId}`;
    const messageId = this.areas.messages.insertQueuedMessage(
      controller.actor_id,
      recipient,
      body,
      sha256(body),
      now,
    );
    this.kernel.database
      .prepare(
        "UPDATE agent_reports SET notified_message_id = ? WHERE project_id = ? AND report_id = ? AND notified_message_id IS NULL",
      )
      .run(messageId, this.kernel.projectId, reportId);
    return true;
  }

  /** The accepted report of this exact commit by this agent generation, if there is one. */
  acceptedReportFor(
    agentId: string,
    generation: number,
    commitSha: string,
  ): AgentReportRecord | undefined {
    this.kernel.assertOpen();
    const row = this.kernel.database
      .prepare(
        "SELECT * FROM agent_reports WHERE project_id = ? AND agent_id = ? AND generation = ? AND commit_sha = ? AND state = 'accepted'",
      )
      .get(this.kernel.projectId, agentId, generation, commitSha) as
      AgentReportRow | undefined;
    return row === undefined ? undefined : reportRecord(row);
  }

  /** Accepted reports whose PM notice has not been queued yet (no PM was active when they arrived). */
  unannouncedReports(
    credential: string,
    limit = 50,
  ): readonly AgentReportRecord[] {
    this.kernel.authorize(credential, "controller:reconcile");
    return (
      this.kernel.database
        .prepare(
          "SELECT * FROM agent_reports WHERE project_id = ? AND state = 'accepted' AND notified_message_id IS NULL ORDER BY sequence LIMIT ?",
        )
        .all(this.kernel.projectId, limit) as AgentReportRow[]
    ).map(reportRecord);
  }

  /** Queues the PM notice for one accepted report, once. */
  announceReport(
    context: MutationContext,
    reportId: string,
  ): { readonly announced: boolean } {
    safeId(reportId, "report id");
    return this.kernel.mutate(
      context,
      "report.announce",
      "controller:reconcile",
      { reportId },
      () => {
        const row = this.kernel.database
          .prepare(
            "SELECT a.role_name AS role_name FROM agent_reports r JOIN agents a ON a.project_id = r.project_id AND a.agent_id = r.agent_id WHERE r.project_id = ? AND r.report_id = ?",
          )
          .get(this.kernel.projectId, reportId) as
          { role_name: string } | undefined;
        const announced =
          row !== undefined &&
          this.#announceReport(reportId, row.role_name, this.kernel.now());
        return {
          value: { announced },
          event: {
            entityType: "agent_report",
            entityId: reportId,
            stateVersion: 0,
            details: { announced },
          },
        };
      },
    );
  }

  agentReports(credential: string, limit = 20): readonly AgentReportRecord[] {
    this.kernel.authorize(credential, "controller:reconcile");
    return (
      this.kernel.database
        .prepare(
          "SELECT * FROM agent_reports WHERE project_id = ? ORDER BY sequence DESC LIMIT ?",
        )
        .all(this.kernel.projectId, limit) as AgentReportRow[]
    ).map(reportRecord);
  }
}
