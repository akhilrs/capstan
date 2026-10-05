import type { ControllerKernel } from "./kernel.js";
import type { ControllerAreas } from "./areas.js";
import { randomUUID } from "node:crypto";
import { type AuthenticatedActor } from "./auth.js";
import { sha256 } from "./canonical.js";
import { isFinalState, type MessageState } from "./messaging.js";
import { type MutationContext } from "./types.js";
import { ControllerError, RunPausedError } from "./errors.js";
import {
  MAX_FINDING_EVIDENCE_BYTES,
  MAX_FINDING_CORRECTION_BYTES,
  MAX_FINDING_CONDITION_BYTES,
  FINDING_INTERVENTIONS,
  FINDING_SEVERITIES,
  type AgentFindingSeverity,
  type AgentFindingState,
  type AgentFindingReason,
  type AgentFindingNoticeEvent,
  type AgentFindingRecord,
  type AgentFindingNoticeRecord,
  type AgentFindingRow,
  type AgentFindingNoticeRow,
  type AgentRow,
  type SupervisionActivity,
} from "./records.js";
import {
  safeId,
  findingText,
  findingTask,
  findingNoticeBody,
} from "./helpers.js";

export class FindingsArea {
  constructor(
    readonly kernel: ControllerKernel,
    readonly areas: ControllerAreas,
  ) {}

