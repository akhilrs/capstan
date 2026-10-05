import type { ControllerKernel } from "./kernel.js";
import type { ControllerAreas } from "./areas.js";
import { FULL_AUTO_RULE } from "../operator-policy.js";
import {
  endedWithoutRunNotice,
  fullAutoNotice,
  grantEndedNotice,
  type FullAutoChange,
} from "../operator.js";
import type {
  MutationContext,
  OperatorGrantEndReason,
  OperatorGrantRecord,
  OperatorProposalRecord,
} from "./types.js";
import { ControllerError } from "./errors.js";
import {
  type AgentRow,
  type OperatorGrantRow,
  type MutationEvent,
} from "./records.js";
import { safeId, safeText } from "./helpers.js";

export class OperatorGrantsArea {
  constructor(
    readonly kernel: ControllerKernel,
    readonly areas: ControllerAreas,
  ) {}

  #grantRecord(row: OperatorGrantRow): OperatorGrantRecord {
    return {
      grantId: row.grant_id,
      sequence: row.sequence,
      kind: row.kind,
      text: row.text,
      commandSha: row.command_sha,
      createdBy: row.created_by,
      sourceProposalId: row.source_proposal_id,
      createdAt: row.created_at,
      expiresAt: row.expires_at,
      revokedAt: row.revoked_at,
      endedReason: row.ended_reason,
    };
  }

  grantOfProposal(proposalId: string): OperatorGrantRecord | null {
    const row = this.kernel.database
      .prepare(
        "SELECT * FROM operator_grants WHERE project_id = ? AND source_proposal_id = ?",
      )
      .get(this.kernel.projectId, proposalId) as OperatorGrantRow | undefined;
    return row === undefined ? null : this.#grantRecord(row);
  }

  grantRow(grantId: string): OperatorGrantRow | undefined {
    return this.kernel.database
      .prepare(
        "SELECT * FROM operator_grants WHERE project_id = ? AND grant_id = ?",
      )
      .get(this.kernel.projectId, grantId) as OperatorGrantRow | undefined;
  }

  /** Grants that have not ended and have not passed their time cap. */
  activeOperatorGrants(): readonly OperatorGrantRecord[] {
    this.kernel.assertOpen();
    const now = this.kernel.now();
    return (
      this.kernel.database
        .prepare(
          "SELECT * FROM operator_grants WHERE project_id = ? AND ended_reason IS NULL ORDER BY sequence",
        )
        .all(this.kernel.projectId) as OperatorGrantRow[]
    )
      .filter((row) => Date.parse(row.expires_at) > Date.parse(now))
      .map((row) => this.#grantRecord(row));
  }

  /** Grants newest first, ended ones included; `limit` bounds the list. */
  listOperatorGrants(limit = 50): readonly OperatorGrantRecord[] {
    this.kernel.assertOpen();
    return (
      this.kernel.database
        .prepare(
          "SELECT * FROM operator_grants WHERE project_id = ? ORDER BY sequence DESC LIMIT ?",
        )
        .all(this.kernel.projectId, limit) as OperatorGrantRow[]
    ).map((row) => this.#grantRecord(row));
  }

  /** The session grants approved for an agent's proposals end when the agent is released or replaced. The caller owns the transaction. */
  endOperatorGrantsOf(agentId: string): void {
    this.kernel.database
      .prepare(
        `UPDATE operator_grants SET ended_reason = 'released'
         WHERE project_id = ? AND ended_reason IS NULL AND source_proposal_id IN
           (SELECT proposal_id FROM operator_proposals WHERE project_id = ? AND proposer_agent_id = ?)`,
      )
      .run(this.kernel.projectId, this.kernel.projectId, agentId);
  }

  /** Open grants (not ended) whose time cap has passed. Read-only. */
  dueOperatorGrantExpiries(
    alsoDue: ReadonlySet<string> = new Set(),
  ): readonly string[] {
    this.kernel.assertOpen();
    const now = Date.parse(this.kernel.now());
    return (
      this.kernel.database
        .prepare(
          "SELECT * FROM operator_grants WHERE project_id = ? AND ended_reason IS NULL ORDER BY sequence",
        )
        .all(this.kernel.projectId) as OperatorGrantRow[]
    )
      .filter(
        (row) => Date.parse(row.expires_at) <= now || alsoDue.has(row.grant_id),
      )
      .map((row) => row.grant_id);
  }

  #endGrantRow(
    row: OperatorGrantRow,
    reason: OperatorGrantEndReason,
    now: string,
  ): MutationEvent {
    this.kernel.database
      .prepare(
        "UPDATE operator_grants SET ended_reason = ?, revoked_at = ? WHERE project_id = ? AND grant_id = ? AND ended_reason IS NULL",
      )
      .run(
        reason,
        reason === "revoked" ? now : null,
        this.kernel.projectId,
        row.grant_id,
      );
    return {
      entityType: "operator_grant",
      entityId: row.grant_id,
      stateVersion: 0,
      fromState: "active",
      toState: reason === "revoked" ? "revoked" : "ended",
      details: {
        event:
          reason === "revoked"
            ? "operator.grant_revoked"
            : reason === "expired"
              ? "operator.grant_expired"
              : "operator.grant_ended",
        reason,
      },
    };
  }

  #noticeGrantEnded(
    row: OperatorGrantRow,
    reason: OperatorGrantEndReason,
    now: string,
  ): void {
    const source = this.areas.operatorProposals.operatorRow(
      row.source_proposal_id,
    );
    if (source === undefined) return;
    this.areas.messageNotices.noticeToAgent(
      source.proposer_agent_id,
      grantEndedNotice(this.#grantRecord(row), reason),
      now,
    );
  }

  /** Ends every grant that is past its cap; the operator tick calls it. */
  expireOperatorGrants(
    context: MutationContext,
    alsoDue: ReadonlySet<string> = new Set(),
  ): number {
    return this.kernel.mutate<number>(
      context,
      "operator.grant_expire",
      "controller:reconcile",
      {},
      () => {
        const now = this.kernel.now();
        const due = new Set(this.dueOperatorGrantExpiries(alsoDue));
        const events: MutationEvent[] = [];
        for (const row of this.kernel.database
          .prepare(
            "SELECT * FROM operator_grants WHERE project_id = ? AND ended_reason IS NULL ORDER BY sequence",
          )
          .all(this.kernel.projectId) as OperatorGrantRow[]) {
          if (!due.has(row.grant_id)) continue;
          events.push(this.#endGrantRow(row, "expired", now));
          this.#noticeGrantEnded(row, "expired", now);
        }
        return {
          value: events.length,
          event: events[0] ?? {
            entityType: "operator_grant",
            entityId: "none",
            stateVersion: 0,
            details: { expired: 0 },
          },
          extraEvents: events.slice(1),
        };
      },
    );
  }

  /** At startup no grant survives: a controller restart ends every open grant. */
  endOperatorGrantsForRestart(context: MutationContext): number {
    return this.kernel.mutate<number>(
      context,
      "operator.grant_restart",
      "controller:reconcile",
      {},
      () => {
        const now = this.kernel.now();
        const events: MutationEvent[] = [];
        for (const row of this.kernel.database
          .prepare(
            "SELECT * FROM operator_grants WHERE project_id = ? AND ended_reason IS NULL ORDER BY sequence",
          )
          .all(this.kernel.projectId) as OperatorGrantRow[])
          events.push(this.#endGrantRow(row, "restart", now));
        return {
          value: events.length,
          event: events[0] ?? {
            entityType: "operator_grant",
            entityId: "none",
            stateVersion: 0,
            details: { ended: 0, reason: "restart" },
          },
          extraEvents: events.slice(1),
        };
      },
    );
  }

  /** The PM or the operator CLI ends one grant at once. */
  revokeOperatorGrant(
    context: MutationContext,
    grantId: string,
  ): OperatorGrantRecord {
    safeId(grantId, "grant id");
    return this.kernel.mutate<OperatorGrantRecord>(
      context,
      "operator.grant_revoke",
      "operator:decide",
      { grantId },
      (actor) => {
        const agent = this.kernel.agentByActor(actor.actorId);
        if (actor.role !== "operator" && agent?.kind !== "PM")
          throw new ControllerError(
            "only the PM or the operator revokes a grant",
          );
        const row = this.grantRow(grantId);
        if (row === undefined)
          throw new ControllerError(
            `unknown_grant: grant ${grantId} does not exist`,
          );
        if (
          row.ended_reason !== null ||
          Date.parse(row.expires_at) <= Date.parse(this.kernel.now())
        )
          throw new ControllerError(
            `grant_not_active: grant ${grantId} has already ended`,
          );
        const now = this.kernel.now();
        const event = this.#endGrantRow(row, "revoked", now);
        this.#noticeGrantEnded(row, "revoked", now);
        return {
          value: this.#grantRecord(this.grantRow(grantId)!),
          event,
        };
      },
    );
  }

  /**
   * Audits a full-auto change and tells the PM and the Operator agent. Full auto itself lives in the
   * service's memory only; this is the durable record. `on` is the PM's alone and carries what the PM
   * says it asked the user; `off` is the PM's or the operator CLI's; `expired` and `startup` are the
   * controller's.
   */
  recordOperatorFullAuto(
    context: MutationContext,
    input: {
      readonly change: FullAutoChange;
      readonly operatorRole: string;
      readonly minutes?: number;
      readonly askedUser?: string;
    },
  ): null {
    const byController =
      input.change === "expired" || input.change === "startup";
    const askedUser =
      input.askedUser === undefined
        ? null
        : safeText(input.askedUser, "asked-user text", 1024, false);
    return this.kernel.mutate<null>(
      context,
      `operator.full_auto_${input.change}`,
      byController ? "controller:reconcile" : "operator:decide",
      {
        change: input.change,
        minutes: input.minutes ?? null,
        askedUser,
      },
      (actor) => {
        const agent = this.kernel.agentByActor(actor.actorId);
        if (input.change === "on") {
          if (agent?.kind !== "PM")
            throw new ControllerError(
              "only an active PM switches full auto on, and only after asking the user",
            );
          if (askedUser === null)
            throw new ControllerError(
              "asked_user_missing: full auto on needs the text of what the user said",
            );
        } else if (
          input.change === "off" &&
          actor.role !== "operator" &&
          agent?.kind !== "PM"
        )
          throw new ControllerError(
            "only the PM or the operator switches full auto off",
          );
        const now = this.kernel.now();
        if (input.change !== "startup") {
          const body = fullAutoNotice(input.change, input.minutes ?? null);
          this.areas.messageNotices.noticeToPm(body, now);
          for (const operator of this.kernel.database
            .prepare(
              "SELECT * FROM agents WHERE project_id = ? AND role_name = ? AND state = 'active'",
            )
            .all(this.kernel.projectId, input.operatorRole) as AgentRow[])
            this.areas.messageNotices.noticeToAgent(
              operator.agent_id,
              body,
              now,
            );
        }
        return {
          value: null,
          event: {
            entityType: "operator_full_auto",
            entityId: "full-auto",
            stateVersion: 0,
            toState: input.change,
            details: {
              event:
                input.change === "startup"
                  ? "operator.full_auto_off"
                  : `operator.full_auto_${input.change}`,
              ...(input.change === "startup" ? { reason: "startup" } : {}),
              minutes: input.minutes ?? null,
              askedUser,
              by: actor.actorId,
            },
          },
        };
      },
    );
  }

  /** An approved proposal that has not started is ended because full auto went off before it began. */
  endFullAutoProposal(
    context: MutationContext,
    proposalId: string,
  ): OperatorProposalRecord {
    safeId(proposalId, "proposal id");
    return this.kernel.mutate<OperatorProposalRecord>(
      context,
      "operator.full_auto_refuse",
      "controller:reconcile",
      { proposalId },
      () => {
        const row = this.areas.operatorProposals.operatorRow(proposalId);
        if (row?.state !== "approved" || row.auto_rule !== FULL_AUTO_RULE)
          throw new ControllerError(
            `proposal ${proposalId} is not waiting under full auto`,
          );
        const now = this.kernel.now();
        this.areas.operatorProposals.setOperatorState(row, "cancelled", now);
        const record = this.areas.operatorProposals.operatorRecord(
          this.areas.operatorProposals.operatorRow(proposalId)!,
        );
        const body = endedWithoutRunNotice(
          record,
          "was not run: full auto ended before it started. Propose it again if it is still needed.",
        );
        this.areas.messageNotices.noticeToAgent(
          row.proposer_agent_id,
          body,
          now,
        );
        this.areas.messageNotices.noticeToPm(body, now);
        return {
          value: record,
          event: this.areas.operatorProposals.operatorEvent(
            proposalId,
            "approved",
            "cancelled",
            {
              reason: "full_auto_ended",
            },
          ),
        };
      },
    );
  }
}
