import type { ControllerKernel } from "./kernel.js";
import type { ControllerAreas } from "./areas.js";
import { randomUUID } from "node:crypto";
import {
  authenticateActor,
  credentialHash,
  issueCredential,
  type AuthenticatedActor,
} from "./auth.js";
import { sha256 } from "./canonical.js";
import { HERDR_STATES, isFinalState, type HerdrState } from "./messaging.js";
import {
  type AgentInput,
  type AgentRecord,
  type Identity,
  type MutationContext,
  type ReplacedAgent,
} from "./types.js";
import { ControllerError, MutationConflictError } from "./errors.js";
import {
  ROLE_NAME_PATTERN,
  MAX_SUMMARY_MESSAGES,
  MAX_SUMMARY_BODY,
  MAX_SUMMARY_WORK,
  MAX_SUMMARY_LINKS,
  MAX_SUMMARY_BYTES,
  TRUNCATION_MARKER,
  type ExternalLinkRow,
  type AgentFindingRow,
  type PmRestartSummary,
  type MutationOutput,
} from "./records.js";
import { cutAtCharacters, objectiveOf, safeId } from "./helpers.js";

export class AgentsArea {
  constructor(
    readonly kernel: ControllerKernel,
    readonly areas: ControllerAreas,
  ) {}

  registerAgent(context: MutationContext, input: AgentInput): AgentRecord {
    safeId(input.agentId, "agent id");
    safeId(input.seatId, "seat id");
    safeId(input.actorId, "actor id");
    if (
      typeof input.roleName !== "string" ||
      !ROLE_NAME_PATTERN.test(input.roleName)
    )
      throw new TypeError("role name must be a lowercase configured role name");
    return this.kernel.mutate(
      context,
      "agent.register",
      "actor:manage",
      {
        agentId: input.agentId,
        roleName: input.roleName,
        seatId: input.seatId,
        actorId: input.actorId,
      },
      () => {
        const definition = this.areas.actors
          .roleDefinitions()
          .find((candidate) => candidate.name === input.roleName);
        if (definition?.state !== "active")
          throw new ControllerError(
            "agent role is not an active configured role definition",
          );
        const seat = this.kernel.database
          .prepare(
            "SELECT role, state FROM seats WHERE project_id = ? AND seat_id = ?",
          )
          .get(this.kernel.projectId, input.seatId) as
          { role: string; state: string } | undefined;
        if (!seat || seat.state !== "active" || seat.role !== definition.kind)
          throw new ControllerError(
            "agent seat must be active and match the role kind",
          );
        const actorRow = this.kernel.database
          .prepare(
            "SELECT role, seat_id, active, revoked_at, is_internal FROM actors WHERE project_id = ? AND actor_id = ?",
          )
          .get(this.kernel.projectId, input.actorId) as
          | {
              role: string;
              seat_id: string | null;
              active: number;
              revoked_at: string | null;
              is_internal: number;
            }
          | undefined;
        if (
          !actorRow ||
          actorRow.active !== 1 ||
          actorRow.revoked_at !== null ||
          actorRow.is_internal !== 0 ||
          actorRow.role !== definition.kind ||
          actorRow.seat_id !== input.seatId
        )
          throw new ControllerError(
            "agent actor must be active, of the role kind and attached to the agent seat",
          );
        const taken = this.kernel.database
          .prepare(
            "SELECT 1 AS present FROM agents WHERE project_id = ? AND (actor_id = ? OR agent_id = ? OR (seat_id = ? AND state = 'active')) LIMIT 1",
          )
          .get(
            this.kernel.projectId,
            input.actorId,
            input.agentId,
            input.seatId,
          );
        if (taken)
          throw new MutationConflictError(
            "agent id, actor or seat is already bound to an agent",
          );
        const now = this.kernel.now();
        this.kernel.database
          .prepare(
            "INSERT INTO agents(project_id, agent_id, role_name, kind, seat_id, actor_id, generation, state, last_activity_at, created_at) VALUES (?, ?, ?, ?, ?, ?, 1, 'active', ?, ?)",
          )
          .run(
            this.kernel.projectId,
            input.agentId,
            input.roleName,
            definition.kind,
            input.seatId,
            input.actorId,
            now,
            now,
          );
        return {
          value: this.kernel.agentRecord(input.agentId)!,
          event: {
            entityType: "agent",
            entityId: input.agentId,
            stateVersion: 1,
            toState: "active",
          },
        };
      },
    );
  }

