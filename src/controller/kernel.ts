import { randomUUID } from "node:crypto";
import type { Database } from "./sqlite.js";
import {
  authenticateActor,
  requireCapability,
  type AuthenticatedActor,
} from "./auth.js";
import { canonicalJson, digestJson } from "./canonical.js";
import { ProjectLock } from "./ownership.js";
import {
  ControllerError,
  IdempotencyConflictError,
  InputRevisionConflictError,
  StateVersionConflictError,
  TransitionAuthorizationError,
} from "./errors.js";
import {
  decryptActorResult,
  encryptActorResult,
  returnsCredential,
} from "./helpers.js";
import type { AgentRow, MutationOutput } from "./records.js";
import type { AgentRecord, Capability, MutationContext } from "./types.js";

/**
 * The state every area of the controller shares: the database, the project identity, the clock, the lock
 * and the one transaction wrapper (`mutate`) through which every state change is written and ledgered.
 */
function toAgentRecord(row: AgentRow): AgentRecord {
  return {
    agentId: row.agent_id,
    roleName: row.role_name,
    kind: row.kind,
    seatId: row.seat_id,
    actorId: row.actor_id,
    generation: row.generation,
    state: row.state,
    lastActivityAt: row.last_activity_at,
  };
}

export class ControllerKernel {
  closed = false;

  constructor(
    readonly database: Database,
    readonly lock: ProjectLock | undefined,
    readonly projectId: string,
    readonly internalActorId: string,
    readonly readOnly: boolean,
    readonly clock: () => Date,
  ) {}

  now(): string {
    return this.clock().toISOString();
  }

  assertOpen(): void {
    if (this.closed) throw new ControllerError("controller is closed");
    this.lock?.assertHeld();
  }

  assertWritable(): void {
    if (this.readOnly)
      throw new ControllerError("controller is open read-only");
  }

  agentRow(agentId: string): AgentRow | undefined {
    return this.database
      .prepare("SELECT * FROM agents WHERE project_id = ? AND agent_id = ?")
      .get(this.projectId, agentId) as AgentRow | undefined;
  }

  agentByActor(actorId: string): AgentRow | undefined {
    return this.database
      .prepare(
        "SELECT * FROM agents WHERE project_id = ? AND actor_id = ? AND state = 'active'",
      )
      .get(this.projectId, actorId) as AgentRow | undefined;
  }

