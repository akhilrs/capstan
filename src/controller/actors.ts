import type { ControllerKernel } from "./kernel.js";
import type { ControllerAreas } from "./areas.js";
import { randomUUID } from "node:crypto";
import {
  authenticateActor,
  credentialHash,
  issueCredential,
  requireCapability,
  type AuthenticatedActor,
} from "./auth.js";
import { sha256 } from "./canonical.js";
import { isFinalState } from "./messaging.js";
import { packageOfBody } from "../plans.js";
import {
  type MutationContext,
  type RoleDefinition,
  type RoleDefinitionInput,
  type RoleSyncResult,
  type SeatInput,
} from "./types.js";
import { ControllerError, MutationConflictError } from "./errors.js";
import {
  ROLE_NAME_PATTERN,
  ROLE_KINDS,
  type AgentReportRow,
  type AgentFindingRow,
  LOST_EVENT_IDS,
  SEED_MESSAGES,
  SEED_REPORTS,
  SEED_FINDINGS,
  type AgentSeedData,
  type AgentRow,
  type ActorInput,
} from "./records.js";
import { safeId, lostNotice } from "./helpers.js";

export class ActorsArea {
  constructor(
    readonly kernel: ControllerKernel,
    readonly areas: ControllerAreas,
  ) {}

  createActor(
    context: MutationContext,
    input: ActorInput,
  ): { readonly actorId: string; readonly credential: string } {
    input = Object.freeze({ ...input });
    const mutationPayload = {
      displayName: input.displayName,
      role: input.role,
      seatId: input.seatId ?? null,
    };
    return this.kernel.mutate(
      context,
      "actor.create",
      "actor:manage",
      mutationPayload,
      (actor) => {
        const actorId = randomUUID();
        const credential = issueCredential();
        const hash = credentialHash(credential);
        let seatId: string | null = null;
        if (
          input.role === "PM" ||
          input.role === "Developer" ||
          input.role === "Verifier" ||
          input.role === "Supervisor"
        ) {
          if (!input.seatId)
            throw new ControllerError("role-scoped actors require a seat");
          const seat = this.kernel.database
            .prepare(
              "SELECT role, state FROM seats WHERE project_id = ? AND seat_id = ?",
            )
            .get(this.kernel.projectId, input.seatId) as
            { role: string; state: string } | undefined;
          if (!seat || seat.role !== input.role || seat.state !== "active")
            throw new ControllerError(
              "actor seat must be active and match its role",
            );
          seatId = input.seatId;
          const activeActor = this.kernel.database
            .prepare(
              "SELECT 1 AS present FROM actors WHERE project_id = ? AND seat_id = ? AND active = 1 AND revoked_at IS NULL LIMIT 1",
            )
            .get(this.kernel.projectId, input.seatId);
          if (activeActor)
            throw new MutationConflictError(
              "seat already has an active actor; revoke it before issuing a replacement",
            );
        } else if (input.seatId) {
          throw new ControllerError(
            "operator and controller actors cannot be attached to a worker seat",
          );
        }
        const now = new Date().toISOString();
        this.insertActor(actor.actorId, {
          actorId,
          displayName: input.displayName,
          role: input.role,
          seatId,
          credentialHash: hash,
          now,
        });
        return {
          value: { actorId, credential },
          event: { entityType: "actor", entityId: actorId, stateVersion: 0 },
        };
      },
    );
  }