  identify(credential: string): Identity {
    this.kernel.assertOpen();
    const actor = authenticateActor(
      this.kernel.database,
      this.kernel.projectId,
      credential,
    );
    return {
      actorId: actor.actorId,
      role: actor.role,
      capabilities: [...actor.capabilities].sort(),
      agent: this.kernel.agentByActorRecord(actor.actorId),
    };
  }

  listAgents(): readonly AgentRecord[] {
    this.kernel.assertOpen();
    return this.kernel.agentRecords();
  }

  /** The agents whose state is active, ordered by agent id: `listAgents()` without the history of ended ones. */
  activeAgents(): readonly AgentRecord[] {
    this.kernel.assertOpen();
    return this.kernel.agentRecords(true);
  }

  agentRecord(agentId: string): AgentRecord | undefined {
    this.kernel.assertOpen();
    return this.kernel.agentRecord(agentId);
  }

  recordAgentObservation(
    context: MutationContext,
    agentId: string,
    state: HerdrState,
  ): { readonly recorded: boolean } {
    safeId(agentId, "agent id");
    if (!HERDR_STATES.includes(state))
      throw new TypeError("unknown Herdr state");
    this.kernel.authorize(context.credential, "controller:reconcile");
    const latest = this.kernel.database
      .prepare(
        "SELECT herdr_state FROM agent_state_history WHERE project_id = ? AND agent_id = ? ORDER BY sequence DESC LIMIT 1",
      )
      .get(this.kernel.projectId, agentId) as
      { herdr_state: string } | undefined;
    if (latest?.herdr_state === state && !this.kernel.hasStoredRequest(context))
      return { recorded: false };
    return this.kernel.mutate(
      context,
      "agent.observe",
      "controller:reconcile",
      { agentId, state },
      () => {
        const agent = this.kernel.agentRow(agentId);
        if (agent?.state !== "active")
          throw new ControllerError("agent is not active");
        const sequence = (
          this.kernel.database
            .prepare(
              "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM agent_state_history WHERE project_id = ? AND agent_id = ?",
            )
            .get(this.kernel.projectId, agentId) as { next: number }
        ).next;
        this.kernel.database
          .prepare(
            "INSERT INTO agent_state_history(project_id, agent_id, sequence, herdr_state, observed_at) SELECT ?, ?, ?, ?, MAX(?, COALESCE((SELECT MAX(observed_at) FROM agent_state_history WHERE project_id = ? AND agent_id = ?), ?))",
          )
          .run(
            this.kernel.projectId,
            agentId,
            sequence,
            state,
            this.kernel.now(),
            this.kernel.projectId,
            agentId,
            "",
          );
        return {
          value: { recorded: true },
          event: {
            entityType: "agent_observation",
            entityId: agentId,
            stateVersion: sequence,
            toState: state,
          },
        };
      },
    );
  }