  #findingRow(findingId: string): AgentFindingRow {
    const row = this.kernel.database
      .prepare(
        "SELECT * FROM agent_findings WHERE project_id = ? AND finding_id = ?",
      )
      .get(this.kernel.projectId, findingId) as AgentFindingRow | undefined;
    if (row === undefined)
      throw new ControllerError("the finding does not exist");
    return row;
  }

  #findingRecord(row: AgentFindingRow): AgentFindingRecord {
    const deliveries = this.kernel.database
      .prepare(
        `SELECT d.attempt, d.message_id, m.state AS message_state FROM agent_finding_deliveries d
         LEFT JOIN messages m ON m.project_id = d.project_id AND m.message_id = d.message_id
         WHERE d.project_id = ? AND d.finding_id = ? ORDER BY d.attempt`,
      )
      .all(this.kernel.projectId, row.finding_id) as Array<{
      attempt: number;
      message_id: string;
      message_state: MessageState | null;
    }>;
    const checks = this.kernel.database
      .prepare(
        "SELECT after_intervention, result, evidence_text, created_at FROM agent_finding_checks WHERE project_id = ? AND finding_id = ? ORDER BY after_intervention",
      )
      .all(this.kernel.projectId, row.finding_id) as Array<{
      after_intervention: number;
      result: "resolved" | "unresolved" | "timed_out";
      evidence_text: string;
      created_at: string;
    }>;
    return {
      findingId: row.finding_id,
      sequence: row.sequence,
      targetAgentId: row.target_agent_id,
      raisedByAgentId: row.raised_by_agent_id,
      severity: row.severity,
      evidence: row.evidence_text,
      requestedCorrection: row.requested_correction,
      resolutionCondition: row.resolution_condition,
      state: row.state,
      interventions: row.interventions,
      stateReason: row.state_reason,
      createdAt: row.created_at,
      closedAt: row.closed_at,
      deliveries: deliveries.map((d) => ({
        attempt: d.attempt,
        messageId: d.message_id,
        messageState: d.message_state,
      })),
      checks: checks.map((c) => ({
        afterIntervention: c.after_intervention,
        result: c.result,
        evidence: c.evidence_text,
        createdAt: c.created_at,
      })),
    };
  }

  /** Writes one intervention: the delivery row and the message from the controller to the target. */
  #deliverFinding(
    finding: AgentFindingRow,
    target: AgentRow,
    attempt: number,
    evidence: string,
    now: string,
  ): string {
    const body = findingTask(finding, attempt, evidence);
    const messageId = this.areas.messages.insertQueuedMessage(
      this.kernel.controllerActorId(),
      target,
      body,
      sha256(body),
      now,
    );
    this.kernel.database
      .prepare(
        "INSERT INTO agent_finding_deliveries(project_id, finding_id, attempt, message_id, evidence_text, created_at) VALUES (?, ?, ?, ?, ?, ?)",
      )
      .run(
        this.kernel.projectId,
        finding.finding_id,
        attempt,
        messageId,
        evidence,
        now,
      );
    return messageId;
  }

  /**
   * Raises a finding about an active Developer or Verifier agent. In one
   * transaction: the finding, the first delivery (a message from the
   * controller to the target) and the PM notice.
   */
  raiseFinding(
    context: MutationContext,
    input: {
      readonly targetAgentId: string;
      readonly severity: string;
      readonly evidence: string;
      readonly correction: string;
      readonly doneWhen: string;
    },
  ): AgentFindingRecord {
    safeId(input.targetAgentId, "target agent id");
    if (!FINDING_SEVERITIES.includes(input.severity as AgentFindingSeverity))
      throw new TypeError(
        `severity must be one of ${FINDING_SEVERITIES.join(", ")}`,
      );
    const evidence = findingText(
      input.evidence,
      "evidence",
      MAX_FINDING_EVIDENCE_BYTES,
    );
    const correction = findingText(
      input.correction,
      "correction",
      MAX_FINDING_CORRECTION_BYTES,
    );
    const doneWhen = findingText(
      input.doneWhen,
      "done-when condition",
      MAX_FINDING_CONDITION_BYTES,
    );
    return this.kernel.mutate<AgentFindingRecord>(
      context,
      "finding.raise",
      "finding:raise",
      {
        targetAgentId: input.targetAgentId,
        severity: input.severity,
        evidenceHash: sha256(evidence),
        correctionHash: sha256(correction),
        doneWhenHash: sha256(doneWhen),
      },
      (actor) => {
        const caller = this.kernel.agentByActor(actor.actorId);
        if (caller?.kind !== "Supervisor" || caller.state !== "active")
          throw new ControllerError(
            "only an active Supervisor can raise a finding",
          );
        const target = this.kernel.agentRow(input.targetAgentId);
        if (target?.state !== "active")
          throw new ControllerError("the target agent is not active");
        if (target.kind !== "Developer" && target.kind !== "Verifier")
          throw new ControllerError(
            "a finding can only be raised about a Developer or Verifier agent",
          );
        if (this.areas.pauses.isDeliveryPaused(target.agent_id))
          throw new RunPausedError(
            `target is paused: ${target.agent_id} is held, so a finding cannot be raised about it`,
          );
        if (target.agent_id === caller.agent_id)
          throw new ControllerError(
            "a Supervisor cannot raise a finding about itself",
          );
        const open = this.kernel.database
          .prepare(
            "SELECT finding_id FROM agent_findings WHERE project_id = ? AND target_agent_id = ? AND state = 'open'",
          )
          .get(this.kernel.projectId, target.agent_id) as
          { finding_id: string } | undefined;
        if (open)
          throw new ControllerError(
            `finding ${open.finding_id} is already open for this agent`,
          );
        const now = this.kernel.now();
        const findingId = randomUUID();
        const sequence = (
          this.kernel.database
            .prepare(
              "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM agent_findings WHERE project_id = ?",
            )
            .get(this.kernel.projectId) as { next: number }
        ).next;
        this.kernel.database
          .prepare(
            `INSERT INTO agent_findings(project_id, finding_id, sequence, target_agent_id, raised_by_agent_id, raised_by_actor_id, severity,
               evidence_text, requested_correction, resolution_condition, state, interventions, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'open', 1, ?)`,
          )
          .run(
            this.kernel.projectId,
            findingId,
            sequence,
            target.agent_id,
            caller.agent_id,
            actor.actorId,
            input.severity,
            evidence,
            correction,
            doneWhen,
            now,
          );
        const row = this.#findingRow(findingId);
        this.#deliverFinding(row, target, 1, evidence, now);
        this.#queueFindingNotice(findingId, "raised", now);
        return {
          value: this.#findingRecord(this.#findingRow(findingId)),
          event: {
            entityType: "agent_finding",
            entityId: findingId,
            stateVersion: 0,
            toState: "open",
            details: {
              targetAgentId: target.agent_id,
              raisedByAgentId: caller.agent_id,
              severity: input.severity,
            },
          },
        };
      },
    );
  }

  /**
   * The raising Supervisor's resolution check. Allowed once the latest
   * delivery is acknowledged. Resolved closes the finding; unresolved sends
   * the second intervention, or escalates after the second.
   */
  checkFinding(
    context: MutationContext,
    input: {
      readonly findingId: string;
      readonly result: string;
      readonly evidence: string;
    },
  ): AgentFindingRecord {
    safeId(input.findingId, "finding id");
    if (input.result !== "resolved" && input.result !== "unresolved")
      throw new TypeError("the result must be resolved or unresolved");
    const evidence = findingText(
      input.evidence,
      "evidence",
      MAX_FINDING_EVIDENCE_BYTES,
    );
    return this.kernel.mutate<AgentFindingRecord>(
      context,
      "finding.check",
      "finding:check",
      {
        findingId: input.findingId,
        result: input.result,
        evidenceHash: sha256(evidence),
      },
      (actor) => {
        const caller = this.kernel.agentByActor(actor.actorId);
        const finding = this.#findingRow(input.findingId);
        if (
          caller?.kind !== "Supervisor" ||
          caller.state !== "active" ||
          caller.agent_id !== finding.raised_by_agent_id
        )
          throw new ControllerError(
            "only the Supervisor that raised the finding can check it",
          );
        if (finding.state !== "open")
          throw new ControllerError(`the finding is already ${finding.state}`);
        const message = this.kernel.database
          .prepare(
            `SELECT m.state FROM agent_finding_deliveries d
             JOIN messages m ON m.project_id = d.project_id AND m.message_id = d.message_id
             WHERE d.project_id = ? AND d.finding_id = ? AND d.attempt = ?`,
          )
          .get(
            this.kernel.projectId,
            finding.finding_id,
            finding.interventions,
          ) as { state: MessageState } | undefined;
        if (message?.state !== "acked" && message?.state !== "acked_late")
          throw new ControllerError(
            "the target has not acknowledged the latest correction yet",
          );
        const now = this.kernel.now();
        this.kernel.database
          .prepare(
            `INSERT INTO agent_finding_checks(project_id, finding_id, check_id, after_intervention, result, evidence_text, checked_by_actor_id, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
          )
          .run(
            this.kernel.projectId,
            finding.finding_id,
            randomUUID(),
            finding.interventions,
            input.result,
            evidence,
            actor.actorId,
            now,
          );
        let toState: AgentFindingState = "open";
        if (input.result === "resolved") {
          this.closeFinding(actor, context, finding, "resolved", null, now);
          toState = "resolved";
        } else if (finding.interventions < FINDING_INTERVENTIONS) {
          const target = this.kernel.agentRow(finding.target_agent_id);
          if (target?.state !== "active")
            throw new ControllerError("the target agent is not active");
          const moved = this.kernel.database
            .prepare(
              "UPDATE agent_findings SET interventions = 2 WHERE project_id = ? AND finding_id = ? AND state = 'open' AND interventions = 1",
            )
            .run(this.kernel.projectId, finding.finding_id);
          if (moved.changes !== 1)
            throw new ControllerError("the finding changed during the check");
          this.#deliverFinding(
            this.#findingRow(finding.finding_id),
            target,
            2,
            evidence,
            now,
          );
        } else {
          this.closeFinding(
            actor,
            context,
            finding,
            "escalated",
            "second_unresolved",
            now,
          );
          toState = "escalated";
        }
        return {
          value: this.#findingRecord(this.#findingRow(finding.finding_id)),
          event: {
            entityType: "agent_finding",
            entityId: finding.finding_id,
            stateVersion: 0,
            fromState: "open",
            toState,
            details: {
              result: input.result,
              afterIntervention: finding.interventions,
            },
          },
        };
      },
    );
  }

  /**
   * Closes an open finding: the state change is conditional, a delivery
   * message that is not final is cancelled, and the PM notice is queued.
   * The caller owns the transaction.
   */
  closeFinding(
    actor: AuthenticatedActor,
    context: MutationContext,
    finding: AgentFindingRow,
    state: "resolved" | "escalated" | "cancelled",
    reason: AgentFindingReason | null,
    now: string,
  ): void {
    const closed = this.kernel.database
      .prepare(
        "UPDATE agent_findings SET state = ?, state_reason = ?, closed_at = ? WHERE project_id = ? AND finding_id = ? AND state = 'open'",
      )
      .run(state, reason, now, this.kernel.projectId, finding.finding_id);
    if (closed.changes !== 1)
      throw new ControllerError("the finding is no longer open");
    const deliveries = this.kernel.database
      .prepare(
        "SELECT message_id FROM agent_finding_deliveries WHERE project_id = ? AND finding_id = ?",
      )
      .all(this.kernel.projectId, finding.finding_id) as Array<{
      message_id: string;
    }>;
    for (const delivery of deliveries) {
      const message = this.areas.messages.messageRow(delivery.message_id);
      if (message === undefined || isFinalState(message.state)) continue;
      const version = this.areas.messages.updateMessage(
        message,
        "cancelled",
        { state_reason: "finding_closed" },
        now,
      );
      this.areas.messages.appendMessageEvent(actor, context, {
        messageId: message.message_id,
        from: message.state,
        to: "cancelled",
        stateVersion: version,
        details: { reason: "finding_closed" },
      });
    }
    this.#queueFindingNotice(
      finding.finding_id,
      state === "resolved"
        ? "resolved"
        : state === "escalated"
          ? "escalated"
          : "cancelled",
      now,
    );
  }

  /** Inserts the PM notice row for an event and announces it at once when it can be (exactly one active PM and no earlier notice of this finding waiting). */
  #queueFindingNotice(
    findingId: string,
    event: AgentFindingNoticeEvent,
    now: string,
  ): void {
    const noticeId = randomUUID();
    const sequence = (
      this.kernel.database
        .prepare(
          "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM agent_finding_notices WHERE project_id = ?",
        )
        .get(this.kernel.projectId) as { next: number }
    ).next;
    this.kernel.database
      .prepare(
        "INSERT INTO agent_finding_notices(project_id, notice_id, sequence, finding_id, event, created_at) VALUES (?, ?, ?, ?, ?, ?)",
      )
      .run(this.kernel.projectId, noticeId, sequence, findingId, event, now);
    this.#announceFindingNotice(noticeId, now);
  }

  /** Queues the PM message for a waiting notice; false when it must wait. The caller owns the transaction. */
  #announceFindingNotice(noticeId: string, now: string): boolean {
    const notice = this.kernel.database
      .prepare(
        "SELECT * FROM agent_finding_notices WHERE project_id = ? AND notice_id = ? AND message_id IS NULL",
      )
      .get(this.kernel.projectId, noticeId) as
      AgentFindingNoticeRow | undefined;
    if (notice === undefined) return false;
    const earlier = this.kernel.database
      .prepare(
        "SELECT 1 AS present FROM agent_finding_notices WHERE project_id = ? AND finding_id = ? AND message_id IS NULL AND sequence < ?",
      )
      .get(this.kernel.projectId, notice.finding_id, notice.sequence);
    if (earlier) return false;
    const parties = this.areas.messageNotices.noticeParties();
    if (parties === undefined) return false;
    const finding = this.#findingRow(notice.finding_id);
    const last = this.kernel.database
      .prepare(
        "SELECT evidence_text FROM agent_finding_checks WHERE project_id = ? AND finding_id = ? ORDER BY after_intervention DESC LIMIT 1",
      )
      .get(this.kernel.projectId, notice.finding_id) as
      { evidence_text: string } | undefined;
    const body = findingNoticeBody(
      finding,
      notice.event,
      last?.evidence_text ?? null,
    );
    const messageId = this.areas.messages.insertQueuedMessage(
      parties.controllerActorId,
      parties.pm,
      body,
      sha256(body),
      now,
    );
    this.kernel.database
      .prepare(
        "UPDATE agent_finding_notices SET message_id = ? WHERE project_id = ? AND notice_id = ? AND message_id IS NULL",
      )
      .run(messageId, this.kernel.projectId, noticeId);
    return true;
  }

  /** Finding notices that are still waiting for a PM, oldest first. */
  unannouncedFindingNotices(
    credential: string,
    limit = 50,
  ): readonly AgentFindingNoticeRecord[] {
    this.kernel.authorize(credential, "controller:reconcile");
    return (
      this.kernel.database
        .prepare(
          "SELECT * FROM agent_finding_notices WHERE project_id = ? AND message_id IS NULL ORDER BY sequence LIMIT ?",
        )
        .all(this.kernel.projectId, limit) as AgentFindingNoticeRow[]
    ).map((n) => ({
      noticeId: n.notice_id,
      sequence: n.sequence,
      findingId: n.finding_id,
      event: n.event,
      messageId: n.message_id,
    }));
  }

  announceFindingNotice(
    context: MutationContext,
    noticeId: string,
  ): { readonly announced: boolean } {
    safeId(noticeId, "notice id");
    return this.kernel.mutate(
      context,
      "finding.announce",
      "controller:reconcile",
      { noticeId },
      () => {
        const announced = this.#announceFindingNotice(
          noticeId,
          this.kernel.now(),
        );
        return {
          value: { announced },
          event: {
            entityType: "agent_finding",
            entityId: noticeId,
            stateVersion: 0,
            details: { announced },
          },
        };
      },
    );
  }

  /** Refuses a caller whose actor lacks the capability to observe agents; a revoked grant stops observation. */
  assertCanObserve(credential: string): void {
    this.kernel.assertOpen();
    this.kernel.authorize(credential, "agent:observe");
  }

  /** Findings, newest first, with their deliveries and checks. */
  findings(credential: string, limit = 20): readonly AgentFindingRecord[] {
    this.kernel.authorize(credential, "controller:reconcile");
    return (
      this.kernel.database
        .prepare(
          "SELECT * FROM agent_findings WHERE project_id = ? ORDER BY sequence DESC LIMIT ?",
        )
        .all(this.kernel.projectId, limit) as AgentFindingRow[]
    ).map((row) => this.#findingRecord(row));
  }

  /**
   * Ends every open finding whose latest delivery has seen no change for
   * `deadlineSeconds` (creation, `queued_at`, `sent_at`, `acked_at`; deferrals
   * and expiry do not count) as escalated, with a check by the controller's
   * own actor. Returns the ids it escalated.
   */
  sweepFindings(
    newContext: () => MutationContext,
    deadlineSeconds: number,
    onError: (findingId: string, error: unknown) => void = (_id, error) => {
      throw error;
    },
  ): readonly string[] {
    if (!Number.isInteger(deadlineSeconds) || deadlineSeconds < 1)
      throw new TypeError("the deadline must be a positive number of seconds");
    const first = newContext();
    this.kernel.authorize(first.credential, "controller:reconcile");
    const due = (this.#dueFindingIds(deadlineSeconds) as string[]).slice();
    const escalated: string[] = [];
    for (const [index, findingId] of due.entries()) {
      const context = index === 0 ? first : newContext();
      let done: boolean;
      try {
        done = this.#sweepOne(context, findingId, deadlineSeconds);
      } catch (error) {
        onError(findingId, error);
        continue;
      }
      if (done) escalated.push(findingId);
    }
    return escalated;
  }

  #sweepOne(
    context: MutationContext,
    findingId: string,
    deadlineSeconds: number,
  ): boolean {
    {
      const done = this.kernel.mutate<boolean>(
        context,
        "finding.deadline",
        "controller:reconcile",
        { findingId, deadlineSeconds },
        (actor) => {
          const finding = this.#findingRow(findingId);
          if (
            finding.state !== "open" ||
            !this.#dueFindingIds(deadlineSeconds).includes(findingId)
          )
            return {
              value: false,
              event: {
                entityType: "agent_finding",
                entityId: findingId,
                stateVersion: 0,
                details: { skipped: true },
              },
            };
          const now = this.kernel.now();
          const latest = this.kernel.database
            .prepare(
              `SELECT m.state FROM agent_finding_deliveries d
               JOIN messages m ON m.project_id = d.project_id AND m.message_id = d.message_id
               WHERE d.project_id = ? AND d.finding_id = ? AND d.attempt = ?`,
            )
            .get(this.kernel.projectId, findingId, finding.interventions) as
            { state: MessageState } | undefined;
          const acknowledged =
            latest?.state === "acked" || latest?.state === "acked_late";
          this.kernel.database
            .prepare(
              `INSERT INTO agent_finding_checks(project_id, finding_id, check_id, after_intervention, result, evidence_text, checked_by_actor_id, created_at)
               VALUES (?, ?, ?, ?, 'timed_out', ?, ?, ?)`,
            )
            .run(
              this.kernel.projectId,
              findingId,
              randomUUID(),
              finding.interventions,
              acknowledged
                ? `no check by the supervisor within ${deadlineSeconds} seconds of the latest change to the delivery (the correction was acknowledged)`
                : `the latest correction was not acknowledged and no check was recorded within ${deadlineSeconds} seconds of its latest change (message state ${latest?.state ?? "unknown"})`,
              actor.actorId,
              now,
            );
          this.closeFinding(
            actor,
            context,
            finding,
            "escalated",
            "timed_out",
            now,
          );
          return {
            value: true,
            event: {
              entityType: "agent_finding",
              entityId: findingId,
              stateVersion: 0,
              fromState: "open",
              toState: "escalated",
              details: { reason: "timed_out", deadlineSeconds },
            },
          };
        },
      );
      return done;
    }
  }

  #dueFindingIds(deadlineSeconds: number): readonly string[] {
    const rows = this.kernel.database
      .prepare(
        `SELECT f.finding_id, f.target_agent_id, d.created_at AS delivered_at, m.queued_at, m.sent_at, m.acked_at
         FROM agent_findings f
         JOIN agent_finding_deliveries d ON d.project_id = f.project_id AND d.finding_id = f.finding_id AND d.attempt = f.interventions
         JOIN messages m ON m.project_id = d.project_id AND m.message_id = d.message_id
         WHERE f.project_id = ? AND f.state = 'open' ORDER BY f.sequence`,
      )
      .all(this.kernel.projectId) as Array<{
      finding_id: string;
      target_agent_id: string;
      delivered_at: string;
      queued_at: string;
      sent_at: string | null;
      acked_at: string | null;
    }>;
    const nowMs = Date.parse(this.kernel.now());
    return rows
      .filter((row) => {
        if (this.areas.pauses.isDeliveryPaused(row.target_agent_id))
          return false;
        const anchor = Math.max(
          this.areas.pauses.resumedMsFor(row.target_agent_id),
          ...[row.delivered_at, row.queued_at, row.sent_at, row.acked_at]
            .filter((value): value is string => value !== null)
            .map((value) => Date.parse(value)),
        );
        return nowMs - anchor >= deadlineSeconds * 1000;
      })
      .map((row) => row.finding_id);
  }

  /** Why supervision is degraded: the reason of the newest degraded event, or null when healthy or when that event carried none. */
  supervisionReason(credential: string): string | null {
    this.kernel.assertOpen();
    this.kernel.authorize(credential, "controller:reconcile");
    const row = this.kernel.database
      .prepare(
        `SELECT json_extract(e.payload_json, '$.details.reason') AS reason
         FROM supervision_control c
         LEFT JOIN controller_events e ON e.project_id = c.project_id
           AND e.sequence = (SELECT MAX(sequence) FROM controller_events
             WHERE project_id = c.project_id AND entity_type = 'run_control' AND to_state = 'degraded')
         WHERE c.project_id = ? AND c.health = 'degraded'`,
      )
      .get(this.kernel.projectId) as { reason: unknown } | undefined;
    return typeof row?.reason === "string" ? row.reason : null;
  }

  /** What the live supervision loop is doing now: its active Supervisor, the newest routine check it queued and the open findings. */
  supervisionActivity(): SupervisionActivity {
    this.kernel.assertOpen();
    const supervisor = this.kernel.database
      .prepare(
        "SELECT agent_id, state FROM agents WHERE project_id = ? AND kind = 'Supervisor' AND state = 'active' ORDER BY created_at DESC LIMIT 1",
      )
      .get(this.kernel.projectId) as
      { agent_id: string; state: string } | undefined;
    const check = this.kernel.database
      .prepare(
        `SELECT m.message_id, m.state, m.queued_at, m.acked_at
         FROM supervision_checks c JOIN messages m ON m.project_id = c.project_id AND m.message_id = c.message_id
         WHERE c.project_id = ? ORDER BY m.sequence DESC LIMIT 1`,
      )
      .get(this.kernel.projectId) as
      | {
          message_id: string;
          state: string;
          queued_at: string;
          acked_at: string | null;
        }
      | undefined;
    const open = this.kernel.database
      .prepare(
        "SELECT COUNT(*) AS n FROM agent_findings WHERE project_id = ? AND state = 'open'",
      )
      .get(this.kernel.projectId) as { n: number };
    return {
      supervisor:
        supervisor === undefined
          ? null
          : { agentId: supervisor.agent_id, state: supervisor.state },
      lastCheck:
        check === undefined
          ? null
          : {
              messageId: check.message_id,
              state: check.state,
              queuedAt: check.queued_at,
              ackedAt: check.acked_at,
            },
      openFindings: open.n,
    };
  }
}