  mutate<T>(
    context: MutationContext,
    action: string,
    capability: Capability,
    payload: unknown,
    apply: (actor: AuthenticatedActor) => MutationOutput<T>,
  ): T {
    this.assertOpen();
    this.assertWritable();
    if (
      !/^[A-Za-z0-9._:-]{1,128}$/.test(context.requestId) ||
      !/^[A-Za-z0-9._:-]{1,128}$/.test(context.idempotencyKey)
    ) {
      throw new TypeError(
        "request and idempotency identifiers must be 1-128 safe ASCII characters",
      );
    }
    if (
      !Number.isInteger(context.expectedVersion) ||
      context.expectedVersion < 1 ||
      !Number.isInteger(context.inputRevision) ||
      context.inputRevision < 1
    ) {
      throw new TypeError(
        "expected state version and input revision must be positive integers",
      );
    }
    this.database.exec("BEGIN IMMEDIATE");
    try {
      const actor = authenticateActor(
        this.database,
        this.projectId,
        context.credential,
      );
      requireCapability(actor, capability);
      const requestHash = digestJson({
        action,
        actorId: actor.actorId,
        projectId: this.projectId,
        requestId: context.requestId,
        expectedVersion: context.expectedVersion,
        inputRevision: context.inputRevision,
        payload,
      });
      const existing = this.database
        .prepare(
          `
        SELECT actor_id, request_hash, result_json FROM mutation_requests WHERE project_id = ? AND idempotency_key = ?
      `,
        )
        .get(this.projectId, context.idempotencyKey) as
        | { actor_id: string; request_hash: string; result_json: string }
        | undefined;
      if (existing) {
        if (
          existing.actor_id !== actor.actorId ||
          existing.request_hash !== requestHash
        ) {
          throw new IdempotencyConflictError(
            "idempotency key was reused with different request content or actor",
          );
        }
        const value = (
          returnsCredential(action)
            ? decryptActorResult(
                existing.result_json,
                context.credential,
                this.projectId,
                requestHash,
              )
            : JSON.parse(existing.result_json)
        ) as T;
        this.database.exec("COMMIT");
        return value;
      }
      const requestReuse = this.database
        .prepare(
          "SELECT idempotency_key FROM mutation_requests WHERE project_id = ? AND request_id = ?",
        )
        .get(this.projectId, context.requestId) as
        { idempotency_key: string } | undefined;
      if (requestReuse)
        throw new IdempotencyConflictError(
          "request id was already used by another mutation",
        );
      const project = this.database
        .prepare(
          "SELECT state_version, current_input_revision FROM projects WHERE project_id = ?",
        )
        .get(this.projectId) as {
        state_version: number;
        current_input_revision: number;
      };
      if (project.state_version !== context.expectedVersion) {
        throw new StateVersionConflictError(
          `expected project version ${context.expectedVersion}, found ${project.state_version}`,
        );
      }
      if (project.current_input_revision !== context.inputRevision) {
        throw new InputRevisionConflictError(
          `expected input revision ${context.inputRevision}, found ${project.current_input_revision}`,
        );
      }
      const output = apply(actor);
      const nextVersion = project.state_version + 1;
      this.database
        .prepare(
          "UPDATE projects SET state_version = ? WHERE project_id = ? AND state_version = ?",
        )
        .run(nextVersion, this.projectId, project.state_version);
      const now = new Date().toISOString();
      let eventSequence = (
        this.database
          .prepare(
            "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM controller_events WHERE project_id = ?",
          )
          .get(this.projectId) as { next: number }
      ).next;
      const insertEvent = this.database.prepare(
        `
        INSERT INTO controller_events(project_id, sequence, event_id, entity_type, entity_id, from_state, to_state,
          state_version, actor_id, request_id, input_revision, payload_json, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
      `,
      );
      for (const event of [output.event, ...(output.extraEvents ?? [])]) {
        insertEvent.run(
          this.projectId,
          eventSequence,
          randomUUID(),
          event.entityType,
          event.entityId,
          event.fromState ?? null,
          event.toState ?? null,
          nextVersion,
          actor.actorId,
          context.requestId,
          context.inputRevision,
          canonicalJson({
            action,
            payload,
            entityVersion: event.stateVersion,
            details: event.details ?? null,
          }),
          now,
        );
        eventSequence += 1;
      }
      this.database
        .prepare(
          `
        INSERT INTO mutation_requests(project_id, idempotency_key, request_id, actor_id, request_hash, result_json, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?)
      `,
        )
        .run(
          this.projectId,
          context.idempotencyKey,
          context.requestId,
          actor.actorId,
          requestHash,
          returnsCredential(action)
            ? encryptActorResult(
                output.value,
                context.credential,
                this.projectId,
                requestHash,
              )
            : canonicalJson(output.value),
          now,
        );
      this.database.exec("COMMIT");
      return output.value;
    } catch (error) {
      this.database.exec("ROLLBACK");
      throw error;
    }
  }

  get stateVersion(): number {
    this.assertOpen();
    const row = this.database
      .prepare("SELECT state_version FROM projects WHERE project_id = ?")
      .get(this.projectId) as { state_version: number };
    return row.state_version;
  }

  get inputRevision(): number {
    this.assertOpen();
    const row = this.database
      .prepare(
        "SELECT current_input_revision FROM projects WHERE project_id = ?",
      )
      .get(this.projectId) as { current_input_revision: number };
    return row.current_input_revision;
  }

  authorize(credential: string, capability: Capability): AuthenticatedActor {
    this.assertOpen();
    this.assertWritable();
    const actor = authenticateActor(this.database, this.projectId, credential);
    requireCapability(actor, capability);
    return actor;
  }

  hasStoredRequest(context: MutationContext): boolean {
    return (
      this.database
        .prepare(
          "SELECT 1 FROM mutation_requests WHERE project_id = ? AND idempotency_key = ?",
        )
        .get(this.projectId, context.idempotencyKey) !== undefined
    );
  }

  internalPrincipal(): AuthenticatedActor {
    const rows = this.database
      .prepare(
        `
      SELECT a.actor_id, a.project_id, a.role, cg.capability
      FROM actors a JOIN capability_grants cg
        ON cg.project_id = a.project_id AND cg.actor_id = a.actor_id AND cg.revoked_at IS NULL
      WHERE a.project_id = ? AND a.actor_id = ? AND a.active = 1 AND a.revoked_at IS NULL
      ORDER BY cg.capability
    `,
      )
      .all(this.projectId, this.internalActorId) as Array<{
      actor_id: string;
      project_id: string;
      role: string;
      capability: string;
    }>;
    const actor = rows[0];
    if (!actor || actor.role !== "controller")
      throw new ControllerError("internal controller principal is not active");
    return {
      actorId: actor.actor_id,
      projectId: actor.project_id,
      role: "controller",
      capabilities: new Set(rows.map((row) => row.capability)),
    };
  }