  /**
   * Ends an active agent: its seat actors are revoked, its waits closed, a
   * started review cancelled and its messages cancelled, except `failed` ones,
   * which stay failed. `lost` also records the loss and tells the PM in the
   * same transaction (an agent found dead at a daemon start).
   */
  endAgent(
    context: MutationContext,
    agentId: string,
    options: {
      readonly lost?: "found_dead_at_start";
      /** The agent's branch, when its pane row was cleared before the end. */
      readonly branch?: string | null;
    } = {},
  ): { readonly cancelledMessageIds: readonly string[] } {
    safeId(agentId, "agent id");
    return this.kernel.mutate(
      context,
      "agent.end",
      "actor:manage",
      { agentId, lost: options.lost ?? null, branch: options.branch ?? null },
      (actor) => {
        const agent = this.kernel.agentRow(agentId);
        if (agent?.state !== "active")
          throw new ControllerError("agent is not active");
        this.areas.actors.assertSeatFreeOfAuthority(agent.seat_id);
        const now = this.kernel.now();
        this.areas.actors.revokeSeatActors(agent.seat_id, now);
        this.areas.messageNotices.closeWaits(agentId, now);
        this.kernel.database
          .prepare(
            "UPDATE agents SET state = 'ended', ended_at = ? WHERE project_id = ? AND agent_id = ?",
          )
          .run(now, this.kernel.projectId, agentId);
        // An agent that is no longer active cannot stay paused: close its pause in the same transaction.
        const openPause = this.areas.pauses
          .pauseRows()
          .find(
            (pause) => pause.scope === "agent" && pause.agentId === agentId,
          );
        if (openPause !== undefined) {
          const reason = "agent ended";
          this.areas.pauses.closePause(
            "agent",
            agentId,
            reason,
            this.kernel.internalPrincipal().actorId,
            now,
          );
          this.kernel.appendEvent(actor, context, {
            entityType: "agent",
            entityId: agentId,
            from: "paused",
            to: "active",
            action: "agent.resume",
            stateVersion: agent.generation,
            details: { reason, scope: "agent", pausedReason: openPause.reason },
          });
        }
        const endedPlanReviews = this.kernel.database
          .prepare(
            "SELECT subject_plan_id FROM reviews WHERE project_id = ? AND reviewer_agent_id = ? AND state = 'started' AND subject_plan_id IS NOT NULL",
          )
          .all(this.kernel.projectId, agentId) as { subject_plan_id: string }[];
        this.kernel.database
          .prepare(
            "UPDATE reviews SET state = 'cancelled', failure_reason = 'agent_ended', completed_at = ? WHERE project_id = ? AND reviewer_agent_id = ? AND state = 'started'",
          )
          .run(now, this.kernel.projectId, agentId);
        for (const { subject_plan_id } of endedPlanReviews)
          this.kernel.database
            .prepare(
              "UPDATE plans SET state = 'draft', updated_at = ? WHERE project_id = ? AND plan_id = ? AND state = 'in_review'",
            )
            .run(now, this.kernel.projectId, subject_plan_id);
        for (const finding of this.kernel.database
          .prepare(
            "SELECT * FROM agent_findings WHERE project_id = ? AND state = 'open' AND (target_agent_id = ? OR raised_by_agent_id = ?) ORDER BY sequence",
          )
          .all(this.kernel.projectId, agentId, agentId) as AgentFindingRow[])
          this.areas.findings.closeFinding(
            actor,
            context,
            finding,
            "cancelled",
            finding.target_agent_id === agentId
              ? "target_ended"
              : "raiser_ended",
            now,
          );
        this.areas.operatorProposals.cancelUnstartedOperatorProposalsOf(
          agentId,
          now,
        );
        this.areas.operatorGrants.endOperatorGrantsOf(agentId);
        const unacknowledged = this.areas.messages
          .messageRowsFor(agentId)
          .filter((row) => !isFinalState(row.state))
          .map((row) => row.message_id);
        const cancelled = this.areas.messages.cancelMessagesOf(
          actor,
          context,
          agentId,
          "agent_ended",
          now,
          true,
        );
        const branch =
          options.branch ??
          (
            this.kernel.database
              .prepare(
                "SELECT branch FROM agent_panes WHERE project_id = ? AND agent_id = ?",
              )
              .get(this.kernel.projectId, agentId) as
              { branch: string | null } | undefined
          )?.branch ??
          null;
        if (options.lost !== undefined)
          this.areas.actors.recordLost(
            actor,
            context,
            agent,
            options.lost,
            unacknowledged,
            now,
            branch,
          );
        return {
          value: { cancelledMessageIds: cancelled },
          event: {
            entityType: "agent",
            entityId: agentId,
            stateVersion: agent.generation,
            fromState: "active",
            toState: "ended",
            details: { cancelledMessageIds: cancelled, branch },
          },
        };
      },
    );
  }

