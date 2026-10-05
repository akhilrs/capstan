import type { ControllerKernel } from "./kernel.js";
import type { ControllerAreas } from "./areas.js";
import { randomUUID } from "node:crypto";
import { type AuthenticatedActor } from "./auth.js";
import { canonicalJson } from "./canonical.js";
import {
  type MutationContext,
  type PauseRecord,
  type RunState,
} from "./types.js";
import {
  ControllerError,
  MutationConflictError,
  RunPausedError,
  TransitionAuthorizationError,
} from "./errors.js";
import { safeId, pauseReasonText } from "./helpers.js";

export class PausesArea {
  constructor(
    readonly kernel: ControllerKernel,
    readonly areas: ControllerAreas,
  ) {}

  transitionRun(
    context: MutationContext,
    toState: RunState,
    reason?: string,
  ): { readonly state: RunState } {
    const pauseReason =
      reason === undefined
        ? toState === "paused"
          ? "no reason given"
          : `run ${toState}`
        : pauseReasonText(reason);
    const apply = (actor: AuthenticatedActor, caller: AuthenticatedActor) => {
      const run = this.kernel.database
        .prepare(
          "SELECT state, state_version FROM run_controls WHERE project_id = ?",
        )
        .get(this.kernel.projectId) as
        { state: string; state_version: number } | undefined;
      if (!run) throw new ControllerError("run control record is missing");
      if (toState === "paused" && run.state === "paused")
        throw new MutationConflictError(
          `run is already paused: ${this.#openRunPause()?.reason ?? "no reason recorded"}`,
        );
      if (toState === "active" && run.state === "active")
        throw new MutationConflictError("run is not paused");
      if (toState === "completed") {
        const unresolvedFinding = this.kernel.database
          .prepare(
            "SELECT 1 AS present FROM findings WHERE project_id = ? AND state <> 'resolved' LIMIT 1",
          )
          .get(this.kernel.projectId);
        if (unresolvedFinding)
          throw new MutationConflictError(
            "run cannot complete with unresolved Supervisor findings",
          );
        const unfinishedWork = this.kernel.database
          .prepare(
            "SELECT 1 AS present FROM work_items WHERE project_id = ? AND state NOT IN ('accepted', 'canceled') LIMIT 1",
          )
          .get(this.kernel.projectId);
        const uncertainAuthority = this.kernel.database
          .prepare(
            "SELECT 1 AS present FROM assignments WHERE project_id = ? AND authority_state IN ('active', 'unknown') LIMIT 1",
          )
          .get(this.kernel.projectId);
        if (unfinishedWork || uncertainAuthority)
          throw new MutationConflictError(
            "run cannot complete while work is open or assignment authority is not contained",
          );
        const staleAcceptedWork = this.kernel.database
          .prepare(
            "SELECT 1 AS present FROM work_items WHERE project_id = ? AND state = 'accepted' AND input_revision <> ? LIMIT 1",
          )
          .get(this.kernel.projectId, context.inputRevision);
        if (staleAcceptedWork)
          throw new MutationConflictError(
            "run cannot complete with work accepted against a stale input revision",
          );
      }
      if (
        !this.kernel.isTransitionAllowed(
          "run_control",
          run.state,
          toState,
          actor,
        )
      ) {
        throw new TransitionAuthorizationError(
          `transition table rejects run state ${run.state} -> ${toState}`,
        );
      }
      const now = new Date().toISOString();
      this.kernel.database
        .prepare(
          "UPDATE run_controls SET state = ?, state_version = state_version + 1, updated_at = ? WHERE project_id = ? AND state = ?",
        )
        .run(toState, now, this.kernel.projectId, run.state);
      if (caller.role === "operator") {
        this.kernel.database
          .prepare(
            `
          INSERT INTO operator_actions(project_id, action_id, action_type, target_id, actor_id, request_id,
            input_revision, payload_json, created_at)
          VALUES (?, ?, 'run.transition', ?, ?, ?, ?, ?, ?)
        `,
          )
          .run(
            this.kernel.projectId,
            randomUUID(),
            this.kernel.projectId,
            caller.actorId,
            context.requestId,
            context.inputRevision,
            canonicalJson(
              reason === undefined
                ? { from: run.state, to: toState }
                : { from: run.state, to: toState, reason: pauseReason },
            ),
            now,
          );
      }
      if (toState === "paused")
        this.#openPause("run", null, pauseReason, caller.actorId, now);
      else if (run.state === "paused")
        this.closePause("run", null, pauseReason, caller.actorId, now);
      if (caller.role === "operator" && toState === "paused")
        this.areas.messageNotices.noticeToPm(
          `The operator paused the run: ${pauseReason}. Workers are held and spawn, plan assign, request-review and integrate are refused until \`cstan resume --reason "<text>"\`. You keep receiving messages.`,
          now,
        );
      else if (
        caller.role === "operator" &&
        run.state === "paused" &&
        toState === "active"
      )
        this.areas.messageNotices.noticeToPm(
          `The operator resumed the run: ${pauseReason}. Held messages are delivered in order.`,
          now,
        );
      return {
        value: { state: toState },
        event: {
          entityType: "run_control",
          entityId: this.kernel.projectId,
          stateVersion: run.state_version + 1,
          fromState: run.state,
          toState,
          ...(reason === undefined ? {} : { details: { reason: pauseReason } }),
        },
      };
    };
    if (
      toState === "canceled" ||
      toState === "completed" ||
      toState === "failed"
    ) {
      return this.kernel.mutateAsController(
        context,
        "run.transition",
        "run:control",
        { toState },
        apply,
      );
    }
    return this.kernel.mutate(
      context,
      "run.transition",
      "run:control",
      { toState },
      (actor) => apply(actor, actor),
    );
  }