  mutateAsController<T>(
    context: MutationContext,
    action: string,
    capability: Capability,
    payload: unknown,
    apply: (
      actor: AuthenticatedActor,
      caller: AuthenticatedActor,
    ) => MutationOutput<T>,
  ): T {
    return this.mutate(context, action, capability, payload, (caller) => {
      if (caller.role !== "operator")
        throw new TransitionAuthorizationError(
          "controller operations require an authenticated operator request",
        );
      return apply(this.internalPrincipal(), caller);
    });
  }

  /** An event row written inside a mutation that already returns its own event. */
  appendEvent(
    actor: AuthenticatedActor,
    context: MutationContext,
    event: {
      readonly entityType: string;
      readonly entityId: string;
      readonly from: string;
      readonly to: string;
      readonly action: string;
      readonly stateVersion: number;
      readonly details: unknown;
    },
  ): void {
    const project = this.database
      .prepare("SELECT state_version FROM projects WHERE project_id = ?")
      .get(this.projectId) as { state_version: number };
    const sequence = (
      this.database
        .prepare(
          "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM controller_events WHERE project_id = ?",
        )
        .get(this.projectId) as { next: number }
    ).next;
    this.database
      .prepare(
        `INSERT INTO controller_events(project_id, sequence, event_id, entity_type, entity_id, from_state, to_state,
          state_version, actor_id, request_id, input_revision, payload_json, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
      )
      .run(
        this.projectId,
        sequence,
        randomUUID(),
        event.entityType,
        event.entityId,
        event.from,
        event.to,
        project.state_version + 1,
        actor.actorId,
        context.requestId,
        context.inputRevision,
        canonicalJson({
          action: event.action,
          payload: { batch: true },
          entityVersion: event.stateVersion,
          details: event.details,
        }),
        new Date().toISOString(),
      );
  }

  controllerActorId(): string {
    const controller = this.database
      .prepare(
        "SELECT actor_id FROM actors WHERE project_id = ? AND is_internal = 1 AND role = 'controller' AND active = 1 AND revoked_at IS NULL",
      )
      .get(this.projectId) as { actor_id: string } | undefined;
    if (controller === undefined)
      throw new ControllerError("the controller actor is missing");
    return controller.actor_id;
  }

  isTransitionAllowed(
    entityType: string,
    fromState: string,
    toState: string,
    actor: AuthenticatedActor,
  ): boolean {
    const rules = this.database
      .prepare(
        `
      SELECT capability FROM transition_rules
      WHERE entity_type = ? AND from_state = ? AND to_state = ? AND role = ?
    `,
      )
      .all(entityType, fromState, toState, actor.role) as Array<{
      capability: string;
    }>;
    return rules.some((rule) => actor.capabilities.has(rule.capability));
  }

  // ------------------------------------------------------------ operator

  agentRecord(agentId: string): AgentRecord | undefined {
    const row = this.agentRow(agentId);
    return row === undefined ? undefined : toAgentRecord(row);
  }

  /** Every agent of the project in one query, ordered by agent id; with `activeOnly`, only those whose state is active. */
  agentRecords(activeOnly = false): AgentRecord[] {
    return (
      this.database
        .prepare(
          `SELECT agent_id, role_name, kind, seat_id, actor_id, generation, state, last_activity_at
           FROM agents WHERE project_id = ?${activeOnly ? " AND state = 'active'" : ""} ORDER BY agent_id`,
        )
        .all(this.projectId) as AgentRow[]
    ).map(toAgentRecord);
  }

  agentByActorRecord(actorId: string): AgentRecord | null {
    const row = this.agentByActor(actorId);
    return row === undefined ? null : (this.agentRecord(row.agent_id) ?? null);
  }

  touchAgent(agentId: string, now: string): void {
    this.database
      .prepare(
        "UPDATE agents SET last_activity_at = ? WHERE project_id = ? AND agent_id = ?",
      )
      .run(now, this.projectId, agentId);
  }
}