  revokeActor(
    context: MutationContext,
    actorId: string,
  ): { readonly revoked: true } {
    return this.kernel.mutate(
      context,
      "actor.revoke",
      "actor:manage",
      { actorId },
      () => {
        const target = this.kernel.database
          .prepare(
            "SELECT role, is_internal, active, seat_id FROM actors WHERE project_id = ? AND actor_id = ?",
          )
          .get(this.kernel.projectId, actorId) as
          | {
              role: string;
              is_internal: number;
              active: number;
              seat_id: string | null;
            }
          | undefined;
        if (!target || target.is_internal || target.role === "operator")
          throw new ControllerError(
            "the internal controller and project operator cannot be revoked through this operation",
          );
        if (!target.active)
          return {
            value: { revoked: true },
            event: { entityType: "actor", entityId: actorId, stateVersion: 0 },
          };
        if (target.seat_id) {
          const otherActiveActor = this.kernel.database
            .prepare(
              `
            SELECT 1 AS present FROM actors
            WHERE project_id = ? AND seat_id = ? AND actor_id <> ?
              AND active = 1 AND revoked_at IS NULL LIMIT 1
          `,
            )
            .get(this.kernel.projectId, target.seat_id, actorId);
          const activeAuthority = this.kernel.database
            .prepare(
              `
            SELECT 1 AS present FROM assignments
            WHERE project_id = ? AND seat_id = ?
              AND authority_state IN ('active', 'unknown') LIMIT 1
          `,
            )
            .get(this.kernel.projectId, target.seat_id);
          if (!otherActiveActor && activeAuthority)
            throw new MutationConflictError(
              "cannot revoke the last active actor for a seat with active or uncertain assignments",
            );
        }
        const now = new Date().toISOString();
        this.kernel.database
          .prepare(
            "UPDATE actors SET active = 0, revoked_at = ? WHERE project_id = ? AND actor_id = ?",
          )
          .run(now, this.kernel.projectId, actorId);
        this.kernel.database
          .prepare(
            "UPDATE capability_grants SET revoked_at = ? WHERE project_id = ? AND actor_id = ? AND revoked_at IS NULL",
          )
          .run(now, this.kernel.projectId, actorId);
        return {
          value: { revoked: true },
          event: {
            entityType: "actor",
            entityId: actorId,
            stateVersion: 1,
            fromState: "active",
            toState: "revoked",
          },
        };
      },
    );
  }

  createSeat(
    context: MutationContext,
    input: SeatInput,
  ): { readonly seatId: string } {
    return this.kernel.mutate(
      context,
      "seat.create",
      "work:write",
      input,
      () => {
        this.kernel.database
          .prepare(
            `
        INSERT INTO seats(project_id, seat_id, name, role, state, state_version, created_at)
        VALUES (?, ?, ?, ?, 'active', 0, ?)
      `,
          )
          .run(
            this.kernel.projectId,
            input.seatId,
            input.name,
            input.role,
            new Date().toISOString(),
          );
        return {
          value: { seatId: input.seatId },
          event: {
            entityType: "seat",
            entityId: input.seatId,
            stateVersion: 0,
          },
        };
      },
    );
  }

  roleDefinitions(): readonly RoleDefinition[] {
    this.kernel.assertOpen();
    return (
      this.kernel.database
        .prepare(
          "SELECT role_name, kind, host, config_hash, state FROM role_definitions WHERE project_id = ? ORDER BY role_name",
        )
        .all(this.kernel.projectId) as Array<{
        role_name: string;
        kind: RoleDefinition["kind"];
        host: string;
        config_hash: string;
        state: RoleDefinition["state"];
      }>
    ).map((row) => ({
      name: row.role_name,
      kind: row.kind,
      host: row.host,
      configHash: row.config_hash,
      state: row.state,
    }));
  }

  roleKind(roleName: string): RoleDefinition["kind"] {
    const definition = this.roleDefinitions().find(
      (candidate) => candidate.name === roleName,
    );
    if (definition?.state !== "active")
      throw new ControllerError(
        "role is not an active configured role definition",
      );
    return definition.kind;
  }