  /** The open pause row of the run, or undefined. */
  #openRunPause(): PauseRecord | undefined {
    return this.pauseRows().find((pause) => pause.scope === "run");
  }

  pauseRows(): PauseRecord[] {
    return (
      this.kernel.database
        .prepare(
          "SELECT scope, agent_id, reason, actor_id, paused_at FROM pauses WHERE project_id = ? AND resumed_at IS NULL ORDER BY paused_at, pause_id",
        )
        .all(this.kernel.projectId) as Array<{
        scope: "run" | "agent";
        agent_id: string | null;
        reason: string;
        actor_id: string;
        paused_at: string;
      }>
    ).map((row) => ({
      scope: row.scope,
      agentId: row.agent_id,
      reason: row.reason,
      actorId: row.actor_id,
      pausedAt: row.paused_at,
    }));
  }

  #openPause(
    scope: "run" | "agent",
    agentId: string | null,
    reason: string,
    actorId: string,
    now: string,
  ): void {
    this.kernel.database
      .prepare(
        "INSERT INTO pauses(project_id, pause_id, scope, agent_id, reason, actor_id, paused_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
      )
      .run(
        this.kernel.projectId,
        randomUUID(),
        scope,
        agentId,
        reason,
        actorId,
        now,
      );
  }

  closePause(
    scope: "run" | "agent",
    agentId: string | null,
    reason: string,
    actorId: string,
    now: string,
  ): void {
    this.kernel.database
      .prepare(
        `UPDATE pauses SET resumed_at = ?, resume_reason = ?, resumed_by = ?
         WHERE project_id = ? AND scope = ? AND COALESCE(agent_id, '') = ? AND resumed_at IS NULL`,
      )
      .run(now, reason, actorId, this.kernel.projectId, scope, agentId ?? "");
  }

  /** What is paused now: the run (while its state is paused) and each paused agent. */
  pauseState(): {
    readonly run: PauseRecord | null;
    readonly agents: readonly PauseRecord[];
  } {
    this.kernel.assertOpen();
    const rows = this.pauseRows();
    const runState = (
      this.kernel.database
        .prepare("SELECT state FROM run_controls WHERE project_id = ?")
        .get(this.kernel.projectId) as { state: string } | undefined
    )?.state;
    return {
      run:
        runState === "paused"
          ? (rows.find((pause) => pause.scope === "run") ?? null)
          : null,
      agents: rows.filter((pause) => pause.scope === "agent"),
    };
  }

  /** True when messages to the agent are held: it is paused, or the run is paused and the agent is not the PM (which coordinates and must see notices). */
  isDeliveryPaused(agentId: string): boolean {
    this.kernel.assertOpen();
    const state = this.pauseState();
    if (state.agents.some((pause) => pause.agentId === agentId)) return true;
    if (state.run === null) return false;
    return this.kernel.agentRow(agentId)?.kind !== "PM";
  }

  /** Refuses a step that must not start while the run is paused. */
  assertRunNotPaused(action: string): void {
    this.kernel.assertOpen();
    const run = this.pauseState().run;
    if (run !== null)
      throw new RunPausedError(
        `run_paused: ${action} is refused while the run is paused: ${run.reason}`,
      );
  }

  /** The latest time a pause that covers the agent ended, in ms; 0 when none did. Timers restart from here. */
  resumedMsFor(agentId: string): number {
    const row = this.kernel.database
      .prepare(
        "SELECT MAX(resumed_at) AS at FROM pauses WHERE project_id = ? AND resumed_at IS NOT NULL AND (scope = 'run' OR agent_id = ?)",
      )
      .get(this.kernel.projectId, agentId) as { at: string | null };
    return row.at === null ? 0 : Date.parse(row.at);
  }

  pauseAgent(
    context: MutationContext,
    input: { readonly agentId: string; readonly reason: string },
  ): PauseRecord {
    return this.#agentPauseChange(context, input, true);
  }

  resumeAgent(
    context: MutationContext,
    input: { readonly agentId: string; readonly reason: string },
  ): PauseRecord {
    return this.#agentPauseChange(context, input, false);
  }

  #agentPauseChange(
    context: MutationContext,
    input: { readonly agentId: string; readonly reason: string },
    pausing: boolean,
  ): PauseRecord {
    safeId(input.agentId, "agent id");
    const reason = pauseReasonText(input.reason);
    return this.kernel.mutate<PauseRecord>(
      context,
      pausing ? "agent.pause" : "agent.resume",
      "run:control",
      { agentId: input.agentId, reason },
      (actor) => {
        const caller = this.kernel.agentByActor(actor.actorId);
        if (actor.role !== "operator" && caller?.kind !== "PM")
          throw new ControllerError(
            "only the operator or the active PM pauses or resumes an agent",
          );
        const target = this.kernel.agentRow(input.agentId);
        if (target?.state !== "active")
          throw new ControllerError("the agent is not active");
        if (
          pausing &&
          caller !== undefined &&
          caller.agent_id === target.agent_id
        )
          throw new ControllerError("an agent cannot pause itself");
        const open = this.pauseRows().find(
          (pause) =>
            pause.scope === "agent" && pause.agentId === target.agent_id,
        );
        const now = this.kernel.now();
        if (pausing) {
          if (open !== undefined)
            throw new MutationConflictError(
              `agent ${target.agent_id} is already paused: ${open.reason}`,
            );
          this.#openPause("agent", target.agent_id, reason, actor.actorId, now);
          if (actor.role === "operator")
            this.areas.messageNotices.noticeToPm(
              `The operator paused ${target.agent_id}: ${reason}. Its messages are held until \`cstan resume ${target.agent_id} --reason "<text>"\`.`,
              now,
            );
        } else {
          if (open === undefined)
            throw new MutationConflictError(
              `agent ${target.agent_id} is not paused`,
            );
          this.closePause("agent", target.agent_id, reason, actor.actorId, now);
          if (actor.role === "operator")
            this.areas.messageNotices.noticeToPm(
              `The operator resumed ${target.agent_id}: ${reason}. Its held messages are delivered in order.`,
              now,
            );
        }
        const record: PauseRecord = open ?? {
          scope: "agent",
          agentId: target.agent_id,
          reason,
          actorId: actor.actorId,
          pausedAt: now,
        };
        return {
          value: record,
          event: {
            entityType: "agent",
            entityId: target.agent_id,
            stateVersion: target.generation,
            fromState: pausing ? "active" : "paused",
            toState: pausing ? "paused" : "active",
            details: { reason, scope: "agent" },
          },
        };
      },
    );
  }
}
