import { randomBytes, randomUUID } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import Database from "better-sqlite3";
import {
  authenticateActor,
  credentialHash,
  issueCredential,
  requireCapability,
  type AuthenticatedActor,
} from "./auth.js";
import { canonicalJson, digestJson } from "./canonical.js";
import { openDatabase, resolveDatabasePath } from "./database.js";
import { M1BridgeAdapter } from "./m1-bridge.js";
import { M1_MAX_FRAME_BYTES, M1_MAX_PROMPT_BYTES } from "./m1-protocol.js";
import { ControllerOwnershipError, ProjectLock } from "./ownership.js";
import type {
  AssignmentResult,
  BridgeReceipt,
  CandidateInput,
  Capability,
  ControllerOptions,
  EvidenceInput,
  InitialProject,
  FindingState,
  InputKind,
  MutationContext,
  ProjectInput,
  Role,
  RunState,
  RuntimeState,
  SeatInput,
  WorkItemInput,
} from "./types.js";

export class ControllerError extends Error {
  override readonly name: string = "ControllerError";
}

export class MutationConflictError extends ControllerError {
  override readonly name: string = "MutationConflictError";
}

export class IdempotencyConflictError extends MutationConflictError {
  override readonly name = "IdempotencyConflictError";
}

export class StateVersionConflictError extends MutationConflictError {
  override readonly name = "StateVersionConflictError";
}

export class InputRevisionConflictError extends MutationConflictError {
  override readonly name = "InputRevisionConflictError";
}

export class TransitionAuthorizationError extends ControllerError {
  override readonly name = "TransitionAuthorizationError";
}

export class ReadinessError extends ControllerError {
  override readonly name = "ReadinessError";
}

export class CandidateBindingError extends ControllerError {
  override readonly name = "CandidateBindingError";
}

interface MutationEvent {
  readonly entityType: string;
  readonly entityId: string;
  readonly stateVersion: number;
  readonly fromState?: string;
  readonly toState?: string;
  readonly details?: unknown;
}

interface MutationOutput<T> {
  readonly value: T;
  readonly event: MutationEvent;
}

export interface ActorInput {
  readonly displayName: string;
  readonly role: Role;
  readonly seatId?: string;
}

export interface ReadinessResult {
  readonly ready: boolean;
  readonly reasons: readonly string[];
  readonly workItemId: string;
  readonly inputRevision: number;
}
export type RecoveryType =
  "worker_replacement" | "finding_correction" | "implementation_remediation";

export interface RecoveryInput {
  readonly recoveryId: string;
  readonly workItemId: string;
  readonly assignmentId: string;
  readonly recoveryType: RecoveryType;
  readonly reason: string;
  readonly findingId?: string;
}
export interface RuntimeSessionInput {
  readonly sessionId: string;
  readonly seatId: string;
  readonly assignmentId?: string;
  readonly provider: string;
  readonly profile: string;
  readonly workspace: string;
}

export interface RuntimeIdentityInput {
  readonly observedPid?: number;
  readonly processStartId?: string;
  readonly containerId?: string;
  readonly cgroupPath?: string;
  readonly endpoint?: string;
}

export interface UsageInput {
  readonly observationId: string;
  readonly sessionId?: string;
  readonly assignmentId?: string;
  readonly provider: string;
  readonly metric: string;
  readonly value?: number;
  readonly availability: "observed" | "unavailable" | "inferred";
  readonly detail: unknown;
}

export interface FindingInput {
  readonly findingId: string;
  readonly workItemId: string;
  readonly assignmentId: string;
  readonly generation: number;
  readonly fingerprint: string;
  readonly severity: "info" | "low" | "medium" | "high" | "critical";
  readonly evidence: unknown;
  readonly requestedCorrection: string;
  readonly resolutionCondition: string;
}

export class ControllerCore {
  readonly #database: Database.Database;
  readonly #lock: ProjectLock;
  readonly #projectId: string;
  readonly #internalActorId: string;
  #closed = false;

  private constructor(
    database: Database.Database,
    lock: ProjectLock,
    projectId: string,
    internalActorId: string,
  ) {
    this.#database = database;
    this.#lock = lock;
    this.#projectId = projectId;
    this.#internalActorId = internalActorId;
  }