  replaceAgentGeneration(
    context: MutationContext,
    agentId: string,
  ): ReplacedAgent {
    safeId(agentId, "agent id");
    return this.kernel.mutate(
      context,
      "agent.replace",
      "actor:manage",
      { agentId },
      (actor) => this.#replaceGeneration(actor, context, agentId),
    );
  }

  #replaceGeneration(
    actor: AuthenticatedActor,
    context: MutationContext,
    agentId: string,
  ): MutationOutput<ReplacedAgent> {
    const agent = this.kernel.agentRow(agentId);
    if (agent?.state !== "active")
      throw new ControllerError("agent is not active");
    this.areas.actors.assertSeatFreeOfAuthority(agent.seat_id);
    const now = this.kernel.now();
    this.areas.actors.revokeSeatActors(agent.seat_id, now);
    const actorId = randomUUID();
    const credential = issueCredential();
    this.areas.actors.insertActor(actor.actorId, {
      actorId,
      displayName: `${agent.role_name} generation ${agent.generation + 1}`,
      role: agent.kind,
      seatId: agent.seat_id,
      credentialHash: credentialHash(credential),
      now,
    });
    const generation = agent.generation + 1;
    this.kernel.database
      .prepare(
        "UPDATE agents SET actor_id = ?, generation = ?, last_activity_at = ? WHERE project_id = ? AND agent_id = ?",
      )
      .run(actorId, generation, now, this.kernel.projectId, agentId);
    this.areas.messageNotices.closeWaits(agentId, now);
    this.areas.operatorProposals.cancelUnstartedOperatorProposalsOf(
      agentId,
      now,
    );
    this.areas.operatorGrants.endOperatorGrantsOf(agentId);
    this.kernel.database
      .prepare(
        "INSERT INTO agent_state_history(project_id, agent_id, sequence, herdr_state, observed_at) SELECT ?, ?, COALESCE(MAX(sequence), 0) + 1, 'unknown', MAX(?, COALESCE(MAX(observed_at), '')) FROM agent_state_history WHERE project_id = ? AND agent_id = ?",
      )
      .run(this.kernel.projectId, agentId, now, this.kernel.projectId, agentId);
    const cancelled = this.areas.messages.cancelMessagesOf(
      actor,
      context,
      agentId,
      "generation_replaced",
      now,
    );
    return {
      value: {
        agentId,
        generation,
        actorId,
        credential,
        cancelledMessageIds: cancelled,
      },
      event: {
        entityType: "agent",
        entityId: agentId,
        stateVersion: generation,
        fromState: String(agent.generation),
        toState: String(generation),
        details: { cancelledMessageIds: cancelled },
      },
    };
  }

  /** The newest summary a restart may carry: bounded so neither the table nor a stored result grows without limit. */
  #restartSummary(
    agentId: string,
    generation: number,
    carried: readonly PmRestartSummary[],
  ): PmRestartSummary {
    const finalStates = new Set(["acked", "acked_late", "cancelled"]);
    const brief = this.kernel.database
      .prepare(
        `SELECT content_json FROM project_revisions
         WHERE project_id = ? AND kind = 'task_brief'
         ORDER BY revision DESC LIMIT 1`,
      )
      .get(this.kernel.projectId) as { content_json: string } | undefined;
    const work = this.areas.status
      .statusSnapshot()
      .work.filter(
        (item) => !["accepted", "canceled", "failed"].includes(item.state),
      );
    const seen = new Set<string>();
    const messages: PmRestartSummary["messages"][number][] = [];
    let truncated = work.length > MAX_SUMMARY_WORK;
    const carriedIds = new Set<string>();
    const add = (message: PmRestartSummary["messages"][number]): void => {
      if (seen.has(message.messageId)) return;
      seen.add(message.messageId);
      messages.push(message);
    };
    for (const row of this.areas.messages.messageRowsFor(agentId)) {
      if (finalStates.has(row.state)) continue;
      const sender = this.areas.messages.senderOf(row.sender_actor_id);
      add({
        messageId: row.message_id,
        from: sender.agentId ?? sender.role,
        body: row.body,
        state: row.state,
      });
    }
    for (const summary of carried) {
      if (summary.truncated) truncated = true;
      for (const message of summary.messages) {
        carriedIds.add(message.messageId);
        add(message);
      }
    }
    if (messages.length > MAX_SUMMARY_MESSAGES) truncated = true;
    const bounded = messages.slice(0, MAX_SUMMARY_MESSAGES).map((message) => {
      // A body carried from an earlier summary was cut once already.
      if (
        carriedIds.has(message.messageId) &&
        message.body.endsWith(TRUNCATION_MARKER)
      )
        return message;
      if (Array.from(message.body).length <= MAX_SUMMARY_BODY) return message;
      truncated = true;
      return {
        ...message,
        body: `${cutAtCharacters(message.body, MAX_SUMMARY_BODY)}${TRUNCATION_MARKER}`,
      };
    });
    const allLinks = (
      this.kernel.database
        .prepare(
          "SELECT * FROM external_links WHERE project_id = ? ORDER BY linked_at, ref_kind, ref_id",
        )
        .all(this.kernel.projectId) as ExternalLinkRow[]
    )
      .map((row) => this.areas.links.linkRecord(row))
      .sort((a, b) => Number(b.drift) - Number(a.drift));
    if (allLinks.length > MAX_SUMMARY_LINKS) truncated = true;
    const linksForSummary = allLinks.slice(0, MAX_SUMMARY_LINKS).map((l) => ({
      refKind: l.refKind,
      refId: l.refId,
      externalId: l.externalId,
      syncedState: l.syncedState,
      wanted: l.wanted,
      drift: l.drift,
      boundAgentId: l.boundAgentId,
    }));
    const summary: {
      objective: unknown;
      openWork: PmRestartSummary["openWork"][number][];
      messages: PmRestartSummary["messages"][number][];
      plans: NonNullable<PmRestartSummary["plans"]>[number][];
      integrations: NonNullable<PmRestartSummary["integrations"]>[number][];
      links: NonNullable<PmRestartSummary["links"]>[number][];
      truncated: boolean;
      summarizedGeneration: number;
      generatedAt: string;
    } = {
      objective: objectiveOf(brief?.content_json),
      openWork: work.slice(0, MAX_SUMMARY_WORK).map((item) => ({
        workItemId: item.workItemId,
        title: item.title,
        role: item.role,
        state: item.state,
        owner: item.owner,
        blockers: item.blockers,
      })),
      messages: bounded,
      plans: this.areas.plans.openPlansForSummary(),
      integrations: this.areas.integrations.mergedIntegrationsForSummary(),
      links: linksForSummary,
      truncated,
      summarizedGeneration: generation,
      generatedAt: this.kernel.now(),
    };
    // A summary must fit the prompt it is rendered into: shed messages, then
    // work items, from the end until its JSON is small enough.
    while (
      Buffer.byteLength(JSON.stringify(summary), "utf8") > MAX_SUMMARY_BYTES &&
      (summary.messages.length > 0 ||
        summary.openWork.length > 0 ||
        summary.plans.length > 0 ||
        summary.integrations.length > 0 ||
        summary.links.length > 0)
    ) {
      if (summary.messages.length > 0) summary.messages.pop();
      else if (summary.openWork.length > 0) summary.openWork.pop();
      else if (summary.plans.length > 0) summary.plans.pop();
      else if (summary.integrations.length > 0) summary.integrations.pop();
      else summary.links.pop();
      summary.truncated = true;
    }
    return summary;
  }

  /**
   * A PM restart in one transaction: the summary is built from the ledger
   * first (the replace cancels the PM's open messages), recorded, and then the
   * generation is replaced. The stored result carries the new credential, so
   * it is encrypted like every other credential-returning result.
   */
  restartAgentGeneration(
    context: MutationContext,
    agentId: string,
  ): ReplacedAgent & {
    readonly restartId: string;
    readonly summary: PmRestartSummary;
  } {
    safeId(agentId, "agent id");
    return this.kernel.mutate(
      context,
      "agent.restart",
      "actor:manage",
      { agentId },
      (actor) => {
        const agent = this.kernel.agentRow(agentId);
        if (agent?.state !== "active")
          throw new ControllerError("agent is not active");
        if (agent.kind !== "PM")
          throw new ControllerError("only a PM is restarted with a summary");
        const carried = (
          this.kernel.database
            .prepare(
              "SELECT summary_json FROM pm_restarts WHERE project_id = ? AND agent_id = ? AND consumed = 0 ORDER BY sequence",
            )
            .all(this.kernel.projectId, agentId) as Array<{
            summary_json: string;
          }>
        ).map((row) => JSON.parse(row.summary_json) as PmRestartSummary);
        const summary = this.#restartSummary(
          agentId,
          agent.generation,
          carried,
        );
        const summaryJson = JSON.stringify(summary);
        const summaryHash = sha256(summaryJson);
        const restartId = randomUUID();
        const sequence =
          (
            this.kernel.database
              .prepare(
                "SELECT COALESCE(MAX(sequence), 0) AS latest FROM pm_restarts WHERE project_id = ? AND agent_id = ?",
              )
              .get(this.kernel.projectId, agentId) as { latest: number }
          ).latest + 1;
        // The replace can refuse (the seat still holds authority); it runs
        // before the row is written so a refusal leaves no attempt behind.
        const replaced = this.#replaceGeneration(actor, context, agentId);
        this.kernel.database
          .prepare(
            `INSERT INTO pm_restarts(project_id, restart_id, agent_id, sequence, summarized_generation, summary_json, summary_hash, consumed, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?)`,
          )
          .run(
            this.kernel.projectId,
            restartId,
            agentId,
            sequence,
            agent.generation,
            summaryJson,
            summaryHash,
            this.kernel.now(),
          );
        return {
          value: { ...replaced.value, restartId, summary },
          event: {
            ...replaced.event,
            details: {
              ...(replaced.event.details as Record<string, unknown>),
              restartId,
              summaryHash,
            },
          },
        };
      },
    );
  }

  /** Marks every restart summary of an agent up to a sequence as used by a started PM. */
  markPmRestartsConsumed(
    context: MutationContext,
    agentId: string,
    upToSequence: number,
  ): { readonly marked: number } {
    safeId(agentId, "agent id");
    if (!Number.isSafeInteger(upToSequence) || upToSequence < 1)
      throw new TypeError("sequence must be a positive integer");
    return this.kernel.mutate(
      context,
      "pm_restart.consume",
      "controller:reconcile",
      { agentId, upToSequence },
      () => {
        const result = this.kernel.database
          .prepare(
            "UPDATE pm_restarts SET consumed = 1 WHERE project_id = ? AND agent_id = ? AND consumed = 0 AND sequence <= ?",
          )
          .run(this.kernel.projectId, agentId, upToSequence);
        return {
          value: { marked: result.changes },
          event: {
            entityType: "pm_restart",
            entityId: agentId,
            stateVersion: upToSequence,
            details: { marked: result.changes },
          },
        };
      },
    );
  }

  pmRestarts(
    credential: string,
    agentId: string,
  ): readonly {
    readonly restartId: string;
    readonly sequence: number;
    readonly summarizedGeneration: number;
    readonly consumed: boolean;
    readonly summaryHash: string;
    readonly summary: PmRestartSummary;
  }[] {
    this.kernel.authorize(credential, "controller:reconcile");
    safeId(agentId, "agent id");
    return (
      this.kernel.database
        .prepare(
          "SELECT restart_id, sequence, summarized_generation, consumed, summary_hash, summary_json FROM pm_restarts WHERE project_id = ? AND agent_id = ? ORDER BY sequence",
        )
        .all(this.kernel.projectId, agentId) as Array<{
        restart_id: string;
        sequence: number;
        summarized_generation: number;
        consumed: number;
        summary_hash: string;
        summary_json: string;
      }>
    ).map((row) => ({
      restartId: row.restart_id,
      sequence: row.sequence,
      summarizedGeneration: row.summarized_generation,
      consumed: row.consumed === 1,
      summaryHash: row.summary_hash,
      summary: JSON.parse(row.summary_json) as PmRestartSummary,
    }));
  }
}