  syncRoleDefinitions(
    context: MutationContext,
    desired: readonly RoleDefinitionInput[],
  ): RoleSyncResult {
    const sorted = desired
      .map((role) => {
        if (
          typeof role.name !== "string" ||
          !ROLE_NAME_PATTERN.test(role.name) ||
          !ROLE_KINDS.includes(role.kind) ||
          typeof role.host !== "string" ||
          !ROLE_NAME_PATTERN.test(role.host) ||
          typeof role.configHash !== "string" ||
          !/^[0-9a-f]{64}$/.test(role.configHash)
        )
          throw new TypeError(
            "role definition needs a lowercase name and host, a role kind and a SHA-256 hex config hash",
          );
        return {
          name: role.name,
          kind: role.kind,
          host: role.host,
          configHash: role.configHash,
        };
      })
      .sort((left, right) =>
        left.name < right.name ? -1 : left.name > right.name ? 1 : 0,
      );
    if (new Set(sorted.map((role) => role.name)).size !== sorted.length)
      throw new TypeError("role names must be unique");
    this.kernel.assertOpen();
    this.kernel.assertWritable();
    requireCapability(
      authenticateActor(
        this.kernel.database,
        this.kernel.projectId,
        context.credential,
      ),
      "actor:manage",
    );
    const isReplay = this.kernel.hasStoredRequest(context);
    const planned = isReplay ? undefined : this.#roleDifference(sorted);
    if (planned !== undefined && !planned.changed) return planned;
    // Every write to role_definitions and seats must bump projects.state_version:
    // the caller creates the context before this pre-check, so a matching
    // expected version proves the rows read above are still current.
    return this.kernel.mutate(
      context,
      "role.sync",
      "actor:manage",
      { roles: sorted },
      () => {
        const difference = planned ?? this.#roleDifference(sorted);
        const now = new Date().toISOString();
        const insert = this.kernel.database.prepare(
          "INSERT INTO role_definitions(project_id, role_name, kind, host, config_hash, state, created_at, updated_at) VALUES (?, ?, ?, ?, ?, 'active', ?, ?)",
        );
        const update = this.kernel.database.prepare(
          "UPDATE role_definitions SET kind = ?, host = ?, config_hash = ?, state = 'active', updated_at = ? WHERE project_id = ? AND role_name = ?",
        );
        const retire = this.kernel.database.prepare(
          "UPDATE role_definitions SET state = 'retired', updated_at = ? WHERE project_id = ? AND role_name = ?",
        );
        const byName = new Map(sorted.map((role) => [role.name, role]));
        for (const name of difference.inserted) {
          const role = byName.get(name)!;
          insert.run(
            this.kernel.projectId,
            name,
            role.kind,
            role.host,
            role.configHash,
            now,
            now,
          );
        }
        for (const name of [...difference.updated, ...difference.reactivated]) {
          const role = byName.get(name)!;
          update.run(
            role.kind,
            role.host,
            role.configHash,
            now,
            this.kernel.projectId,
            name,
          );
        }
        for (const name of difference.retired)
          retire.run(now, this.kernel.projectId, name);
        return {
          value: difference,
          event: {
            entityType: "role_definition",
            entityId: this.kernel.projectId,
            stateVersion: 0,
            details: difference,
          },
        };
      },
    );
  }