  static async open(options: ControllerOptions): Promise<ControllerCore> {
    const project = options.project;
    if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/.test(project.projectId)) {
      throw new TypeError("project id must be 1-64 safe ASCII characters");
    }
    if (!path.isAbsolute(options.stateDirectory))
      throw new TypeError("state directory must be absolute");
    const stateDirectory = path.resolve(options.stateDirectory);
    fs.mkdirSync(stateDirectory, { recursive: true, mode: 0o700 });
    const directoryStat = fs.lstatSync(stateDirectory);
    if (
      !directoryStat.isDirectory() ||
      directoryStat.isSymbolicLink() ||
      (process.getuid && directoryStat.uid !== process.getuid()) ||
      (directoryStat.mode & 0o077) !== 0
    ) {
      throw new ControllerOwnershipError(
        "controller state directory must be a private directory owned by the current user",
      );
    }
    const lock = ProjectLock.acquire(
      path.join(stateDirectory, "controller.lock"),
    );
    let database: Database.Database | undefined;
    try {
      const databasePath = resolveDatabasePath(stateDirectory);
      try {
        const existing = fs.lstatSync(databasePath);
        if (!existing.isFile() || existing.isSymbolicLink())
          throw new ControllerOwnershipError(
            "controller database path must be a regular file",
          );
      } catch (error) {
        if (
          !(error instanceof Error) ||
          !("code" in error) ||
          error.code !== "ENOENT"
        )
          throw error;
      }
      database = await openDatabase(databasePath);
      fs.chmodSync(databasePath, 0o600);
      const projectRow = database
        .prepare("SELECT name FROM projects WHERE project_id = ?")
        .get(project.projectId) as { name: string } | undefined;
      if (!projectRow) {
        ControllerCore.#initializeProject(database, project);
      } else {
        if (projectRow.name !== project.name)
          throw new ControllerError(
            "project name does not match durable project state",
          );
        const actor = authenticateActor(
          database,
          project.projectId,
          project.ownerCredential,
        );
        if (actor.role !== "operator")
          throw new ControllerError(
            "opening an existing project requires its operator credential",
          );
      }
      const internalActor = database
        .prepare(
          "SELECT actor_id FROM actors WHERE project_id = ? AND is_internal = 1 AND role = 'controller' AND active = 1 AND revoked_at IS NULL",
        )
        .get(project.projectId) as { actor_id: string } | undefined;
      if (!internalActor)
        throw new ControllerError(
          "durable internal controller principal is missing",
        );
      const core = new ControllerCore(
        database,
        lock,
        project.projectId,
        internalActor.actor_id,
      );
      core.#reconcileUncertainAssignments();
      return core;
    } catch (error) {
      database?.close();
      lock.close();
      throw error;
    }
  }

  static #initializeProject(
    database: Database.Database,
    project: InitialProject,
  ): void {
    const requiredKinds: readonly InputKind[] = [
      "project_config",
      "task_brief",
      "acceptance_criteria",
      "policy",
      "plan",
    ];
    if (
      project.initialInputs.length !== requiredKinds.length ||
      requiredKinds.some(
        (kind) =>
          project.initialInputs.filter((input) => input.kind === kind)
            .length !== 1,
      )
    ) {
      throw new TypeError(
        "initial project requires exactly one revision of each durable input kind",
      );
    }
    if (project.name.trim().length === 0)
      throw new TypeError("project name must not be empty");
    const ownerHash = credentialHash(project.ownerCredential);
    const internalHash = credentialHash(randomBytes(32).toString("base64url"));
    const now = new Date().toISOString();
    const ownerId = randomUUID();
    const internalId = randomUUID();
    database.exec("BEGIN IMMEDIATE");
    try {
      database
        .prepare(
          "INSERT INTO projects(project_id, name, current_input_revision, state_version, created_at) VALUES (?, ?, 1, 1, ?)",
        )
        .run(project.projectId, project.name, now);
      const insertActor = database.prepare(`
        INSERT INTO actors(actor_id, project_id, display_name, role, seat_id, credential_hash, active, is_internal, created_at)
        VALUES (?, ?, ?, ?, NULL, ?, 1, ?, ?)
      `);
      insertActor.run(
        ownerId,
        project.projectId,
        "project operator",
        "operator",
        ownerHash,
        0,
        now,
      );
      insertActor.run(
        internalId,
        project.projectId,
        "capstan internal controller",
        "controller",
        internalHash,
        1,
        now,
      );
      const grantRole = database.prepare(`
        INSERT INTO capability_grants(project_id, actor_id, capability, granted_by, granted_at)
        SELECT ?, ?, capability, ?, ? FROM role_capabilities WHERE role = ?
      `);
      grantRole.run(project.projectId, ownerId, ownerId, now, "operator");
      grantRole.run(project.projectId, internalId, ownerId, now, "controller");
      const insertRevision = database.prepare(`
        INSERT INTO project_revisions(project_id, revision, kind, content_json, content_hash, created_by, request_id, created_at)
        VALUES (?, 1, ?, ?, ?, ?, ?, ?)
      `);
      for (const input of project.initialInputs) {
        const content = canonicalJson(input.content);
        insertRevision.run(
          project.projectId,
          input.kind,
          content,
          digestJson(input.content),
          ownerId,
          `bootstrap:${project.projectId}`,
          now,
        );
      }
      database
        .prepare(
          "INSERT INTO run_controls(project_id, state, state_version, updated_at) VALUES (?, 'active', 0, ?)",
        )
        .run(project.projectId, now);
      database
        .prepare(
          `
        INSERT INTO controller_events(project_id, sequence, event_id, entity_type, entity_id, from_state, to_state,
          state_version, actor_id, request_id, input_revision, payload_json, created_at)
        VALUES (?, 1, ?, 'project', ?, NULL, 'initialized', 1, ?, ?, 1, ?, ?)
      `,
        )
        .run(
          project.projectId,
          randomUUID(),
          project.projectId,
          ownerId,
          `bootstrap:${project.projectId}`,
          canonicalJson({ inputKinds: requiredKinds }),
          now,
        );
      database.exec("COMMIT");
    } catch (error) {
      database.exec("ROLLBACK");
      throw error;
    }
  }

  get projectId(): string {
    return this.#projectId;
  }

  get stateVersion(): number {
    this.#assertOpen();
    const row = this.#database
      .prepare("SELECT state_version FROM projects WHERE project_id = ?")
      .get(this.#projectId) as { state_version: number };
    return row.state_version;
  }

  get inputRevision(): number {
    this.#assertOpen();
    const row = this.#database
      .prepare(
        "SELECT current_input_revision FROM projects WHERE project_id = ?",
      )
      .get(this.#projectId) as { current_input_revision: number };
    return row.current_input_revision;
  }

  createActor(
    context: MutationContext,
    input: ActorInput,
  ): { readonly actorId: string; readonly credential: string } {
    const mutationPayload = {
      displayName: input.displayName,
      role: input.role,
      seatId: input.seatId ?? null,
    };
    return this.#mutate(
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
          const seat = this.#database
            .prepare(
              "SELECT role, state FROM seats WHERE project_id = ? AND seat_id = ?",
            )
            .get(this.#projectId, input.seatId) as
            { role: string; state: string } | undefined;
          if (!seat || seat.role !== input.role || seat.state !== "active")
            throw new ControllerError(
              "actor seat must be active and match its role",
            );
          seatId = input.seatId;
        } else if (input.seatId) {
          throw new ControllerError(
            "operator and controller actors cannot be attached to a worker seat",
          );
        }
        const now = new Date().toISOString();
        this.#database
          .prepare(
            `
        INSERT INTO actors(actor_id, project_id, display_name, role, seat_id, credential_hash, active, is_internal, created_at)
        VALUES (?, ?, ?, ?, ?, ?, 1, 0, ?)
      `,
          )
          .run(
            actorId,
            this.#projectId,
            input.displayName,
            input.role,
            seatId,
            hash,
            now,
          );
        this.#database
          .prepare(
            `
        INSERT INTO capability_grants(project_id, actor_id, capability, granted_by, granted_at)
        SELECT ?, ?, capability, ?, ? FROM role_capabilities WHERE role = ?
      `,
          )
          .run(this.#projectId, actorId, actor.actorId, now, input.role);
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
    return this.#mutate(
      context,
      "actor.revoke",
      "actor:manage",
      { actorId },
      () => {
        const target = this.#database
          .prepare(
            "SELECT role, is_internal, active, seat_id FROM actors WHERE project_id = ? AND actor_id = ?",
          )
          .get(this.#projectId, actorId) as
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
          const otherActiveActor = this.#database
            .prepare(
              `
            SELECT 1 AS present FROM actors
            WHERE project_id = ? AND seat_id = ? AND actor_id <> ?
              AND active = 1 AND revoked_at IS NULL LIMIT 1
          `,
            )
            .get(this.#projectId, target.seat_id, actorId);
          const activeAuthority = this.#database
            .prepare(
              `
            SELECT 1 AS present FROM assignments
            WHERE project_id = ? AND seat_id = ?
              AND authority_state IN ('active', 'unknown') LIMIT 1
          `,
            )
            .get(this.#projectId, target.seat_id);
          if (!otherActiveActor && activeAuthority)
            throw new MutationConflictError(
              "cannot revoke the last active actor for a seat with active or uncertain assignments",
            );
        }
        const now = new Date().toISOString();
        this.#database
          .prepare(
            "UPDATE actors SET active = 0, revoked_at = ? WHERE project_id = ? AND actor_id = ?",
          )
          .run(now, this.#projectId, actorId);
        this.#database
          .prepare(
            "UPDATE capability_grants SET revoked_at = ? WHERE project_id = ? AND actor_id = ? AND revoked_at IS NULL",
          )
          .run(now, this.#projectId, actorId);
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

  recordInputRevision(
    context: MutationContext,
    input: ProjectInput,
  ): { readonly revision: number; readonly contentHash: string } {
    return this.#mutate(
      context,
      "project.input.revise",
      "project:inputs:write",
      input,
      (actor) => {
        const unfinishedWork = this.#database
          .prepare(
            `
          SELECT 1 AS present FROM work_items
          WHERE project_id = ? AND state IN ('running', 'awaiting_verification') LIMIT 1
        `,
          )
          .get(this.#projectId);
        const uncontainedAssignment = this.#database
          .prepare(
            `
          SELECT 1 AS present FROM assignments
          WHERE project_id = ? AND authority_state IN ('active', 'unknown') LIMIT 1
        `,
          )
          .get(this.#projectId);
        if (unfinishedWork || uncontainedAssignment) {
          throw new MutationConflictError(
            "project inputs cannot change while work is running or worker authority is active/uncertain",
          );
        }
        const revision = context.inputRevision + 1;
        const content = canonicalJson(input.content);
        const contentHash = digestJson(input.content);
        this.#database
          .prepare(
            `
        INSERT INTO project_revisions(project_id, revision, kind, content_json, content_hash, created_by, request_id, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
      `,
          )
          .run(
            this.#projectId,
            revision,
            input.kind,
            content,
            contentHash,
            actor.actorId,
            context.requestId,
            new Date().toISOString(),
          );
        this.#database
          .prepare(
            "UPDATE projects SET current_input_revision = ? WHERE project_id = ?",
          )
          .run(revision, this.#projectId);
        return {
          value: { revision, contentHash },
          event: {
            entityType: "project_input",
            entityId: input.kind,
            stateVersion: context.expectedVersion + 1,
            fromState: String(context.inputRevision),
            toState: String(revision),
          },
        };
      },
    );
  }

  createSeat(
    context: MutationContext,
    input: SeatInput,
  ): { readonly seatId: string } {
    return this.#mutate(context, "seat.create", "work:write", input, () => {
      this.#database
        .prepare(
          `
        INSERT INTO seats(project_id, seat_id, name, role, state, state_version, created_at)
        VALUES (?, ?, ?, ?, 'active', 0, ?)
      `,
        )
        .run(
          this.#projectId,
          input.seatId,
          input.name,
          input.role,
          new Date().toISOString(),
        );
      return {
        value: { seatId: input.seatId },
        event: { entityType: "seat", entityId: input.seatId, stateVersion: 0 },
      };
    });
  }

  createWorkItem(
    context: MutationContext,
    input: WorkItemInput,
  ): { readonly workItemId: string } {
    return this.#mutate(
      context,
      "work.create",
      "work:write",
      input,
      (actor) => {
        const run = this.#database
          .prepare("SELECT state FROM run_controls WHERE project_id = ?")
          .get(this.#projectId) as { state: string } | undefined;
        if (run?.state !== "active")
          throw new ReadinessError(
            `work creation requires an active run; run is ${run?.state ?? "missing"}`,
          );
        this.#database
          .prepare(
            `
        INSERT INTO work_items(project_id, work_item_id, parent_work_item_id, title, description, required_role,
          state, state_version, input_revision, accepted_candidate_id, created_by, created_at)
        VALUES (?, ?, ?, ?, ?, ?, 'pending', 0, ?, NULL, ?, ?)
      `,
          )
          .run(
            this.#projectId,
            input.workItemId,
            input.parentWorkItemId ?? null,
            input.title,
            input.description,
            input.requiredRole,
            context.inputRevision,
            actor.actorId,
            new Date().toISOString(),
          );
        return {
          value: { workItemId: input.workItemId },
          event: {
            entityType: "work_item",
            entityId: input.workItemId,
            stateVersion: 0,
            toState: "pending",
          },
        };
      },
    );
  }

  addDependency(
    context: MutationContext,
    workItemId: string,
    prerequisiteId: string,
  ): { readonly added: true } {
    return this.#mutate(
      context,
      "dependency.add",
      "work:write",
      { workItemId, prerequisiteId },
      (actor) => {
        if (workItemId === prerequisiteId)
          throw new ControllerError("a work item cannot depend on itself");
        const workItem = this.#database
          .prepare(
            "SELECT state FROM work_items WHERE project_id = ? AND work_item_id = ?",
          )
          .get(this.#projectId, workItemId) as { state: string } | undefined;
        if (!workItem) throw new ControllerError("work item does not exist");
        if (!["pending", "blocked", "ready"].includes(workItem.state))
          throw new MutationConflictError(
            "dependencies can change only for pending, blocked, or ready work",
          );
        const priorAssignment = this.#database
          .prepare(
            "SELECT 1 AS present FROM assignments WHERE project_id = ? AND work_item_id = ? LIMIT 1",
          )
          .get(this.#projectId, workItemId);
        if (priorAssignment && workItem.state !== "blocked")
          throw new MutationConflictError(
            "dependencies can change after assignment only while blocked",
          );
        const uncontainedAssignment = this.#database
          .prepare(
            "SELECT 1 AS present FROM assignments WHERE project_id = ? AND work_item_id = ? AND authority_state <> 'contained' LIMIT 1",
          )
          .get(this.#projectId, workItemId);
        if (uncontainedAssignment)
          throw new MutationConflictError(
            "dependencies can change only after every prior assignment is contained",
          );
        const cycle = this.#database
          .prepare(
            `
        WITH RECURSIVE reaches(work_item_id) AS (
          SELECT depends_on_work_item_id FROM dependency_edges WHERE project_id = ? AND work_item_id = ?
          UNION
          SELECT d.depends_on_work_item_id FROM dependency_edges d JOIN reaches r ON d.work_item_id = r.work_item_id
          WHERE d.project_id = ?
        ) SELECT 1 AS cycle FROM reaches WHERE work_item_id = ? LIMIT 1
      `,
          )
          .get(this.#projectId, prerequisiteId, this.#projectId, workItemId);
        if (cycle) throw new ControllerError("dependency would create a cycle");
        this.#database
          .prepare(
            `
        INSERT INTO dependency_edges(project_id, work_item_id, depends_on_work_item_id, required_candidate_id, created_by, created_at)
        VALUES (?, ?, ?, NULL, ?, ?)
      `,
          )
          .run(
            this.#projectId,
            workItemId,
            prerequisiteId,
            actor.actorId,
            new Date().toISOString(),
          );
        return {
          value: { added: true },
          event: {
            entityType: "dependency",
            entityId: `${workItemId}:${prerequisiteId}`,
            stateVersion: 0,
          },
        };
      },
    );
  }
  removeDependency(
    context: MutationContext,
    workItemId: string,
    prerequisiteId: string,
  ): { readonly removed: true } {
    return this.#mutate(
      context,
      "dependency.remove",
      "work:write",
      { workItemId, prerequisiteId },
      () => {
        const workItem = this.#database
          .prepare(
            "SELECT state FROM work_items WHERE project_id = ? AND work_item_id = ?",
          )
          .get(this.#projectId, workItemId) as { state: string } | undefined;
        if (!workItem) throw new ControllerError("work item does not exist");
        if (!["pending", "blocked", "ready"].includes(workItem.state))
          throw new MutationConflictError(
            "dependencies can change only for pending, blocked, or ready work",
          );
        const priorAssignment = this.#database
          .prepare(
            "SELECT 1 AS present FROM assignments WHERE project_id = ? AND work_item_id = ? LIMIT 1",
          )
          .get(this.#projectId, workItemId);
        if (priorAssignment && workItem.state !== "blocked")
          throw new MutationConflictError(
            "dependencies can change after assignment only while blocked",
          );
        const uncontainedAssignment = this.#database
          .prepare(
            "SELECT 1 AS present FROM assignments WHERE project_id = ? AND work_item_id = ? AND authority_state <> 'contained' LIMIT 1",
          )
          .get(this.#projectId, workItemId);
        if (uncontainedAssignment)
          throw new MutationConflictError(
            "dependencies can change only after every prior assignment is contained",
          );
        const result = this.#database
          .prepare(
            "DELETE FROM dependency_edges WHERE project_id = ? AND work_item_id = ? AND depends_on_work_item_id = ?",
          )
          .run(this.#projectId, workItemId, prerequisiteId);
        if (result.changes === 0)
          throw new ControllerError("dependency does not exist");
        return {
          value: { removed: true },
          event: {
            entityType: "dependency",
            entityId: `${workItemId}:${prerequisiteId}`,
            stateVersion: 0,
            toState: "removed",
          },
        };
      },
    );
  }

  readiness(workItemId: string): ReadinessResult {
    this.#assertOpen();
    const item = this.#database
      .prepare(
        `
      SELECT w.state, w.input_revision, w.required_role, p.current_input_revision, r.state AS run_state
      FROM work_items w JOIN projects p ON p.project_id = w.project_id
      JOIN run_controls r ON r.project_id = w.project_id
      WHERE w.project_id = ? AND w.work_item_id = ?
    `,
      )
      .get(this.#projectId, workItemId) as
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
    const seats = this.#database
      .prepare(
        "SELECT 1 AS present FROM seats WHERE project_id = ? AND role = ? AND state = 'active' LIMIT 1",
      )
      .get(this.#projectId, item.required_role);
    if (!seats) reasons.push(`no active ${item.required_role} seat`);
    const activeAssignment = this.#database
      .prepare(
        `
      SELECT 1 AS present FROM assignments WHERE project_id = ? AND work_item_id = ?
        AND authority_state IN ('active', 'unknown') LIMIT 1
    `,
      )
      .get(this.#projectId, workItemId);
    if (activeAssignment)
      reasons.push("work item has active or unknown assignment authority");
    const dependencies = this.#database
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
      .all(this.#projectId, workItemId) as Array<{
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
        dependency.required_candidate_id &&
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

  markReady(
    context: MutationContext,
    workItemId: string,
  ): { readonly state: "ready" } {
    return this.#mutateAsController(
      context,
      "work.transition.ready",
      "work:assign",
      { workItemId },
      (actor) => {
        const item = this.#database
          .prepare(
            "SELECT state, state_version FROM work_items WHERE project_id = ? AND work_item_id = ?",
          )
          .get(this.#projectId, workItemId) as
          { state: string; state_version: number } | undefined;
        if (!item) throw new ControllerError("work item does not exist");
        if (!this.#isTransitionAllowed("work_item", item.state, "ready", actor))
          throw new TransitionAuthorizationError(
            "transition table rejects work item readiness",
          );
        const readiness = this.readiness(workItemId);
        if (!readiness.ready)
          throw new ReadinessError(
            `work item is not ready: ${readiness.reasons.join("; ")}`,
          );
        this.#database
          .prepare(
            "UPDATE work_items SET state = 'ready', state_version = state_version + 1 WHERE project_id = ? AND work_item_id = ? AND state = ?",
          )
          .run(this.#projectId, workItemId, item.state);
        return {
          value: { state: "ready" },
          event: {
            entityType: "work_item",
            entityId: workItemId,
            stateVersion: item.state_version + 1,
            fromState: item.state,
            toState: "ready",
          },
        };
      },
    );
  }
  rebindWorkItem(
    context: MutationContext,
    workItemId: string,
  ): { readonly inputRevision: number } {
    return this.#mutate(
      context,
      "work.inputs.rebind",
      "work:write",
      { workItemId },
      () => {
        const item = this.#database
          .prepare(
            "SELECT state, state_version, input_revision FROM work_items WHERE project_id = ? AND work_item_id = ?",
          )
          .get(this.#projectId, workItemId) as
          | { state: string; state_version: number; input_revision: number }
          | undefined;
        if (!item) throw new ControllerError("work item does not exist");
        if (!["pending", "blocked", "ready"].includes(item.state)) {
          throw new ControllerError(
            "only pending, blocked, or ready work can be rebound",
          );
        }
        if (item.input_revision === context.inputRevision)
          throw new ControllerError(
            "work item is already bound to the current input revision",
          );
        const active = this.#database
          .prepare(
            `
        SELECT 1 AS present FROM assignments WHERE project_id = ? AND work_item_id = ?
          AND authority_state IN ('active', 'unknown') LIMIT 1
      `,
          )
          .get(this.#projectId, workItemId);
        if (active)
          throw new ControllerError(
            "work cannot be rebound while an assignment may still write",
          );
        this.#database
          .prepare(
            `
        UPDATE work_items SET input_revision = ?, state_version = state_version + 1
        WHERE project_id = ? AND work_item_id = ? AND state = ?
      `,
          )
          .run(context.inputRevision, this.#projectId, workItemId, item.state);
        return {
          value: { inputRevision: context.inputRevision },
          event: {
            entityType: "work_item",
            entityId: workItemId,
            stateVersion: item.state_version + 1,
            fromState: item.state,
            toState: item.state,
          },
        };
      },
    );
  }
  assignWorkItem(
    context: MutationContext,
    workItemId: string,
    seatId: string,
    candidateId?: string,
    recoveryId?: string,
  ): AssignmentResult {
    return this.#mutateAsController(
      context,
      "work.assign",
      "work:assign",
      {
        workItemId,
        seatId,
        candidateId: candidateId ?? null,
        recoveryId: recoveryId ?? null,
      },
      (actor) => {
        const item = this.#database
          .prepare(
            `
        SELECT title, description, required_role, parent_work_item_id, state, state_version, input_revision
        FROM work_items WHERE project_id = ? AND work_item_id = ?
      `,
          )
          .get(this.#projectId, workItemId) as
          | {
              title: string;
              description: string;
              required_role: string;
              parent_work_item_id: string | null;
              state: string;
              state_version: number;
              input_revision: number;
            }
          | undefined;
        if (!item) throw new ControllerError("work item does not exist");
        const latestAssignment = this.#database
          .prepare(
            `
        SELECT a.assignment_id, a.authority_state FROM assignments a
        JOIN assignment_attempts at ON at.project_id = a.project_id
          AND at.assignment_id = a.assignment_id AND at.generation = a.active_generation
        WHERE a.project_id = ? AND a.work_item_id = ? ORDER BY at.generation DESC LIMIT 1
      `,
          )
          .get(this.#projectId, workItemId) as
          { assignment_id: string; authority_state: string } | undefined;
        let recovery:
          | {
              recovery_id: string;
              recovery_type: string;
              outcome: string;
              containment_state: string;
            }
          | undefined;
        if (latestAssignment) {
          if (!recoveryId || latestAssignment.authority_state !== "contained") {
            throw new MutationConflictError(
              "a replacement assignment requires contained prior authority and a pending recovery record",
            );
          }
          recovery = this.#database
            .prepare(
              `
          SELECT recovery_id, recovery_type, outcome, containment_state
          FROM recovery_attempts
          WHERE project_id = ? AND recovery_id = ? AND work_item_id = ? AND assignment_id = ?
        `,
            )
            .get(
              this.#projectId,
              recoveryId,
              workItemId,
              latestAssignment.assignment_id,
            ) as typeof recovery;
          if (
            !recovery ||
            recovery.recovery_id !== recoveryId ||
            recovery.outcome !== "pending" ||
            recovery.containment_state !== "contained" ||
            recovery.recovery_type === "finding_correction"
          ) {
            throw new MutationConflictError(
              "recovery record is not a pending contained replacement for the prior assignment",
            );
          }
        } else if (recoveryId) {
          throw new MutationConflictError(
            "recovery record cannot be used before an assignment exists",
          );
        }
        const readiness = this.readiness(workItemId);
        if (!readiness.ready || item.state !== "ready")
          throw new ReadinessError(
            `work item cannot be assigned: ${readiness.reasons.join("; ")}`,
          );
        if (
          !this.#isTransitionAllowed("work_item", item.state, "running", actor)
        ) {
          throw new TransitionAuthorizationError(
            "transition table rejects assignment",
          );
        }
        const seat = this.#database
          .prepare(
            "SELECT role, state FROM seats WHERE project_id = ? AND seat_id = ?",
          )
          .get(this.#projectId, seatId) as
          { role: string; state: string } | undefined;
        if (
          !seat ||
          seat.state !== "active" ||
          seat.role !== item.required_role
        ) {
          throw new ControllerError(
            "assignment seat must be active and match the work item role",
          );
        }
        let verifierCandidate:
          | {
              candidate_id: string;
              work_item_id: string;
              input_revision: number;
              commit_sha: string;
              report_hash: string;
              work_state: string;
            }
          | undefined;
        if (item.required_role === "Verifier") {
          if (!candidateId)
            throw new CandidateBindingError(
              "Verifier assignment requires an exact candidate id",
            );
          verifierCandidate = this.#database
            .prepare(
              `
          SELECT c.candidate_id, a.work_item_id, c.input_revision, c.commit_sha, c.report_hash, w.state AS work_state
          FROM candidates c JOIN assignments a
            ON a.project_id = c.project_id AND a.assignment_id = c.assignment_id
          JOIN work_items w ON w.project_id = a.project_id AND w.work_item_id = a.work_item_id
          WHERE c.project_id = ? AND c.candidate_id = ?
        `,
            )
            .get(this.#projectId, candidateId) as typeof verifierCandidate;
          if (
            !verifierCandidate ||
            verifierCandidate.work_item_id !== item.parent_work_item_id ||
            verifierCandidate.input_revision !== context.inputRevision ||
            verifierCandidate.work_state !== "awaiting_verification"
          ) {
            throw new CandidateBindingError(
              "Verifier task parent and input revision must match the candidate",
            );
          }
        } else if (candidateId) {
          throw new CandidateBindingError(
            "only a Verifier assignment may bind a candidate",
          );
        }
        const snapshots = this.#database
          .prepare(
            `
        SELECT r.kind, r.revision, r.content_json, r.content_hash
        FROM project_revisions r
        WHERE r.project_id = ? AND r.revision = (
          SELECT MAX(latest.revision) FROM project_revisions latest
          WHERE latest.project_id = r.project_id AND latest.kind = r.kind AND latest.revision <= ?
        )
        ORDER BY r.kind
      `,
          )
          .all(this.#projectId, context.inputRevision) as Array<{
          kind: string;
          revision: number;
          content_json: string;
          content_hash: string;
        }>;
        if (snapshots.length !== 5)
          throw new ControllerError(
            "effective project input snapshot is incomplete",
          );
        const dependencies = this.#database
          .prepare(
            `
        SELECT d.depends_on_work_item_id, w.accepted_candidate_id,
          COALESCE(c.input_revision, w.input_revision) AS input_revision,
          c.commit_sha, c.report_hash AS candidate_report_hash,
          report.command_id AS report_command_id, report.receipt_json AS report_json,
          report.receipt_hash AS accepted_report_hash
        FROM dependency_edges d
        JOIN work_items w ON w.project_id = d.project_id AND w.work_item_id = d.depends_on_work_item_id
        LEFT JOIN candidates c ON c.project_id = w.project_id AND c.candidate_id = w.accepted_candidate_id
        LEFT JOIN (
          SELECT a.project_id, a.work_item_id, a.active_generation, r.command_id,
            r.receipt_json, r.receipt_hash,
            ROW_NUMBER() OVER (
              PARTITION BY a.project_id, a.work_item_id ORDER BY a.active_generation DESC
            ) AS report_rank
          FROM assignments a
          JOIN seats s ON s.project_id = a.project_id AND s.seat_id = a.seat_id
          JOIN commands cmd ON cmd.project_id = a.project_id AND cmd.assignment_id = a.assignment_id
            AND cmd.generation = a.active_generation AND cmd.state = 'completed'
          JOIN command_receipts r ON r.project_id = cmd.project_id AND r.command_id = cmd.command_id
            AND r.assignment_id = cmd.assignment_id AND r.attempt = cmd.attempt
            AND r.generation = cmd.generation AND r.receipt_type = 'completed' AND r.role = s.role
          WHERE a.state = 'completed' AND s.role IN ('PM', 'Supervisor')
        ) report ON report.project_id = w.project_id AND report.work_item_id = w.work_item_id
          AND report.report_rank = 1 AND w.accepted_candidate_id IS NULL
        WHERE d.project_id = ? AND d.work_item_id = ? ORDER BY d.depends_on_work_item_id
      `,
          )
          .all(this.#projectId, workItemId) as Array<{
          depends_on_work_item_id: string;
          accepted_candidate_id: string | null;
          input_revision: number;
          commit_sha: string | null;
          candidate_report_hash: string | null;
          report_command_id: string | null;
          report_json: string | null;
          accepted_report_hash: string | null;
        }>;
        const assignmentId = randomUUID();
        const commandId = randomUUID();
        const attempt = 1;
        const previousGeneration = this.#database
          .prepare(
            `
        SELECT COALESCE(MAX(at.generation), 0) AS generation
        FROM assignments a JOIN assignment_attempts at
          ON at.project_id = a.project_id AND at.assignment_id = a.assignment_id
        WHERE a.project_id = ? AND a.work_item_id = ?
      `,
          )
          .get(this.#projectId, workItemId) as { generation: number };
        const generation = previousGeneration.generation + 1;
        const capsule = {
          projectId: this.#projectId,
          assignment: { commandId, assignmentId, attempt, generation },
          workItem: {
            workItemId,
            title: item.title,
            description: item.description,
            requiredRole: item.required_role,
            inputRevision: context.inputRevision,
          },
          seat: { seatId, role: seat.role },
          inputs: Object.fromEntries(
            snapshots.map((snapshot) => [
              snapshot.kind,
              JSON.parse(snapshot.content_json) as unknown,
            ]),
          ),
          dependencies: dependencies.map((dependency) => {
            if (dependency.accepted_candidate_id) {
              if (!dependency.commit_sha || !dependency.candidate_report_hash)
                throw new ControllerError(
                  "accepted dependency candidate evidence is missing",
                );
              return {
                workItemId: dependency.depends_on_work_item_id,
                candidateId: dependency.accepted_candidate_id,
                inputRevision: dependency.input_revision,
                commitSha: dependency.commit_sha,
                reportHash: dependency.candidate_report_hash,
              };
            }
            if (
              !dependency.report_command_id ||
              !dependency.report_json ||
              !dependency.accepted_report_hash
            )
              throw new ControllerError(
                "accepted dependency report receipt is missing",
              );
            return {
              workItemId: dependency.depends_on_work_item_id,
              candidateId: null,
              inputRevision: dependency.input_revision,
              report: JSON.parse(dependency.report_json) as unknown,
              reportHash: dependency.accepted_report_hash,
            };
          }),
          ...(verifierCandidate
            ? {
                candidate: {
                  candidateId: verifierCandidate.candidate_id,
                  commitSha: verifierCandidate.commit_sha,
                  inputRevision: verifierCandidate.input_revision,
                  reportHash: verifierCandidate.report_hash,
                },
              }
            : {}),
        };
        const wirePayload = {
          type: "dispatch",
          commandId,
          assignmentId,
          attempt,
          generation,
          singleResponse: true,
          prompt: canonicalJson(capsule),
        };
        const wireJson = canonicalJson(wirePayload);
        if (Buffer.byteLength(wirePayload.prompt) > M1_MAX_PROMPT_BYTES)
          throw new ControllerError(
            "M1 dispatch prompt exceeds its byte limit",
          );
        if (Buffer.byteLength(wireJson) + 1 > M1_MAX_FRAME_BYTES)
          throw new ControllerError(
            "M1 dispatch request exceeds the frame limit",
          );
        const now = new Date().toISOString();
        this.#database
          .prepare(
            `
        INSERT INTO assignments(project_id, assignment_id, work_item_id, seat_id, state, state_version, input_revision,
          active_generation, authority_state, created_by, created_at, ended_at)
        VALUES (?, ?, ?, ?, 'created', 0, ?, ?, 'active', ?, ?, NULL)
      `,
          )
          .run(
            this.#projectId,
            assignmentId,
            workItemId,
            seatId,
            context.inputRevision,
            generation,
            actor.actorId,
            now,
          );
        this.#database
          .prepare(
            `
        INSERT INTO assignment_attempts(project_id, assignment_id, attempt, generation, state, state_version,
          authority_state, created_at, ended_at)
        VALUES (?, ?, ?, ?, 'created', 0, 'active', ?, NULL)
      `,
          )
          .run(this.#projectId, assignmentId, attempt, generation, now);
        const bindInput = this.#database.prepare(`
        INSERT INTO assignment_input_bindings(project_id, assignment_id, input_revision, input_kind, source_revision,
          source_id, content_hash, bound_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
      `);
        for (const snapshot of snapshots) {
          bindInput.run(
            this.#projectId,
            assignmentId,
            context.inputRevision,
            snapshot.kind,
            snapshot.revision,
            `${snapshot.kind}:${snapshot.revision}`,
            snapshot.content_hash,
            now,
          );
        }
        for (const dependency of dependencies) {
          const candidateBound = dependency.accepted_candidate_id !== null;
          const sourceId = candidateBound
            ? dependency.accepted_candidate_id
            : dependency.report_command_id;
          const contentHash = candidateBound
            ? dependency.candidate_report_hash
            : dependency.accepted_report_hash;
          if (!sourceId || !contentHash)
            throw new ControllerError(
              "accepted dependency source evidence is missing",
            );
          bindInput.run(
            this.#projectId,
            assignmentId,
            context.inputRevision,
            candidateBound ? "dependency_candidate" : "dependency_report",
            dependency.input_revision,
            sourceId,
            contentHash,
            now,
          );
        }
        if (verifierCandidate) {
          bindInput.run(
            this.#projectId,
            assignmentId,
            context.inputRevision,
            "candidate",
            verifierCandidate.input_revision,
            verifierCandidate.candidate_id,
            verifierCandidate.report_hash,
            now,
          );
        }
        this.#database
          .prepare(
            `
        INSERT INTO commands(project_id, command_id, assignment_id, attempt, generation, command_type, payload_json,
          payload_hash, state, state_version, created_at, updated_at)
        VALUES (?, ?, ?, ?, ?, 'dispatch', ?, ?, 'queued', 0, ?, ?)
      `,
          )
          .run(
            this.#projectId,
            commandId,
            assignmentId,
            attempt,
            generation,
            wireJson,
            digestJson(wirePayload),
            now,
            now,
          );
        this.#database
          .prepare(
            `
        UPDATE work_items SET state = 'running', state_version = state_version + 1
        WHERE project_id = ? AND work_item_id = ? AND state = 'ready'
      `,
          )
          .run(this.#projectId, workItemId);
        if (recovery) {
          const recovered = this.#database
            .prepare(
              `
          UPDATE recovery_attempts SET outcome = 'replacement_created'
          WHERE project_id = ? AND recovery_id = ? AND outcome = 'pending'
        `,
            )
            .run(this.#projectId, recoveryId);
          if (recovered.changes !== 1)
            throw new MutationConflictError(
              "recovery reservation was already consumed",
            );
        }
        const result: AssignmentResult = {
          assignmentId,
          attempt,
          generation,
          commandId,
          workItemId,
          seatId,
          inputRevision: context.inputRevision,
        };
        return {
          value: result,
          event: {
            entityType: "work_item",
            entityId: workItemId,
            stateVersion: item.state_version + 1,
            fromState: "ready",
            toState: "running",
            details: { recoveryId: recovery?.recovery_id ?? null },
          },
        };
      },
    );
  }

  beginCommandDelivery(
    context: MutationContext,
    commandId: string,
  ): { readonly payload: unknown; readonly ordinal: number } {
    return this.#mutateAsController(
      context,
      "command.delivery.begin",
      "controller:reconcile",
      { commandId },
      (actor) => {
        const command = this.#database
          .prepare(
            `
        SELECT c.assignment_id, c.attempt, c.generation, c.payload_json, c.state, c.state_version,
          a.state AS assignment_state, a.authority_state AS assignment_authority,
          at.state AS attempt_state, at.authority_state AS attempt_authority
        FROM commands c JOIN assignments a
          ON a.project_id = c.project_id AND a.assignment_id = c.assignment_id
        JOIN assignment_attempts at
          ON at.project_id = c.project_id AND at.assignment_id = c.assignment_id AND at.attempt = c.attempt
        WHERE c.project_id = ? AND c.command_id = ?
      `,
          )
          .get(this.#projectId, commandId) as
          | {
              assignment_id: string;
              attempt: number;
              generation: number;
              payload_json: string;
              state: string;
              state_version: number;
              assignment_state: string;
              assignment_authority: string;
              attempt_state: string;
              attempt_authority: string;
            }
          | undefined;
        if (!command) throw new ControllerError("command does not exist");
        if (
          command.assignment_authority !== "active" ||
          command.attempt_authority !== "active"
        )
          throw new MutationConflictError(
            "contained or uncertain assignment authority cannot be dispatched",
          );
        if (command.state !== "queued" && command.state !== "attempting")
          throw new MutationConflictError(
            `command is ${command.state}; delivery cannot be retried in this state`,
          );
        const run = this.#database
          .prepare("SELECT state FROM run_controls WHERE project_id = ?")
          .get(this.#projectId) as { state: string };
        if (run.state !== "active")
          throw new ReadinessError(
            `run is ${run.state}; command dispatch is paused`,
          );
        const firstDelivery = command.state === "queued";
        if (
          firstDelivery
            ? command.assignment_state !== "created" ||
              command.attempt_state !== "created"
            : command.assignment_state !== "dispatched" ||
              command.attempt_state !== "dispatched"
        ) {
          throw new MutationConflictError(
            "assignment state does not match the durable command delivery state",
          );
        }
        if (
          firstDelivery
            ? !this.#isTransitionAllowed(
                "command",
                "queued",
                "attempting",
                actor,
              ) ||
              !this.#isTransitionAllowed(
                "assignment_attempt",
                "created",
                "dispatched",
                actor,
              )
            : !this.#isTransitionAllowed(
                "command",
                "attempting",
                "attempting",
                actor,
              )
        ) {
          throw new TransitionAuthorizationError(
            "transition table rejects command delivery",
          );
        }
        const payload = JSON.parse(command.payload_json) as {
          type: string;
          commandId: string;
          assignmentId: string;
          attempt: number;
          generation: number;
          prompt: string;
        };
        if (
          payload.type !== "dispatch" ||
          payload.commandId !== commandId ||
          payload.assignmentId !== command.assignment_id ||
          payload.attempt !== command.attempt ||
          payload.generation !== command.generation ||
          typeof payload.prompt !== "string"
        ) {
          throw new ControllerError(
            "durable command payload does not match its assignment identity",
          );
        }
        const now = new Date().toISOString();
        const ordinal = (
          this.#database
            .prepare(
              "SELECT COALESCE(MAX(ordinal), 0) + 1 AS next FROM outbox_delivery_attempts WHERE project_id = ? AND command_id = ?",
            )
            .get(this.#projectId, commandId) as { next: number }
        ).next;
        this.#database
          .prepare(
            `
        INSERT INTO outbox_delivery_attempts(project_id, command_id, ordinal, outcome, started_at, completed_at, response_json)
        VALUES (?, ?, ?, 'attempting', ?, NULL, NULL)
      `,
          )
          .run(this.#projectId, commandId, ordinal, now);
        this.#database
          .prepare(
            "UPDATE commands SET state = 'attempting', state_version = state_version + 1, updated_at = ? WHERE project_id = ? AND command_id = ?",
          )
          .run(now, this.#projectId, commandId);
        if (firstDelivery) {
          this.#database
            .prepare(
              `
          UPDATE assignments SET state = 'dispatched', state_version = state_version + 1
          WHERE project_id = ? AND assignment_id = ? AND state = 'created'
        `,
            )
            .run(this.#projectId, command.assignment_id);
          this.#database
            .prepare(
              `
          UPDATE assignment_attempts SET state = 'dispatched', state_version = state_version + 1
          WHERE project_id = ? AND assignment_id = ? AND attempt = ? AND state = 'created'
        `,
            )
            .run(this.#projectId, command.assignment_id, command.attempt);
        }
        return {
          value: { payload, ordinal },
          event: {
            entityType: "command",
            entityId: commandId,
            stateVersion: command.state_version + 1,
            fromState: command.state,
            toState: "attempting",
          },
        };
      },
    );
  }

  beginCommandStart(
    context: MutationContext,
    commandId: string,
  ): { readonly startRequested: true } {
    return this.#mutateAsController(
      context,
      "command.start.request",
      "controller:reconcile",
      { commandId },
      () => {
        const command = this.#database
          .prepare(
            `
          SELECT c.state, c.state_version, a.state AS assignment_state,
            a.authority_state AS assignment_authority, at.state AS attempt_state,
            at.authority_state AS attempt_authority
          FROM commands c JOIN assignments a
            ON a.project_id = c.project_id AND a.assignment_id = c.assignment_id
          JOIN assignment_attempts at
            ON at.project_id = c.project_id AND at.assignment_id = c.assignment_id
            AND at.attempt = c.attempt
          WHERE c.project_id = ? AND c.command_id = ?
        `,
          )
          .get(this.#projectId, commandId) as
          | {
              state: string;
              state_version: number;
              assignment_state: string;
              assignment_authority: string;
              attempt_state: string;
              attempt_authority: string;
            }
          | undefined;
        if (!command) throw new ControllerError("command does not exist");
        if (
          command.state !== "acknowledged" ||
          command.assignment_state !== "acknowledged" ||
          command.attempt_state !== "acknowledged" ||
          command.assignment_authority !== "active" ||
          command.attempt_authority !== "active"
        )
          throw new MutationConflictError(
            "start requires a durably acknowledged command with active assignment authority",
          );
        const run = this.#database
          .prepare("SELECT state FROM run_controls WHERE project_id = ?")
          .get(this.#projectId) as { state: string };
        if (run.state !== "active")
          throw new ReadinessError(
            `run is ${run.state}; command start is paused`,
          );
        const now = new Date().toISOString();
        this.#database
          .prepare(
            "UPDATE commands SET start_requested = 1, state_version = state_version + 1, updated_at = ? WHERE project_id = ? AND command_id = ?",
          )
          .run(now, this.#projectId, commandId);
        return {
          value: { startRequested: true },
          event: {
            entityType: "command",
            entityId: commandId,
            stateVersion: command.state_version + 1,
            fromState: "acknowledged",
            toState: "acknowledged",
            details: { startRequested: true },
          },
        };
      },
    );
  }
  recordBridgeReceipt(receipt: BridgeReceipt): {
    readonly duplicate: boolean;
    readonly fenced?: boolean;
  } {
    this.#assertOpen();
    const receiptTypes: readonly BridgeReceipt["type"][] = [
      "accepted",
      "submitted",
      "working",
      "tool_started",
      "tool_completed",
      "aborted",
      "dispatch_error",
      "completed",
      "agent_end_without_reply",
    ];
    if (
      !receipt ||
      !receiptTypes.includes(receipt.type) ||
      typeof receipt.role !== "string" ||
      !/^[A-Za-z0-9._-]{1,64}$/.test(receipt.role) ||
      !Number.isSafeInteger(receipt.sequence) ||
      receipt.sequence < 1 ||
      !Number.isSafeInteger(receipt.attempt) ||
      receipt.attempt < 1 ||
      !Number.isSafeInteger(receipt.generation) ||
      receipt.generation < 1 ||
      typeof receipt.commandId !== "string" ||
      typeof receipt.assignmentId !== "string" ||
      !/^[A-Za-z0-9._-]{1,128}$/.test(receipt.commandId) ||
      !/^[A-Za-z0-9._-]{1,128}$/.test(receipt.assignmentId) ||
      typeof receipt.timestamp !== "string" ||
      !Number.isFinite(Date.parse(receipt.timestamp))
    ) {
      throw new ControllerError(
        "bridge receipt has an invalid identity or timestamp",
      );
    }
    const receiptJson = canonicalJson(receipt);
    const receiptHash = digestJson(receipt);
    this.#database.exec("BEGIN IMMEDIATE");
    try {
      const prior = this.#database
        .prepare(
          `
        SELECT receipt_hash FROM command_receipts WHERE project_id = ? AND role = ? AND sequence = ?
      `,
        )
        .get(this.#projectId, receipt.role, receipt.sequence) as
        { receipt_hash: string } | undefined;
      if (prior) {
        if (prior.receipt_hash !== receiptHash)
          throw new IdempotencyConflictError(
            "bridge sequence was reused with different receipt content",
          );
        this.#database.exec("COMMIT");
        return { duplicate: true };
      }
      const lastSequence = (
        this.#database
          .prepare(
            "SELECT COALESCE(MAX(sequence), 0) AS sequence FROM command_receipts WHERE project_id = ? AND role = ?",
          )
          .get(this.#projectId, receipt.role) as { sequence: number }
      ).sequence;
      if (receipt.sequence !== lastSequence + 1)
        throw new MutationConflictError(
          "bridge receipt sequence is not the next durable sequence",
        );
      const command = this.#database
        .prepare(
          `
        SELECT c.state AS command_state, c.state_version AS command_version, c.assignment_id,
          c.attempt, c.generation, a.state AS assignment_state, a.authority_state AS assignment_authority,
          a.seat_id, a.work_item_id, a.input_revision, at.state AS attempt_state,
          at.authority_state AS attempt_authority, s.role AS seat_role, w.state AS work_state,
          w.state_version AS work_version
        FROM commands c
        JOIN assignments a ON a.project_id = c.project_id AND a.assignment_id = c.assignment_id
        JOIN assignment_attempts at ON at.project_id = c.project_id AND at.assignment_id = c.assignment_id
          AND at.attempt = c.attempt
        JOIN seats s ON s.project_id = a.project_id AND s.seat_id = a.seat_id
        JOIN work_items w ON w.project_id = a.project_id AND w.work_item_id = a.work_item_id
        WHERE c.project_id = ? AND c.command_id = ?
      `,
        )
        .get(this.#projectId, receipt.commandId) as
        | {
            command_state: string;
            command_version: number;
            assignment_id: string;
            attempt: number;
            generation: number;
            assignment_state: string;
            assignment_authority: string;
            attempt_authority: string;
            seat_id: string;
            work_item_id: string;
            input_revision: number;
            attempt_state: string;
            seat_role: string;
            work_state: string;
            work_version: number;
          }
        | undefined;
      if (
        !command ||
        command.assignment_id !== receipt.assignmentId ||
        command.attempt !== receipt.attempt ||
        command.generation !== receipt.generation ||
        command.seat_role !== receipt.role
      ) {
        throw new ControllerError(
          "bridge receipt identity does not match the durable assignment",
        );
      }
      const fenced =
        command.assignment_authority !== "active" ||
        command.attempt_authority !== "active";
      if (
        !fenced &&
        receipt.type === "completed" &&
        (command.seat_role === "PM" || command.seat_role === "Supervisor") &&
        (typeof receipt.reply !== "string" || receipt.reply.trim().length === 0)
      )
        throw new ControllerError(
          "completed PM and Supervisor reports require non-empty reply text",
        );
      const now = new Date().toISOString();
      const controller = this.#internalPrincipal();
      const receiptActor =
        !fenced &&
        (receipt.type === "submitted" || receipt.type === "completed")
          ? this.#principalForSeat(command.seat_id)
          : controller;
      this.#database
        .prepare(
          `
        INSERT INTO command_receipts(project_id, role, sequence, command_id, assignment_id, attempt, generation,
          receipt_type, receipt_json, receipt_hash, received_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
      `,
        )
        .run(
          this.#projectId,
          receipt.role,
          receipt.sequence,
          receipt.commandId,
          receipt.assignmentId,
          receipt.attempt,
          receipt.generation,
          receipt.type,
          receiptJson,
          receiptHash,
          now,
        );
      let commandToState: string | undefined;
      let attemptToState: string | undefined;
      let workToState: string | undefined;
      if (!fenced) {
        if (receipt.type === "accepted") {
          if (
            !this.#isTransitionAllowed(
              "command",
              "attempting",
              "acknowledged",
              controller,
            ) ||
            !this.#isTransitionAllowed(
              "assignment_attempt",
              "dispatched",
              "acknowledged",
              controller,
            )
          ) {
            throw new TransitionAuthorizationError(
              "transition table rejects accepted receipt",
            );
          }
          if (
            command.command_state !== "attempting" ||
            command.attempt_state !== "dispatched"
          ) {
            throw new MutationConflictError(
              "accepted receipt arrived outside command dispatch",
            );
          }
          commandToState = "acknowledged";
          attemptToState = "acknowledged";
          this.#database
            .prepare(
              "UPDATE commands SET state = ?, state_version = state_version + 1, updated_at = ? WHERE project_id = ? AND command_id = ?",
            )
            .run(commandToState, now, this.#projectId, receipt.commandId);
          this.#database
            .prepare(
              "UPDATE assignments SET state = ?, state_version = state_version + 1 WHERE project_id = ? AND assignment_id = ?",
            )
            .run(attemptToState, this.#projectId, receipt.assignmentId);
          this.#database
            .prepare(
              "UPDATE assignment_attempts SET state = ?, state_version = state_version + 1 WHERE project_id = ? AND assignment_id = ? AND attempt = ?",
            )
            .run(
              attemptToState,
              this.#projectId,
              receipt.assignmentId,
              receipt.attempt,
            );
          this.#appendOutboxOutcome(
            receipt.commandId,
            "acknowledged",
            receiptJson,
            now,
          );
        } else if (receipt.type === "working") {
          if (
            !this.#isTransitionAllowed(
              "command",
              "acknowledged",
              "started",
              controller,
            ) ||
            !this.#isTransitionAllowed(
              "assignment_attempt",
              "acknowledged",
              "running",
              controller,
            )
          ) {
            throw new TransitionAuthorizationError(
              "transition table rejects working receipt",
            );
          }
          if (
            command.command_state !== "acknowledged" ||
            command.attempt_state !== "acknowledged"
          ) {
            throw new MutationConflictError(
              "working receipt arrived outside acknowledged dispatch",
            );
          }
          commandToState = "started";
          attemptToState = "running";
          this.#database
            .prepare(
              "UPDATE commands SET state = ?, start_requested = 1, state_version = state_version + 1, updated_at = ? WHERE project_id = ? AND command_id = ?",
            )
            .run(commandToState, now, this.#projectId, receipt.commandId);
          this.#database
            .prepare(
              "UPDATE assignments SET state = ?, state_version = state_version + 1 WHERE project_id = ? AND assignment_id = ?",
            )
            .run(attemptToState, this.#projectId, receipt.assignmentId);
          this.#database
            .prepare(
              "UPDATE assignment_attempts SET state = ?, state_version = state_version + 1 WHERE project_id = ? AND assignment_id = ? AND attempt = ?",
            )
            .run(
              attemptToState,
              this.#projectId,
              receipt.assignmentId,
              receipt.attempt,
            );
        } else if (receipt.type === "submitted") {
          if (
            command.command_state !== "acknowledged" ||
            command.attempt_state !== "acknowledged" ||
            command.work_state !== "running"
          ) {
            throw new MutationConflictError(
              "submitted receipt requires a durably acknowledged worker dispatch",
            );
          }
        } else if (receipt.type === "completed") {
          if (
            command.command_state !== "started" ||
            command.attempt_state !== "running" ||
            command.work_state !== "running"
          ) {
            throw new MutationConflictError(
              "completed receipt requires an active assigned worker generation",
            );
          }
          if (
            !this.#isTransitionAllowed(
              "command",
              "started",
              "completed",
              controller,
            ) ||
            !this.#isTransitionAllowed(
              "assignment_attempt",
              "running",
              "reported",
              receiptActor,
            ) ||
            !this.#isTransitionAllowed(
              "work_item",
              "running",
              "awaiting_verification",
              receiptActor,
            )
          ) {
            throw new TransitionAuthorizationError(
              "transition table rejects completed worker report",
            );
          }
          commandToState = "completed";
          attemptToState = "reported";
          workToState = "awaiting_verification";
          this.#database
            .prepare(
              "UPDATE commands SET state = 'completed', state_version = state_version + 1, updated_at = ? WHERE project_id = ? AND command_id = ?",
            )
            .run(now, this.#projectId, receipt.commandId);
          this.#database
            .prepare(
              "UPDATE assignments SET state = 'reported', state_version = state_version + 1 WHERE project_id = ? AND assignment_id = ?",
            )
            .run(this.#projectId, receipt.assignmentId);
          this.#database
            .prepare(
              "UPDATE assignment_attempts SET state = 'reported', state_version = state_version + 1 WHERE project_id = ? AND assignment_id = ? AND attempt = ?",
            )
            .run(this.#projectId, receipt.assignmentId, receipt.attempt);
          this.#database
            .prepare(
              "UPDATE work_items SET state = 'awaiting_verification', state_version = state_version + 1 WHERE project_id = ? AND work_item_id = ?",
            )
            .run(this.#projectId, command.work_item_id);
          this.#appendOutboxOutcome(
            receipt.commandId,
            "completed",
            receiptJson,
            now,
          );
        } else if (
          receipt.type === "aborted" ||
          receipt.type === "dispatch_error" ||
          receipt.type === "agent_end_without_reply"
        ) {
          if (
            !["attempting", "acknowledged", "started"].includes(
              command.command_state,
            ) ||
            !this.#isTransitionAllowed(
              "command",
              command.command_state,
              "unknown",
              controller,
            ) ||
            !["dispatched", "acknowledged", "running"].includes(
              command.attempt_state,
            ) ||
            !this.#isTransitionAllowed(
              "assignment_attempt",
              command.attempt_state,
              "revoked",
              controller,
            )
          ) {
            throw new MutationConflictError(
              "ambiguous receipt arrived outside an active assignment",
            );
          }
          commandToState = "unknown";
          attemptToState = "revoked";
          this.#database
            .prepare(
              "UPDATE commands SET state = 'unknown', state_version = state_version + 1, updated_at = ? WHERE project_id = ? AND command_id = ?",
            )
            .run(now, this.#projectId, receipt.commandId);
          this.#database
            .prepare(
              `
          UPDATE assignments SET state = 'revoked', state_version = state_version + 1, authority_state = 'unknown'
          WHERE project_id = ? AND assignment_id = ?
        `,
            )
            .run(this.#projectId, receipt.assignmentId);
          this.#database
            .prepare(
              `
          UPDATE assignment_attempts SET state = 'revoked', state_version = state_version + 1, authority_state = 'unknown'
          WHERE project_id = ? AND assignment_id = ? AND attempt = ?
        `,
            )
            .run(this.#projectId, receipt.assignmentId, receipt.attempt);
          if (command.work_state === "running") {
            if (
              !this.#isTransitionAllowed(
                "work_item",
                "running",
                "blocked",
                controller,
              )
            ) {
              throw new TransitionAuthorizationError(
                "transition table rejects blocking uncertain work",
              );
            }
            workToState = "blocked";
            this.#database
              .prepare(
                "UPDATE work_items SET state = 'blocked', state_version = state_version + 1 WHERE project_id = ? AND work_item_id = ?",
              )
              .run(this.#projectId, command.work_item_id);
          }
          this.#appendOutboxOutcome(
            receipt.commandId,
            "unknown",
            receiptJson,
            now,
          );
        }
      }
      const nextVersion =
        (
          this.#database
            .prepare("SELECT state_version FROM projects WHERE project_id = ?")
            .get(this.#projectId) as { state_version: number }
        ).state_version + 1;
      this.#database
        .prepare("UPDATE projects SET state_version = ? WHERE project_id = ?")
        .run(nextVersion, this.#projectId);
      const eventSequence = (
        this.#database
          .prepare(
            "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM controller_events WHERE project_id = ?",
          )
          .get(this.#projectId) as { next: number }
      ).next;
      this.#database
        .prepare(
          `
        INSERT INTO controller_events(project_id, sequence, event_id, entity_type, entity_id, from_state, to_state,
          state_version, actor_id, request_id, input_revision, payload_json, created_at)
        VALUES (?, ?, ?, 'command', ?, ?, ?, ?, ?, ?, ?, ?, ?)
      `,
        )
        .run(
          this.#projectId,
          eventSequence,
          randomUUID(),
          receipt.commandId,
          command.command_state,
          commandToState ?? command.command_state,
          nextVersion,
          this.#internalActorId,
          `bridge:${receipt.role}:${receipt.sequence}`,
          command.input_revision,
          canonicalJson({
            receiptType: receipt.type,
            receiptHash,
            attemptState: attemptToState ?? command.attempt_state,
            workState: workToState ?? command.work_state,
            fenced,
          }),
          now,
        );
      this.#database.exec("COMMIT");
      return { duplicate: false, ...(fenced ? { fenced: true } : {}) };
    } catch (error) {
      this.#database.exec("ROLLBACK");
      throw error;
    }
  }

  submitCandidate(
    context: MutationContext,
    input: CandidateInput,
  ): { readonly candidateId: string; readonly reportHash: string } {
    return this.#mutate(
      context,
      "candidate.submit",
      "work:report",
      input,
      (actor) => {
        if (
          actor.role !== "Developer" ||
          !/^[a-fA-F0-9]{40}([a-fA-F0-9]{24})?$/.test(input.commitSha) ||
          !/^[a-fA-F0-9]{40}([a-fA-F0-9]{24})?$/.test(input.baseSha) ||
          input.changedScope.some((entry) => typeof entry !== "string") ||
          input.limitations.some((entry) => typeof entry !== "string")
        ) {
          throw new CandidateBindingError(
            "candidate requires a Developer actor and valid immutable commit identities",
          );
        }
        const assignment = this.#database
          .prepare(
            `
        SELECT a.work_item_id, a.seat_id, a.input_revision, a.active_generation, a.state,
          at.attempt, at.generation, at.state AS attempt_state, w.state AS work_state, w.input_revision AS work_revision
        FROM assignments a JOIN assignment_attempts at
          ON at.project_id = a.project_id AND at.assignment_id = a.assignment_id AND at.generation = a.active_generation
        JOIN work_items w ON w.project_id = a.project_id AND w.work_item_id = a.work_item_id
        WHERE a.project_id = ? AND a.assignment_id = ?
      `,
          )
          .get(this.#projectId, input.assignmentId) as
          | {
              work_item_id: string;
              seat_id: string;
              input_revision: number;
              active_generation: number;
              state: string;
              attempt: number;
              generation: number;
              attempt_state: string;
              work_state: string;
              work_revision: number;
            }
          | undefined;
        if (
          !assignment ||
          actor.seatId !== assignment.seat_id ||
          assignment.state !== "reported" ||
          assignment.attempt_state !== "reported" ||
          assignment.work_state !== "awaiting_verification" ||
          assignment.input_revision !== context.inputRevision ||
          assignment.work_revision !== context.inputRevision ||
          assignment.generation !== assignment.active_generation
        ) {
          throw new CandidateBindingError(
            "candidate must match the active reported Developer assignment and current input revision",
          );
        }
        const commitSha = input.commitSha.toLowerCase();
        const baseSha = input.baseSha.toLowerCase();
        const reportHash = digestJson({
          candidateId: input.candidateId,
          assignmentId: input.assignmentId,
          attempt: assignment.attempt,
          generation: assignment.generation,
          inputRevision: assignment.input_revision,
          commitSha,
          baseSha,
          changedScope: input.changedScope,
          limitations: input.limitations,
        });
        this.#database
          .prepare(
            `
        INSERT INTO candidates(project_id, candidate_id, assignment_id, attempt, generation, input_revision,
          commit_sha, base_sha, changed_scope_json, limitations_json, report_hash, created_by, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
      `,
          )
          .run(
            this.#projectId,
            input.candidateId,
            input.assignmentId,
            assignment.attempt,
            assignment.generation,
            assignment.input_revision,
            commitSha,
            baseSha,
            canonicalJson(input.changedScope),
            canonicalJson(input.limitations),
            reportHash,
            actor.actorId,
            new Date().toISOString(),
          );
        return {
          value: { candidateId: input.candidateId, reportHash },
          event: {
            entityType: "work_item",
            entityId: assignment.work_item_id,
            stateVersion: 0,
            toState: "candidate_submitted",
          },
        };
      },
    );
  }

  recordEvidence(
    context: MutationContext,
    verifierAssignmentId: string,
    input: EvidenceInput,
  ): { readonly evidenceId: string; readonly evidenceHash: string } {
    return this.#mutate(
      context,
      "candidate.evidence.record",
      "candidate:verify",
      { verifierAssignmentId, ...input },
      (actor) => {
        if (actor.role !== "Verifier")
          throw new TransitionAuthorizationError(
            "only a Verifier may record candidate evidence",
          );
        const candidate = this.#database
          .prepare(
            `
        SELECT c.input_revision, c.report_hash, a.work_item_id
        FROM candidates c JOIN assignments a
          ON a.project_id = c.project_id AND a.assignment_id = c.assignment_id
        WHERE c.project_id = ? AND c.candidate_id = ?
      `,
          )
          .get(this.#projectId, input.candidateId) as
          | {
              input_revision: number;
              report_hash: string;
              work_item_id: string;
            }
          | undefined;
        const verifier = this.#database
          .prepare(
            `
        SELECT a.seat_id, a.input_revision, a.state, a.authority_state, a.work_item_id, t.parent_work_item_id,
          at.state AS attempt_state, at.authority_state AS attempt_authority, s.role
        FROM assignments a
        JOIN assignment_attempts at ON at.project_id = a.project_id AND at.assignment_id = a.assignment_id
          AND at.generation = a.active_generation
        JOIN seats s ON s.project_id = a.project_id AND s.seat_id = a.seat_id
        JOIN work_items t ON t.project_id = a.project_id AND t.work_item_id = a.work_item_id
        WHERE a.project_id = ? AND a.assignment_id = ?
      `,
          )
          .get(this.#projectId, verifierAssignmentId) as
          | {
              seat_id: string;
              input_revision: number;
              state: string;
              authority_state: string;
              work_item_id: string;
              parent_work_item_id: string | null;
              attempt_state: string;
              attempt_authority: string;
              role: string;
            }
          | undefined;
        const binding = this.#database
          .prepare(
            `
        SELECT content_hash FROM assignment_input_bindings
        WHERE project_id = ? AND assignment_id = ? AND input_kind = 'candidate' AND source_id = ?
      `,
          )
          .get(this.#projectId, verifierAssignmentId, input.candidateId) as
          { content_hash: string } | undefined;
        if (
          !candidate ||
          !verifier ||
          actor.seatId !== verifier.seat_id ||
          verifier.role !== "Verifier" ||
          verifier.parent_work_item_id !== candidate.work_item_id ||
          verifier.state !== "reported" ||
          verifier.attempt_state !== "reported" ||
          verifier.authority_state !== "contained" ||
          verifier.attempt_authority !== "contained" ||
          verifier.input_revision !== context.inputRevision ||
          candidate.input_revision !== context.inputRevision ||
          !binding ||
          binding.content_hash !== candidate.report_hash
        ) {
          throw new CandidateBindingError(
            "evidence is not bound to this current candidate and reported Verifier assignment",
          );
        }
        if (typeof input.passed !== "boolean")
          throw new CandidateBindingError(
            "candidate evidence pass status must be boolean",
          );
        if (
          typeof input.artifactRef !== "string" ||
          input.artifactRef.trim().length === 0
        )
          throw new CandidateBindingError(
            "candidate evidence requires a non-empty artifact reference",
          );
        if (!this.#acceptanceCriteria().includes(input.criterion)) {
          throw new CandidateBindingError(
            "evidence criterion is not in the current acceptance revision",
          );
        }
        const evidenceHash = digestJson({
          evidenceId: input.evidenceId,
          candidateId: input.candidateId,
          verifierAssignmentId,
          inputRevision: context.inputRevision,
          criterion: input.criterion,
          passed: input.passed,
          artifactRef: input.artifactRef,
        });
        this.#database
          .prepare(
            `
        INSERT INTO candidate_evidence(project_id, evidence_id, candidate_id, verifier_assignment_id, input_revision,
          criterion, passed, artifact_ref, evidence_hash, created_by, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
      `,
          )
          .run(
            this.#projectId,
            input.evidenceId,
            input.candidateId,
            verifierAssignmentId,
            context.inputRevision,
            input.criterion,
            input.passed ? 1 : 0,
            input.artifactRef,
            evidenceHash,
            actor.actorId,
            new Date().toISOString(),
          );
        return {
          value: { evidenceId: input.evidenceId, evidenceHash },
          event: {
            entityType: "work_item",
            entityId: candidate.work_item_id,
            stateVersion: 0,
            toState: "verification_evidence_recorded",
          },
        };
      },
    );
  }

  acceptCandidate(
    context: MutationContext,
    workItemId: string,
    candidateId: string,
  ): { readonly acceptedCandidateId: string } {
    return this.#mutateAsController(
      context,
      "candidate.accept",
      "candidate:accept",
      { workItemId, candidateId },
      (actor) => {
        const candidate = this.#database
          .prepare(
            `
        SELECT c.assignment_id, c.attempt, c.generation, c.input_revision, c.report_hash, a.work_item_id,
          a.state AS assignment_state, a.authority_state, at.state AS attempt_state,
          w.state AS work_state, w.state_version AS work_version, w.input_revision AS work_revision
        FROM candidates c
        JOIN assignments a ON a.project_id = c.project_id AND a.assignment_id = c.assignment_id
        JOIN assignment_attempts at ON at.project_id = c.project_id AND at.assignment_id = c.assignment_id
          AND at.attempt = c.attempt
        JOIN work_items w ON w.project_id = c.project_id AND w.work_item_id = a.work_item_id
        WHERE c.project_id = ? AND c.candidate_id = ?
      `,
          )
          .get(this.#projectId, candidateId) as
          | {
              assignment_id: string;
              attempt: number;
              generation: number;
              input_revision: number;
              report_hash: string;
              work_item_id: string;
              assignment_state: string;
              authority_state: string;
              attempt_state: string;
              work_state: string;
              work_version: number;
              work_revision: number;
            }
          | undefined;
        if (
          !candidate ||
          candidate.work_item_id !== workItemId ||
          candidate.input_revision !== context.inputRevision ||
          candidate.work_revision !== context.inputRevision ||
          candidate.work_state !== "awaiting_verification" ||
          candidate.assignment_state !== "reported" ||
          candidate.attempt_state !== "reported" ||
          candidate.authority_state !== "contained" ||
          !this.#isTransitionAllowed(
            "work_item",
            "awaiting_verification",
            "accepted",
            actor,
          ) ||
          !this.#isTransitionAllowed(
            "assignment_attempt",
            "reported",
            "completed",
            actor,
          )
        ) {
          throw new CandidateBindingError(
            "candidate does not match the current awaiting-verification assignment",
          );
        }
        const criteria = this.#acceptanceCriteria();
        const evidence = this.#database
          .prepare(
            `
        SELECT e.verifier_assignment_id, e.criterion, e.passed, e.input_revision,
          va.work_item_id AS verifier_work_item_id, va.input_revision AS verifier_input_revision,
          va.state AS verifier_assignment_state, va.authority_state AS verifier_authority_state,
          at.attempt AS verifier_attempt, at.state AS verifier_attempt_state,
          at.authority_state AS verifier_attempt_authority,
          w.state AS verifier_work_state, b.content_hash AS bound_candidate_hash
        FROM candidate_evidence e
        JOIN assignments va ON va.project_id = e.project_id AND va.assignment_id = e.verifier_assignment_id
        JOIN assignment_attempts at ON at.project_id = va.project_id AND at.assignment_id = va.assignment_id
          AND at.generation = va.active_generation
        JOIN work_items w ON w.project_id = va.project_id AND w.work_item_id = va.work_item_id
        JOIN seats s ON s.project_id = va.project_id AND s.seat_id = va.seat_id
        JOIN assignment_input_bindings b ON b.project_id = e.project_id AND b.assignment_id = e.verifier_assignment_id
          AND b.input_kind = 'candidate' AND b.source_id = e.candidate_id
        WHERE e.project_id = ? AND e.candidate_id = ? AND s.role = 'Verifier'
      `,
          )
          .all(this.#projectId, candidateId) as Array<{
          verifier_assignment_id: string;
          criterion: string;
          passed: number;
          input_revision: number;
          verifier_work_item_id: string;
          verifier_input_revision: number;
          verifier_assignment_state: string;
          verifier_authority_state: string;
          verifier_attempt: number;
          verifier_attempt_state: string;
          verifier_attempt_authority: string;
          verifier_work_state: string;
          bound_candidate_hash: string;
        }>;
        for (const criterion of criteria) {
          const observations = evidence.filter(
            (row) => row.criterion === criterion,
          );
          if (
            observations.length === 0 ||
            observations.some(
              (row) =>
                row.passed !== 1 ||
                row.input_revision !== context.inputRevision ||
                row.verifier_input_revision !== context.inputRevision ||
                row.bound_candidate_hash !== candidate.report_hash,
            )
          ) {
            throw new CandidateBindingError(
              `candidate lacks passing current Verifier evidence for: ${criterion}`,
            );
          }
        }
        if (evidence.some((row) => !criteria.includes(row.criterion))) {
          throw new CandidateBindingError(
            "candidate has evidence for an unknown criterion",
          );
        }
        const verifierAssignments = [
          ...new Map(
            evidence.map((row) => [row.verifier_assignment_id, row]),
          ).values(),
        ];
        for (const verifier of verifierAssignments) {
          if (
            verifier.verifier_assignment_state !== "reported" ||
            verifier.verifier_attempt_state !== "reported" ||
            verifier.verifier_authority_state !== "contained" ||
            verifier.verifier_attempt_authority !== "contained" ||
            !this.#isTransitionAllowed(
              "assignment_attempt",
              "reported",
              "completed",
              actor,
            ) ||
            !this.#isTransitionAllowed(
              "work_item",
              "awaiting_verification",
              "accepted",
              actor,
            )
          ) {
            throw new CandidateBindingError(
              "Verifier assignment or work state is no longer eligible for acceptance",
            );
          }
        }
        const now = new Date().toISOString();
        this.#database
          .prepare(
            `
        UPDATE work_items SET state = 'accepted', accepted_candidate_id = ?, state_version = state_version + 1
        WHERE project_id = ? AND work_item_id = ? AND state = 'awaiting_verification'
      `,
          )
          .run(candidateId, this.#projectId, workItemId);
        this.#database
          .prepare(
            `
        UPDATE assignments SET state = 'completed', authority_state = 'contained', state_version = state_version + 1, ended_at = ?
        WHERE project_id = ? AND assignment_id = ?
      `,
          )
          .run(now, this.#projectId, candidate.assignment_id);
        this.#database
          .prepare(
            `
        UPDATE assignment_attempts SET state = 'completed', authority_state = 'contained',
          state_version = state_version + 1, ended_at = ?
        WHERE project_id = ? AND assignment_id = ? AND attempt = ?
      `,
          )
          .run(
            now,
            this.#projectId,
            candidate.assignment_id,
            candidate.attempt,
          );
        for (const verifier of verifierAssignments) {
          this.#database
            .prepare(
              `
              UPDATE work_items SET state = 'accepted', accepted_candidate_id = ?,
                state_version = state_version + 1
              WHERE project_id = ? AND work_item_id = ? AND state = 'awaiting_verification'
        `,
            )
            .run(candidateId, this.#projectId, verifier.verifier_work_item_id);
          this.#database
            .prepare(
              `
          UPDATE assignments SET state = 'completed', authority_state = 'contained',
            state_version = state_version + 1, ended_at = ?
          WHERE project_id = ? AND assignment_id = ?
        `,
            )
            .run(now, this.#projectId, verifier.verifier_assignment_id);
          this.#database
            .prepare(
              `
          UPDATE assignment_attempts SET state = 'completed', authority_state = 'contained',
            state_version = state_version + 1, ended_at = ?
          WHERE project_id = ? AND assignment_id = ? AND attempt = ?
        `,
            )
            .run(
              now,
              this.#projectId,
              verifier.verifier_assignment_id,
              verifier.verifier_attempt,
            );
        }
        return {
          value: { acceptedCandidateId: candidateId },
          event: {
            entityType: "work_item",
            entityId: workItemId,
            stateVersion: candidate.work_version + 1,
            fromState: "awaiting_verification",
            toState: "accepted",
            details: {
              verifierWorkItemIds: verifierAssignments.map(
                (row) => row.verifier_work_item_id,
              ),
            },
          },
        };
      },
    );
  }
  acceptNonCandidateReport(
    context: MutationContext,
    workItemId: string,
    assignmentId: string,
  ): { readonly acceptedWorkItemId: string } {
    return this.#mutateAsController(
      context,
      "work.report.accept",
      "candidate:accept",
      { workItemId, assignmentId },
      (actor) => {
        if (actor.role !== "controller")
          throw new TransitionAuthorizationError(
            "only the controller may accept a non-candidate work report",
          );
        const report = this.#database
          .prepare(
            `
          SELECT s.role, a.state AS assignment_state, a.authority_state AS assignment_authority,
            a.input_revision, at.attempt, at.generation, at.state AS attempt_state,
            at.authority_state AS attempt_authority, w.state AS work_state,
            w.state_version AS work_version, w.input_revision AS work_revision,
            w.accepted_candidate_id, p.current_input_revision, c.command_id,
            c.state AS command_state,
            (
              SELECT r.receipt_json FROM command_receipts r
              WHERE r.project_id = a.project_id AND r.command_id = c.command_id
                AND r.assignment_id = a.assignment_id AND r.attempt = at.attempt
                AND r.generation = at.generation AND r.role = s.role
                AND r.receipt_type = 'completed'
              ORDER BY r.sequence DESC LIMIT 1
            ) AS completed_receipt_json
          FROM assignments a
          JOIN assignment_attempts at ON at.project_id = a.project_id
            AND at.assignment_id = a.assignment_id AND at.generation = a.active_generation
          JOIN seats s ON s.project_id = a.project_id AND s.seat_id = a.seat_id
          JOIN work_items w ON w.project_id = a.project_id AND w.work_item_id = a.work_item_id
          JOIN projects p ON p.project_id = a.project_id
          JOIN commands c ON c.project_id = a.project_id AND c.assignment_id = a.assignment_id
            AND c.attempt = at.attempt AND c.generation = at.generation
          WHERE a.project_id = ? AND a.work_item_id = ? AND a.assignment_id = ?
            AND EXISTS (
              SELECT 1 FROM command_receipts r
              WHERE r.project_id = a.project_id AND r.command_id = c.command_id
                AND r.assignment_id = a.assignment_id AND r.attempt = at.attempt
                AND r.generation = at.generation AND r.role = s.role
                AND r.receipt_type = 'completed'
            )
        `,
          )
          .get(this.#projectId, workItemId, assignmentId) as
          | {
              role: string;
              assignment_state: string;
              assignment_authority: string;
              input_revision: number;
              attempt: number;
              generation: number;
              attempt_state: string;
              attempt_authority: string;
              work_state: string;
              work_version: number;
              work_revision: number;
              accepted_candidate_id: string | null;
              current_input_revision: number;
              command_id: string;
              command_state: string;
              completed_receipt_json: string | null;
            }
          | undefined;
        let reportReply: string | undefined;
        if (report?.completed_receipt_json) {
          try {
            const completion = JSON.parse(report.completed_receipt_json) as {
              readonly reply?: unknown;
            };
            if (
              typeof completion.reply === "string" &&
              completion.reply.trim().length > 0
            )
              reportReply = completion.reply;
          } catch {
            // Invalid historical receipts remain ineligible for acceptance.
          }
        }
        if (
          !reportReply ||
          !report ||
          (report.role !== "PM" && report.role !== "Supervisor") ||
          report.assignment_state !== "reported" ||
          report.assignment_authority !== "contained" ||
          report.attempt_state !== "reported" ||
          report.attempt_authority !== "contained" ||
          report.command_state !== "completed" ||
          report.work_state !== "awaiting_verification" ||
          report.accepted_candidate_id !== null ||
          report.input_revision !== context.inputRevision ||
          report.work_revision !== context.inputRevision ||
          report.current_input_revision !== context.inputRevision ||
          !this.#isTransitionAllowed(
            "work_item",
            "awaiting_verification",
            "accepted",
            actor,
          ) ||
          !this.#isTransitionAllowed(
            "assignment_attempt",
            "reported",
            "completed",
            actor,
          )
        ) {
          throw new MutationConflictError(
            "non-candidate report is not contained, current, and eligible for acceptance",
          );
        }
        const now = new Date().toISOString();
        this.#database
          .prepare(
            `
          UPDATE work_items SET state = 'accepted', state_version = state_version + 1
          WHERE project_id = ? AND work_item_id = ? AND state = 'awaiting_verification'
            AND accepted_candidate_id IS NULL
        `,
          )
          .run(this.#projectId, workItemId);
        this.#database
          .prepare(
            `
          UPDATE assignments SET state = 'completed', state_version = state_version + 1,
            ended_at = ? WHERE project_id = ? AND assignment_id = ? AND state = 'reported'
        `,
          )
          .run(now, this.#projectId, assignmentId);
        this.#database
          .prepare(
            `
          UPDATE assignment_attempts SET state = 'completed', state_version = state_version + 1,
            ended_at = ? WHERE project_id = ? AND assignment_id = ? AND attempt = ? AND state = 'reported'
        `,
          )
          .run(now, this.#projectId, assignmentId, report.attempt);
        return {
          value: { acceptedWorkItemId: workItemId },
          event: {
            entityType: "work_item",
            entityId: workItemId,
            stateVersion: report.work_version + 1,
            fromState: "awaiting_verification",
            toState: "accepted",
            details: {
              assignmentId,
              commandId: report.command_id,
              role: report.role,
            },
          },
        };
      },
    );
  }

  confirmContainment(
    context: MutationContext,
    assignmentId: string,
    proofRef: string,
    bridgeSnapshot?: {
      readonly commandId: string;
      readonly bridgeState: string;
      readonly durable: boolean;
    },
  ): { readonly contained: true } {
    return this.#mutateAsController(
      context,
      "assignment.containment.confirmed",
      "recovery:write",
      { assignmentId, proofRef, bridgeSnapshot: bridgeSnapshot ?? null },
      (actor) => {
        if (
          actor.role !== "controller" ||
          typeof proofRef !== "string" ||
          proofRef.trim().length === 0 ||
          proofRef.length > 2048
        ) {
          throw new TransitionAuthorizationError(
            "only an authenticated controller may record a bounded containment proof reference",
          );
        }
        const assignment = this.#database
          .prepare(
            `
          SELECT a.state, a.authority_state, a.active_generation,
            at.state AS attempt_state, at.authority_state AS attempt_authority,
            at.state_version AS attempt_version, c.command_id, c.state AS command_state,
            c.start_requested, w.work_item_id, w.state AS work_state, w.state_version AS work_version
          FROM assignments a JOIN assignment_attempts at
            ON at.project_id = a.project_id AND at.assignment_id = a.assignment_id
            AND at.generation = a.active_generation
          JOIN commands c ON c.project_id = a.project_id AND c.assignment_id = a.assignment_id
            AND c.generation = a.active_generation
          JOIN work_items w ON w.project_id = a.project_id AND w.work_item_id = a.work_item_id
          WHERE a.project_id = ? AND a.assignment_id = ?
        `,
          )
          .get(this.#projectId, assignmentId) as
          | {
              state: string;
              authority_state: string;
              active_generation: number;
              attempt_state: string;
              attempt_authority: string;
              attempt_version: number;
              command_id: string;
              command_state: string;
              start_requested: number;
              work_item_id: string;
              work_state: string;
              work_version: number;
            }
          | undefined;
        const prestartUncertain =
          assignment?.authority_state === "unknown" &&
          assignment.command_state === "unknown" &&
          assignment.start_requested === 0;
        const reconciledPrestart =
          prestartUncertain &&
          bridgeSnapshot?.commandId === assignment.command_id &&
          M1BridgeAdapter.isVerifiedSnapshot(bridgeSnapshot) &&
          (((bridgeSnapshot.bridgeState === "acknowledged" ||
            bridgeSnapshot.bridgeState === "completed") &&
            bridgeSnapshot.durable === true) ||
            (bridgeSnapshot.bridgeState === "unknown" &&
              typeof bridgeSnapshot.durable === "boolean"));
        if (
          assignment?.command_state === "attempting" ||
          (prestartUncertain && !reconciledPrestart) ||
          (bridgeSnapshot && !reconciledPrestart)
        )
          throw new MutationConflictError(
            "M1 start was not durably requested; reconcile the same command through the bridge before containment",
          );
        const restoresReportedWork =
          assignment?.authority_state === "unknown" &&
          assignment.state === "reported" &&
          assignment.attempt_state === "reported" &&
          assignment.command_state === "completed" &&
          assignment.work_state === "blocked";
        if (
          !assignment ||
          (assignment.authority_state !== "active" &&
            assignment.authority_state !== "unknown") ||
          assignment.attempt_authority !== assignment.authority_state ||
          (assignment.authority_state === "active" &&
            assignment.state !== "running" &&
            assignment.state !== "reported") ||
          ((restoresReportedWork || assignment.work_state === "running") &&
            !this.#isTransitionAllowed(
              "work_item",
              assignment.work_state,
              restoresReportedWork ? "awaiting_verification" : "blocked",
              actor,
            ))
        ) {
          throw new MutationConflictError(
            "assignment has no running or uncertain authority to contain",
          );
        }
        this.#database
          .prepare(
            `
          UPDATE assignments SET authority_state = 'contained', containment_proof_ref = ?,
            state_version = state_version + 1 WHERE project_id = ? AND assignment_id = ?
        `,
          )
          .run(proofRef, this.#projectId, assignmentId);
        this.#database
          .prepare(
            `
          UPDATE assignment_attempts SET authority_state = 'contained', state_version = state_version + 1
          WHERE project_id = ? AND assignment_id = ? AND generation = ?
        `,
          )
          .run(this.#projectId, assignmentId, assignment.active_generation);
        if (restoresReportedWork) {
          this.#database
            .prepare(
              `
            UPDATE work_items SET state = 'awaiting_verification', state_version = state_version + 1
            WHERE project_id = ? AND work_item_id = ? AND state = 'blocked'
          `,
            )
            .run(this.#projectId, assignment.work_item_id);
        }
        if (assignment.work_state === "running") {
          this.#database
            .prepare(
              "UPDATE work_items SET state = 'blocked', state_version = state_version + 1 WHERE project_id = ? AND work_item_id = ? AND state = 'running'",
            )
            .run(this.#projectId, assignment.work_item_id);
        }
        return {
          value: { contained: true },
          event:
            restoresReportedWork || assignment.work_state === "running"
              ? {
                  entityType: "work_item",
                  entityId: assignment.work_item_id,
                  stateVersion: assignment.work_version + 1,
                  fromState: assignment.work_state,
                  toState: restoresReportedWork
                    ? "awaiting_verification"
                    : "blocked",
                  details: {
                    assignmentId,
                    proofRef,
                    authorityState: "contained",
                    bridgeSnapshot: bridgeSnapshot ?? null,
                  },
                }
              : {
                  entityType: "assignment_attempt",
                  entityId: assignmentId,
                  stateVersion: assignment.attempt_version + 1,
                  fromState: assignment.authority_state,
                  toState: "contained",
                  details: { proofRef, bridgeSnapshot: bridgeSnapshot ?? null },
                },
        };
      },
    );
  }

  recordRecovery(
    context: MutationContext,
    input: RecoveryInput,
  ): {
    readonly recoveryId: string;
    readonly outcome: "pending" | "blocked";
    readonly limit: number;
  } {
    return this.#mutateAsController(
      context,
      "recovery.request",
      "recovery:write",
      input,
      (actor) => {
        if (
          actor.role !== "controller" ||
          input.reason.trim().length === 0 ||
          (input.recoveryType === "finding_correction") !==
            Boolean(input.findingId)
        ) {
          throw new TransitionAuthorizationError(
            "recovery requires a controller, reason, and matching finding identity",
          );
        }
        const assignment = this.#database
          .prepare(
            `
        SELECT work_item_id, active_generation, authority_state, containment_proof_ref
        FROM assignments WHERE project_id = ? AND assignment_id = ?
      `,
          )
          .get(this.#projectId, input.assignmentId) as
          | {
              work_item_id: string;
              active_generation: number;
              authority_state: string;
              containment_proof_ref: string | null;
            }
          | undefined;
        if (!assignment || assignment.work_item_id !== input.workItemId)
          throw new ControllerError(
            "recovery assignment does not belong to the work item",
          );
        if (input.findingId) {
          const finding = this.#database
            .prepare(
              "SELECT work_item_id FROM findings WHERE project_id = ? AND finding_id = ?",
            )
            .get(this.#projectId, input.findingId) as
            { work_item_id: string } | undefined;
          if (!finding || finding.work_item_id !== input.workItemId)
            throw new ControllerError(
              "recovery finding does not belong to the work item",
            );
        }
        const limit = this.#recoveryLimit(input.recoveryType);
        const count =
          input.recoveryType === "finding_correction"
            ? (
                this.#database
                  .prepare(
                    `
            SELECT COUNT(*) AS count FROM recovery_attempts
            WHERE project_id = ? AND recovery_type = ? AND finding_id = ? AND outcome IN ('pending', 'replacement_created')
          `,
                  )
                  .get(
                    this.#projectId,
                    input.recoveryType,
                    input.findingId,
                  ) as { count: number }
              ).count
            : (
                this.#database
                  .prepare(
                    `
            SELECT COUNT(*) AS count FROM recovery_attempts
            WHERE project_id = ? AND recovery_type = ? AND work_item_id = ? AND outcome IN ('pending', 'replacement_created')
          `,
                  )
                  .get(
                    this.#projectId,
                    input.recoveryType,
                    input.workItemId,
                  ) as { count: number }
              ).count;
        const replacementContained =
          input.recoveryType !== "worker_replacement" ||
          (assignment.authority_state === "contained" &&
            assignment.containment_proof_ref !== null);
        const outcome =
          count >= limit || !replacementContained ? "blocked" : "pending";
        const now = new Date().toISOString();
        this.#database
          .prepare(
            `
        INSERT INTO recovery_attempts(project_id, recovery_id, work_item_id, assignment_id, recovery_type, finding_id,
          generation, reason, containment_state, containment_proof_ref, outcome, created_by, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
      `,
          )
          .run(
            this.#projectId,
            input.recoveryId,
            input.workItemId,
            input.assignmentId,
            input.recoveryType,
            input.findingId ?? null,
            assignment.active_generation,
            input.reason,
            assignment.authority_state === "contained"
              ? "contained"
              : "unknown",
            assignment.containment_proof_ref,
            outcome,
            actor.actorId,
            now,
          );
        return {
          value: { recoveryId: input.recoveryId, outcome, limit },
          event: {
            entityType: "recovery_attempt",
            entityId: input.recoveryId,
            stateVersion: 0,
            toState: outcome,
            details: {
              recoveryType: input.recoveryType,
              containmentState: assignment.authority_state,
            },
          },
        };
      },
    );
  }
  transitionRun(
    context: MutationContext,
    toState: RunState,
  ): { readonly state: RunState } {
    const apply = (actor: AuthenticatedActor, caller: AuthenticatedActor) => {
      const run = this.#database
        .prepare(
          "SELECT state, state_version FROM run_controls WHERE project_id = ?",
        )
        .get(this.#projectId) as
        { state: string; state_version: number } | undefined;
      if (!run) throw new ControllerError("run control record is missing");
      if (toState === "completed") {
        const unfinishedWork = this.#database
          .prepare(
            "SELECT 1 AS present FROM work_items WHERE project_id = ? AND state NOT IN ('accepted', 'canceled') LIMIT 1",
          )
          .get(this.#projectId);
        const uncertainAuthority = this.#database
          .prepare(
            "SELECT 1 AS present FROM assignments WHERE project_id = ? AND authority_state IN ('active', 'unknown') LIMIT 1",
          )
          .get(this.#projectId);
        if (unfinishedWork || uncertainAuthority)
          throw new MutationConflictError(
            "run cannot complete while work is open or assignment authority is not contained",
          );
      }
      if (
        !this.#isTransitionAllowed("run_control", run.state, toState, actor)
      ) {
        throw new TransitionAuthorizationError(
          `transition table rejects run state ${run.state} -> ${toState}`,
        );
      }
      const now = new Date().toISOString();
      this.#database
        .prepare(
          "UPDATE run_controls SET state = ?, state_version = state_version + 1, updated_at = ? WHERE project_id = ? AND state = ?",
        )
        .run(toState, now, this.#projectId, run.state);
      if (caller.role === "operator") {
        this.#database
          .prepare(
            `
          INSERT INTO operator_actions(project_id, action_id, action_type, target_id, actor_id, request_id,
            input_revision, payload_json, created_at)
          VALUES (?, ?, 'run.transition', ?, ?, ?, ?, ?, ?)
        `,
          )
          .run(
            this.#projectId,
            randomUUID(),
            this.#projectId,
            caller.actorId,
            context.requestId,
            context.inputRevision,
            canonicalJson({ from: run.state, to: toState }),
            now,
          );
      }
      return {
        value: { state: toState },
        event: {
          entityType: "run_control",
          entityId: this.#projectId,
          stateVersion: run.state_version + 1,
          fromState: run.state,
          toState,
        },
      };
    };
    if (
      toState === "canceled" ||
      toState === "completed" ||
      toState === "failed"
    ) {
      return this.#mutateAsController(
        context,
        "run.transition",
        "run:control",
        { toState },
        apply,
      );
    }
    return this.#mutate(
      context,
      "run.transition",
      "run:control",
      { toState },
      (actor) => apply(actor, actor),
    );
  }

  createRuntimeSession(
    context: MutationContext,
    input: RuntimeSessionInput,
  ): { readonly sessionId: string } {
    return this.#mutateAsController(
      context,
      "runtime.session.create",
      "controller:reconcile",
      input,
      (actor) => {
        if (
          actor.role !== "controller" ||
          [
            input.sessionId,
            input.seatId,
            input.provider,
            input.profile,
            input.workspace,
          ].some(
            (value) => typeof value !== "string" || value.trim().length === 0,
          )
        ) {
          throw new TransitionAuthorizationError(
            "runtime session requires a controller and complete identity fields",
          );
        }
        const seat = this.#database
          .prepare(
            "SELECT state FROM seats WHERE project_id = ? AND seat_id = ?",
          )
          .get(this.#projectId, input.seatId) as { state: string } | undefined;
        if (!seat || seat.state !== "active")
          throw new ControllerError("runtime session requires an active seat");
        if (input.assignmentId) {
          const assignment = this.#database
            .prepare(
              "SELECT seat_id, authority_state FROM assignments WHERE project_id = ? AND assignment_id = ?",
            )
            .get(this.#projectId, input.assignmentId) as
            { seat_id: string; authority_state: string } | undefined;
          if (
            !assignment ||
            assignment.seat_id !== input.seatId ||
            assignment.authority_state !== "active"
          ) {
            throw new ControllerError(
              "runtime session assignment must be active and bound to the same seat",
            );
          }
        }
        const now = new Date().toISOString();
        this.#database
          .prepare(
            `
        INSERT INTO runtime_sessions(project_id, session_id, seat_id, assignment_id, state, state_version,
          provider, profile, workspace, started_at, ended_at)
        VALUES (?, ?, ?, ?, 'starting', 0, ?, ?, ?, ?, NULL)
      `,
          )
          .run(
            this.#projectId,
            input.sessionId,
            input.seatId,
            input.assignmentId ?? null,
            input.provider,
            input.profile,
            input.workspace,
            now,
          );
        return {
          value: { sessionId: input.sessionId },
          event: {
            entityType: "runtime_session",
            entityId: input.sessionId,
            stateVersion: 0,
            toState: "starting",
          },
        };
      },
    );
  }

  recordRuntimeIdentity(
    context: MutationContext,
    sessionId: string,
    identity: RuntimeIdentityInput,
  ): { readonly observationId: string; readonly observedAt: string } {
    return this.#mutateAsController(
      context,
      "runtime.identity.observe",
      "controller:reconcile",
      { sessionId, identity },
      (actor) => {
        if (actor.role !== "controller")
          throw new TransitionAuthorizationError(
            "only the controller records runtime identities",
          );
        const fields = [
          identity.processStartId,
          identity.containerId,
          identity.cgroupPath,
          identity.endpoint,
        ];
        if (
          fields.some(
            (field) =>
              field !== undefined &&
              (typeof field !== "string" || field.trim().length === 0),
          ) ||
          (!Number.isSafeInteger(identity.observedPid) &&
            fields.every((field) => field === undefined)) ||
          (identity.observedPid !== undefined &&
            (!Number.isSafeInteger(identity.observedPid) ||
              identity.observedPid <= 0))
        ) {
          throw new ControllerError(
            "runtime identity requires a positive PID or non-empty process identity",
          );
        }
        const session = this.#database
          .prepare(
            "SELECT state_version FROM runtime_sessions WHERE project_id = ? AND session_id = ?",
          )
          .get(this.#projectId, sessionId) as
          { state_version: number } | undefined;
        if (!session)
          throw new ControllerError("runtime session does not exist");
        const observationId = randomUUID();
        const observedAt = new Date().toISOString();
        this.#database
          .prepare(
            `
        INSERT INTO runtime_identities(project_id, observation_id, session_id, observed_pid, process_start_id,
          container_id, cgroup_path, endpoint, observed_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
      `,
          )
          .run(
            this.#projectId,
            observationId,
            sessionId,
            identity.observedPid ?? null,
            identity.processStartId ?? null,
            identity.containerId ?? null,
            identity.cgroupPath ?? null,
            identity.endpoint ?? null,
            observedAt,
          );
        return {
          value: { observationId, observedAt },
          event: {
            entityType: "runtime_session",
            entityId: sessionId,
            stateVersion: session.state_version,
            details: { observationId, observedAt },
          },
        };
      },
    );
  }

  transitionRuntimeSession(
    context: MutationContext,
    sessionId: string,
    toState: RuntimeState,
  ): { readonly state: RuntimeState } {
    return this.#mutateAsController(
      context,
      "runtime.session.transition",
      "controller:reconcile",
      { sessionId, toState },
      (actor) => {
        if (actor.role !== "controller")
          throw new TransitionAuthorizationError(
            "only the controller transitions runtime sessions",
          );
        const session = this.#database
          .prepare(
            "SELECT state, state_version FROM runtime_sessions WHERE project_id = ? AND session_id = ?",
          )
          .get(this.#projectId, sessionId) as
          { state: string; state_version: number } | undefined;
        if (!session)
          throw new ControllerError("runtime session does not exist");
        if (
          !this.#isTransitionAllowed(
            "runtime_session",
            session.state,
            toState,
            actor,
          )
        ) {
          throw new TransitionAuthorizationError(
            `transition table rejects runtime state ${session.state} -> ${toState}`,
          );
        }
        const endedAt = toState === "exited" ? new Date().toISOString() : null;
        this.#database
          .prepare(
            "UPDATE runtime_sessions SET state = ?, state_version = state_version + 1, ended_at = COALESCE(?, ended_at) WHERE project_id = ? AND session_id = ?",
          )
          .run(toState, endedAt, this.#projectId, sessionId);
        return {
          value: { state: toState },
          event: {
            entityType: "runtime_session",
            entityId: sessionId,
            stateVersion: session.state_version + 1,
            fromState: session.state,
            toState,
          },
        };
      },
    );
  }

  recordUsage(
    context: MutationContext,
    input: UsageInput,
  ): { readonly observationId: string } {
    return this.#mutate(
      context,
      "usage.record",
      "usage:write",
      input,
      (actor) => {
        if (
          input.provider.trim().length === 0 ||
          input.metric.trim().length === 0 ||
          (input.value !== undefined && !Number.isFinite(input.value))
        ) {
          throw new ControllerError(
            "usage provider, metric, and numeric value must be valid",
          );
        }
        if (
          input.sessionId &&
          !this.#database
            .prepare(
              "SELECT 1 AS present FROM runtime_sessions WHERE project_id = ? AND session_id = ?",
            )
            .get(this.#projectId, input.sessionId)
        )
          throw new ControllerError(
            "usage session does not belong to this project",
          );
        if (
          input.assignmentId &&
          !this.#database
            .prepare(
              "SELECT 1 AS present FROM assignments WHERE project_id = ? AND assignment_id = ?",
            )
            .get(this.#projectId, input.assignmentId)
        )
          throw new ControllerError(
            "usage assignment does not belong to this project",
          );
        this.#database
          .prepare(
            `
        INSERT INTO usage_observations(project_id, observation_id, session_id, assignment_id, provider, metric,
          value, availability, detail_json, observed_at, created_by)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
      `,
          )
          .run(
            this.#projectId,
            input.observationId,
            input.sessionId ?? null,
            input.assignmentId ?? null,
            input.provider,
            input.metric,
            input.value ?? null,
            input.availability,
            canonicalJson(input.detail),
            new Date().toISOString(),
            actor.actorId,
          );
        return {
          value: { observationId: input.observationId },
          event: {
            entityType: "usage_observation",
            entityId: input.observationId,
            stateVersion: 0,
          },
        };
      },
    );
  }

  createFinding(
    context: MutationContext,
    input: FindingInput,
  ): { readonly findingId: string } {
    return this.#mutate(
      context,
      "finding.create",
      "finding:write",
      input,
      (actor) => {
        if (actor.role !== "Supervisor")
          throw new TransitionAuthorizationError(
            "only the Supervisor creates findings",
          );
        const assignment = this.#database
          .prepare(
            `
        SELECT a.work_item_id, a.seat_id, a.active_generation, a.authority_state, s.role
        FROM assignments a JOIN seats s ON s.project_id = a.project_id AND s.seat_id = a.seat_id
        WHERE a.project_id = ? AND a.assignment_id = ?
      `,
          )
          .get(this.#projectId, input.assignmentId) as
          | {
              work_item_id: string;
              seat_id: string;
              active_generation: number;
              authority_state: string;
              role: string;
            }
          | undefined;
        if (
          !assignment ||
          assignment.work_item_id !== input.workItemId ||
          assignment.seat_id !== actor.seatId ||
          assignment.role !== "Supervisor" ||
          assignment.active_generation !== input.generation ||
          assignment.authority_state !== "active"
        ) {
          throw new ControllerError(
            "finding must be bound to the active Supervisor assignment generation",
          );
        }
        const now = new Date().toISOString();
        this.#database
          .prepare(
            `
        INSERT INTO findings(project_id, finding_id, work_item_id, assignment_id, generation, fingerprint, severity,
          evidence_json, requested_correction, resolution_condition, state, state_version, created_by, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'detected', 0, ?, ?)
      `,
          )
          .run(
            this.#projectId,
            input.findingId,
            input.workItemId,
            input.assignmentId,
            input.generation,
            input.fingerprint,
            input.severity,
            canonicalJson(input.evidence),
            input.requestedCorrection,
            input.resolutionCondition,
            actor.actorId,
            now,
          );
        return {
          value: { findingId: input.findingId },
          event: {
            entityType: "finding",
            entityId: input.findingId,
            stateVersion: 0,
            toState: "detected",
          },
        };
      },
    );
  }

  transitionFinding(
    context: MutationContext,
    findingId: string,
    toState: FindingState,
    content: unknown,
  ): { readonly state: FindingState } {
    const apply = (actor: AuthenticatedActor) => {
      const finding = this.#database
        .prepare(
          `
        SELECT work_item_id, assignment_id, resolution_condition, state, state_version
        FROM findings WHERE project_id = ? AND finding_id = ?
      `,
        )
        .get(this.#projectId, findingId) as
        | {
            work_item_id: string;
            assignment_id: string | null;
            resolution_condition: string;
            state: string;
            state_version: number;
          }
        | undefined;
      if (!finding) throw new ControllerError("finding does not exist");
      if (
        !this.#isTransitionAllowed("finding", finding.state, toState, actor)
      ) {
        throw new TransitionAuthorizationError(
          `transition table rejects finding state ${finding.state} -> ${toState}`,
        );
      }
      const now = new Date().toISOString();
      if (
        toState === "reported" ||
        toState === "acknowledged" ||
        toState === "disputed"
      ) {
        const responseAssignment =
          actor.role === "controller"
            ? (finding.assignment_id ?? undefined)
            : actor.seatId
              ? toState === "reported"
                ? (
                    this.#database
                      .prepare(
                        `
                    SELECT assignment_id FROM assignments
                    WHERE project_id = ? AND assignment_id = ? AND work_item_id = ?
                      AND seat_id = ? AND authority_state = 'active'
                  `,
                      )
                      .get(
                        this.#projectId,
                        finding.assignment_id,
                        finding.work_item_id,
                        actor.seatId,
                      ) as { assignment_id: string } | undefined
                  )?.assignment_id
                : (
                    this.#database
                      .prepare(
                        `
                    SELECT a.assignment_id FROM assignments a
                    JOIN work_items responding_work ON responding_work.project_id = a.project_id
                      AND responding_work.work_item_id = a.work_item_id
                    WHERE a.project_id = ? AND responding_work.parent_work_item_id = ?
                      AND a.seat_id = ? AND a.authority_state = 'active'
                    ORDER BY a.created_at DESC, a.assignment_id DESC LIMIT 1
                  `,
                      )
                      .get(
                        this.#projectId,
                        finding.work_item_id,
                        actor.seatId,
                      ) as { assignment_id: string } | undefined
                  )?.assignment_id
              : undefined;
        if (!responseAssignment)
          throw new ControllerError(
            "finding response requires its source assignment or an active responding child assignment",
          );
        const responseType =
          toState === "reported"
            ? "report"
            : toState === "acknowledged"
              ? "acknowledged"
              : "dispute";
        this.#database
          .prepare(
            `
          INSERT INTO finding_responses(project_id, response_id, finding_id, assignment_id, response_type,
            content_json, created_by, created_at)
          VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        `,
          )
          .run(
            this.#projectId,
            randomUUID(),
            findingId,
            responseAssignment,
            responseType,
            canonicalJson(content),
            actor.actorId,
            now,
          );
      } else if (toState === "resolved" || toState === "escalated") {
        if (toState === "resolved") {
          const resolution = content as {
            condition?: unknown;
            evidence?: unknown;
          } | null;
          if (
            !resolution ||
            typeof resolution !== "object" ||
            Array.isArray(content) ||
            resolution.condition !== finding.resolution_condition ||
            typeof resolution.evidence !== "string" ||
            resolution.evidence.trim().length === 0
          ) {
            throw new ControllerError(
              "finding resolution must repeat its condition and include non-empty evidence",
            );
          }
        }
        this.#database
          .prepare(
            `
          INSERT INTO finding_dispositions(project_id, disposition_id, finding_id, disposition, content_json, created_by, created_at)
          VALUES (?, ?, ?, ?, ?, ?, ?)
        `,
          )
          .run(
            this.#projectId,
            randomUUID(),
            findingId,
            toState,
            canonicalJson(content),
            actor.actorId,
            now,
          );
      }
      this.#database
        .prepare(
          "UPDATE findings SET state = ?, state_version = state_version + 1 WHERE project_id = ? AND finding_id = ?",
        )
        .run(toState, this.#projectId, findingId);
      return {
        value: { state: toState },
        event: {
          entityType: "finding",
          entityId: findingId,
          stateVersion: finding.state_version + 1,
          fromState: finding.state,
          toState,
        },
      };
    };
    if (
      toState === "correcting" ||
      toState === "resolved" ||
      toState === "escalated"
    ) {
      return this.#mutateAsController(
        context,
        "finding.transition",
        "finding:write",
        { findingId, toState, content },
        apply,
      );
    }
    return this.#mutate(
      context,
      "finding.transition",
      "finding:write",
      { findingId, toState, content },
      apply,
    );
  }

  commandState(commandId: string): string | undefined {
    this.#assertOpen();
    const command = this.#database
      .prepare(
        "SELECT state FROM commands WHERE project_id = ? AND command_id = ?",
      )
      .get(this.#projectId, commandId) as { state: string } | undefined;
    return command?.state;
  }

  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    try {
      this.#database.close();
    } finally {
      this.#lock.close();
    }
  }

  #reconcileUncertainAssignments(): void {
    this.#database.exec("BEGIN IMMEDIATE");
    try {
      const rows = this.#database
        .prepare(
          `
        SELECT c.command_id, c.state AS command_state, c.start_requested,
          a.assignment_id, a.work_item_id, a.state AS assignment_state,
          at.attempt, at.state AS attempt_state, at.state_version AS attempt_version,
          w.state AS work_state
        FROM commands c
        JOIN assignments a ON a.project_id = c.project_id AND a.assignment_id = c.assignment_id
        JOIN assignment_attempts at ON at.project_id = c.project_id
          AND at.assignment_id = c.assignment_id AND at.attempt = c.attempt
        JOIN work_items w ON w.project_id = a.project_id AND w.work_item_id = a.work_item_id
        WHERE c.project_id = ? AND c.state IN ('attempting', 'acknowledged', 'started', 'completed')
          AND a.authority_state = 'active'
          AND at.state IN ('dispatched', 'acknowledged', 'running', 'reported')
        ORDER BY c.command_id
      `,
        )
        .all(this.#projectId) as Array<{
        command_id: string;
        command_state: string;
        start_requested: number;
        assignment_id: string;
        work_item_id: string;
        assignment_state: string;
        attempt: number;
        attempt_state: string;
        attempt_version: number;
        work_state: string;
      }>;
      const controller = this.#internalPrincipal();
      for (const row of rows) {
        const preserveReportedCompletion =
          row.command_state === "completed" &&
          row.assignment_state === "reported" &&
          row.attempt_state === "reported";
        if (
          row.command_state !== "completed" &&
          !this.#isTransitionAllowed(
            "command",
            row.command_state,
            "unknown",
            controller,
          )
        ) {
          throw new TransitionAuthorizationError(
            `transition table rejects restart reconciliation of ${row.command_state} command`,
          );
        }
        if (
          !preserveReportedCompletion &&
          !this.#isTransitionAllowed(
            "assignment_attempt",
            row.attempt_state,
            "revoked",
            controller,
          )
        ) {
          throw new TransitionAuthorizationError(
            `transition table rejects restart reconciliation of ${row.attempt_state} attempt`,
          );
        }
        if (
          (row.work_state === "running" ||
            row.work_state === "awaiting_verification") &&
          !this.#isTransitionAllowed(
            "work_item",
            row.work_state,
            "blocked",
            controller,
          )
        ) {
          throw new TransitionAuthorizationError(
            `transition table rejects restart blocking of ${row.work_state} work`,
          );
        }
        const now = new Date().toISOString();
        if (row.command_state !== "completed") {
          this.#database
            .prepare(
              "UPDATE commands SET state = 'unknown', state_version = state_version + 1, updated_at = ? WHERE project_id = ? AND command_id = ?",
            )
            .run(now, this.#projectId, row.command_id);
          this.#appendOutboxOutcome(
            row.command_id,
            "unknown",
            canonicalJson({
              reason:
                "controller restarted before worker authority was contained",
            }),
            now,
          );
        }
        if (preserveReportedCompletion) {
          this.#database
            .prepare(
              "UPDATE assignments SET authority_state = 'unknown', state_version = state_version + 1 WHERE project_id = ? AND assignment_id = ?",
            )
            .run(this.#projectId, row.assignment_id);
          this.#database
            .prepare(
              "UPDATE assignment_attempts SET authority_state = 'unknown', state_version = state_version + 1 WHERE project_id = ? AND assignment_id = ? AND attempt = ?",
            )
            .run(this.#projectId, row.assignment_id, row.attempt);
        } else {
          this.#database
            .prepare(
              `
            UPDATE assignments SET state = 'revoked', state_version = state_version + 1,
              authority_state = 'unknown', ended_at = COALESCE(ended_at, ?)
            WHERE project_id = ? AND assignment_id = ?
          `,
            )
            .run(now, this.#projectId, row.assignment_id);
          this.#database
            .prepare(
              `
            UPDATE assignment_attempts SET state = 'revoked', state_version = state_version + 1,
              authority_state = 'unknown', ended_at = COALESCE(ended_at, ?)
            WHERE project_id = ? AND assignment_id = ? AND attempt = ?
          `,
            )
            .run(now, this.#projectId, row.assignment_id, row.attempt);
        }
        if (
          row.work_state === "running" ||
          row.work_state === "awaiting_verification"
        ) {
          this.#database
            .prepare(
              "UPDATE work_items SET state = 'blocked', state_version = state_version + 1 WHERE project_id = ? AND work_item_id = ?",
            )
            .run(this.#projectId, row.work_item_id);
        }
        const projectVersion =
          (
            this.#database
              .prepare(
                "SELECT state_version FROM projects WHERE project_id = ?",
              )
              .get(this.#projectId) as { state_version: number }
          ).state_version + 1;
        const sequence = (
          this.#database
            .prepare(
              "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM controller_events WHERE project_id = ?",
            )
            .get(this.#projectId) as { next: number }
        ).next;
        this.#database
          .prepare("UPDATE projects SET state_version = ? WHERE project_id = ?")
          .run(projectVersion, this.#projectId);
        this.#database
          .prepare(
            `
          INSERT INTO controller_events(project_id, sequence, event_id, entity_type, entity_id, from_state, to_state,
            state_version, actor_id, request_id, input_revision, payload_json, created_at)
          SELECT ?, ?, ?, 'assignment_attempt', ?, ?, ?, ?, ?, ?, current_input_revision, ?, ?
          FROM projects WHERE project_id = ?
        `,
          )
          .run(
            this.#projectId,
            sequence,
            randomUUID(),
            row.assignment_id,
            row.attempt_state,
            preserveReportedCompletion ? "reported" : "revoked",
            row.attempt_version + 1,
            this.#internalActorId,
            `restart-reconcile:${row.command_id}`,
            canonicalJson({
              commandState: row.command_state,
              authorityState: "unknown",
              preservedReport: preserveReportedCompletion,
              startRequested: row.start_requested === 1,
            }),
            now,
            this.#projectId,
          );
      }
      this.#database.exec("COMMIT");
    } catch (error) {
      this.#database.exec("ROLLBACK");
      throw error;
    }
  }

  #assertOpen(): void {
    if (this.#closed) throw new ControllerError("controller is closed");
    this.#lock.assertHeld();
  }

  #mutateAsController<T>(
    context: MutationContext,
    action: string,
    capability: Capability,
    payload: unknown,
    apply: (
      actor: AuthenticatedActor,
      caller: AuthenticatedActor,
    ) => MutationOutput<T>,
  ): T {
    return this.#mutate(context, action, capability, payload, (caller) => {
      if (caller.role !== "operator")
        throw new TransitionAuthorizationError(
          "controller operations require an authenticated operator request",
        );
      return apply(this.#internalPrincipal(), caller);
    });
  }

  #internalPrincipal(): AuthenticatedActor {
    const rows = this.#database
      .prepare(
        `
      SELECT a.actor_id, a.project_id, a.role, cg.capability
      FROM actors a JOIN capability_grants cg
        ON cg.project_id = a.project_id AND cg.actor_id = a.actor_id AND cg.revoked_at IS NULL
      WHERE a.project_id = ? AND a.actor_id = ? AND a.active = 1 AND a.revoked_at IS NULL
      ORDER BY cg.capability
    `,
      )
      .all(this.#projectId, this.#internalActorId) as Array<{
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

  #principalForSeat(seatId: string): AuthenticatedActor {
    const rows = this.#database
      .prepare(
        `
      SELECT a.actor_id, a.project_id, a.role, cg.capability
      FROM actors a JOIN capability_grants cg
        ON cg.project_id = a.project_id AND cg.actor_id = a.actor_id AND cg.revoked_at IS NULL
      WHERE a.project_id = ? AND a.seat_id = ? AND a.active = 1 AND a.revoked_at IS NULL
      ORDER BY a.actor_id, cg.capability
    `,
      )
      .all(this.#projectId, seatId) as Array<{
      actor_id: string;
      project_id: string;
      role: string;
      capability: string;
    }>;
    const actor = rows[0];
    if (!actor)
      throw new ControllerError(
        "bridge receipt has no active authenticated actor for its seat",
      );
    if (
      actor.role !== "PM" &&
      actor.role !== "Developer" &&
      actor.role !== "Verifier" &&
      actor.role !== "Supervisor"
    ) {
      throw new ControllerError("seat actor has an invalid worker role");
    }
    const actorRows = rows.filter((row) => row.actor_id === actor.actor_id);
    return {
      actorId: actor.actor_id,
      projectId: actor.project_id,
      role: actor.role as Role,
      seatId,
      capabilities: new Set(actorRows.map((row) => row.capability)),
    };
  }

  #appendOutboxOutcome(
    commandId: string,
    outcome: "acknowledged" | "unknown" | "completed" | "failed",
    responseJson: string,
    now: string,
  ): void {
    const ordinal = (
      this.#database
        .prepare(
          "SELECT COALESCE(MAX(ordinal), 0) + 1 AS next FROM outbox_delivery_attempts WHERE project_id = ? AND command_id = ?",
        )
        .get(this.#projectId, commandId) as { next: number }
    ).next;
    this.#database
      .prepare(
        `
      INSERT INTO outbox_delivery_attempts(project_id, command_id, ordinal, outcome, started_at, completed_at, response_json)
      VALUES (?, ?, ?, ?, ?, ?, ?)
    `,
      )
      .run(
        this.#projectId,
        commandId,
        ordinal,
        outcome,
        now,
        now,
        responseJson,
      );
  }

  #recoveryLimit(recoveryType: RecoveryType): number {
    let key: string;
    let fallback: number;
    switch (recoveryType) {
      case "worker_replacement":
        key = "max_worker_replacements_per_work_item";
        fallback = 1;
        break;
      case "finding_correction":
        key = "max_corrective_interventions_per_finding";
        fallback = 2;
        break;
      case "implementation_remediation":
        key = "max_implementation_remediation_cycles";
        fallback = 2;
        break;
    }
    const currentRevision = this.#database
      .prepare(
        "SELECT current_input_revision FROM projects WHERE project_id = ?",
      )
      .get(this.#projectId) as { current_input_revision: number };
    const row = this.#database
      .prepare(
        `
      SELECT content_json FROM project_revisions
      WHERE project_id = ? AND kind = 'policy' AND revision <= ?
      ORDER BY revision DESC LIMIT 1
    `,
      )
      .get(this.#projectId, currentRevision.current_input_revision) as
      { content_json: string } | undefined;
    if (!row)
      throw new ControllerError("current recovery policy revision is missing");
    const policy: unknown = JSON.parse(row.content_json);
    let recoveryConfig: unknown;
    if (typeof policy === "object" && policy !== null) {
      const descriptor = Object.getOwnPropertyDescriptor(policy, "recovery");
      if (descriptor && "value" in descriptor)
        recoveryConfig = descriptor.value;
    }
    if (recoveryConfig === undefined) return fallback;
    if (typeof recoveryConfig !== "object" || recoveryConfig === null)
      throw new ControllerError("recovery policy must be an object");
    const limitDescriptor = Object.getOwnPropertyDescriptor(
      recoveryConfig,
      key,
    );
    const value =
      limitDescriptor && "value" in limitDescriptor
        ? limitDescriptor.value
        : fallback;
    if (
      typeof value !== "number" ||
      !Number.isSafeInteger(value) ||
      value < 0 ||
      value > 100
    ) {
      throw new ControllerError(
        `recovery policy ${key} must be an integer from 0 to 100`,
      );
    }
    return value;
  }

  #acceptanceCriteria(): readonly string[] {
    const currentRevision = this.#database
      .prepare(
        "SELECT current_input_revision FROM projects WHERE project_id = ?",
      )
      .get(this.#projectId) as { current_input_revision: number };
    const row = this.#database
      .prepare(
        `
      SELECT content_json FROM project_revisions
      WHERE project_id = ? AND kind = 'acceptance_criteria' AND revision <= ?
      ORDER BY revision DESC LIMIT 1
    `,
      )
      .get(this.#projectId, currentRevision.current_input_revision) as
      { content_json: string } | undefined;
    if (!row)
      throw new CandidateBindingError(
        "current acceptance criteria revision is missing",
      );
    const content: unknown = JSON.parse(row.content_json);
    let rawCriteria: unknown;
    if (Array.isArray(content)) {
      rawCriteria = content;
    } else if (typeof content === "object" && content !== null) {
      const descriptor = Object.getOwnPropertyDescriptor(content, "criteria");
      if (descriptor && "value" in descriptor) rawCriteria = descriptor.value;
    }
    if (
      !Array.isArray(rawCriteria) ||
      rawCriteria.length === 0 ||
      rawCriteria.some(
        (entry: unknown) =>
          typeof entry !== "string" || entry.trim().length === 0,
      )
    ) {
      throw new CandidateBindingError(
        "acceptance criteria must be a non-empty list of non-empty strings",
      );
    }
    const criteria = rawCriteria as string[];
    if (new Set(criteria.map((entry) => entry.trim())).size !== criteria.length)
      throw new CandidateBindingError("acceptance criteria must be unique");
    return criteria;
  }

  #isTransitionAllowed(
    entityType: string,
    fromState: string,
    toState: string,
    actor: AuthenticatedActor,
  ): boolean {
    const rules = this.#database
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

  #mutate<T>(
    context: MutationContext,
    action: string,
    capability: Capability,
    payload: unknown,
    apply: (actor: AuthenticatedActor) => MutationOutput<T>,
  ): T {
    this.#assertOpen();
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
    this.#database.exec("BEGIN IMMEDIATE");
    try {
      const actor = authenticateActor(
        this.#database,
        this.#projectId,
        context.credential,
      );
      requireCapability(actor, capability);
      const requestHash = digestJson({
        action,
        actorId: actor.actorId,
        projectId: this.#projectId,
        requestId: context.requestId,
        expectedVersion: context.expectedVersion,
        inputRevision: context.inputRevision,
        payload,
      });
      const existing = this.#database
        .prepare(
          `
        SELECT actor_id, request_hash, result_json FROM mutation_requests WHERE project_id = ? AND idempotency_key = ?
      `,
        )
        .get(this.#projectId, context.idempotencyKey) as
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
        const value = JSON.parse(existing.result_json) as T;
        this.#database.exec("COMMIT");
        return value;
      }
      const requestReuse = this.#database
        .prepare(
          "SELECT idempotency_key FROM mutation_requests WHERE project_id = ? AND request_id = ?",
        )
        .get(this.#projectId, context.requestId) as
        { idempotency_key: string } | undefined;
      if (requestReuse)
        throw new IdempotencyConflictError(
          "request id was already used by another mutation",
        );
      const project = this.#database
        .prepare(
          "SELECT state_version, current_input_revision FROM projects WHERE project_id = ?",
        )
        .get(this.#projectId) as {
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
      this.#database
        .prepare(
          "UPDATE projects SET state_version = ? WHERE project_id = ? AND state_version = ?",
        )
        .run(nextVersion, this.#projectId, project.state_version);
      const now = new Date().toISOString();
      const eventSequence = (
        this.#database
          .prepare(
            "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM controller_events WHERE project_id = ?",
          )
          .get(this.#projectId) as { next: number }
      ).next;
      this.#database
        .prepare(
          `
        INSERT INTO controller_events(project_id, sequence, event_id, entity_type, entity_id, from_state, to_state,
          state_version, actor_id, request_id, input_revision, payload_json, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
      `,
        )
        .run(
          this.#projectId,
          eventSequence,
          randomUUID(),
          output.event.entityType,
          output.event.entityId,
          output.event.fromState ?? null,
          output.event.toState ?? null,
          nextVersion,
          actor.actorId,
          context.requestId,
          context.inputRevision,
          canonicalJson({
            action,
            payload,
            entityVersion: output.event.stateVersion,
            details: output.event.details ?? null,
          }),
          now,
        );
      this.#database
        .prepare(
          `
        INSERT INTO mutation_requests(project_id, idempotency_key, request_id, actor_id, request_hash, result_json, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?)
      `,
        )
        .run(
          this.#projectId,
          context.idempotencyKey,
          context.requestId,
          actor.actorId,
          requestHash,
          canonicalJson(output.value),
          now,
        );
      this.#database.exec("COMMIT");
      return output.value;
    } catch (error) {
      this.#database.exec("ROLLBACK");
      throw error;
    }
  }

  #isInternalActor(actorId: string): boolean {
    return actorId === this.#internalActorId;
  }
}