  #roleDifference(desired: readonly RoleDefinitionInput[]): RoleSyncResult {
    const stored = new Map(
      this.roleDefinitions().map((definition) => [definition.name, definition]),
    );
    const seatRoles = new Map<string, Set<string>>();
    for (const seat of this.kernel.database
      .prepare("SELECT name, role FROM seats WHERE project_id = ?")
      .all(this.kernel.projectId) as Array<{ name: string; role: string }>)
      seatRoles.set(
        seat.name,
        (seatRoles.get(seat.name) ?? new Set()).add(seat.role),
      );
    const inserted: string[] = [];
    const updated: string[] = [];
    const reactivated: string[] = [];
    const retired: string[] = [];
    for (const role of desired) {
      if (
        [...(seatRoles.get(role.name) ?? [])].some((kind) => kind !== role.kind)
      )
        throw new MutationConflictError(
          "a seat named after the role has a different kind than the role",
        );
      const current = stored.get(role.name);
      if (!current) {
        inserted.push(role.name);
        continue;
      }
      const differs =
        current.kind !== role.kind ||
        current.host !== role.host ||
        current.configHash !== role.configHash;
      if (current.state === "retired") reactivated.push(role.name);
      else if (differs) updated.push(role.name);
    }
    const wanted = new Set(desired.map((role) => role.name));
    for (const definition of stored.values())
      if (definition.state === "active" && !wanted.has(definition.name))
        retired.push(definition.name);
    return {
      changed:
        inserted.length + updated.length + reactivated.length + retired.length >
        0,
      inserted,
      updated,
      reactivated,
      retired,
    };
  }

  #lostRecorded(agentId: string, generation: number): boolean {
    return (
      this.kernel.database
        .prepare(
          `SELECT 1 AS present FROM controller_events WHERE project_id = ? AND entity_type = 'agent' AND entity_id = ?
             AND to_state = 'lost' AND json_extract(payload_json, '$.details.generation') = ?`,
        )
        .get(this.kernel.projectId, agentId, generation) !== undefined
    );
  }

  /**
   * Records that an agent is lost (once per agent and generation, whatever the
   * reason) and tells the PM when exactly one is active. The caller owns the
   * transaction. Returns whether it recorded.
   */
  recordLost(
    actor: AuthenticatedActor,
    context: MutationContext,
    agent: AgentRow,
    reason: "pane_gone" | "found_dead_at_start",
    unacknowledgedMessageIds: readonly string[],
    now: string,
    knownBranch?: string | null,
  ): boolean {
    if (this.#lostRecorded(agent.agent_id, agent.generation)) return false;
    const branch =
      knownBranch !== undefined
        ? knownBranch
        : ((
            this.kernel.database
              .prepare(
                "SELECT branch FROM agent_panes WHERE project_id = ? AND agent_id = ?",
              )
              .get(this.kernel.projectId, agent.agent_id) as
              { branch: string | null } | undefined
          )?.branch ?? null);
    this.kernel.appendEvent(actor, context, {
      entityType: "agent",
      entityId: agent.agent_id,
      from: "active",
      to: "lost",
      action: "agent.lost",
      stateVersion: 0,
      details: {
        generation: agent.generation,
        reason,
        branch,
        unacknowledgedMessageIds: unacknowledgedMessageIds.slice(
          0,
          LOST_EVENT_IDS,
        ),
        unacknowledgedCount: unacknowledgedMessageIds.length,
      },
    });
    const parties = this.areas.messageNotices.noticeParties();
    if (parties !== undefined && parties.pm.agent_id !== agent.agent_id) {
      const body = lostNotice(
        agent,
        reason,
        branch,
        unacknowledgedMessageIds,
        this.areas.pauses.isDeliveryPaused(agent.agent_id),
      );
      this.areas.messages.insertQueuedMessage(
        parties.controllerActorId,
        parties.pm,
        body,
        sha256(body),
        now,
      );
    }
    return true;
  }

  /**
   * An active agent whose pane is gone (the driver saw Herdr not find it three
   * times in a row). Written once per agent and generation; the agent stays
   * active, nothing is cancelled and nothing is replaced.
   */
  recordAgentLost(
    context: MutationContext,
    input: { readonly agentId: string },
  ): { readonly recorded: boolean } {
    safeId(input.agentId, "agent id");
    return this.kernel.mutate<{ recorded: boolean }>(
      context,
      "agent.lost",
      "controller:reconcile",
      { agentId: input.agentId },
      (actor) => {
        const agent = this.kernel.agentRow(input.agentId);
        if (agent?.state !== "active")
          throw new ControllerError("agent is not active");
        const now = this.kernel.now();
        const unacknowledged = this.areas.messages
          .messageRowsFor(input.agentId)
          .filter((row) => !isFinalState(row.state))
          .map((row) => row.message_id);
        const recorded = this.recordLost(
          actor,
          context,
          agent,
          "pane_gone",
          unacknowledged,
          now,
        );
        return {
          value: { recorded },
          event: {
            entityType: "agent",
            entityId: input.agentId,
            stateVersion: agent.generation,
            details: { recorded },
          },
        };
      },
    );
  }

  /** Whether the ledger holds a replacement of this agent. */
  isAgentReplaced(agentId: string): boolean {
    this.kernel.assertOpen();
    safeId(agentId, "agent id");
    return (
      this.kernel.database
        .prepare(
          "SELECT 1 AS present FROM controller_events WHERE project_id = ? AND entity_type = 'agent' AND entity_id = ? AND to_state = 'replaced'",
        )
        .get(this.kernel.projectId, agentId) !== undefined
    );
  }

  /**
   * Records that a started agent replaces an earlier one. One transaction
   * checks that the predecessor was not replaced already.
   */
  recordAgentReplaced(
    context: MutationContext,
    input: {
      readonly predecessorId: string;
      readonly successorId: string;
      /** The branch the successor continues, and where the predecessor's unreported commits were saved, when there were any. */
      readonly branch?: string;
      readonly keptRef?: string;
      readonly keptTip?: string;
    },
  ): { readonly recorded: true } {
    safeId(input.predecessorId, "predecessor agent id");
    safeId(input.successorId, "successor agent id");
    return this.kernel.mutate<{ recorded: true }>(
      context,
      "agent.replaced",
      "controller:reconcile",
      { ...input },
      () => {
        const predecessor = this.kernel.agentRow(input.predecessorId);
        const successor = this.kernel.agentRow(input.successorId);
        if (predecessor === undefined || successor === undefined)
          throw new ControllerError(
            "the predecessor and the successor must exist",
          );
        if (predecessor.kind === "PM")
          throw new ControllerError("a PM is restarted, not replaced");
        if (
          predecessor.agent_id === successor.agent_id ||
          predecessor.role_name !== successor.role_name ||
          successor.state !== "active"
        )
          throw new ControllerError(
            "the successor must be another active agent of the same role",
          );
        if (this.isAgentReplaced(input.predecessorId))
          throw new ControllerError("the agent was already replaced");
        const rebound = this.kernel.database
          .prepare(
            "UPDATE plan_packages SET assignee_agent_id = ? WHERE project_id = ? AND assignee_agent_id = ? AND cancelled_at IS NULL",
          )
          .run(
            input.successorId,
            this.kernel.projectId,
            input.predecessorId,
          ).changes;
        this.kernel.database
          .prepare(
            "UPDATE external_links SET bound_agent_id = ? WHERE project_id = ? AND bound_agent_id = ?",
          )
          .run(input.successorId, this.kernel.projectId, input.predecessorId);
        return {
          value: { recorded: true },
          event: {
            entityType: "agent",
            entityId: input.predecessorId,
            stateVersion: predecessor.generation,
            fromState: predecessor.state,
            toState: "replaced",
            details: {
              successorId: input.successorId,
              packagesRebound: rebound,
              ...(input.branch === undefined ? {} : { branch: input.branch }),
              ...(input.keptRef === undefined
                ? {}
                : { keptRef: input.keptRef, keptTip: input.keptTip ?? null }),
            },
          },
        };
      },
    );
  }

  /** The branch an agent had when it ended, from the end event. */
  #branchAtEnd(agentId: string): string | null {
    const row = this.kernel.database
      .prepare(
        `SELECT json_extract(payload_json, '$.details.branch') AS branch FROM controller_events
         WHERE project_id = ? AND entity_type = 'agent' AND entity_id = ? AND to_state = 'ended' ORDER BY sequence DESC LIMIT 1`,
      )
      .get(this.kernel.projectId, agentId) as
      { branch: string | null } | undefined;
    return typeof row?.branch === "string" ? row.branch : null;
  }

  /** What a replacement of this agent is seeded with, newest entries only, in ledger order. A read; works for an ended agent. */
  agentSeed(agentId: string): AgentSeedData {
    this.kernel.assertOpen();
    safeId(agentId, "agent id");
    const agent = this.kernel.agentRow(agentId);
    if (agent === undefined)
      throw new ControllerError("the agent does not exist");
    const messages = this.areas.messages.messageRowsFor(agentId);
    const shown = messages.slice(-SEED_MESSAGES);
    const reports = this.kernel.database
      .prepare(
        "SELECT * FROM agent_reports WHERE project_id = ? AND agent_id = ? AND state = 'accepted' ORDER BY sequence",
      )
      .all(this.kernel.projectId, agentId) as AgentReportRow[];
    const openFindings = this.kernel.database
      .prepare(
        "SELECT * FROM agent_findings WHERE project_id = ? AND target_agent_id = ? AND state = 'open' ORDER BY sequence",
      )
      .all(this.kernel.projectId, agentId) as AgentFindingRow[];
    const findings = openFindings.slice(-SEED_FINDINGS);
    const pane = this.kernel.database
      .prepare(
        "SELECT branch, base_sha FROM agent_panes WHERE project_id = ? AND agent_id = ?",
      )
      .get(this.kernel.projectId, agentId) as
      { branch: string | null; base_sha: string | null } | undefined;
    const last = reports.at(-1);
    return {
      agentId,
      roleName: agent.role_name,
      kind: agent.kind,
      state: agent.state,
      generation: agent.generation,
      branch: pane?.branch ?? last?.branch ?? this.#branchAtEnd(agentId),
      baseSha: pane?.base_sha ?? null,
      messages: shown.map((row) => ({
        messageId: row.message_id,
        sender:
          this.areas.messages.senderOf(row.sender_actor_id).agentId ??
          this.areas.messages.senderOf(row.sender_actor_id).role,
        state: row.state,
        stateReason: row.state_reason,
        body: row.body,
      })),
      messagesOmitted: messages.length - shown.length,
      reports: reports.slice(-SEED_REPORTS).map((r) => ({
        reportId: r.report_id,
        commitSha: r.commit_sha,
        branch: r.branch,
        summary: r.summary,
      })),
      reportsOmitted: Math.max(0, reports.length - SEED_REPORTS),
      lastAcceptedCommit: last?.commit_sha ?? null,
      findingsOmitted: openFindings.length - findings.length,
      findings: findings.map((f) => ({
        findingId: f.finding_id,
        severity: f.severity,
        requestedCorrection: f.requested_correction,
        interventions: f.interventions,
      })),
      packages: (
        this.kernel.database
          .prepare(
            `SELECT p.plan_id, p.package_id, pl.architect_agent_id, r.body_json FROM plan_packages p
             JOIN plans pl ON pl.project_id = p.project_id AND pl.plan_id = p.plan_id
             LEFT JOIN plan_revisions r ON r.project_id = pl.project_id AND r.plan_id = pl.plan_id AND r.revision = pl.approved_revision
             WHERE p.project_id = ? AND p.assignee_agent_id = ? AND p.cancelled_at IS NULL ORDER BY pl.sequence, p.package_id`,
          )
          .all(this.kernel.projectId, agentId) as {
          plan_id: string;
          package_id: string;
          architect_agent_id: string | null;
          body_json: string | null;
        }[]
      ).map((row) => ({
        planId: row.plan_id,
        packageId: row.package_id,
        architectAgentId: row.architect_agent_id,
        view:
          row.body_json === null
            ? null
            : (packageOfBody(row.body_json, row.package_id) ?? null),
      })),
    };
  }

  // ------------------------------------------------------------ findings

  /** The active actors of a seat, so a crash between creating an actor and registering its agent can be undone. */
  seatActorIds(credential: string, seatId: string): readonly string[] {
    this.kernel.authorize(credential, "controller:reconcile");
    safeId(seatId, "seat id");
    return (
      this.kernel.database
        .prepare(
          "SELECT actor_id FROM actors WHERE project_id = ? AND seat_id = ? AND active = 1 AND revoked_at IS NULL ORDER BY actor_id",
        )
        .all(this.kernel.projectId, seatId) as Array<{ actor_id: string }>
    ).map((row) => row.actor_id);
  }

  assertSeatFreeOfAuthority(seatId: string): void {
    const activeAuthority = this.kernel.database
      .prepare(
        "SELECT 1 AS present FROM assignments WHERE project_id = ? AND seat_id = ? AND authority_state IN ('active', 'unknown') LIMIT 1",
      )
      .get(this.kernel.projectId, seatId);
    if (activeAuthority)
      throw new MutationConflictError(
        "the agent seat has an assignment with active or uncertain authority; end or revoke it first",
      );
  }

  revokeSeatActors(seatId: string, now: string): void {
    const actors = this.kernel.database
      .prepare(
        "SELECT actor_id FROM actors WHERE project_id = ? AND seat_id = ? AND active = 1 AND revoked_at IS NULL",
      )
      .all(this.kernel.projectId, seatId) as Array<{ actor_id: string }>;
    for (const { actor_id: actorId } of actors) {
      this.kernel.database
        .prepare(
          "UPDATE actors SET active = 0, revoked_at = ? WHERE project_id = ? AND actor_id = ?",
        )
        .run(now, this.kernel.projectId, actorId);
      this.kernel.database
        .prepare(
          "UPDATE capability_grants SET revoked_at = ? WHERE project_id = ? AND actor_id = ? AND revoked_at IS NULL",
        )
        .run(now, this.kernel.projectId, actorId);
    }
  }

  insertActor(
    grantedBy: string,
    actor: {
      readonly actorId: string;
      readonly displayName: string;
      readonly role: string;
      readonly seatId: string | null;
      readonly credentialHash: string;
      readonly now: string;
    },
  ): void {
    this.kernel.database
      .prepare(
        `INSERT INTO actors(actor_id, project_id, display_name, role, seat_id, credential_hash, active, is_internal, created_at)
         VALUES (?, ?, ?, ?, ?, ?, 1, 0, ?)`,
      )
      .run(
        actor.actorId,
        this.kernel.projectId,
        actor.displayName,
        actor.role,
        actor.seatId,
        actor.credentialHash,
        actor.now,
      );
    this.kernel.database
      .prepare(
        "INSERT INTO capability_grants(project_id, actor_id, capability, granted_by, granted_at) SELECT ?, ?, capability, ?, ? FROM role_capabilities WHERE role = ?",
      )
      .run(
        this.kernel.projectId,
        actor.actorId,
        grantedBy,
        actor.now,
        actor.role,
      );
  }
}
