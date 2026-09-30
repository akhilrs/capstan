import {
  createCipheriv,
  createDecipheriv,
  createHmac,
  randomBytes,
  randomUUID,
} from "node:crypto";
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
import { canonicalJson, digestJson, sha256 } from "./canonical.js";
import {
  openDatabase,
  openDatabaseReadOnly,
  resolveDatabasePath,
} from "./database.js";
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
  ReplacementInput,
  Role,
  RunState,
  RuntimeSessionSummary,
  RuntimeState,
  RoleDefinition,
  RoleDefinitionInput,
  RoleSyncResult,
  SeatInput,
  WorkItemInput,
} from "./types.js";

const ROLE_NAME_PATTERN = /^[a-z][a-z0-9-]{0,31}$/;
const ROLE_KINDS: readonly string[] = [
  "PM",
  "Developer",
  "Verifier",
  "Supervisor",
];

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

function acceptanceCriteriaFromContent(content: unknown): readonly string[] {
  let criteria: unknown;
  if (Array.isArray(content)) {
    criteria = content;
  } else if (typeof content === "object" && content !== null) {
    const descriptor = Object.getOwnPropertyDescriptor(content, "criteria");
    if (descriptor && "value" in descriptor) criteria = descriptor.value;
  }
  if (!Array.isArray(criteria) || criteria.length === 0)
    throw new CandidateBindingError(
      "acceptance criteria must be a non-empty list of non-empty strings",
    );
  const unique = new Set<string>();
  for (let index = 0; index < criteria.length; index += 1) {
    if (
      !Object.hasOwn(criteria, index) ||
      typeof criteria[index] !== "string" ||
      criteria[index].trim().length === 0
    )
      throw new CandidateBindingError(
        "acceptance criteria must be a non-empty list of non-empty strings",
      );
    unique.add(criteria[index].trim());
  }
  if (unique.size !== criteria.length)
    throw new CandidateBindingError("acceptance criteria must be unique");
  return criteria;
}

function actorResultKey(credential: string, projectId: string): Buffer {
  return createHmac("sha256", credential)
    .update("capstan:actor.create:result:v1:")
    .update(projectId)
    .digest();
}

function encryptActorResult(
  result: unknown,
  credential: string,
  projectId: string,
  requestHash: string,
): string {
  const nonce = randomBytes(12);
  const cipher = createCipheriv(
    "aes-256-gcm",
    actorResultKey(credential, projectId),
    nonce,
  );
  cipher.setAAD(Buffer.from(requestHash, "hex"));
  const ciphertext = Buffer.concat([
    cipher.update(canonicalJson(result), "utf8"),
    cipher.final(),
  ]);
  return canonicalJson({
    nonce: nonce.toString("base64url"),
    ciphertext: ciphertext.toString("base64url"),
    tag: cipher.getAuthTag().toString("base64url"),
  });
}

function decryptActorResult(
  resultJson: string,
  credential: string,
  projectId: string,
  requestHash: string,
): unknown {
  const result = JSON.parse(resultJson) as {
    nonce: string;
    ciphertext: string;
    tag: string;
  };
  const decipher = createDecipheriv(
    "aes-256-gcm",
    actorResultKey(credential, projectId),
    Buffer.from(result.nonce, "base64url"),
  );
  decipher.setAAD(Buffer.from(requestHash, "hex"));
  decipher.setAuthTag(Buffer.from(result.tag, "base64url"));
  return JSON.parse(
    Buffer.concat([
      decipher.update(Buffer.from(result.ciphertext, "base64url")),
      decipher.final(),
    ]).toString("utf8"),
  );
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
  readonly affectedWorkItemId: string;
  readonly affectedSeatId: string;
  readonly affectedAssignmentId: string;
  readonly affectedGeneration: number;
  readonly fingerprint: string;
  readonly severity: "info" | "low" | "medium" | "high" | "critical";
  readonly evidence: unknown;
  readonly requestedCorrection: string;
  readonly acknowledgementDeadline: string;
  readonly resolutionCondition: string;
  readonly escalationRoute: string;
}

export interface ControllerStatus {
  readonly projectId: string;
  readonly run: { readonly state: string; readonly stateVersion: number };
  readonly stateVersion: number;
  readonly inputRevision: number;
  readonly supervision: {
    readonly enabled: boolean;
    readonly health: "healthy" | "evaluating" | "degraded";
    readonly targetEpoch: number;
    readonly checkpointEpoch: number | null;
    readonly checkpointAssignmentId: string | null;
    readonly replacementAttempts: number;
  };
  readonly roles: readonly {
    readonly role: string;
    readonly seatId: string;
    readonly seatState: string;
    readonly actorActive: boolean;
    readonly sessionState: string | null;
    readonly assignmentId: string | null;
  }[];
  readonly work: readonly {
    readonly workItemId: string;
    readonly title: string;
    readonly role: string;
    readonly state: string;
    readonly owner: string | null;
    readonly blockers: readonly string[];
    readonly nextLegalActions: readonly string[];
  }[];
  readonly findings: readonly {
    readonly findingId: string;
    readonly affectedWorkItemId: string;
    readonly affectedSeatId: string;
    readonly affectedAssignmentId: string;
    readonly affectedGeneration: number;
    readonly fingerprint: string;
    readonly severity: string;
    readonly state: string;
    readonly evidence: unknown;
    readonly requestedCorrection: string;
    readonly acknowledgementDeadline: string;
    readonly resolutionCondition: string;
    readonly escalationRoute: string;
    readonly interventionCount: number;
    readonly reopenedFromFindingId: string | null;
  }[];
  readonly evidence: readonly {
    readonly candidateId: string;
    readonly commitSha: string;
    readonly reportHash: string;
    readonly developerEvidence: readonly string[] | null;
    readonly evidenceRef: string | null;
    readonly verifierEvidence: readonly {
      readonly evidenceId: string;
      readonly criterion: string;
      readonly passed: boolean;
      readonly artifactRef: string;
      readonly observation: string | null;
      readonly exitStatus: number | null;
      readonly evidenceHash: string;
    }[];
  }[];
  readonly finalVerification: readonly {
    readonly workItemId: string;
    readonly assignmentId: string;
    readonly commitSha: string;
    readonly evidence: readonly {
      readonly evidenceId: string;
      readonly criterion: string;
      readonly passed: boolean;
      readonly artifactRef: string;
      readonly observation: string | null;
      readonly exitStatus: number | null;
      readonly evidenceHash: string;
    }[];
  }[];
}

export interface CompletedWorkReport {
  readonly assignmentId: string;
  readonly commandId: string;
  readonly role: string;
  readonly generation: number;
  readonly inputRevision: number;
  readonly authorityState: string;
  readonly workState: string;
  readonly receiptHash: string;
  readonly reply: string;
  readonly receipt: Readonly<Record<string, unknown>>;
}

export class ControllerCore {
  readonly #database: Database.Database;
  readonly #lock: ProjectLock | undefined;
  readonly #readOnly: boolean;
  readonly #projectId: string;
  readonly #internalActorId: string;
  readonly #workspaceRoot: string;
  readonly #runtimeWorkspacePath: string | undefined;
  #closed = false;

  private constructor(
    database: Database.Database,
    lock: ProjectLock | undefined,
    projectId: string,
    internalActorId: string,
    workspaceRoot: string,
    runtimeWorkspacePath: string | undefined,
    readOnly = false,
  ) {
    this.#database = database;
    this.#lock = lock;
    this.#readOnly = readOnly;
    this.#projectId = projectId;
    this.#internalActorId = internalActorId;
    this.#workspaceRoot = workspaceRoot;
    this.#runtimeWorkspacePath = runtimeWorkspacePath;
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
      const workspaceRoot = path.resolve(
        options.workspaceRoot ?? process.cwd(),
      );
      const workspaceStat = fs.lstatSync(workspaceRoot);
      if (!workspaceStat.isDirectory() || workspaceStat.isSymbolicLink())
        throw new ControllerError(
          "workspace root must be an existing non-symlink directory",
        );
      if (
        options.runtimeWorkspacePath !== undefined &&
        !path.isAbsolute(options.runtimeWorkspacePath)
      )
        throw new TypeError("runtime workspace path must be absolute");
      const runtimeWorkspacePath =
        options.runtimeWorkspacePath === undefined
          ? undefined
          : path.resolve(options.runtimeWorkspacePath);
      const core = new ControllerCore(
        database,
        lock,
        project.projectId,
        internalActor.actor_id,
        workspaceRoot,
        runtimeWorkspacePath,
      );
      core.#reconcileUncertainAssignments();
      return core;
    } catch (error) {
      database?.close();
      lock.close();
      throw error;
    }
  }

  static async openReadOnly(
    options: ControllerOptions,
  ): Promise<ControllerCore> {
    const project = options.project;
    if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/.test(project.projectId)) {
      throw new TypeError("project id must be 1-64 safe ASCII characters");
    }
    if (!path.isAbsolute(options.stateDirectory))
      throw new TypeError("state directory must be absolute");
    const stateDirectory = path.resolve(options.stateDirectory);
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
    const databasePath = resolveDatabasePath(stateDirectory);
    const databaseStat = fs.lstatSync(databasePath);
    if (!databaseStat.isFile() || databaseStat.isSymbolicLink())
      throw new ControllerOwnershipError(
        "controller database path must be a regular file",
      );
    let database: Database.Database | undefined;
    try {
      database = openDatabaseReadOnly(databasePath);
      const projectRow = database
        .prepare("SELECT name FROM projects WHERE project_id = ?")
        .get(project.projectId) as { name: string } | undefined;
      if (!projectRow)
        throw new ControllerError("durable project state does not exist");
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
      const internalActor = database
        .prepare(
          "SELECT actor_id FROM actors WHERE project_id = ? AND is_internal = 1 AND role = 'controller' AND active = 1 AND revoked_at IS NULL",
        )
        .get(project.projectId) as { actor_id: string } | undefined;
      if (!internalActor)
        throw new ControllerError(
          "durable internal controller principal is missing",
        );
      const workspaceRoot = path.resolve(
        options.workspaceRoot ?? process.cwd(),
      );
      const workspaceStat = fs.lstatSync(workspaceRoot);
      if (!workspaceStat.isDirectory() || workspaceStat.isSymbolicLink())
        throw new ControllerError(
          "workspace root must be an existing non-symlink directory",
        );
      if (
        options.runtimeWorkspacePath !== undefined &&
        !path.isAbsolute(options.runtimeWorkspacePath)
      )
        throw new TypeError("runtime workspace path must be absolute");
      const runtimeWorkspacePath =
        options.runtimeWorkspacePath === undefined
          ? undefined
          : path.resolve(options.runtimeWorkspacePath);
      return new ControllerCore(
        database,
        undefined,
        project.projectId,
        internalActor.actor_id,
        workspaceRoot,
        runtimeWorkspacePath,
        true,
      );
    } catch (error) {
      database?.close();
      throw error;
    }
  }

  static #initializeProject(
    database: Database.Database,
    project: InitialProject,
  ): void {
    const initialInputs = project.initialInputs.map((input) => ({
      kind: input.kind,
      content: canonicalJson(input.content),
    }));
    const name = project.name;
    const requiredKinds: readonly InputKind[] = [
      "project_config",
      "task_brief",
      "acceptance_criteria",
      "policy",
      "plan",
    ];
    if (
      initialInputs.length !== requiredKinds.length ||
      requiredKinds.some(
        (kind) =>
          initialInputs.filter((input) => input.kind === kind).length !== 1,
      )
    ) {
      throw new TypeError(
        "initial project requires exactly one revision of each durable input kind",
      );
    }
    if (name.trim().length === 0)
      throw new TypeError("project name must not be empty");
    canonicalJson(name);
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
        .run(project.projectId, name, now);
      database
        .prepare(
          "INSERT INTO supervision_control(project_id, enabled, health, target_epoch, updated_at) VALUES (?, 0, 'degraded', 0, ?)",
        )
        .run(project.projectId, now);
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
      for (const input of initialInputs) {
        if (input.kind === "acceptance_criteria")
          acceptanceCriteriaFromContent(JSON.parse(input.content));
        insertRevision.run(
          project.projectId,
          input.kind,
          input.content,
          sha256(input.content),
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
    input = Object.freeze({ ...input });
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
          const activeActor = this.#database
            .prepare(
              "SELECT 1 AS present FROM actors WHERE project_id = ? AND seat_id = ? AND active = 1 AND revoked_at IS NULL LIMIT 1",
            )
            .get(this.#projectId, input.seatId);
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
        if (input.kind === "acceptance_criteria")
          acceptanceCriteriaFromContent(input.content);
        const run = this.#database
          .prepare("SELECT state FROM run_controls WHERE project_id = ?")
          .get(this.#projectId) as { state: string };
        if (run.state !== "active")
          throw new MutationConflictError(
            `project inputs cannot change while run is ${run.state}`,
          );
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
        const acceptedWork = this.#database
          .prepare(
            "SELECT 1 AS present FROM work_items WHERE project_id = ? AND state = 'accepted' LIMIT 1",
          )
          .get(this.#projectId);
        if (acceptedWork)
          throw new MutationConflictError(
            "project inputs cannot change after work has been accepted",
          );
        const revision = context.inputRevision + 1;
        const content = canonicalJson(input.content);
        const contentHash = sha256(content);
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

  roleDefinitions(): readonly RoleDefinition[] {
    this.#assertOpen();
    return (
      this.#database
        .prepare(
          "SELECT role_name, kind, host, config_hash, state FROM role_definitions WHERE project_id = ? ORDER BY role_name",
        )
        .all(this.#projectId) as Array<{
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
    this.#assertOpen();
    this.#assertWritable();
    requireCapability(
      authenticateActor(this.#database, this.#projectId, context.credential),
      "actor:manage",
    );
    const isReplay =
      this.#database
        .prepare(
          "SELECT 1 FROM mutation_requests WHERE project_id = ? AND idempotency_key = ?",
        )
        .get(this.#projectId, context.idempotencyKey) !== undefined;
    const planned = isReplay ? undefined : this.#roleDifference(sorted);
    if (planned !== undefined && !planned.changed) return planned;
    // Every write to role_definitions and seats must bump projects.state_version:
    // the caller creates the context before this pre-check, so a matching
    // expected version proves the rows read above are still current.
    return this.#mutate(
      context,
      "role.sync",
      "actor:manage",
      { roles: sorted },
      () => {
        const difference = planned ?? this.#roleDifference(sorted);
        const now = new Date().toISOString();
        const insert = this.#database.prepare(
          "INSERT INTO role_definitions(project_id, role_name, kind, host, config_hash, state, created_at, updated_at) VALUES (?, ?, ?, ?, ?, 'active', ?, ?)",
        );
        const update = this.#database.prepare(
          "UPDATE role_definitions SET kind = ?, host = ?, config_hash = ?, state = 'active', updated_at = ? WHERE project_id = ? AND role_name = ?",
        );
        const retire = this.#database.prepare(
          "UPDATE role_definitions SET state = 'retired', updated_at = ? WHERE project_id = ? AND role_name = ?",
        );
        const byName = new Map(sorted.map((role) => [role.name, role]));
        for (const name of difference.inserted) {
          const role = byName.get(name)!;
          insert.run(
            this.#projectId,
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
            this.#projectId,
            name,
          );
        }
        for (const name of difference.retired)
          retire.run(now, this.#projectId, name);
        return {
          value: difference,
          event: {
            entityType: "role_definition",
            entityId: this.#projectId,
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
    for (const seat of this.#database
      .prepare("SELECT name, role FROM seats WHERE project_id = ?")
      .all(this.#projectId) as Array<{ name: string; role: string }>)
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
        if (input.requiredRole === "Developer") {
          const criteria = acceptanceCriteriaFromContent(
            input.acceptanceCriteria ?? this.#acceptanceCriteria(),
          );
          if (criteria.length > 32)
            throw new CandidateBindingError(
              "Developer work-item acceptance criteria exceed 32 entries",
            );
        }
        if (input.finalVerification && input.requiredRole !== "Verifier")
          throw new CandidateBindingError(
            "final verification work must be assigned to a Verifier",
          );
        if (input.requiredRole === "Verifier") {
          if (input.finalVerification) {
            if (input.parentWorkItemId)
              throw new ReadinessError(
                "final Verifier work depends on accepted slices, not an open Developer parent",
              );
            const criteria = acceptanceCriteriaFromContent(
              input.acceptanceCriteria,
            );
            if (criteria.length > 32)
              throw new CandidateBindingError(
                "final Verifier acceptance criteria exceed 32 entries",
              );
            const parentCriteria = this.#acceptanceCriteria();
            if (
              criteria.length !== parentCriteria.length ||
              criteria.some((criterion) => !parentCriteria.includes(criterion))
            )
              throw new CandidateBindingError(
                "final Verifier criteria must match every parent acceptance criterion",
              );
          } else {
            const correctionFinding = input.findingId
              ? (this.#database
                  .prepare(
                    `SELECT f.state, s.role, w.parent_work_item_id
                     FROM findings f JOIN assignments a
                       ON a.project_id = f.project_id AND a.assignment_id = f.affected_assignment_id
                     JOIN seats s ON s.project_id = a.project_id AND s.seat_id = a.seat_id
                     JOIN work_items w ON w.project_id = a.project_id AND w.work_item_id = a.work_item_id
                     WHERE f.project_id = ? AND f.finding_id = ?`,
                  )
                  .get(this.#projectId, input.findingId) as
                  | {
                      state: string;
                      role: string;
                      parent_work_item_id: string | null;
                    }
                  | undefined)
              : undefined;
            const isFindingCorrection =
              !!correctionFinding &&
              ["reported", "acknowledged"].includes(correctionFinding.state) &&
              correctionFinding.role === "Verifier" &&
              correctionFinding.parent_work_item_id === input.parentWorkItemId;
            const parent = input.parentWorkItemId
              ? (this.#database
                  .prepare(
                    "SELECT required_role, state FROM work_items WHERE project_id = ? AND work_item_id = ?",
                  )
                  .get(this.#projectId, input.parentWorkItemId) as
                  { required_role: string; state: string } | undefined)
              : undefined;
            if (parent?.required_role !== "Developer")
              throw new ReadinessError(
                "Verifier work requires an existing Developer parent",
              );
            if (
              parent &&
              ["accepted", "canceled", "failed"].includes(parent.state) &&
              !isFindingCorrection
            )
              throw new ReadinessError(
                "Verifier work cannot be added to a terminal Developer parent",
              );
          }
        }
        this.#database
          .prepare(
            `
        INSERT INTO work_items(project_id, work_item_id, parent_work_item_id, title, description, required_role,
          state, state_version, input_revision, accepted_candidate_id, created_by, created_at, acceptance_criteria_json,
          final_verification)
        VALUES (?, ?, ?, ?, ?, ?, 'pending', 0, ?, NULL, ?, ?, ?, ?)
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
            input.acceptanceCriteria
              ? canonicalJson(
                  acceptanceCriteriaFromContent(input.acceptanceCriteria),
                )
              : input.requiredRole === "Developer"
                ? canonicalJson(this.#acceptanceCriteria())
                : null,
            input.finalVerification ? 1 : 0,
          );
        if (input.findingId) {
          const finding = this.#database
            .prepare(
              `SELECT affected_assignment_id, affected_generation, affected_seat_id,
                state, intervention_count FROM findings
               WHERE project_id = ? AND finding_id = ?`,
            )
            .get(this.#projectId, input.findingId) as
            | {
                affected_assignment_id: string | null;
                affected_generation: number | null;
                affected_seat_id: string | null;
                state: string;
                intervention_count: number;
              }
            | undefined;
          const target = finding?.affected_assignment_id
            ? (this.#database
                .prepare(
                  `SELECT a.active_generation, a.authority_state, a.seat_id, s.role,
                    w.final_verification
                   FROM assignments a JOIN seats s
                     ON s.project_id = a.project_id AND s.seat_id = a.seat_id
                   JOIN work_items w ON w.project_id = a.project_id AND w.work_item_id = a.work_item_id
                   WHERE a.project_id = ? AND a.assignment_id = ?`,
                )
                .get(this.#projectId, finding.affected_assignment_id) as
                | {
                    active_generation: number;
                    authority_state: string;
                    seat_id: string;
                    role: string;
                    final_verification: number;
                  }
                | undefined)
            : undefined;
          const finalVerifierCorrection =
            input.finalVerification === true &&
            target?.role === "Verifier" &&
            target.final_verification === 1;
          if (
            !finding ||
            !["reported", "acknowledged", "correcting"].includes(
              finding.state,
            ) ||
            !target ||
            finding.intervention_count >= 2 ||
            finding.affected_generation !== target.active_generation ||
            finding.affected_seat_id !== target.seat_id ||
            target.authority_state !== "contained" ||
            target.role !== input.requiredRole ||
            !["PM", "Developer", "Verifier"].includes(target.role) ||
            (input.finalVerification && !finalVerifierCorrection)
          )
            throw new ReadinessError(
              "finding correction work must target its reported, contained worker assignment within the intervention budget",
            );
          this.#database
            .prepare(
              `INSERT INTO finding_correction_work(project_id, work_item_id, finding_id,
                target_assignment_id, target_generation, target_seat_id, target_role, created_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
            )
            .run(
              this.#projectId,
              input.workItemId,
              input.findingId,
              finding.affected_assignment_id,
              finding.affected_generation,
              finding.affected_seat_id,
              target.role,
              new Date().toISOString(),
            );
          this.#database
            .prepare(
              "UPDATE findings SET intervention_count = intervention_count + 1 WHERE project_id = ? AND finding_id = ?",
            )
            .run(this.#projectId, input.findingId);
        }
        return {
          value: { workItemId: input.workItemId },
          event: {
            entityType: "work_item",
            entityId: input.workItemId,
            stateVersion: 0,
            toState: "pending",
            details: input.findingId
              ? { findingId: input.findingId }
              : undefined,
          },
        };
      },
    );
  }

  addDependency(
    context: MutationContext,
    workItemId: string,
    prerequisiteId: string,
    requiredCandidateId: string | null = null,
  ): { readonly added: true } {
    return this.#mutate(
      context,
      "dependency.add",
      "work:write",
      { workItemId, prerequisiteId, requiredCandidateId },
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
        if (requiredCandidateId !== null) {
          if (requiredCandidateId.trim().length === 0)
            throw new ControllerError(
              "required candidate ID must not be empty",
            );
          const candidateOwner = this.#database
            .prepare(
              `
            SELECT a.work_item_id FROM candidates c
            JOIN assignments a ON a.project_id = c.project_id AND a.assignment_id = c.assignment_id
            WHERE c.project_id = ? AND c.candidate_id = ?
          `,
            )
            .get(this.#projectId, requiredCandidateId) as
            { work_item_id: string } | undefined;
          if (candidateOwner?.work_item_id !== prerequisiteId)
            throw new ControllerError(
              "required candidate must belong to the prerequisite work item",
            );
        }
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
        VALUES (?, ?, ?, ?, ?, ?)
      `,
          )
          .run(
            this.#projectId,
            workItemId,
            prerequisiteId,
            requiredCandidateId,
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
    const supervision = this.#database
      .prepare(
        `SELECT enabled, health, target_epoch, checkpoint_epoch, bootstrap_pm_allowed
         FROM supervision_control WHERE project_id = ?`,
      )
      .get(this.#projectId) as
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
        !this.#database
          .prepare(
            `SELECT 1 FROM assignments a JOIN seats s
              ON s.project_id = a.project_id AND s.seat_id = a.seat_id
             WHERE a.project_id = ? AND s.role = 'PM'
               AND a.authority_state IN ('active', 'unknown') LIMIT 1`,
          )
          .get(this.#projectId);
      const correction = this.#database
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
        .get(this.#projectId, workItemId, this.#projectId, workItemId) as
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
        const openFinding = this.#database
          .prepare(
            "SELECT 1 FROM findings WHERE project_id = ? AND state <> 'resolved' LIMIT 1",
          )
          .get(this.#projectId);
        if (openFinding && !correction)
          reasons.push("an open finding blocks ordinary dispatch");
        if (correction) {
          const occupied = this.#database
            .prepare(
              `SELECT 1 FROM assignments WHERE project_id = ? AND seat_id = ?
               AND assignment_id <> ? AND authority_state IN ('active', 'unknown') LIMIT 1`,
            )
            .get(
              this.#projectId,
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
    const seats = this.#database
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
      .get(this.#projectId, item.required_role);
    if (!seats)
      reasons.push(`no active ${item.required_role} seat with an active actor`);
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
        SELECT title, description, required_role, parent_work_item_id, state, state_version, input_revision,
          final_verification, acceptance_criteria_json
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
              final_verification: number;
              acceptance_criteria_json: string | null;
            }
          | undefined;
        if (!item) throw new ControllerError("work item does not exist");
        const latestAssignment = this.#latestAssignmentRow(workItemId);
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
            `
          SELECT s.role, s.state,
            (
              SELECT a.actor_id FROM actors a
              WHERE a.project_id = s.project_id AND a.seat_id = s.seat_id
                AND a.role = s.role AND a.active = 1 AND a.revoked_at IS NULL
              LIMIT 1
            ) AS worker_actor_id
          FROM seats s WHERE s.project_id = ? AND s.seat_id = ?
        `,
          )
          .get(this.#projectId, seatId) as
          | { role: string; state: string; worker_actor_id: string | null }
          | undefined;
        if (
          !seat ||
          seat.state !== "active" ||
          seat.role !== item.required_role ||
          !seat.worker_actor_id
        ) {
          throw new ControllerError(
            "assignment seat must be active, match the work item role, and have an active actor",
          );
        }
        const correctionSeat = this.#database
          .prepare(
            `SELECT target_seat_id FROM finding_correction_work
             WHERE project_id = ? AND work_item_id = ?`,
          )
          .get(this.#projectId, workItemId) as
          { target_seat_id: string } | undefined;
        if (correctionSeat && correctionSeat.target_seat_id !== seatId)
          throw new ControllerError(
            "finding correction must be assigned to its affected seat",
          );
        const occupiedSeat = this.#database
          .prepare(
            `
          SELECT 1 AS present FROM assignments
          WHERE project_id = ? AND seat_id = ? AND authority_state IN ('active', 'unknown')
          LIMIT 1
        `,
          )
          .get(this.#projectId, seatId);
        if (occupiedSeat)
          throw new MutationConflictError(
            "seat has active or uncertain assignment authority",
          );
        let verifierCandidate:
          | {
              candidate_id: string;
              work_item_id: string;
              input_revision: number;
              commit_sha: string;
              report_hash: string;
              work_state: string;
              accepted_candidate_id: string | null;
              developer_authority: string;
              is_latest_generation: number;
            }
          | undefined;
        let verifierFindingCorrection = false;
        if (item.required_role === "Verifier") {
          if (item.final_verification === 1) {
            if (candidateId !== undefined && candidateId !== null)
              throw new CandidateBindingError(
                "final Verifier assignment cannot bind a candidate",
              );
            if (!item.acceptance_criteria_json)
              throw new CandidateBindingError(
                "final Verifier work requires its exact acceptance criteria",
              );
            const dependencyCount = this.#database
              .prepare(
                "SELECT COUNT(*) AS count FROM dependency_edges WHERE project_id = ? AND work_item_id = ?",
              )
              .get(this.#projectId, workItemId) as { count: number };
            if (dependencyCount.count === 0)
              throw new ReadinessError(
                "final Verifier work requires accepted slice dependencies",
              );
          } else {
            if (!candidateId)
              throw new CandidateBindingError(
                "Verifier assignment requires an exact candidate id",
              );
            verifierFindingCorrection = !!this.#database
              .prepare(
                `SELECT 1 FROM finding_correction_work c
                 JOIN findings f ON f.project_id = c.project_id AND f.finding_id = c.finding_id
                 WHERE c.project_id = ? AND c.work_item_id = ?
                   AND c.target_role = 'Verifier'
                   AND f.state IN ('reported', 'acknowledged')
                   AND EXISTS (
                     SELECT 1 FROM assignment_input_bindings b
                     WHERE b.project_id = c.project_id
                       AND b.assignment_id = c.target_assignment_id
                       AND b.input_kind = 'candidate' AND b.source_id = ?
                   )
                 LIMIT 1`,
              )
              .get(this.#projectId, workItemId, candidateId);
            verifierCandidate = this.#database
              .prepare(
                `
          SELECT c.candidate_id, a.work_item_id, c.input_revision, c.commit_sha, c.report_hash,
            w.state AS work_state, w.accepted_candidate_id,
            a.authority_state AS developer_authority,
            c.generation = (
              SELECT MAX(latest_attempt.generation)
              FROM assignments latest
              JOIN assignment_attempts latest_attempt
                ON latest_attempt.project_id = latest.project_id
                AND latest_attempt.assignment_id = latest.assignment_id
              JOIN seats latest_seat
                ON latest_seat.project_id = latest.project_id AND latest_seat.seat_id = latest.seat_id
              WHERE latest.project_id = a.project_id AND latest.work_item_id = a.work_item_id
                AND latest_seat.role = 'Developer'
            ) AS is_latest_generation
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
              (verifierCandidate.work_state !== "awaiting_verification" &&
                !(
                  verifierFindingCorrection &&
                  verifierCandidate.work_state === "accepted" &&
                  verifierCandidate.accepted_candidate_id === candidateId
                )) ||
              verifierCandidate.developer_authority !== "contained" ||
              verifierCandidate.is_latest_generation !== 1
            ) {
              throw new CandidateBindingError(
                "Verifier task parent and input revision must match the candidate",
              );
            }
          }
        } else if (candidateId !== undefined && candidateId !== null) {
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
        SELECT d.depends_on_work_item_id, w.accepted_candidate_id, w.final_verification,
          COALESCE(c.input_revision, w.input_revision) AS input_revision,
          c.commit_sha, c.report_hash AS candidate_report_hash,
          report.command_id AS report_command_id, report.receipt_json AS report_json,
          report.receipt_hash AS accepted_report_hash, final.commit_sha AS final_commit_sha,
          final.input_revision AS final_input_revision
        FROM dependency_edges d
        JOIN work_items w ON w.project_id = d.project_id AND w.work_item_id = d.depends_on_work_item_id
        LEFT JOIN candidates c ON c.project_id = w.project_id AND c.candidate_id = w.accepted_candidate_id
        LEFT JOIN final_verification_commits final
          ON final.project_id = w.project_id AND final.work_item_id = w.work_item_id
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
          WHERE a.state = 'completed' AND (
            s.role IN ('PM', 'Supervisor') OR (s.role = 'Verifier' AND EXISTS (
              SELECT 1 FROM final_verification_commits verified
              WHERE verified.project_id = a.project_id AND verified.work_item_id = a.work_item_id
            ))
          )
        ) report ON report.project_id = w.project_id AND report.work_item_id = w.work_item_id
          AND report.report_rank = 1 AND w.accepted_candidate_id IS NULL
        WHERE d.project_id = ? AND d.work_item_id = ? ORDER BY d.depends_on_work_item_id
      `,
          )
          .all(this.#projectId, workItemId) as Array<{
          depends_on_work_item_id: string;
          accepted_candidate_id: string | null;
          final_verification: number;
          input_revision: number;
          commit_sha: string | null;
          candidate_report_hash: string | null;
          report_command_id: string | null;
          report_json: string | null;
          accepted_report_hash: string | null;
          final_commit_sha: string | null;
          final_input_revision: number | null;
        }>;
        const artifacts = dependencies.flatMap((dependency) => {
          if (!dependency.accepted_candidate_id) return [];
          const evidence = this.#database
            .prepare(
              `
            SELECT e.evidence_id, e.verifier_assignment_id, e.input_revision,
              e.criterion, e.passed, e.artifact_ref, e.observation, e.exit_status, e.evidence_hash
            FROM candidate_evidence e
            JOIN assignments va ON va.project_id = e.project_id
              AND va.assignment_id = e.verifier_assignment_id
            JOIN assignment_attempts at ON at.project_id = va.project_id
              AND at.assignment_id = va.assignment_id
              AND at.generation = va.active_generation
            JOIN seats s ON s.project_id = va.project_id AND s.seat_id = va.seat_id
            JOIN assignment_input_bindings b ON b.project_id = e.project_id
              AND b.assignment_id = va.assignment_id
              AND b.input_kind = 'candidate' AND b.source_id = e.candidate_id
            WHERE e.project_id = ? AND e.candidate_id = ? AND e.passed = 1
              AND s.role = 'Verifier' AND va.state = 'completed'
              AND va.authority_state = 'contained' AND at.authority_state = 'contained'
              AND at.state = 'completed'
              AND at.generation = (
                SELECT MAX(latest_attempt.generation)
                FROM assignments latest
                JOIN assignment_attempts latest_attempt
                  ON latest_attempt.project_id = latest.project_id
                  AND latest_attempt.assignment_id = latest.assignment_id
                WHERE latest.project_id = va.project_id
                  AND latest.work_item_id = va.work_item_id
              )
            ORDER BY e.criterion, e.evidence_id
          `,
            )
            .all(this.#projectId, dependency.accepted_candidate_id) as Array<{
            evidence_id: string;
            verifier_assignment_id: string;
            input_revision: number;
            criterion: string;
            passed: number;
            artifact_ref: string;
            observation: string | null;
            exit_status: number | null;
            evidence_hash: string;
          }>;
          if (evidence.length === 0)
            throw new ControllerError(
              `accepted candidate ${dependency.accepted_candidate_id} has no contained verification artifacts`,
            );
          return evidence.map((entry) => ({
            workItemId: dependency.depends_on_work_item_id,
            candidateId: dependency.accepted_candidate_id,
            evidenceId: entry.evidence_id,
            verifierAssignmentId: entry.verifier_assignment_id,
            inputRevision: entry.input_revision,
            criterion: entry.criterion,
            passed: entry.passed === 1,
            artifactRef: entry.artifact_ref,
            observation: entry.observation,
            exitStatus: entry.exit_status,
            evidenceHash: entry.evidence_hash,
          }));
        });

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
        const inputValues = Object.fromEntries(
          snapshots.map((snapshot) => [
            snapshot.kind,
            JSON.parse(snapshot.content_json) as unknown,
          ]),
        );
        const planValue = inputValues.plan as {
          taskId?: string;
          slices?: readonly {
            id?: string;
            writeScope?: readonly string[];
          }[];
        };
        const planSlice = planValue.slices?.find((slice) => {
          const stableWorkItemId =
            typeof planValue.taskId === "string" && typeof slice.id === "string"
              ? `wf-${sha256(`${planValue.taskId}:${slice.id}`).slice(0, 24)}`
              : undefined;
          return (
            slice.id === workItemId ||
            stableWorkItemId === workItemId ||
            workItemId.endsWith(`:${slice.id}`) ||
            workItemId.endsWith(`/${slice.id}`)
          );
        });
        const openFindings = this.#database
          .prepare(
            `SELECT finding_id, severity, evidence_json, requested_correction,
              resolution_condition, state FROM findings
              WHERE project_id = ? AND work_item_id = ?
                AND state NOT IN ('resolved', 'escalated')
              ORDER BY created_at, finding_id`,
          )
          .all(this.#projectId, workItemId) as Array<{
          finding_id: string;
          severity: string;
          evidence_json: string;
          requested_correction: string;
          resolution_condition: string;
          state: string;
        }>;
        const recoveryHistory = this.#database
          .prepare(
            `SELECT recovery_id, recovery_type, generation, reason,
              containment_state, outcome, created_at FROM recovery_attempts
              WHERE project_id = ? AND work_item_id = ?
              ORDER BY created_at, recovery_id`,
          )
          .all(this.#projectId, workItemId);
        const role = item.required_role;
        const resultSchema =
          role === "PM"
            ? { type: "object", required: ["planHash"] }
            : role === "Developer"
              ? {
                  type: "object",
                  required: [
                    "candidateId",
                    "commitSha",
                    "baseSha",
                    "changedScope",
                    "limitations",
                    "evidence",
                  ],
                }
              : role === "Verifier"
                ? item.final_verification === 1
                  ? { type: "object", required: ["commitSha", "evidence"] }
                  : { type: "object", required: ["candidateId", "evidence"] }
                : { type: "object", required: ["outcome", "observation"] };
        const nextLegalActions =
          role === "PM"
            ? ["report_plan_review"]
            : role === "Developer"
              ? [
                  "implement_assigned_scope",
                  "report_blocker",
                  "submit_candidate",
                ]
              : role === "Verifier"
                ? item.final_verification === 1
                  ? [
                      "verify_composed_checkout",
                      "report_evidence",
                      "report_blocker",
                    ]
                  : [
                      "verify_exact_candidate",
                      "report_evidence",
                      "report_blocker",
                    ]
                : ["report_run_observation"];
        const assignmentWorkspace =
          this.#runtimeWorkspacePath ??
          path.join(
            this.#workspaceRoot,
            ".capstan",
            "workspaces",
            seatId,
            workItemId,
            String(generation),
          );
        const acceptedFacts = snapshots.map((snapshot) => ({
          kind: snapshot.kind,
          revision: snapshot.revision,
          contentHash: snapshot.content_hash,
        }));
        const capsule = {
          projectId: this.#projectId,
          assignment: { commandId, assignmentId, attempt, generation },
          workItem: {
            workItemId,
            title: item.title,
            description: item.description,
            requiredRole: item.required_role,
            inputRevision: context.inputRevision,
            ...(item.final_verification === 1
              ? {
                  finalVerification: true,
                  acceptanceCriteria: JSON.parse(
                    item.acceptance_criteria_json!,
                  ) as readonly string[],
                }
              : {}),
          },
          seat: { seatId, role: seat.role },
          inputs: inputValues,
          acceptedFacts,
          policy: inputValues.policy,
          scope: {
            workspace: assignmentWorkspace,
            writePaths: planSlice?.writeScope ?? [],
            readOnly:
              role === "Verifier" || role === "Supervisor" || role === "PM",
          },
          openFindings: openFindings.map((finding) => ({
            findingId: finding.finding_id,
            severity: finding.severity,
            evidence: JSON.parse(finding.evidence_json) as unknown,
            requestedCorrection: finding.requested_correction,
            resolutionCondition: finding.resolution_condition,
            state: finding.state,
          })),
          recoveryHistory,
          resultSchema,
          nextLegalActions,
          artifacts,
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
            const finalEvidence =
              dependency.final_verification === 1
                ? (this.#database
                    .prepare(
                      `SELECT evidence_id, criterion, passed, artifact_ref, observation, exit_status, evidence_hash
                       FROM final_verification_evidence
                       WHERE project_id = ? AND work_item_id = ? AND input_revision = ?
                         AND commit_sha = ?
                       ORDER BY criterion`,
                    )
                    .all(
                      this.#projectId,
                      dependency.depends_on_work_item_id,
                      dependency.final_input_revision,
                      dependency.final_commit_sha,
                    ) as Array<{
                    evidence_id: string;
                    criterion: string;
                    passed: number;
                    artifact_ref: string;
                    observation: string | null;
                    exit_status: number | null;
                    evidence_hash: string;
                  }>)
                : undefined;
            if (
              dependency.final_verification === 1 &&
              (!dependency.final_commit_sha ||
                dependency.final_input_revision !== dependency.input_revision ||
                !finalEvidence?.length)
            )
              throw new ControllerError(
                "accepted final Verifier evidence is missing or stale",
              );
            return {
              workItemId: dependency.depends_on_work_item_id,
              candidateId: null,
              inputRevision: dependency.input_revision,
              report: JSON.parse(dependency.report_json) as unknown,
              reportHash: dependency.accepted_report_hash,
              ...(dependency.final_verification === 1
                ? {
                    finalVerification: {
                      commitSha: dependency.final_commit_sha!,
                      evidence: finalEvidence!.map((entry) => ({
                        evidenceId: entry.evidence_id,
                        criterion: entry.criterion,
                        passed: entry.passed === 1,
                        artifactRef: entry.artifact_ref,
                        observation: entry.observation,
                        exitStatus: entry.exit_status,
                        evidenceHash: entry.evidence_hash,
                      })),
                    },
                  }
                : {}),
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
          active_generation, authority_state, created_by, created_at, ended_at, worker_actor_id)
        VALUES (?, ?, ?, ?, 'created', 0, ?, ?, 'active', ?, ?, NULL, ?)
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
            seat.worker_actor_id,
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
        const dependencyBindingSources = new Set<string>();
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
          const inputKind = candidateBound
            ? "dependency_candidate"
            : "dependency_report";
          const bindingKey = `${inputKind}:${sourceId}`;
          if (dependencyBindingSources.has(bindingKey)) continue;
          dependencyBindingSources.add(bindingKey);
          bindInput.run(
            this.#projectId,
            assignmentId,
            context.inputRevision,
            inputKind,
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
          a.work_item_id, a.state AS assignment_state, a.authority_state AS assignment_authority,
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
              work_item_id: string;
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
        this.#assertSupervisionReady(command.work_item_id, true);
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
          SELECT c.state, c.state_version, a.work_item_id, a.state AS assignment_state,
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
              work_item_id: string;
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
        this.#assertSupervisionReady(command.work_item_id, true);
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
  bridgeReceiptJournal(role: Role): string {
    this.#assertOpen();
    const receipts = this.#database
      .prepare(
        `SELECT r.receipt_json, r.sequence, r.command_id, r.assignment_id,
           r.attempt, r.generation, r.receipt_type, a.authority_state,
           a.containment_proof_ref
         FROM command_receipts r JOIN assignments a
           ON a.project_id = r.project_id AND a.assignment_id = r.assignment_id
         WHERE r.project_id = ? AND r.role = ? ORDER BY r.sequence`,
      )
      .all(this.#projectId, role) as Array<{
      receipt_json: string;
      sequence: number;
      command_id: string;
      assignment_id: string;
      attempt: number;
      generation: number;
      receipt_type: string;
      authority_state: string;
      containment_proof_ref: string | null;
    }>;
    const journal: string[] = [];
    for (let index = 0; index < receipts.length; index++) {
      const receipt = receipts[index]!;
      journal.push(receipt.receipt_json);
      if (
        receipt.command_id !== receipts[index + 1]?.command_id &&
        receipt.receipt_type !== "completed" &&
        receipt.authority_state === "contained" &&
        receipt.containment_proof_ref !== null
      )
        journal.push(
          canonicalJson({
            type: "contained",
            role,
            sequence: receipt.sequence,
            commandId: receipt.command_id,
            assignmentId: receipt.assignment_id,
            attempt: receipt.attempt,
            generation: receipt.generation,
          }),
        );
    }
    return journal.length ? `${journal.join("\n")}\n` : "";
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
          c.attempt, c.generation, c.start_requested, a.state AS assignment_state, a.authority_state AS assignment_authority,
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
            start_requested: number;
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
            command.attempt_state !== "acknowledged" ||
            command.start_requested !== 1
          ) {
            throw new MutationConflictError(
              "working receipt arrived without durable start intent",
            );
          }
          commandToState = "started";
          attemptToState = "running";
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
        } else if (receipt.type === "submitted") {
          if (
            command.command_state !== "acknowledged" ||
            command.attempt_state !== "acknowledged" ||
            command.start_requested !== 1 ||
            command.work_state !== "running"
          ) {
            throw new MutationConflictError(
              "submitted receipt requires durable acknowledgement and start intent",
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
          input.candidateId.trim().length === 0 ||
          !/^[a-fA-F0-9]{40}([a-fA-F0-9]{24})?$/.test(input.commitSha) ||
          !/^[a-fA-F0-9]{40}([a-fA-F0-9]{24})?$/.test(input.baseSha) ||
          !Array.isArray(input.changedScope) ||
          !Array.isArray(input.limitations) ||
          !Array.isArray(input.evidence) ||
          input.evidence.length === 0 ||
          input.evidence.length > 64 ||
          Array.from(input.evidence).some(
            (entry) =>
              typeof entry !== "string" ||
              entry.trim().length === 0 ||
              entry.length > 4096,
          ) ||
          Array.from(input.changedScope).some(
            (entry) => typeof entry !== "string",
          ) ||
          Array.from(input.limitations).some(
            (entry) => typeof entry !== "string",
          )
        ) {
          throw new CandidateBindingError(
            "candidate requires a Developer actor and valid immutable commit identities",
          );
        }
        const assignment = this.#database
          .prepare(
            `
        SELECT a.work_item_id, a.seat_id, a.worker_actor_id, a.input_revision, a.active_generation, a.state,
          a.authority_state, at.attempt, at.generation, at.state AS attempt_state, w.state AS work_state, w.input_revision AS work_revision
        FROM assignments a JOIN assignment_attempts at
          ON at.project_id = a.project_id AND at.assignment_id = a.assignment_id AND at.generation = a.active_generation
        JOIN work_items w ON w.project_id = a.project_id AND w.work_item_id = a.work_item_id
        WHERE a.project_id = ? AND a.assignment_id = ?
      `,
          )
          .get(this.#projectId, input.assignmentId) as
          | {
              work_item_id: string;
              worker_actor_id: string | null;
              seat_id: string;
              input_revision: number;
              active_generation: number;
              state: string;
              authority_state: string;
              attempt: number;
              generation: number;
              attempt_state: string;
              work_state: string;
              work_revision: number;
            }
          | undefined;
        if (
          !assignment ||
          actor.actorId !== assignment.worker_actor_id ||
          actor.seatId !== assignment.seat_id ||
          (assignment.authority_state !== "active" &&
            assignment.authority_state !== "contained") ||
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
          evidence: input.evidence,
        });
        const existingCandidate = this.#database
          .prepare(
            "SELECT 1 AS present FROM candidates WHERE project_id = ? AND assignment_id = ? LIMIT 1",
          )
          .get(this.#projectId, input.assignmentId);
        if (existingCandidate)
          throw new CandidateBindingError(
            "a Developer assignment may submit only one candidate",
          );
        this.#database
          .prepare(
            `
        INSERT INTO candidates(project_id, candidate_id, assignment_id, attempt, generation, input_revision,
          commit_sha, base_sha, changed_scope_json, limitations_json, evidence_json, report_hash, created_by, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
            canonicalJson(input.evidence),
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
    const result = this.recordEvidenceBatch(context, verifierAssignmentId, [
      input,
    ]);
    return result.evidence[0]!;
  }

  recordEvidenceBatch(
    context: MutationContext,
    verifierAssignmentId: string,
    inputs: readonly EvidenceInput[],
  ): {
    readonly evidence: readonly {
      readonly evidenceId: string;
      readonly evidenceHash: string;
    }[];
  } {
    inputs = inputs.map((input) => Object.freeze({ ...input }));
    if (
      inputs.length === 0 ||
      inputs.some((input) => input.candidateId !== inputs[0]!.candidateId)
    )
      throw new CandidateBindingError(
        "evidence batch must contain observations for one candidate",
      );
    return this.#mutate(
      context,
      "candidate.evidence.record",
      "candidate:verify",
      { inputs, verifierAssignmentId },
      (actor) => {
        if (actor.role !== "Verifier")
          throw new TransitionAuthorizationError(
            "only a Verifier may record candidate evidence",
          );
        const candidateId = inputs[0]!.candidateId;
        const candidate = this.#database
          .prepare(
            `
        SELECT c.input_revision, c.report_hash, c.created_by, a.work_item_id, a.worker_actor_id,
          parent.state AS work_state, parent.accepted_candidate_id,
          c.generation = (
            SELECT MAX(latest_attempt.generation)
            FROM assignments latest
            JOIN assignment_attempts latest_attempt
              ON latest_attempt.project_id = latest.project_id
              AND latest_attempt.assignment_id = latest.assignment_id
            JOIN seats latest_seat
              ON latest_seat.project_id = latest.project_id AND latest_seat.seat_id = latest.seat_id
            WHERE latest.project_id = a.project_id AND latest.work_item_id = a.work_item_id
              AND latest_seat.role = 'Developer'
          ) AS is_latest_generation
        FROM candidates c JOIN assignments a
          ON a.project_id = c.project_id AND a.assignment_id = c.assignment_id
        JOIN work_items parent ON parent.project_id = a.project_id AND parent.work_item_id = a.work_item_id
        WHERE c.project_id = ? AND c.candidate_id = ?
      `,
          )
          .get(this.#projectId, candidateId) as
          | {
              input_revision: number;
              report_hash: string;
              created_by: string;
              worker_actor_id: string | null;
              work_item_id: string;
              work_state: string;
              accepted_candidate_id: string | null;
              is_latest_generation: number;
            }
          | undefined;
        const verifier = this.#database
          .prepare(
            `
        SELECT a.seat_id, a.worker_actor_id, a.input_revision, a.state, a.authority_state, a.work_item_id,
          t.parent_work_item_id, t.state AS verifier_work_state,
          a.active_generation = (
            SELECT MAX(latest_attempt.generation)
            FROM assignments latest
            JOIN assignment_attempts latest_attempt
              ON latest_attempt.project_id = latest.project_id
              AND latest_attempt.assignment_id = latest.assignment_id
            WHERE latest.project_id = a.project_id AND latest.work_item_id = a.work_item_id
          ) AS is_latest_generation,
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
              worker_actor_id: string | null;
              input_revision: number;
              state: string;
              authority_state: string;
              work_item_id: string;
              parent_work_item_id: string | null;
              verifier_work_state: string;
              attempt_state: string;
              attempt_authority: string;
              role: string;
              is_latest_generation: number;
            }
          | undefined;
        const findingCorrection = this.#database
          .prepare(
            `SELECT f.finding_id FROM finding_correction_work c
             JOIN findings f ON f.project_id = c.project_id AND f.finding_id = c.finding_id
             WHERE c.project_id = ? AND c.work_item_id = ?
               AND c.target_role = 'Verifier' AND f.state = 'correcting'
             LIMIT 1`,
          )
          .get(this.#projectId, verifier?.work_item_id);
        const binding = this.#database
          .prepare(
            `
        SELECT content_hash FROM assignment_input_bindings
        WHERE project_id = ? AND assignment_id = ? AND input_kind = 'candidate' AND source_id = ?
      `,
          )
          .get(this.#projectId, verifierAssignmentId, candidateId) as
          { content_hash: string } | undefined;
        if (
          !candidate ||
          !verifier ||
          (candidate.work_state !== "awaiting_verification" &&
            !(
              candidate.work_state === "accepted" &&
              candidate.accepted_candidate_id === candidateId &&
              !!findingCorrection
            )) ||
          candidate.is_latest_generation !== 1 ||
          verifier.verifier_work_state !== "awaiting_verification" ||
          actor.actorId !== verifier.worker_actor_id ||
          actor.seatId !== verifier.seat_id ||
          candidate.created_by !== candidate.worker_actor_id ||
          verifier.role !== "Verifier" ||
          verifier.parent_work_item_id !== candidate.work_item_id ||
          verifier.state !== "reported" ||
          verifier.attempt_state !== "reported" ||
          verifier.authority_state !== "contained" ||
          verifier.is_latest_generation !== 1 ||
          verifier.input_revision !== context.inputRevision ||
          candidate.input_revision !== context.inputRevision ||
          !binding ||
          binding.content_hash !== candidate.report_hash
        ) {
          throw new CandidateBindingError(
            "evidence is not bound to this current candidate and reported Verifier assignment",
          );
        }
        const criteria = this.#acceptanceCriteria(candidate.work_item_id);
        const evidence = inputs.map((input) => {
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
          if (
            typeof input.observation !== "string" ||
            input.observation.trim().length === 0
          )
            throw new CandidateBindingError(
              "candidate evidence requires a non-empty observation",
            );
          if (
            !Number.isInteger(input.exitStatus) ||
            input.exitStatus < 0 ||
            input.exitStatus > 255
          )
            throw new CandidateBindingError(
              "candidate evidence exit status must be an integer from 0 to 255",
            );
          if (input.passed && input.exitStatus !== 0)
            throw new CandidateBindingError(
              "passing evidence must have exit status 0",
            );
          if (!criteria.includes(input.criterion))
            throw new CandidateBindingError(
              "evidence criterion is not in the current work-item acceptance criteria",
            );
          const evidenceHash = digestJson({
            evidenceId: input.evidenceId,
            candidateId: input.candidateId,
            verifierAssignmentId,
            inputRevision: context.inputRevision,
            criterion: input.criterion,
            passed: input.passed,
            artifactRef: input.artifactRef,
            observation: input.observation,
            exitStatus: input.exitStatus,
          });
          return { input, evidenceHash };
        });
        const insertEvidence = this.#database.prepare(
          `
        INSERT INTO candidate_evidence(project_id, evidence_id, candidate_id, verifier_assignment_id, input_revision,
          criterion, passed, artifact_ref, observation, exit_status, evidence_hash, created_by, created_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
      `,
        );
        for (const { input, evidenceHash } of evidence) {
          insertEvidence.run(
            this.#projectId,
            input.evidenceId,
            input.candidateId,
            verifierAssignmentId,
            context.inputRevision,
            input.criterion,
            input.passed ? 1 : 0,
            input.artifactRef,
            input.observation,
            input.exitStatus,
            evidenceHash,
            actor.actorId,
            new Date().toISOString(),
          );
        }
        let canceledVerifierWorkItemIds: string[] = [];
        if (inputs.some((input) => !input.passed)) {
          const controller = this.#internalPrincipal();
          if (
            !this.#isTransitionAllowed(
              "work_item",
              "awaiting_verification",
              "blocked",
              controller,
            ) ||
            !this.#isTransitionAllowed(
              "work_item",
              "awaiting_verification",
              "canceled",
              controller,
            )
          )
            throw new TransitionAuthorizationError(
              "transition table rejects failed verification disposition",
            );
          const verifierAssignments = this.#database
            .prepare(
              `
            SELECT DISTINCT a.work_item_id, a.authority_state,
              at.authority_state AS attempt_authority, w.state AS work_state
            FROM assignment_input_bindings b
            JOIN assignments a ON a.project_id = b.project_id AND a.assignment_id = b.assignment_id
            JOIN assignment_attempts at ON at.project_id = a.project_id
              AND at.assignment_id = a.assignment_id AND at.generation = a.active_generation
            JOIN seats s ON s.project_id = a.project_id AND s.seat_id = a.seat_id
            JOIN work_items w ON w.project_id = a.project_id AND w.work_item_id = a.work_item_id
            WHERE b.project_id = ? AND b.input_kind = 'candidate' AND b.source_id = ? AND s.role = 'Verifier'
          `,
            )
            .all(this.#projectId, candidateId) as Array<{
            work_item_id: string;
            authority_state: string;
            attempt_authority: string;
            work_state: string;
          }>;
          const unassignedVerifierChild = this.#database
            .prepare(
              `
            SELECT 1 AS present FROM work_items child
            WHERE child.project_id = ? AND child.parent_work_item_id = ?
              AND child.required_role = 'Verifier'
              AND child.state NOT IN ('accepted', 'canceled')
              AND NOT EXISTS (
                SELECT 1 FROM assignments verifier
                JOIN assignment_input_bindings binding
                  ON binding.project_id = verifier.project_id
                  AND binding.assignment_id = verifier.assignment_id
                  AND binding.input_kind = 'candidate' AND binding.source_id = ?
                WHERE verifier.project_id = child.project_id
                  AND verifier.work_item_id = child.work_item_id
              )
            LIMIT 1
          `,
            )
            .get(this.#projectId, candidate.work_item_id, candidateId);
          if (unassignedVerifierChild)
            throw new MutationConflictError(
              "all Verifier children must be assigned to the failing candidate before recording failed evidence",
            );
          if (
            verifierAssignments.some(
              (assignment) =>
                assignment.authority_state !== "contained" ||
                assignment.attempt_authority !== "contained",
            )
          )
            throw new MutationConflictError(
              "all Verifier assignments bound to a failing candidate must be contained",
            );
          canceledVerifierWorkItemIds = [
            ...new Set(
              verifierAssignments
                .filter(
                  (assignment) =>
                    assignment.work_state === "awaiting_verification",
                )
                .map((assignment) => assignment.work_item_id),
            ),
          ];
          this.#database
            .prepare(
              `
            UPDATE work_items SET state = 'blocked', state_version = state_version + 1
            WHERE project_id = ? AND work_item_id = ? AND state = 'awaiting_verification'
          `,
            )
            .run(this.#projectId, candidate.work_item_id);
          for (const verifierWorkItemId of canceledVerifierWorkItemIds) {
            this.#database
              .prepare(
                `
              UPDATE work_items SET state = 'canceled', state_version = state_version + 1
              WHERE project_id = ? AND work_item_id = ? AND state = 'awaiting_verification'
            `,
              )
              .run(this.#projectId, verifierWorkItemId);
          }
        }
        return {
          value: {
            evidence: evidence.map(({ input, evidenceHash }) => ({
              evidenceId: input.evidenceId,
              evidenceHash,
            })),
          },
          event: {
            entityType: "work_item",
            entityId: candidate.work_item_id,
            stateVersion: 0,
            fromState: "awaiting_verification",
            toState: inputs.some((input) => !input.passed)
              ? "blocked"
              : "verification_evidence_recorded",
            details: {
              ...(inputs.length === 1
                ? { evidenceId: inputs[0]!.evidenceId }
                : { evidenceIds: inputs.map((input) => input.evidenceId) }),
              ...(inputs.some((input) => !input.passed)
                ? {
                    failedVerifierWorkItemId: verifier.work_item_id,
                    failedVerifierWorkFrom: "awaiting_verification",
                    failedVerifierWorkTo: "canceled",
                    canceledVerifierWorkItemIds,
                  }
                : {}),
            },
          },
        };
      },
    );
  }

  acceptVerifierCorrection(
    context: MutationContext,
    input: {
      readonly findingId: string;
      readonly workItemId: string;
      readonly assignmentId: string;
      readonly candidateId: string;
    },
  ): { readonly acceptedWorkItemId: string; readonly candidateId: string } {
    return this.#mutateAsController(
      context,
      "finding.verifier_correction.accept",
      "candidate:accept",
      input,
      () => {
        this.#assertSupervisionReady(input.workItemId);
        const correction = this.#database
          .prepare(
            `SELECT c.target_assignment_id, c.target_generation, c.target_seat_id,
              c.target_role, f.state AS finding_state,
              w.required_role, w.state AS work_state, w.state_version AS work_version,
              w.parent_work_item_id,
              w.input_revision, a.seat_id AS assignment_seat_id,
              a.worker_actor_id, a.state AS assignment_state,
              a.authority_state AS assignment_authority,
              at.state AS attempt_state, at.authority_state AS attempt_authority,
              at.generation, cmd.state AS command_state,
              target.active_generation AS target_active_generation,
              target.authority_state AS target_authority,
              target.seat_id AS target_assignment_seat_id,
              target_seat.role AS target_assignment_role, run.state AS run_state
             FROM finding_correction_work c
             JOIN findings f ON f.project_id = c.project_id
               AND f.finding_id = c.finding_id
             JOIN work_items w ON w.project_id = c.project_id
               AND w.work_item_id = c.work_item_id
             JOIN assignments a ON a.project_id = c.project_id
               AND a.assignment_id = ?
             JOIN assignment_attempts at ON at.project_id = a.project_id
               AND at.assignment_id = a.assignment_id
               AND at.generation = a.active_generation
             JOIN commands cmd ON cmd.project_id = a.project_id
               AND cmd.assignment_id = a.assignment_id
               AND cmd.attempt = at.attempt AND cmd.generation = at.generation
             JOIN assignments target ON target.project_id = c.project_id
               AND target.assignment_id = c.target_assignment_id
             JOIN seats target_seat ON target_seat.project_id = target.project_id
               AND target_seat.seat_id = target.seat_id
             JOIN run_controls run ON run.project_id = c.project_id
             WHERE c.project_id = ? AND c.finding_id = ?
               AND c.work_item_id = ? AND a.work_item_id = w.work_item_id`,
          )
          .get(
            input.assignmentId,
            this.#projectId,
            input.findingId,
            input.workItemId,
          ) as
          | {
              target_assignment_id: string;
              target_generation: number;
              target_seat_id: string;
              target_role: string;
              finding_state: string;
              required_role: string;
              work_state: string;
              work_version: number;
              parent_work_item_id: string | null;
              input_revision: number;
              assignment_seat_id: string;
              worker_actor_id: string | null;
              assignment_state: string;
              assignment_authority: string;
              attempt_state: string;
              attempt_authority: string;
              generation: number;
              command_state: string;
              target_active_generation: number;
              target_authority: string;
              target_assignment_seat_id: string;
              target_assignment_role: string;
              run_state: string;
            }
          | undefined;
        const candidate = this.#database
          .prepare(
            `SELECT c.assignment_id, c.input_revision, c.report_hash,
              c.created_by, a.worker_actor_id, a.work_item_id,
              w.state AS parent_state, w.accepted_candidate_id,
              a.authority_state AS developer_authority
             FROM candidates c
             JOIN assignments a ON a.project_id = c.project_id
               AND a.assignment_id = c.assignment_id
             JOIN work_items w ON w.project_id = a.project_id
               AND w.work_item_id = a.work_item_id
             WHERE c.project_id = ? AND c.candidate_id = ?
               AND c.generation = a.active_generation`,
          )
          .get(this.#projectId, input.candidateId) as
          | {
              assignment_id: string;
              input_revision: number;
              report_hash: string;
              created_by: string;
              worker_actor_id: string | null;
              work_item_id: string;
              parent_state: string;
              accepted_candidate_id: string | null;
              developer_authority: string;
            }
          | undefined;
        const binding = this.#database
          .prepare(
            `SELECT content_hash FROM assignment_input_bindings
             WHERE project_id = ? AND assignment_id = ?
               AND input_kind = 'candidate' AND source_id = ?`,
          )
          .get(this.#projectId, input.assignmentId, input.candidateId) as
          { content_hash: string } | undefined;
        const targetBinding = this.#database
          .prepare(
            `SELECT 1 AS present FROM assignment_input_bindings
             WHERE project_id = ? AND assignment_id = ?
               AND input_kind = 'candidate' AND source_id = ?`,
          )
          .get(
            this.#projectId,
            correction?.target_assignment_id,
            input.candidateId,
          );
        if (
          !correction ||
          correction.target_role !== "Verifier" ||
          correction.finding_state !== "correcting" ||
          correction.required_role !== "Verifier" ||
          correction.work_state !== "awaiting_verification" ||
          correction.parent_work_item_id !== candidate?.work_item_id ||
          correction.target_seat_id !== correction.assignment_seat_id ||
          correction.assignment_state !== "reported" ||
          correction.assignment_authority !== "contained" ||
          correction.attempt_state !== "reported" ||
          correction.attempt_authority !== "contained" ||
          correction.command_state !== "completed" ||
          correction.target_assignment_role !== "Verifier" ||
          correction.target_assignment_seat_id !== correction.target_seat_id ||
          correction.target_active_generation !==
            correction.target_generation ||
          correction.run_state !== "active" ||
          correction.target_authority !== "contained" ||
          !candidate ||
          candidate.parent_state !== "accepted" ||
          candidate.accepted_candidate_id !== input.candidateId ||
          candidate.developer_authority !== "contained" ||
          candidate.created_by !== candidate.worker_actor_id ||
          candidate.input_revision !== context.inputRevision ||
          !binding ||
          binding.content_hash !== candidate.report_hash ||
          !targetBinding
        )
          throw new CandidateBindingError(
            "Verifier correction does not match its exact finding, candidate, and contained reported assignment",
          );
        const criteria = this.#acceptanceCriteria(candidate.work_item_id);
        const evidence = this.#database
          .prepare(
            `SELECT criterion, passed, observation, exit_status, input_revision,
              created_by
             FROM candidate_evidence WHERE project_id = ? AND candidate_id = ?
               AND verifier_assignment_id = ? ORDER BY criterion, evidence_id`,
          )
          .all(
            this.#projectId,
            input.candidateId,
            input.assignmentId,
          ) as Array<{
          criterion: string;
          passed: number;
          observation: string | null;
          exit_status: number | null;
          input_revision: number;
          created_by: string;
        }>;
        if (
          evidence.length !== criteria.length ||
          criteria.some(
            (criterion) =>
              evidence.filter((entry) => entry.criterion === criterion)
                .length !== 1 ||
              !evidence.some(
                (entry) =>
                  entry.criterion === criterion &&
                  entry.passed === 1 &&
                  !!entry.observation?.trim() &&
                  entry.created_by === correction.worker_actor_id &&
                  entry.exit_status === 0 &&
                  entry.input_revision === context.inputRevision,
              ),
          ) ||
          evidence.some((entry) => !criteria.includes(entry.criterion))
        )
          throw new CandidateBindingError(
            "Verifier correction requires fresh passing evidence for every exact parent criterion",
          );
        const controller = this.#internalPrincipal();
        if (
          !this.#isTransitionAllowed(
            "work_item",
            "awaiting_verification",
            "accepted",
            controller,
          ) ||
          !this.#isTransitionAllowed(
            "assignment_attempt",
            "reported",
            "completed",
            controller,
          )
        )
          throw new TransitionAuthorizationError(
            "transition table rejects Verifier correction acceptance",
          );
        const now = new Date().toISOString();
        this.#database
          .prepare(
            `UPDATE work_items SET state = 'accepted', accepted_candidate_id = ?,
              state_version = state_version + 1
             WHERE project_id = ? AND work_item_id = ?
               AND state = 'awaiting_verification'`,
          )
          .run(input.candidateId, this.#projectId, input.workItemId);
        this.#database
          .prepare(
            `UPDATE assignments SET state = 'completed', authority_state = 'contained',
              state_version = state_version + 1, ended_at = ?
             WHERE project_id = ? AND assignment_id = ? AND state = 'reported'`,
          )
          .run(now, this.#projectId, input.assignmentId);
        this.#database
          .prepare(
            `UPDATE assignment_attempts SET state = 'completed',
              authority_state = 'contained', state_version = state_version + 1,
              ended_at = ? WHERE project_id = ? AND assignment_id = ?
                AND generation = ? AND state = 'reported'`,
          )
          .run(now, this.#projectId, input.assignmentId, correction.generation);
        return {
          value: {
            acceptedWorkItemId: input.workItemId,
            candidateId: input.candidateId,
          },
          event: {
            entityType: "work_item",
            entityId: input.workItemId,
            stateVersion: correction.work_version + 1,
            fromState: "awaiting_verification",
            toState: "accepted",
            details: {
              findingId: input.findingId,
              candidateId: input.candidateId,
              assignmentId: input.assignmentId,
              verifierCorrection: true,
            },
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
        this.#assertSupervisionReady(workItemId);
        const candidate = this.#database
          .prepare(
            `
        SELECT c.assignment_id, c.attempt, c.generation, c.input_revision, c.report_hash, c.created_by,
          a.work_item_id, a.state AS assignment_state, a.authority_state, a.worker_actor_id,
          at.state AS attempt_state,
          c.generation = (
            SELECT MAX(latest_attempt.generation)
            FROM assignments latest
            JOIN assignment_attempts latest_attempt ON latest_attempt.project_id = latest.project_id
              AND latest_attempt.assignment_id = latest.assignment_id
            JOIN seats latest_seat ON latest_seat.project_id = latest.project_id
              AND latest_seat.seat_id = latest.seat_id
            WHERE latest.project_id = a.project_id AND latest.work_item_id = a.work_item_id
              AND latest_seat.role = 'Developer'
          ) AS is_latest_developer_assignment,
          w.state AS work_state, w.state_version AS work_version, w.input_revision AS work_revision,
          r.state AS run_state
        FROM candidates c
        JOIN assignments a ON a.project_id = c.project_id AND a.assignment_id = c.assignment_id
        JOIN assignment_attempts at ON at.project_id = c.project_id AND at.assignment_id = c.assignment_id
          AND at.attempt = c.attempt
        JOIN work_items w ON w.project_id = c.project_id AND w.work_item_id = a.work_item_id
        JOIN run_controls r ON r.project_id = c.project_id
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
              created_by: string;
              work_item_id: string;
              assignment_state: string;
              authority_state: string;
              worker_actor_id: string;
              is_latest_developer_assignment: number;
              attempt_state: string;
              work_state: string;
              work_version: number;
              work_revision: number;
              run_state: string;
            }
          | undefined;
        if (
          !candidate ||
          candidate.worker_actor_id === null ||
          candidate.created_by !== candidate.worker_actor_id ||
          candidate.is_latest_developer_assignment !== 1 ||
          candidate.run_state !== "active" ||
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
        const criteria = this.#acceptanceCriteria(workItemId);
        const evidence = this.#database
          .prepare(
            `
        SELECT e.verifier_assignment_id, e.criterion, e.passed, e.observation, e.exit_status,
          e.input_revision, e.created_by,
          va.worker_actor_id, va.work_item_id AS verifier_work_item_id, va.input_revision AS verifier_input_revision,
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
          AND at.generation = (
            SELECT MAX(latest_attempt.generation)
            FROM assignments latest
            JOIN assignment_attempts latest_attempt
              ON latest_attempt.project_id = latest.project_id
              AND latest_attempt.assignment_id = latest.assignment_id
            WHERE latest.project_id = va.project_id AND latest.work_item_id = va.work_item_id
          )
      `,
          )
          .all(this.#projectId, candidateId) as Array<{
          verifier_assignment_id: string;
          criterion: string;
          created_by: string;
          worker_actor_id: string | null;
          passed: number;
          observation: string | null;
          exit_status: number | null;
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
        const verifierTasks = this.#database
          .prepare(
            `
          SELECT a.assignment_id, a.state, a.authority_state,
            at.state AS attempt_state, at.authority_state AS attempt_authority,
            w.state AS work_state
          FROM assignment_input_bindings b
          JOIN assignments a ON a.project_id = b.project_id AND a.assignment_id = b.assignment_id
          JOIN assignment_attempts at ON at.project_id = a.project_id AND at.assignment_id = a.assignment_id
            AND at.generation = a.active_generation
          JOIN seats s ON s.project_id = a.project_id AND s.seat_id = a.seat_id
          JOIN work_items w ON w.project_id = a.project_id AND w.work_item_id = a.work_item_id
          WHERE b.project_id = ? AND b.input_kind = 'candidate' AND b.source_id = ? AND s.role = 'Verifier'
          AND at.generation = (
            SELECT MAX(latest_attempt.generation)
            FROM assignments latest
            JOIN assignment_attempts latest_attempt
              ON latest_attempt.project_id = latest.project_id
              AND latest_attempt.assignment_id = latest.assignment_id
            WHERE latest.project_id = a.project_id AND latest.work_item_id = a.work_item_id
          )
        `,
          )
          .all(this.#projectId, candidateId) as Array<{
          assignment_id: string;
          state: string;
          authority_state: string;
          attempt_state: string;
          attempt_authority: string;
          work_state: string;
        }>;
        if (verifierTasks.length === 0)
          throw new CandidateBindingError(
            "candidate has no bound Verifier assignments",
          );
        const openVerifierChildren = this.#database
          .prepare(
            `
          SELECT COUNT(*) AS count FROM work_items
          WHERE project_id = ? AND parent_work_item_id = ?
            AND required_role = 'Verifier' AND state NOT IN ('accepted', 'canceled')
        `,
          )
          .get(this.#projectId, workItemId) as { count: number };
        if (openVerifierChildren.count !== verifierTasks.length)
          throw new CandidateBindingError(
            "every open Verifier child must be bound to the current candidate",
          );
        for (const task of verifierTasks) {
          if (
            task.state !== "reported" ||
            task.authority_state !== "contained" ||
            task.attempt_state !== "reported" ||
            task.attempt_authority !== "contained" ||
            task.work_state !== "awaiting_verification" ||
            criteria.some(
              (criterion) =>
                !evidence.some(
                  (row) =>
                    row.verifier_assignment_id === task.assignment_id &&
                    row.criterion === criterion &&
                    row.passed === 1,
                ),
            )
          )
            throw new CandidateBindingError(
              "every bound Verifier assignment must be contained and pass every current criterion",
            );
        }
        for (const criterion of criteria) {
          const observations = evidence.filter(
            (row) => row.criterion === criterion,
          );
          if (
            observations.length === 0 ||
            observations.some(
              (row) =>
                row.passed !== 1 ||
                row.observation === null ||
                row.observation.trim().length === 0 ||
                row.exit_status !== 0 ||
                row.input_revision !== context.inputRevision ||
                row.verifier_input_revision !== context.inputRevision ||
                row.bound_candidate_hash !== candidate.report_hash,
            )
          )
            throw new CandidateBindingError(
              `candidate lacks passing current Verifier evidence for: ${criterion}`,
            );
        }
        if (evidence.some((row) => !criteria.includes(row.criterion)))
          throw new CandidateBindingError(
            "candidate has evidence for an unknown criterion",
          );
        const verifierAssignments = [
          ...new Map(
            evidence.map((row) => [row.verifier_assignment_id, row]),
          ).values(),
        ];
        for (const verifier of verifierAssignments) {
          if (
            verifier.worker_actor_id === null ||
            verifier.created_by !== verifier.worker_actor_id ||
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
          )
            throw new CandidateBindingError(
              "Verifier assignment or work state is no longer eligible for acceptance",
            );
        }
        const developerCorrection = this.#database
          .prepare(
            `SELECT f.affected_work_item_id, original.state AS original_state,
              accepted.commit_sha AS accepted_commit_sha
             FROM finding_correction_work correction
             JOIN findings f ON f.project_id = correction.project_id
               AND f.finding_id = correction.finding_id
             JOIN work_items original ON original.project_id = f.project_id
               AND original.work_item_id = f.affected_work_item_id
             LEFT JOIN candidates accepted ON accepted.project_id = original.project_id
               AND accepted.candidate_id = original.accepted_candidate_id
             WHERE correction.project_id = ? AND correction.work_item_id = ?
               AND correction.target_role = 'Developer'`,
          )
          .get(this.#projectId, workItemId) as
          | {
              affected_work_item_id: string;
              original_state: string;
              accepted_commit_sha: string | null;
            }
          | undefined;
        if (
          developerCorrection &&
          !(
            developerCorrection.original_state === "awaiting_verification" ||
            (developerCorrection.original_state === "accepted" &&
              developerCorrection.accepted_commit_sha ===
                (
                  this.#database
                    .prepare(
                      "SELECT base_sha FROM candidates WHERE project_id = ? AND candidate_id = ?",
                    )
                    .get(this.#projectId, candidateId) as
                    { base_sha: string } | undefined
                )?.base_sha)
          )
        )
          throw new CandidateBindingError(
            "Developer correction no longer targets the exact original candidate",
          );
        const now = new Date().toISOString();
        this.#database
          .prepare(
            `
        UPDATE work_items SET state = 'accepted', accepted_candidate_id = ?, state_version = state_version + 1
        WHERE project_id = ? AND work_item_id = ? AND state = 'awaiting_verification'
      `,
          )
          .run(candidateId, this.#projectId, workItemId);
        if (developerCorrection) {
          const result = this.#database
            .prepare(
              `UPDATE work_items SET state = 'accepted', accepted_candidate_id = ?,
                state_version = state_version + 1
               WHERE project_id = ? AND work_item_id = ?
                 AND state IN ('awaiting_verification', 'accepted')`,
            )
            .run(
              candidateId,
              this.#projectId,
              developerCorrection.affected_work_item_id,
            );
          if (result.changes !== 1)
            throw new CandidateBindingError(
              "Developer correction original changed before acceptance",
            );
        }
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
              ...(developerCorrection
                ? {
                    affectedOriginalWorkItemId:
                      developerCorrection.affected_work_item_id,
                  }
                : {}),
            },
          },
        };
      },
    );
  }
  acceptFinalVerification(
    context: MutationContext,
    input: {
      readonly workItemId: string;
      readonly assignmentId: string;
      readonly commitSha: string;
      readonly evidence: readonly {
        readonly evidenceId: string;
        readonly criterion: string;
        readonly passed: boolean;
        readonly artifactRef: string;
        readonly observation: string;
        readonly exitStatus: number;
      }[];
    },
  ): { readonly acceptedWorkItemId: string; readonly commitSha: string } {
    input = Object.freeze({
      ...input,
      evidence: input.evidence.map((entry) => Object.freeze({ ...entry })),
    });
    return this.#mutateAsController(
      context,
      "final_verification.accept",
      "candidate:accept",
      input,
      (actor) => {
        if (!/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/i.test(input.commitSha))
          throw new CandidateBindingError(
            "final verification commit SHA must be 40 or 64 hexadecimal characters",
          );
        const commitSha = input.commitSha.toLowerCase();
        const report = this.#database
          .prepare(
            `
          SELECT s.role, a.state AS assignment_state, a.authority_state AS assignment_authority,
            a.input_revision, at.attempt, at.generation, at.state AS attempt_state,
            at.authority_state AS attempt_authority, w.state AS work_state,
            w.state_version AS work_version, w.input_revision AS work_revision,
            w.accepted_candidate_id, w.final_verification, w.acceptance_criteria_json,
            w.parent_work_item_id,
            p.current_input_revision, c.command_id, c.state AS command_state, run.state AS run_state,
            (SELECT r.receipt_json FROM command_receipts r
              WHERE r.project_id = a.project_id AND r.command_id = c.command_id
                AND r.assignment_id = a.assignment_id AND r.attempt = at.attempt
                AND r.generation = at.generation AND r.role = s.role AND r.receipt_type = 'completed'
              ORDER BY r.sequence DESC LIMIT 1) AS completed_receipt_json
          FROM assignments a
          JOIN assignment_attempts at ON at.project_id = a.project_id
            AND at.assignment_id = a.assignment_id AND at.generation = a.active_generation
          JOIN seats s ON s.project_id = a.project_id AND s.seat_id = a.seat_id
          JOIN work_items w ON w.project_id = a.project_id AND w.work_item_id = a.work_item_id
          JOIN projects p ON p.project_id = a.project_id
          JOIN run_controls run ON run.project_id = a.project_id
          JOIN commands c ON c.project_id = a.project_id AND c.assignment_id = a.assignment_id
            AND c.attempt = at.attempt AND c.generation = at.generation
          WHERE a.project_id = ? AND a.work_item_id = ? AND a.assignment_id = ?
            AND at.generation = (SELECT MAX(latest_attempt.generation) FROM assignments latest
              JOIN assignment_attempts latest_attempt
                ON latest_attempt.project_id = latest.project_id
                AND latest_attempt.assignment_id = latest.assignment_id
              WHERE latest.project_id = a.project_id AND latest.work_item_id = a.work_item_id)
        `,
          )
          .get(this.#projectId, input.workItemId, input.assignmentId) as
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
              final_verification: number;
              acceptance_criteria_json: string | null;
              parent_work_item_id: string | null;
              current_input_revision: number;
              command_id: string;
              command_state: string;
              run_state: string;
              completed_receipt_json: string | null;
            }
          | undefined;
        let reportReply:
          | {
              readonly commitSha?: unknown;
              readonly startingSha?: unknown;
              readonly evidence?: unknown;
            }
          | undefined;
        if (report?.completed_receipt_json) {
          try {
            const receipt = JSON.parse(report.completed_receipt_json) as {
              readonly reply?: unknown;
            };
            if (typeof receipt.reply === "string") {
              const parsed = JSON.parse(receipt.reply) as unknown;
              if (
                parsed !== null &&
                typeof parsed === "object" &&
                !Array.isArray(parsed)
              )
                reportReply = parsed as typeof reportReply;
            }
          } catch {
            // Malformed or unstructured reports cannot bind final acceptance.
          }
        }
        if (
          !report ||
          !reportReply ||
          actor.role !== "controller" ||
          report.role !== "Verifier" ||
          report.final_verification !== 1 ||
          report.run_state !== "active" ||
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
          !report.acceptance_criteria_json ||
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
        )
          throw new MutationConflictError(
            "final Verifier report is not contained, current, and eligible for acceptance",
          );

        const unresolvedDependency = this.#database
          .prepare(
            `SELECT d.depends_on_work_item_id FROM dependency_edges d
             JOIN work_items dependency ON dependency.project_id = d.project_id
               AND dependency.work_item_id = d.depends_on_work_item_id
             WHERE d.project_id = ? AND d.work_item_id = ?
               AND (dependency.state <> 'accepted'
                 OR (d.required_candidate_id IS NOT NULL
                   AND dependency.accepted_candidate_id IS NOT d.required_candidate_id)
                 OR dependency.input_revision <> ?)
             LIMIT 1`,
          )
          .get(this.#projectId, input.workItemId, context.inputRevision);
        const unresolvedFinding = this.#database
          .prepare(
            `SELECT 1 AS present FROM findings f
             WHERE f.project_id = ? AND f.state <> 'resolved'
               AND NOT EXISTS (
                 SELECT 1 FROM finding_correction_work fcw
                 WHERE fcw.project_id = f.project_id
                   AND fcw.finding_id = f.finding_id
                   AND fcw.work_item_id = ?
                   AND fcw.target_role = 'Verifier'
                   AND f.state = 'correcting'
               )
             LIMIT 1`,
          )
          .get(this.#projectId, input.workItemId);
        if (unresolvedDependency || unresolvedFinding)
          throw new MutationConflictError(
            "final parent cannot be accepted with unresolved dependencies or Supervisor findings",
          );

        const criteria = acceptanceCriteriaFromContent(
          JSON.parse(report.acceptance_criteria_json),
        );
        const parentCriteria = this.#acceptanceCriteria();
        if (
          criteria.length !== parentCriteria.length ||
          criteria.some((criterion) => !parentCriteria.includes(criterion))
        )
          throw new CandidateBindingError(
            "final Verifier criteria no longer match every parent acceptance criterion",
          );
        if (input.evidence.length !== criteria.length)
          throw new CandidateBindingError(
            "final verification must contain exactly one passing artifact for every criterion",
          );
        const byCriterion = new Map<string, (typeof input.evidence)[number]>();
        const evidenceIds = new Set<string>();
        for (const entry of input.evidence) {
          if (
            typeof entry.evidenceId !== "string" ||
            entry.evidenceId.trim().length === 0 ||
            entry.evidenceId.length > 256 ||
            typeof entry.criterion !== "string" ||
            typeof entry.passed !== "boolean" ||
            typeof entry.artifactRef !== "string" ||
            entry.artifactRef.trim().length === 0 ||
            entry.artifactRef.length > 2048 ||
            typeof entry.observation !== "string" ||
            entry.observation.trim().length === 0 ||
            !Number.isInteger(entry.exitStatus) ||
            entry.exitStatus < 0 ||
            entry.exitStatus > 255 ||
            (entry.passed && entry.exitStatus !== 0) ||
            !criteria.includes(entry.criterion) ||
            byCriterion.has(entry.criterion) ||
            evidenceIds.has(entry.evidenceId)
          )
            throw new CandidateBindingError(
              "final verification evidence must uniquely match current criteria and include artifact references",
            );
          byCriterion.set(entry.criterion, entry);
          evidenceIds.add(entry.evidenceId);
        }
        const failed = criteria.find(
          (criterion) => byCriterion.get(criterion)?.passed !== true,
        );
        if (failed)
          throw new CandidateBindingError(
            `final verification failed or omitted criterion: ${failed}`,
          );
        if (
          reportReply.commitSha !== commitSha ||
          reportReply.startingSha !== commitSha ||
          !Array.isArray(reportReply.evidence) ||
          reportReply.evidence.length !== criteria.length
        )
          throw new CandidateBindingError(
            "final verification evidence does not match the reported composed commit",
          );
        const reportedCriteria = new Set<string>();
        for (const raw of reportReply.evidence) {
          if (raw === null || typeof raw !== "object" || Array.isArray(raw))
            throw new CandidateBindingError(
              "final verification report contains invalid evidence",
            );
          const observed = raw as Record<string, unknown>;
          const criterion = observed.criterion;
          const artifactRef = observed.artifactRef;
          const persisted =
            typeof criterion === "string"
              ? byCriterion.get(criterion)
              : undefined;
          if (
            !persisted ||
            reportedCriteria.has(criterion as string) ||
            observed.passed !== persisted.passed ||
            observed.observation !== persisted.observation ||
            observed.exitStatus !== persisted.exitStatus ||
            typeof artifactRef !== "string" ||
            !artifactRef.startsWith("/evidence/")
          )
            throw new CandidateBindingError(
              "final verification evidence differs from its immutable Verifier report",
            );
          const relative = path.posix.normalize(
            artifactRef.slice("/evidence/".length),
          );
          if (
            !relative ||
            relative === "." ||
            path.posix.isAbsolute(relative) ||
            relative.split("/").includes("..") ||
            !persisted.artifactRef.endsWith(`${path.sep}${relative}`)
          )
            throw new CandidateBindingError(
              "final verification artifact does not match its reported evidence path",
            );
          reportedCriteria.add(criterion as string);
        }

        const now = new Date().toISOString();
        this.#database
          .prepare(
            `INSERT INTO final_verification_commits
              (project_id, work_item_id, assignment_id, input_revision, commit_sha, created_by, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)`,
          )
          .run(
            this.#projectId,
            input.workItemId,
            input.assignmentId,
            report.current_input_revision,
            commitSha,
            actor.actorId,
            now,
          );
        const insertEvidence = this.#database.prepare(
          `INSERT INTO final_verification_evidence
            (project_id, work_item_id, assignment_id, evidence_id, input_revision, commit_sha, criterion,
             passed, artifact_ref, observation, exit_status, evidence_hash, created_by, created_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?, ?, ?, ?)`,
        );
        for (const criterion of criteria) {
          const entry = byCriterion.get(criterion)!;
          const evidenceHash = digestJson({
            workItemId: input.workItemId,
            assignmentId: input.assignmentId,
            inputRevision: report.current_input_revision,
            commitSha,
            evidenceId: entry.evidenceId,
            criterion,
            passed: true,
            artifactRef: entry.artifactRef,
            observation: entry.observation,
            exitStatus: entry.exitStatus,
          });
          insertEvidence.run(
            this.#projectId,
            input.workItemId,
            input.assignmentId,
            entry.evidenceId,
            report.current_input_revision,
            commitSha,
            criterion,
            entry.artifactRef,
            entry.observation,
            entry.exitStatus,
            evidenceHash,
            actor.actorId,
            now,
          );
        }
        this.#database
          .prepare(
            `UPDATE work_items SET state = 'accepted', state_version = state_version + 1
             WHERE project_id = ? AND work_item_id = ? AND state = 'awaiting_verification'`,
          )
          .run(this.#projectId, input.workItemId);
        this.#database
          .prepare(
            `UPDATE assignments SET state = 'completed', state_version = state_version + 1,
             ended_at = ? WHERE project_id = ? AND assignment_id = ? AND state = 'reported'`,
          )
          .run(now, this.#projectId, input.assignmentId);
        this.#database
          .prepare(
            `UPDATE assignment_attempts SET state = 'completed', state_version = state_version + 1,
             ended_at = ? WHERE project_id = ? AND assignment_id = ? AND attempt = ? AND state = 'reported'`,
          )
          .run(now, this.#projectId, input.assignmentId, report.attempt);
        return {
          value: {
            acceptedWorkItemId: input.workItemId,
            commitSha,
          },
          event: {
            entityType: "work_item",
            entityId: input.workItemId,
            stateVersion: report.work_version + 1,
            fromState: "awaiting_verification",
            toState: "accepted",
            details: {
              assignmentId: input.assignmentId,
              commandId: report.command_id,
              commitSha,
              finalVerification: true,
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
            c.state AS command_state, run.state AS run_state,
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
          JOIN run_controls run ON run.project_id = a.project_id
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
            AND at.generation = (
              SELECT MAX(latest_attempt.generation)
              FROM assignments latest
              JOIN assignment_attempts latest_attempt
                ON latest_attempt.project_id = latest.project_id
                AND latest_attempt.assignment_id = latest.assignment_id
              WHERE latest.project_id = a.project_id
                AND latest.work_item_id = a.work_item_id
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
              run_state: string;
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
          report.run_state !== "active" ||
          report.assignment_state !== "reported" ||
          (report.role !== "PM" && report.role !== "Supervisor") ||
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

  cancelSupervisorReport(
    context: MutationContext,
    workItemId: string,
    assignmentId: string,
    reason: string,
  ): { readonly canceledWorkItemId: string } {
    return this.#mutateAsController(
      context,
      "work.report.reject",
      "work:assign",
      { workItemId, assignmentId, reason },
      (actor) => {
        const report = this.#database
          .prepare(
            `SELECT w.state, w.state_version, a.authority_state, s.role
             FROM work_items w
             JOIN assignments a ON a.project_id = w.project_id AND a.work_item_id = w.work_item_id
             JOIN seats s ON s.project_id = a.project_id AND s.seat_id = a.seat_id
             WHERE w.project_id = ? AND w.work_item_id = ? AND a.assignment_id = ?`,
          )
          .get(this.#projectId, workItemId, assignmentId) as
          | {
              state: string;
              state_version: number;
              authority_state: string;
              role: string;
            }
          | undefined;
        if (
          actor.role !== "controller" ||
          typeof reason !== "string" ||
          !reason.trim() ||
          reason.length > 2048 ||
          !report ||
          report.role !== "Supervisor" ||
          report.authority_state !== "contained" ||
          !["blocked", "awaiting_verification"].includes(report.state) ||
          !this.#isTransitionAllowed(
            "work_item",
            report.state,
            "canceled",
            actor,
          )
        )
          throw new MutationConflictError(
            "only a contained, unaccepted Supervisor report can be canceled",
          );
        this.#database
          .prepare(
            `UPDATE work_items SET state = 'canceled', state_version = state_version + 1
             WHERE project_id = ? AND work_item_id = ? AND state = ? AND state_version = ?`,
          )
          .run(this.#projectId, workItemId, report.state, report.state_version);
        return {
          value: { canceledWorkItemId: workItemId },
          event: {
            entityType: "work_item",
            entityId: workItemId,
            stateVersion: report.state_version + 1,
            fromState: report.state,
            toState: "canceled",
            details: { assignmentId, reason },
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
            c.start_requested,
            (SELECT COUNT(*) FROM outbox_delivery_attempts delivery
             WHERE delivery.project_id = c.project_id AND delivery.command_id = c.command_id
               AND delivery.outcome = 'attempting') AS delivery_attempt_count,
            w.work_item_id, w.state AS work_state, w.state_version AS work_version
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
              delivery_attempt_count: number;
              work_item_id: string;
              work_state: string;
              work_version: number;
            }
          | undefined;
        const prestartUncertain =
          assignment?.authority_state === "unknown" &&
          assignment.command_state === "unknown" &&
          assignment.start_requested === 0;
        const neverDeliveredPrestart =
          prestartUncertain && assignment?.delivery_attempt_count === 0;
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
          (prestartUncertain &&
            !reconciledPrestart &&
            !neverDeliveredPrestart) ||
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
        const neverDeliveredQueued =
          assignment?.authority_state === "active" &&
          assignment.state === "created" &&
          assignment.attempt_state === "created" &&
          assignment.command_state === "queued" &&
          assignment.start_requested === 0;
        if (
          !assignment ||
          (assignment.authority_state !== "active" &&
            assignment.authority_state !== "unknown") ||
          assignment.attempt_authority !== assignment.authority_state ||
          (assignment.authority_state === "active" &&
            assignment.state !== "running" &&
            assignment.state !== "reported" &&
            !neverDeliveredQueued) ||
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
        this.#database
          .prepare(
            `UPDATE recovery_attempts SET containment_state = 'contained',
              containment_proof_ref = ?
             WHERE project_id = ? AND assignment_id = ? AND generation = ?
               AND outcome IN ('pending', 'blocked') AND containment_state = 'unknown'`,
          )
          .run(
            proofRef,
            this.#projectId,
            assignmentId,
            assignment.active_generation,
          );
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

  beginReplacement(
    context: MutationContext,
    input: ReplacementInput,
  ): {
    readonly recoveryId: string;
    readonly outcome: "pending" | "blocked";
    readonly limit: number;
    readonly assignmentId: string;
    readonly generation: number;
    readonly commandId: string;
  } {
    return this.#mutateAsController(
      context,
      "recovery.replacement.begin",
      "recovery:write",
      input,
      (actor) => {
        if (actor.role !== "controller" || input.reason.trim().length === 0)
          throw new TransitionAuthorizationError(
            "replacement requires a controller and non-empty reason",
          );
        const assignment = this.#database
          .prepare(
            `SELECT a.work_item_id, a.active_generation, a.authority_state,
              at.state AS attempt_state, w.state AS work_state,
              w.state_version AS work_version, c.command_id, c.state AS command_state
             FROM assignments a JOIN assignment_attempts at
               ON at.project_id = a.project_id AND at.assignment_id = a.assignment_id
                 AND at.generation = a.active_generation
             JOIN work_items w ON w.project_id = a.project_id AND w.work_item_id = a.work_item_id
             JOIN commands c ON c.project_id = a.project_id AND c.assignment_id = a.assignment_id
               AND c.generation = a.active_generation
             WHERE a.project_id = ? AND a.assignment_id = ?`,
          )
          .get(this.#projectId, input.assignmentId) as
          | {
              work_item_id: string;
              active_generation: number;
              authority_state: string;
              attempt_state: string;
              work_state: string;
              work_version: number;
              command_id: string;
              command_state: string;
            }
          | undefined;
        if (
          !assignment ||
          assignment.work_item_id !== input.workItemId ||
          assignment.authority_state !== "active" ||
          ![
            "created",
            "dispatched",
            "acknowledged",
            "running",
            "reported",
          ].includes(assignment.attempt_state) ||
          !["running", "awaiting_verification"].includes(assignment.work_state)
        )
          throw new MutationConflictError(
            "replacement must target the active assignment for running work",
          );
        const count = (
          this.#database
            .prepare(
              `SELECT COUNT(*) AS count FROM recovery_attempts
               WHERE project_id = ? AND recovery_type = 'worker_replacement'
                 AND work_item_id = ? AND outcome IN ('pending', 'replacement_created')`,
            )
            .get(this.#projectId, input.workItemId) as { count: number }
        ).count;
        const limit = this.#recoveryLimit("worker_replacement");
        const outcome = count >= limit ? "blocked" : "pending";
        const now = new Date().toISOString();
        this.#database
          .prepare(
            `INSERT INTO recovery_attempts(project_id, recovery_id, work_item_id, assignment_id,
              recovery_type, finding_id, generation, reason, containment_state,
              containment_proof_ref, outcome, created_by, created_at)
             VALUES (?, ?, ?, ?, 'worker_replacement', NULL, ?, ?, 'unknown', NULL, ?, ?, ?)`,
          )
          .run(
            this.#projectId,
            input.recoveryId,
            input.workItemId,
            input.assignmentId,
            assignment.active_generation,
            input.reason,
            outcome,
            actor.actorId,
            now,
          );
        this.#database
          .prepare(
            `UPDATE assignments SET state = 'revoked', authority_state = 'unknown',
              state_version = state_version + 1, ended_at = COALESCE(ended_at, ?)
             WHERE project_id = ? AND assignment_id = ?`,
          )
          .run(now, this.#projectId, input.assignmentId);
        this.#database
          .prepare(
            `UPDATE assignment_attempts SET state = 'revoked', authority_state = 'unknown',
              state_version = state_version + 1, ended_at = COALESCE(ended_at, ?)
             WHERE project_id = ? AND assignment_id = ? AND generation = ?`,
          )
          .run(
            now,
            this.#projectId,
            input.assignmentId,
            assignment.active_generation,
          );
        if (assignment.command_state === "queued") {
          this.#database
            .prepare(
              `UPDATE commands SET state = 'unknown', state_version = state_version + 1,
                updated_at = ? WHERE project_id = ? AND command_id = ? AND state = 'queued'`,
            )
            .run(now, this.#projectId, assignment.command_id);
          this.#appendOutboxOutcome(
            assignment.command_id,
            "unknown",
            canonicalJson({ reason: "replacement revoked queued assignment" }),
            now,
          );
        }
        this.#database
          .prepare(
            `UPDATE work_items SET state = 'blocked', state_version = state_version + 1
             WHERE project_id = ? AND work_item_id = ? AND state IN ('running', 'awaiting_verification')`,
          )
          .run(this.#projectId, input.workItemId);
        return {
          value: {
            recoveryId: input.recoveryId,
            outcome,
            limit,
            assignmentId: input.assignmentId,
            generation: assignment.active_generation,
            commandId: assignment.command_id,
          },
          event: {
            entityType: "work_item",
            entityId: input.workItemId,
            stateVersion: assignment.work_version + 1,
            fromState: assignment.work_state,
            toState: "blocked",
            details: {
              recoveryId: input.recoveryId,
              assignmentId: input.assignmentId,
              generation: assignment.active_generation,
              reason: input.reason,
            },
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
        SELECT a.work_item_id, a.active_generation, a.authority_state, a.containment_proof_ref,
          w.state AS work_state, w.state_version AS work_version
        FROM assignments a JOIN work_items w
          ON w.project_id = a.project_id AND w.work_item_id = a.work_item_id
        WHERE a.project_id = ? AND a.assignment_id = ?
      `,
          )
          .get(this.#projectId, input.assignmentId) as
          | {
              work_item_id: string;
              active_generation: number;
              authority_state: string;
              containment_proof_ref: string | null;
              work_state: string;
              work_version: number;
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
        const requiresContainment =
          input.recoveryType === "worker_replacement" ||
          input.recoveryType === "implementation_remediation";
        const replacementContained =
          !requiresContainment ||
          (assignment.authority_state === "contained" &&
            assignment.containment_proof_ref !== null);
        if (requiresContainment && !replacementContained)
          throw new MutationConflictError(
            "recovery records require confirmed physical containment of the prior assignment",
          );
        const outcome = count >= limit ? "blocked" : "pending";
        const returnsToBlocked =
          input.recoveryType === "implementation_remediation" &&
          outcome === "pending" &&
          assignment.work_state === "awaiting_verification";
        if (
          returnsToBlocked &&
          !this.#isTransitionAllowed(
            "work_item",
            "awaiting_verification",
            "blocked",
            actor,
          )
        )
          throw new TransitionAuthorizationError(
            "transition table rejects implementation remediation",
          );
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
        if (returnsToBlocked) {
          this.#database
            .prepare(
              `
            UPDATE work_items SET state = 'blocked', state_version = state_version + 1
            WHERE project_id = ? AND work_item_id = ? AND state = 'awaiting_verification'
          `,
            )
            .run(this.#projectId, input.workItemId);
        }
        return {
          value: { recoveryId: input.recoveryId, outcome, limit },
          event: {
            entityType: returnsToBlocked ? "work_item" : "recovery_attempt",
            entityId: returnsToBlocked ? input.workItemId : input.recoveryId,
            stateVersion: returnsToBlocked ? assignment.work_version + 1 : 0,
            fromState: returnsToBlocked ? "awaiting_verification" : "pending",
            toState: returnsToBlocked ? "blocked" : outcome,
            details: {
              recoveryId: input.recoveryId,
              recoveryType: input.recoveryType,
              containmentState: assignment.authority_state,
            },
          },
        };
      },
    );
  }
  pendingReplacementRecovery(
    workItemId: string,
    assignmentId: string,
  ): string | undefined {
    this.#assertOpen();
    const row = this.#database
      .prepare(
        `SELECT recovery_id FROM recovery_attempts
         WHERE project_id = ? AND work_item_id = ? AND assignment_id = ?
           AND recovery_type IN ('worker_replacement', 'implementation_remediation')
           AND containment_state = 'contained' AND outcome = 'pending'
         ORDER BY created_at DESC, recovery_id DESC LIMIT 1`,
      )
      .get(this.#projectId, workItemId, assignmentId) as
      { recovery_id: string } | undefined;
    return row?.recovery_id;
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
        const unresolvedFinding = this.#database
          .prepare(
            "SELECT 1 AS present FROM findings WHERE project_id = ? AND state <> 'resolved' LIMIT 1",
          )
          .get(this.#projectId);
        if (unresolvedFinding)
          throw new MutationConflictError(
            "run cannot complete with unresolved Supervisor findings",
          );
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
        const staleAcceptedWork = this.#database
          .prepare(
            "SELECT 1 AS present FROM work_items WHERE project_id = ? AND state = 'accepted' AND input_revision <> ? LIMIT 1",
          )
          .get(this.#projectId, context.inputRevision);
        if (staleAcceptedWork)
          throw new MutationConflictError(
            "run cannot complete with work accepted against a stale input revision",
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
          (input.value !== undefined && !Number.isFinite(input.value)) ||
          (input.availability === "unavailable" && input.value !== undefined) ||
          !["observed", "unavailable", "inferred"].includes(input.availability)
        ) {
          throw new ControllerError(
            "usage requires a valid provider and metric, with values only for observed or inferred measurements",
          );
        }
        const assignment = input.assignmentId
          ? (this.#database
              .prepare(
                "SELECT worker_actor_id, seat_id, authority_state, active_generation FROM assignments WHERE project_id = ? AND assignment_id = ?",
              )
              .get(this.#projectId, input.assignmentId) as
              | {
                  worker_actor_id: string | null;
                  seat_id: string;
                  authority_state: string;
                  active_generation: number;
                }
              | undefined)
          : undefined;
        if (input.assignmentId && !assignment)
          throw new ControllerError(
            "usage assignment does not belong to this project",
          );
        const session = input.sessionId
          ? (this.#database
              .prepare(
                "SELECT assignment_id, seat_id FROM runtime_sessions WHERE project_id = ? AND session_id = ?",
              )
              .get(this.#projectId, input.sessionId) as
              { assignment_id: string | null; seat_id: string } | undefined)
          : undefined;
        if (input.sessionId && !session)
          throw new ControllerError(
            "usage session does not belong to this project",
          );
        if (
          session &&
          input.assignmentId &&
          session.assignment_id !== input.assignmentId
        )
          throw new ControllerError(
            "usage session and assignment do not belong together",
          );
        if (actor.seatId && session && !input.assignmentId)
          throw new TransitionAuthorizationError(
            "worker usage requires a session bound to its assignment",
          );
        if (actor.seatId && !input.sessionId && !input.assignmentId)
          throw new TransitionAuthorizationError(
            "worker usage requires a session or assignment binding",
          );
        if (
          actor.seatId &&
          assignment &&
          (assignment.worker_actor_id !== actor.actorId ||
            assignment.seat_id !== actor.seatId ||
            assignment.authority_state !== "active")
        )
          throw new TransitionAuthorizationError(
            "worker usage must belong to the actor's active assignment generation",
          );
        if (
          actor.seatId &&
          session &&
          (session.seat_id !== actor.seatId ||
            session.assignment_id !== (input.assignmentId ?? null))
        )
          throw new TransitionAuthorizationError(
            "worker usage must belong to the actor's assigned session",
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
        const source = this.#database
          .prepare(
            `SELECT a.work_item_id, a.seat_id, a.active_generation, a.authority_state, s.role
             FROM assignments a JOIN seats s ON s.project_id = a.project_id AND s.seat_id = a.seat_id
             WHERE a.project_id = ? AND a.assignment_id = ?`,
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
        const sourceSupervisorOverlap =
          source?.role === "Supervisor" &&
          !!this.#database
            .prepare(
              `SELECT 1 FROM assignments
               WHERE project_id = ? AND seat_id = ?
                 AND authority_state IN ('active', 'unknown')
               GROUP BY seat_id HAVING COUNT(*) > 1`,
            )
            .get(this.#projectId, source.seat_id);
        if (
          !source ||
          source.work_item_id !== input.workItemId ||
          source.seat_id !== actor.seatId ||
          source.role !== "Supervisor" ||
          source.active_generation !== input.generation ||
          (source.authority_state !== "active" &&
            !(source.authority_state === "unknown" && sourceSupervisorOverlap))
        )
          throw new ControllerError(
            "finding must be bound to an active or overlapping unknown Supervisor assignment generation",
          );
        const target = this.#database
          .prepare(
            `SELECT a.work_item_id, a.seat_id, a.active_generation, a.authority_state, s.role
             FROM assignments a JOIN seats s ON s.project_id = a.project_id AND s.seat_id = a.seat_id
             WHERE a.project_id = ? AND a.assignment_id = ?`,
          )
          .get(this.#projectId, input.affectedAssignmentId) as
          | {
              work_item_id: string;
              seat_id: string;
              active_generation: number;
              authority_state: string;
              role: string;
            }
          | undefined;
        const supervisorOverlap =
          target?.role === "Supervisor" &&
          !!this.#database
            .prepare(
              `SELECT 1 FROM assignments
               WHERE project_id = ? AND seat_id = ?
                 AND authority_state IN ('active', 'unknown')
               GROUP BY seat_id HAVING COUNT(*) > 1`,
            )
            .get(this.#projectId, target.seat_id);
        if (
          !target ||
          target.work_item_id !== input.affectedWorkItemId ||
          target.seat_id !== input.affectedSeatId ||
          target.active_generation !== input.affectedGeneration ||
          !["active", "contained", "unknown"].includes(
            target.authority_state,
          ) ||
          (!["PM", "Developer", "Verifier"].includes(target.role) &&
            !supervisorOverlap)
        )
          throw new ControllerError(
            "finding target must be a current worker or overlapping Supervisor seat",
          );
        const now = new Date().toISOString();
        const deadline = Date.parse(input.acknowledgementDeadline);
        if (
          !Number.isFinite(deadline) ||
          deadline <= Date.parse(now) ||
          input.escalationRoute !== "operator" ||
          !input.fingerprint.trim() ||
          !input.requestedCorrection.trim() ||
          !input.resolutionCondition.trim()
        )
          throw new TypeError(
            "finding deadline, route, and instructions are invalid",
          );
        const duplicate = this.#database
          .prepare(
            `SELECT finding_id FROM findings
             WHERE project_id = ? AND affected_assignment_id = ? AND fingerprint = ?
               AND (state <> 'resolved' OR cooldown_until > ?)
             ORDER BY created_at DESC, finding_id DESC LIMIT 1`,
          )
          .get(
            this.#projectId,
            input.affectedAssignmentId,
            input.fingerprint,
            now,
          ) as { finding_id: string } | undefined;
        if (duplicate)
          return {
            value: { findingId: duplicate.finding_id },
            event: {
              entityType: "finding",
              entityId: duplicate.finding_id,
              stateVersion: 0,
              toState: "detected",
              details: { deduplicated: true },
            },
          };
        const reopened = this.#database
          .prepare(
            `SELECT finding_id FROM findings
             WHERE project_id = ? AND affected_assignment_id = ?
               AND fingerprint = ? AND state = 'resolved'
               AND cooldown_until <= ?
             ORDER BY created_at DESC, finding_id DESC LIMIT 1`,
          )
          .get(
            this.#projectId,
            input.affectedAssignmentId,
            input.fingerprint,
            now,
          ) as { finding_id: string } | undefined;
        const detectedAfter = (
          this.#database
            .prepare(
              "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM controller_events WHERE project_id = ?",
            )
            .get(this.#projectId) as { next: number }
        ).next;
        this.#database
          .prepare(
            `INSERT INTO findings(project_id, finding_id, work_item_id, assignment_id, generation,
              fingerprint, severity, evidence_json, requested_correction, resolution_condition,
              state, state_version, created_by, created_at, affected_seat_id, affected_work_item_id,
              affected_assignment_id, affected_generation, acknowledgement_deadline, escalation_route,
              detected_after_sequence, reopened_from_finding_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'detected', 0, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
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
            input.affectedSeatId,
            input.affectedWorkItemId,
            input.affectedAssignmentId,
            input.affectedGeneration,
            input.acknowledgementDeadline,
            input.escalationRoute,
            detectedAfter,
            reopened?.finding_id ?? null,
          );
        return {
          value: { findingId: input.findingId },
          event: {
            entityType: "finding",
            entityId: input.findingId,
            stateVersion: 0,
            toState: "detected",
            details: {
              affectedAssignmentId: input.affectedAssignmentId,
              affectedGeneration: input.affectedGeneration,
              reopenedFromFindingId: reopened?.finding_id ?? null,
            },
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
          `SELECT work_item_id, assignment_id, generation,
            resolution_condition, state, state_version,
            affected_work_item_id, affected_assignment_id, affected_seat_id,
            affected_generation, detected_after_sequence
           FROM findings WHERE project_id = ? AND finding_id = ?`,
        )
        .get(this.#projectId, findingId) as
        | {
            work_item_id: string;
            assignment_id: string | null;
            generation: number | null;
            resolution_condition: string;
            state: string;
            state_version: number;
            affected_work_item_id: string | null;
            affected_assignment_id: string | null;
            affected_seat_id: string | null;
            affected_generation: number | null;
            detected_after_sequence: number;
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
        let responseAssignment: string | undefined;
        if (toState === "reported" && actor.role === "Supervisor") {
          responseAssignment = (
            this.#database
              .prepare(
                `SELECT assignment_id FROM assignments
                 WHERE project_id = ? AND assignment_id = ? AND work_item_id = ?
                   AND seat_id = ? AND active_generation = ? AND authority_state = 'active'`,
              )
              .get(
                this.#projectId,
                finding.assignment_id,
                finding.work_item_id,
                actor.seatId,
                finding.generation,
              ) as { assignment_id: string } | undefined
          )?.assignment_id;
        } else if (
          (toState === "acknowledged" || toState === "disputed") &&
          actor.seatId &&
          finding.affected_assignment_id &&
          finding.affected_seat_id &&
          finding.affected_generation
        ) {
          responseAssignment = (
            this.#database
              .prepare(
                `SELECT a.assignment_id FROM assignments a
                 WHERE a.project_id = ? AND a.seat_id = ?
                   AND a.seat_id = ? AND a.authority_state = 'active'
                   AND (
                     (a.assignment_id = ? AND a.active_generation = ?)
                     OR EXISTS (
                       SELECT 1 FROM finding_correction_work correction
                       WHERE correction.project_id = ? AND correction.finding_id = ?
                         AND correction.work_item_id = a.work_item_id
                         AND correction.target_assignment_id = ?
                         AND correction.target_generation = ?
                     )
                   )
                 ORDER BY CASE WHEN a.assignment_id = ? THEN 0 ELSE 1 END LIMIT 1`,
              )
              .get(
                this.#projectId,
                actor.seatId,
                finding.affected_seat_id,
                finding.affected_assignment_id,
                finding.affected_generation,
                this.#projectId,
                findingId,
                finding.affected_assignment_id,
                finding.affected_generation,
                finding.affected_assignment_id,
              ) as { assignment_id: string } | undefined
          )?.assignment_id;
        }
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
      } else if (toState === "correcting") {
        const correction = content as { workItemId?: unknown } | null;
        if (
          !["acknowledged", "correcting"].includes(finding.state) ||
          typeof correction?.workItemId !== "string" ||
          !finding.affected_assignment_id ||
          !finding.affected_work_item_id ||
          !finding.affected_seat_id ||
          !finding.affected_generation
        )
          throw new ControllerError(
            "correction requires an acknowledged or correcting finding and a bound worker assignment",
          );
        const target = this.#database
          .prepare(
            `SELECT a.seat_id, a.active_generation, a.authority_state, s.role
             FROM assignments a JOIN seats s ON s.project_id = a.project_id AND s.seat_id = a.seat_id
             WHERE a.project_id = ? AND a.assignment_id = ?`,
          )
          .get(this.#projectId, finding.affected_assignment_id) as
          | {
              seat_id: string;
              active_generation: number;
              authority_state: string;
              role: string;
            }
          | undefined;
        const work = this.#database
          .prepare(
            "SELECT required_role, state FROM work_items WHERE project_id = ? AND work_item_id = ?",
          )
          .get(this.#projectId, correction.workItemId) as
          { required_role: string; state: string } | undefined;
        const interventions = this.#database
          .prepare(
            "SELECT intervention_count FROM findings WHERE project_id = ? AND finding_id = ?",
          )
          .get(this.#projectId, findingId) as
          { intervention_count: number } | undefined;
        const existingCorrection = this.#database
          .prepare(
            `SELECT target_assignment_id, target_generation, target_seat_id, target_role
             FROM finding_correction_work WHERE project_id = ? AND work_item_id = ? AND finding_id = ?`,
          )
          .get(this.#projectId, correction.workItemId, findingId) as
          | {
              target_assignment_id: string;
              target_generation: number;
              target_seat_id: string;
              target_role: string;
            }
          | undefined;
        if (
          !target ||
          !work ||
          !["pending", "ready", "running", "awaiting_verification"].includes(
            work.state,
          ) ||
          work.required_role !== target.role ||
          target.seat_id !== finding.affected_seat_id ||
          target.active_generation !== finding.affected_generation ||
          target.authority_state !== "contained" ||
          !interventions ||
          (!existingCorrection && interventions.intervention_count >= 2)
        )
          throw new ControllerError(
            "correction work must target the contained affected assignment within the intervention budget",
          );
        if (
          existingCorrection &&
          (existingCorrection.target_assignment_id !==
            finding.affected_assignment_id ||
            existingCorrection.target_generation !==
              finding.affected_generation ||
            existingCorrection.target_seat_id !== target.seat_id ||
            existingCorrection.target_role !== target.role)
        )
          throw new ControllerError(
            "correction work mapping does not match the finding",
          );
        if (!existingCorrection) {
          this.#database
            .prepare(
              `INSERT INTO finding_correction_work(project_id, work_item_id, finding_id,
                target_assignment_id, target_generation, target_seat_id, target_role, created_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
            )
            .run(
              this.#projectId,
              correction.workItemId,
              findingId,
              finding.affected_assignment_id,
              finding.affected_generation,
              target.seat_id,
              target.role,
              now,
            );
          this.#database
            .prepare(
              "UPDATE findings SET intervention_count = intervention_count + 1 WHERE project_id = ? AND finding_id = ?",
            )
            .run(this.#projectId, findingId);
        }
      } else if (toState === "resolved" || toState === "escalated") {
        if (toState === "resolved") {
          const resolution = content as {
            condition?: unknown;
            evidenceEventId?: unknown;
            assignmentId?: unknown;
            generation?: unknown;
            evidence?: unknown;
          } | null;
          const correctionWork = this.#database
            .prepare(
              `SELECT fcw.work_item_id, fcw.target_generation, fcw.target_role,
                (SELECT parent_work_item_id FROM work_items
                 WHERE project_id = fcw.project_id AND work_item_id = fcw.work_item_id)
                  AS parent_work_item_id
               FROM finding_correction_work fcw
               WHERE fcw.project_id = ? AND fcw.finding_id = ?
               ORDER BY fcw.created_at DESC LIMIT 1`,
            )
            .get(this.#projectId, findingId) as
            | {
                work_item_id: string;
                target_generation: number;
                target_role: string;
                parent_work_item_id: string | null;
              }
            | undefined;
          const evidence = resolution?.evidence;
          const proofEvidence =
            evidence && typeof evidence === "object" && !Array.isArray(evidence)
              ? (evidence as Record<string, unknown>)
              : undefined;
          if (
            !resolution ||
            typeof resolution !== "object" ||
            Array.isArray(resolution) ||
            resolution.condition !== finding.resolution_condition ||
            typeof resolution.evidenceEventId !== "string" ||
            typeof resolution.assignmentId !== "string" ||
            typeof resolution.generation !== "number" ||
            !Number.isSafeInteger(resolution.generation) ||
            resolution.generation < 1 ||
            !proofEvidence ||
            typeof proofEvidence.correctionEventId !== "string" ||
            proofEvidence.correctionEventId !== resolution.evidenceEventId ||
            !proofEvidence.correctionEventId.trim() ||
            typeof proofEvidence.supervisorCheckpointAssignmentId !==
              "string" ||
            !proofEvidence.supervisorCheckpointAssignmentId.trim() ||
            typeof proofEvidence.supervisorVerificationAssignmentId !==
              "string" ||
            !proofEvidence.supervisorVerificationAssignmentId.trim() ||
            proofEvidence.supervisorCheckpointAssignmentId !==
              proofEvidence.supervisorVerificationAssignmentId ||
            proofEvidence.condition !== finding.resolution_condition ||
            typeof proofEvidence.supervisorVerificationEvidence !== "string" ||
            !proofEvidence.supervisorVerificationEvidence.trim() ||
            proofEvidence.supervisorVerificationEvidence.length > 2048 ||
            (!correctionWork &&
              resolution.generation <= (finding.affected_generation ?? 0)) ||
            !finding.affected_work_item_id ||
            !finding.affected_seat_id
          )
            throw new ControllerError(
              "finding resolution requires a newer assignment and authoritative evidence event",
            );
          const proof = this.#database
            .prepare(
              `SELECT sequence, entity_type, entity_id, to_state, payload_json
               FROM controller_events WHERE project_id = ? AND event_id = ?`,
            )
            .get(this.#projectId, resolution.evidenceEventId) as
            | {
                sequence: number;
                entity_type: string;
                entity_id: string;
                to_state: string | null;
                payload_json: string;
              }
            | undefined;
          const replacement = this.#database
            .prepare(
              `SELECT a.work_item_id, a.seat_id, a.active_generation, a.authority_state,
                at.state AS attempt_state, at.authority_state AS attempt_authority,
                s.role, w.state AS work_state
               FROM assignments a JOIN seats s ON s.project_id = a.project_id AND s.seat_id = a.seat_id
               JOIN assignment_attempts at ON at.project_id = a.project_id
                 AND at.assignment_id = a.assignment_id AND at.generation = a.active_generation
               JOIN work_items w ON w.project_id = a.project_id AND w.work_item_id = a.work_item_id
               WHERE a.project_id = ? AND a.assignment_id = ?`,
            )
            .get(this.#projectId, resolution.assignmentId) as
            | {
                work_item_id: string;
                seat_id: string;
                active_generation: number;
                authority_state: string;
                attempt_state: string;
                attempt_authority: string;
                role: string;
                work_state: string;
              }
            | undefined;
          let acceptedVerifierChildMatches = false;
          let acceptedFinalVerifierCorrectionMatches = false;
          let acceptedBoundVerifierCorrectionMatches = false;
          if (proof) {
            try {
              const eventPayload = JSON.parse(proof.payload_json) as {
                details?: {
                  verifierWorkItemIds?: unknown;
                  assignmentId?: unknown;
                  finalVerification?: unknown;
                  verifierCorrection?: unknown;
                };
              };
              acceptedVerifierChildMatches =
                Array.isArray(eventPayload.details?.verifierWorkItemIds) &&
                eventPayload.details.verifierWorkItemIds.includes(
                  correctionWork?.work_item_id,
                );
              acceptedFinalVerifierCorrectionMatches =
                eventPayload.details?.finalVerification === true &&
                eventPayload.details.assignmentId === resolution.assignmentId;
              acceptedBoundVerifierCorrectionMatches =
                eventPayload.details?.verifierCorrection === true &&
                eventPayload.details.assignmentId === resolution.assignmentId;
            } catch {
              acceptedVerifierChildMatches = false;
              acceptedFinalVerifierCorrectionMatches = false;
              acceptedBoundVerifierCorrectionMatches = false;
            }
          }
          const supervisionProof = this.#database
            .prepare(
              `SELECT enabled, health, target_epoch, checkpoint_epoch,
                checkpoint_assignment_id, checkpoint_event_sequence
               FROM supervision_control WHERE project_id = ?`,
            )
            .get(this.#projectId) as
            | {
                enabled: number;
                health: string;
                target_epoch: number;
                checkpoint_epoch: number | null;
                checkpoint_assignment_id: string | null;
                checkpoint_event_sequence: number | null;
              }
            | undefined;
          const supervisorVerificationAssignmentId =
            proofEvidence?.supervisorVerificationAssignmentId;
          const verificationAssignment =
            typeof supervisorVerificationAssignmentId === "string"
              ? (this.#database
                  .prepare(
                    `SELECT a.work_item_id, a.authority_state, s.role, w.state
                     FROM assignments a
                     JOIN seats s ON s.project_id = a.project_id AND s.seat_id = a.seat_id
                     JOIN work_items w ON w.project_id = a.project_id AND w.work_item_id = a.work_item_id
                     WHERE a.project_id = ? AND a.assignment_id = ?`,
                  )
                  .get(this.#projectId, supervisorVerificationAssignmentId) as
                  | {
                      work_item_id: string;
                      authority_state: string;
                      role: string;
                      state: string;
                    }
                  | undefined)
              : undefined;
          const verificationReport =
            verificationAssignment?.role === "Supervisor" &&
            verificationAssignment.authority_state === "contained" &&
            verificationAssignment.state === "accepted"
              ? this.latestCompletedReport(
                  verificationAssignment.work_item_id,
                  supervisorVerificationAssignmentId as string,
                )
              : undefined;
          let verifiesExactFinding = false;
          if (verificationReport?.inputRevision === context.inputRevision) {
            try {
              const report = JSON.parse(verificationReport.reply) as {
                verifiedFindings?: unknown;
              };
              verifiesExactFinding =
                Array.isArray(report.verifiedFindings) &&
                report.verifiedFindings.some((entry) => {
                  if (
                    !entry ||
                    typeof entry !== "object" ||
                    Array.isArray(entry)
                  )
                    return false;
                  const verified = entry as Record<string, unknown>;
                  return (
                    verified.findingId === findingId &&
                    verified.condition === finding.resolution_condition &&
                    verified.evidence ===
                      proofEvidence?.supervisorVerificationEvidence
                  );
                });
            } catch {
              verifiesExactFinding = false;
            }
          }
          const independentlyRechecked =
            !!proof &&
            verifiesExactFinding &&
            supervisionProof?.health === "healthy" &&
            supervisionProof.checkpoint_epoch ===
              supervisionProof.target_epoch &&
            supervisionProof.checkpoint_assignment_id ===
              supervisorVerificationAssignmentId &&
            supervisionProof.checkpoint_assignment_id !==
              finding.assignment_id &&
            supervisionProof.checkpoint_event_sequence !== null &&
            supervisionProof.checkpoint_event_sequence >= proof.sequence;
          if (
            !proof ||
            !independentlyRechecked ||
            proof.entity_type !== "work_item" ||
            (correctionWork?.target_role === "Verifier"
              ? !(
                  (acceptedVerifierChildMatches &&
                    !!correctionWork.parent_work_item_id &&
                    proof.entity_id === correctionWork.parent_work_item_id) ||
                  (acceptedFinalVerifierCorrectionMatches &&
                    proof.entity_id === correctionWork.work_item_id) ||
                  (acceptedBoundVerifierCorrectionMatches &&
                    proof.entity_id === correctionWork.work_item_id)
                )
              : proof.entity_id !==
                (correctionWork?.work_item_id ??
                  finding.affected_work_item_id)) ||
            proof.to_state !== "accepted" ||
            !replacement ||
            replacement.work_item_id !==
              (correctionWork?.work_item_id ?? finding.affected_work_item_id) ||
            replacement.seat_id !== finding.affected_seat_id ||
            replacement.role !== correctionWork?.target_role ||
            replacement.active_generation !== resolution.generation ||
            replacement.authority_state !== "contained" ||
            replacement.attempt_state !== "completed" ||
            replacement.attempt_authority !== "contained" ||
            replacement.work_state !== "accepted"
          )
            throw new ControllerError(
              "finding resolution evidence does not prove an accepted, contained correction after detection",
            );
          this.#database
            .prepare(
              `INSERT INTO finding_resolution_evidence(project_id, evidence_id, finding_id,
                event_id, assignment_id, generation, condition, evidence_json, created_by, created_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
            )
            .run(
              this.#projectId,
              randomUUID(),
              findingId,
              resolution.evidenceEventId,
              resolution.assignmentId,
              resolution.generation,
              finding.resolution_condition,
              canonicalJson(resolution.evidence),
              actor.actorId,
              now,
            );
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
          `UPDATE findings SET state = ?, state_version = state_version + 1,
            cooldown_until = CASE WHEN ? IN ('resolved', 'escalated') THEN ? ELSE cooldown_until END
           WHERE project_id = ? AND finding_id = ?`,
        )
        .run(
          toState,
          toState,
          new Date(Date.parse(now) + 15 * 60_000).toISOString(),
          this.#projectId,
          findingId,
        );
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
  attemptedPrestartCommand(assignmentId: string): string | undefined {
    this.#assertOpen();
    const row = this.#database
      .prepare(
        `SELECT c.command_id FROM commands c
         WHERE c.project_id = ? AND c.assignment_id = ?
           AND c.state = 'unknown' AND c.start_requested = 0
           AND EXISTS (
             SELECT 1 FROM outbox_delivery_attempts attempt
             WHERE attempt.project_id = c.project_id
               AND attempt.command_id = c.command_id
               AND attempt.outcome = 'attempting'
           )
         ORDER BY c.generation DESC LIMIT 1`,
      )
      .get(this.#projectId, assignmentId) as { command_id: string } | undefined;
    return row?.command_id;
  }

  enableSupervision(context: MutationContext): { readonly enabled: true } {
    return this.#mutateAsController(
      context,
      "supervision.enable",
      "finding:write",
      {},
      () => {
        const existing = this.#database
          .prepare(
            "SELECT enabled FROM supervision_control WHERE project_id = ?",
          )
          .get(this.#projectId) as { enabled: number } | undefined;
        if (existing?.enabled)
          throw new MutationConflictError("Supervisor is already enabled");
        this.#database
          .prepare(
            `UPDATE supervision_control SET enabled = 1, health = 'degraded',
              bootstrap_pm_allowed = 1,
              checkpoint_epoch = NULL, checkpoint_assignment_id = NULL,
              checkpoint_event_sequence = NULL, checkpoint_fingerprint = NULL,
              replacement_attempts = 0, updated_at = ?
             WHERE project_id = ?`,
          )
          .run(new Date().toISOString(), this.#projectId);
        return {
          value: { enabled: true },
          event: {
            entityType: "run_control",
            entityId: this.#projectId,
            stateVersion: 0,
            toState: "degraded",
          },
        };
      },
    );
  }

  beginSupervisorEvaluation(context: MutationContext): {
    readonly targetEpoch: number;
    readonly eventUpperSequence: number;
  } {
    return this.#mutateAsController(
      context,
      "supervision.evaluation.begin",
      "finding:write",
      {},
      () => {
        const control = this.#database
          .prepare(
            "SELECT enabled, target_epoch FROM supervision_control WHERE project_id = ?",
          )
          .get(this.#projectId) as
          { enabled: number; target_epoch: number } | undefined;
        if (!control?.enabled)
          throw new MutationConflictError("Supervisor is not enabled");
        const eventUpperSequence = (
          this.#database
            .prepare(
              "SELECT COALESCE(MAX(sequence), 0) AS sequence FROM controller_events WHERE project_id = ?",
            )
            .get(this.#projectId) as { sequence: number }
        ).sequence;
        const now = new Date().toISOString();
        this.#database
          .prepare(
            `INSERT INTO supervisor_evaluations(project_id, target_epoch, event_upper_sequence, created_at)
             VALUES (?, ?, ?, ?)`,
          )
          .run(this.#projectId, control.target_epoch, eventUpperSequence, now);
        this.#database
          .prepare(
            "UPDATE supervision_control SET health = 'evaluating', bootstrap_pm_allowed = 0, updated_at = ? WHERE project_id = ?",
          )
          .run(now, this.#projectId);
        return {
          value: {
            targetEpoch: control.target_epoch,
            eventUpperSequence,
          },
          event: {
            entityType: "run_control",
            entityId: this.#projectId,
            stateVersion: 0,
            toState: "evaluating",
            details: {
              targetEpoch: control.target_epoch,
              eventUpperSequence,
            },
          },
        };
      },
    );
  }
  bindSupervisorEvaluation(
    context: MutationContext,
    input: {
      readonly assignmentId: string;
      readonly generation: number;
      readonly targetEpoch: number;
      readonly eventUpperSequence: number;
    },
  ): { readonly bound: true } {
    return this.#mutateAsController(
      context,
      "supervision.evaluation.bind",
      "finding:write",
      input,
      () => {
        const result = this.#database
          .prepare(
            `UPDATE supervisor_evaluations SET assignment_id = ?, generation = ?
             WHERE project_id = ? AND target_epoch = ? AND event_upper_sequence = ?
               AND assignment_id IS NULL
               AND EXISTS (
                 SELECT 1 FROM assignments a JOIN seats s
                   ON s.project_id = a.project_id AND s.seat_id = a.seat_id
                 WHERE a.project_id = supervisor_evaluations.project_id
                   AND a.assignment_id = ? AND a.active_generation = ?
                   AND s.role = 'Supervisor'
               )`,
          )
          .run(
            input.assignmentId,
            input.generation,
            this.#projectId,
            input.targetEpoch,
            input.eventUpperSequence,
            input.assignmentId,
            input.generation,
          );
        if (result.changes !== 1)
          throw new MutationConflictError(
            "Supervisor evaluation cannot bind a different or missing evaluation assignment",
          );
        return {
          value: { bound: true },
          event: {
            entityType: "run_control",
            entityId: this.#projectId,
            stateVersion: 0,
            toState: "evaluating",
            details: input,
          },
        };
      },
    );
  }

  markSupervisionDegraded(
    context: MutationContext,
    reason: string,
  ): { readonly health: "degraded" } {
    return this.#mutateAsController(
      context,
      "supervision.degraded",
      "finding:write",
      { reason },
      () => {
        if (!reason.trim() || reason.length > 2_048)
          throw new TypeError("degraded supervision requires a bounded reason");
        this.#database
          .prepare(
            "UPDATE supervision_control SET enabled = 1, health = 'degraded', bootstrap_pm_allowed = 0, updated_at = ? WHERE project_id = ?",
          )
          .run(new Date().toISOString(), this.#projectId);
        return {
          value: { health: "degraded" },
          event: {
            entityType: "run_control",
            entityId: this.#projectId,
            stateVersion: 0,
            toState: "degraded",
            details: { reason },
          },
        };
      },
    );
  }

  claimSupervisorReplacement(context: MutationContext): {
    readonly replacementAttempts: 1;
  } {
    return this.#mutateAsController(
      context,
      "supervision.replacement.claim",
      "finding:write",
      {},
      () => {
        const result = this.#database
          .prepare(
            `UPDATE supervision_control SET replacement_attempts = 1,
              health = 'degraded', updated_at = ?
             WHERE project_id = ? AND enabled = 1 AND health = 'degraded'
               AND replacement_attempts = 0`,
          )
          .run(new Date().toISOString(), this.#projectId);
        if (result.changes !== 1)
          throw new MutationConflictError(
            "Supervisor replacement budget is exhausted or supervision is not degraded",
          );
        return {
          value: { replacementAttempts: 1 },
          event: {
            entityType: "run_control",
            entityId: this.#projectId,
            stateVersion: 0,
            toState: "degraded",
            details: { replacementAttempts: 1 },
          },
        };
      },
    );
  }

  supervisorWindow(
    eventUpperSequence: number,
    eventLimit = 32,
    assignmentLimit = 16,
  ): {
    readonly events: readonly {
      sequence: number;
      eventId: string;
      entityType: string;
      entityId: string;
      fromState: string | null;
      toState: string | null;
      payloadExcerpt: string;
      createdAt: string;
    }[];
    readonly assignments: readonly {
      assignmentId: string;
      workItemId: string;
      seatId: string;
      role: string;
      generation: number;
      state: string;
      authorityState: string;
      createdAt: string;
      handoffAgeMs: number;
    }[];
    readonly eventRefs: readonly {
      assignmentId: string;
      workItemId: string;
      role: string;
      generation: number;
      eventId: string;
    }[];
    readonly dependencies: readonly {
      workItemId: string;
      blockers: readonly string[];
    }[];
    readonly fingerprints: readonly {
      findingId: string;
      fingerprint: string;
      severity: string;
      state: string;
    }[];
    readonly hardViolations: readonly {
      code: string;
      seatId: string;
      assignmentIds: readonly string[];
    }[];
  } {
    this.#assertOpen();
    if (
      !Number.isSafeInteger(eventUpperSequence) ||
      eventUpperSequence < 0 ||
      !Number.isInteger(eventLimit) ||
      eventLimit < 1 ||
      eventLimit > 128 ||
      !Number.isInteger(assignmentLimit) ||
      assignmentLimit < 1 ||
      assignmentLimit > 48
    )
      throw new TypeError("Supervisor window bounds are invalid");
    const now = Date.now();
    const events = this.#database
      .prepare(
        `SELECT sequence, event_id, entity_type, entity_id, from_state, to_state,
          substr(payload_json, 1, 128) AS payload_excerpt, created_at
         FROM controller_events WHERE project_id = ? AND sequence <= ?
         ORDER BY sequence DESC LIMIT ?`,
      )
      .all(this.#projectId, eventUpperSequence, eventLimit) as Array<{
      sequence: number;
      event_id: string;
      entity_type: string;
      entity_id: string;
      from_state: string | null;
      to_state: string | null;
      payload_excerpt: string;
      created_at: string;
    }>;
    const relevantEvents = this.#database
      .prepare(
        `SELECT relevant_assignments.assignment_id, relevant_assignments.work_item_id,
          relevant_assignments.active_generation, seat.role,
          event.sequence, event.event_id, event.entity_type, event.entity_id,
          event.from_state, event.to_state,
          substr(event.payload_json, 1, 128) AS payload_excerpt, event.created_at
         FROM assignments relevant_assignments
         JOIN seats seat ON seat.project_id = relevant_assignments.project_id
           AND seat.seat_id = relevant_assignments.seat_id
         JOIN controller_events event ON event.project_id = relevant_assignments.project_id
           AND event.sequence = (
             SELECT MAX(relevant.sequence) FROM controller_events relevant
             WHERE relevant.project_id = event.project_id
               AND relevant.sequence <= ?
               AND ((relevant.entity_type = 'assignment_attempt'
                     AND relevant.entity_id = relevant_assignments.assignment_id)
                 OR (relevant.entity_type = 'work_item'
                     AND relevant.entity_id = relevant_assignments.work_item_id)
                 OR (relevant.entity_type = 'work_item'
                     AND relevant.to_state = 'accepted'
                     AND EXISTS (
                       SELECT 1 FROM finding_correction_work correction
                       WHERE correction.project_id = relevant.project_id
                         AND correction.work_item_id = relevant.entity_id
                         AND correction.target_assignment_id = relevant_assignments.assignment_id
                         AND correction.target_generation = relevant_assignments.active_generation
                     )))
               AND relevant.payload_json NOT LIKE '%"action":"assignment.containment.confirmed"%'
               AND relevant.payload_json NOT LIKE '%"action":"finding.transition"%'
               AND relevant.payload_json NOT LIKE '%"action":"work.report.accept"%'
           )
         WHERE relevant_assignments.project_id = ? AND event.sequence <= ?
         ORDER BY event.sequence DESC LIMIT ?`,
      )
      .all(
        eventUpperSequence,
        this.#projectId,
        eventUpperSequence,
        64,
      ) as Array<
      (typeof events)[number] & {
        assignment_id: string;
        work_item_id: string;
        role: string;
        active_generation: number;
      }
    >;
    const eventRefs = relevantEvents.map((row) => ({
      assignmentId: row.assignment_id,
      workItemId: row.work_item_id,
      role: row.role,
      generation: row.active_generation,
      eventId: row.event_id,
    }));
    const eventById = new Map(
      [...events, ...relevantEvents.slice(0, assignmentLimit)].map((event) => [
        event.event_id,
        event,
      ]),
    );
    const boundedEvents = [...eventById.values()]
      .sort((left, right) => left.sequence - right.sequence)
      .map((row) => ({
        sequence: row.sequence,
        eventId: row.event_id,
        entityType: row.entity_type,
        entityId: row.entity_id,
        fromState: row.from_state,
        toState: row.to_state,
        payloadExcerpt: row.payload_excerpt,
        createdAt: row.created_at,
      }));
    const assignments = this.#database
      .prepare(
        `SELECT a.assignment_id, a.work_item_id, a.seat_id, s.role,
          a.active_generation, a.state, a.authority_state, a.created_at
         FROM assignments a JOIN seats s ON s.project_id = a.project_id AND s.seat_id = a.seat_id
         WHERE a.project_id = ?
         ORDER BY a.created_at DESC, a.assignment_id DESC LIMIT ?`,
      )
      .all(this.#projectId, assignmentLimit) as Array<{
      assignment_id: string;
      work_item_id: string;
      seat_id: string;
      role: string;
      active_generation: number;
      state: string;
      authority_state: string;
      created_at: string;
    }>;
    const snapshot = this.statusSnapshot();
    const overlappingAssignments = this.#database
      .prepare(
        `SELECT seat_id, assignment_id FROM assignments
         WHERE project_id = ? AND authority_state IN ('active', 'unknown')
         ORDER BY seat_id, assignment_id`,
      )
      .all(this.#projectId) as Array<{
      seat_id: string;
      assignment_id: string;
    }>;
    const overlappingBySeat = new Map<string, string[]>();
    for (const row of overlappingAssignments) {
      const assignmentIds = overlappingBySeat.get(row.seat_id) ?? [];
      assignmentIds.push(row.assignment_id);
      overlappingBySeat.set(row.seat_id, assignmentIds);
    }
    return {
      eventRefs,
      events: boundedEvents,
      assignments: assignments.map((row) => ({
        assignmentId: row.assignment_id,
        workItemId: row.work_item_id,
        seatId: row.seat_id,
        role: row.role,
        generation: row.active_generation,
        state: row.state,
        authorityState: row.authority_state,
        createdAt: row.created_at,
        handoffAgeMs: Math.max(0, now - Date.parse(row.created_at)),
      })),
      dependencies: snapshot.work.slice(-48).map((work) => ({
        workItemId: work.workItemId,
        blockers: work.blockers.slice(0, 16),
      })),
      fingerprints: snapshot.findings.slice(-24).map((finding) => ({
        findingId: finding.findingId,
        fingerprint: finding.fingerprint,
        severity: finding.severity,
        state: finding.state,
      })),
      hardViolations: [...overlappingBySeat]
        .filter(([, assignmentIds]) => assignmentIds.length > 1)
        .map(([seatId, assignmentIds]) => ({
          code: "overlapping_authority",
          seatId,
          assignmentIds,
        })),
    };
  }
  captureSupervisorEvaluation(
    eventUpperSequence: number,
    targetEpoch: number,
  ): {
    readonly window: ReturnType<ControllerCore["supervisorWindow"]>;
    readonly snapshot: ReturnType<ControllerCore["statusSnapshot"]>;
  } {
    this.#assertOpen();
    return this.#database.transaction(() => {
      const control = this.#database
        .prepare(
          "SELECT health, target_epoch FROM supervision_control WHERE project_id = ?",
        )
        .get(this.#projectId) as
        { health: string; target_epoch: number } | undefined;
      const latest = this.#database
        .prepare(
          "SELECT COALESCE(MAX(sequence), 0) AS sequence FROM controller_events WHERE project_id = ?",
        )
        .get(this.#projectId) as { sequence: number };
      if (
        control?.health !== "evaluating" ||
        control.target_epoch !== targetEpoch ||
        latest.sequence !== eventUpperSequence + 1 ||
        !this.#database
          .prepare(
            "SELECT 1 FROM supervisor_evaluations WHERE project_id = ? AND target_epoch = ? AND event_upper_sequence = ?",
          )
          .get(this.#projectId, targetEpoch, eventUpperSequence)
      )
        throw new MutationConflictError(
          "Supervisor evaluation context changed after its event boundary",
        );
      return {
        window: this.supervisorWindow(eventUpperSequence),
        snapshot: this.statusSnapshot(),
      };
    })();
  }

  recordSupervisorCheckpoint(
    context: MutationContext,
    input: {
      readonly assignmentId: string;
      readonly generation: number;
      readonly targetEpoch: number;
      readonly eventUpperSequence: number;
      readonly fingerprint: string;
    },
  ): { readonly health: "healthy"; readonly targetEpoch: number } {
    return this.#mutateAsController(
      context,
      "supervision.checkpoint",
      "finding:write",
      input,
      () => {
        const assignment = this.#database
          .prepare(
            `SELECT a.work_item_id, a.active_generation, a.authority_state, a.state,
              at.state AS attempt_state, at.authority_state AS attempt_authority,
              s.role, w.state AS work_state, c.state AS command_state
             FROM assignments a JOIN assignment_attempts at
               ON at.project_id = a.project_id AND at.assignment_id = a.assignment_id
                 AND at.generation = a.active_generation
             JOIN seats s ON s.project_id = a.project_id AND s.seat_id = a.seat_id
             JOIN work_items w ON w.project_id = a.project_id AND w.work_item_id = a.work_item_id
             JOIN commands c ON c.project_id = a.project_id
               AND c.assignment_id = a.assignment_id AND c.generation = at.generation
             WHERE a.project_id = ? AND a.assignment_id = ?`,
          )
          .get(this.#projectId, input.assignmentId) as
          | {
              work_item_id: string;
              active_generation: number;
              authority_state: string;
              state: string;
              attempt_state: string;
              attempt_authority: string;
              role: string;
              work_state: string;
              command_state: string;
            }
          | undefined;
        const control = this.#database
          .prepare(
            "SELECT enabled, health, target_epoch, replacement_attempts FROM supervision_control WHERE project_id = ?",
          )
          .get(this.#projectId) as
          | {
              enabled: number;
              health: string;
              target_epoch: number;
              replacement_attempts: number;
            }
          | undefined;
        const evaluation = this.#database
          .prepare(
            `SELECT 1 AS present FROM supervisor_evaluations
             WHERE project_id = ? AND target_epoch = ? AND event_upper_sequence = ?
               AND assignment_id = ? AND generation = ?
               AND event_upper_sequence = (
                 SELECT MAX(event_upper_sequence) FROM supervisor_evaluations
                 WHERE project_id = ? AND target_epoch = ?
               )`,
          )
          .get(
            this.#projectId,
            input.targetEpoch,
            input.eventUpperSequence,
            input.assignmentId,
            input.generation,
            this.#projectId,
            input.targetEpoch,
          );
        if (
          !assignment ||
          assignment.role !== "Supervisor" ||
          assignment.active_generation !== input.generation ||
          assignment.authority_state !== "contained" ||
          assignment.state !== "completed" ||
          assignment.attempt_state !== "completed" ||
          assignment.attempt_authority !== "contained" ||
          assignment.command_state !== "completed" ||
          assignment.work_state !== "accepted" ||
          !control?.enabled ||
          control.health !== "evaluating" ||
          control.target_epoch !== input.targetEpoch ||
          !evaluation ||
          !Number.isSafeInteger(input.eventUpperSequence) ||
          input.eventUpperSequence < 0 ||
          !/^[a-f0-9]{64}$/.test(input.fingerprint)
        )
          throw new MutationConflictError(
            "Supervisor checkpoint requires an unchanged, receipt-verified, contained evaluation",
          );
        this.#database
          .prepare(
            `UPDATE supervision_control SET health = 'healthy', checkpoint_epoch = ?,
              checkpoint_assignment_id = ?, checkpoint_event_sequence = ?,
              checkpoint_fingerprint = ?, updated_at = ? WHERE project_id = ?`,
          )
          .run(
            input.targetEpoch,
            input.assignmentId,
            input.eventUpperSequence,
            input.fingerprint,
            new Date().toISOString(),
            this.#projectId,
          );
        return {
          value: { health: "healthy", targetEpoch: input.targetEpoch },
          event: {
            entityType: "run_control",
            entityId: this.#projectId,
            stateVersion: 0,
            fromState: "evaluating",
            toState: "healthy",
            details: {
              assignmentId: input.assignmentId,
              generation: input.generation,
              targetEpoch: input.targetEpoch,
              eventUpperSequence: input.eventUpperSequence,
              fingerprint: input.fingerprint,
            },
          },
        };
      },
    );
  }

  #latestAssignmentRow(workItemId: string):
    | {
        assignment_id: string;
        active_generation: number;
        authority_state: string;
      }
    | undefined {
    return this.#database
      .prepare(
        `SELECT a.assignment_id, a.active_generation, a.authority_state
         FROM assignments a
         JOIN assignment_attempts at ON at.project_id = a.project_id
           AND at.assignment_id = a.assignment_id AND at.generation = a.active_generation
         WHERE a.project_id = ? AND a.work_item_id = ?
         ORDER BY at.generation DESC LIMIT 1`,
      )
      .get(this.#projectId, workItemId) as
      | {
          assignment_id: string;
          active_generation: number;
          authority_state: string;
        }
      | undefined;
  }

  latestAssignmentForWorkItem(workItemId: string):
    | {
        readonly assignmentId: string;
        readonly generation: number;
        readonly authorityState: string;
      }
    | undefined {
    this.#assertOpen();
    const row = this.#latestAssignmentRow(workItemId);
    return row
      ? {
          assignmentId: row.assignment_id,
          generation: row.active_generation,
          authorityState: row.authority_state,
        }
      : undefined;
  }

  latestAssignmentForRole(
    role: "PM" | "Developer" | "Verifier" | "Supervisor",
    assignmentId?: string,
  ):
    | {
        readonly workItemId: string;
        readonly parentWorkItemId: string | null;
        readonly role: "PM" | "Developer" | "Verifier" | "Supervisor";
        readonly seatId: string;
        readonly assignmentId: string;
        readonly generation: number;
        readonly authorityState: string;
      }
    | undefined {
    this.#assertOpen();
    const row = this.#database
      .prepare(
        `SELECT a.work_item_id, a.seat_id, a.assignment_id, a.active_generation,
          a.authority_state, w.parent_work_item_id
         FROM assignments a JOIN seats s ON s.project_id = a.project_id AND s.seat_id = a.seat_id
         JOIN work_items w ON w.project_id = a.project_id AND w.work_item_id = a.work_item_id
         WHERE a.project_id = ? AND s.role = ? AND (? IS NULL OR a.assignment_id = ?)
         ORDER BY a.created_at DESC, a.assignment_id DESC LIMIT 1`,
      )
      .get(
        this.#projectId,
        role,
        assignmentId ?? null,
        assignmentId ?? null,
      ) as
      | {
          work_item_id: string;
          parent_work_item_id: string | null;
          seat_id: string;
          assignment_id: string;
          active_generation: number;
          authority_state: string;
        }
      | undefined;
    return row
      ? {
          workItemId: row.work_item_id,
          parentWorkItemId: row.parent_work_item_id,
          role,
          seatId: row.seat_id,
          assignmentId: row.assignment_id,
          generation: row.active_generation,
          authorityState: row.authority_state,
        }
      : undefined;
  }
  candidateForAssignment(assignmentId: string):
    | {
        readonly candidateId: string;
        readonly commitSha: string;
        readonly developerAssignmentId: string;
        readonly workItemId: string;
      }
    | undefined {
    this.#assertOpen();
    const row = this.#database
      .prepare(
        `SELECT c.candidate_id, c.commit_sha, c.assignment_id AS developer_assignment_id,
          a.work_item_id
         FROM candidates c JOIN assignments a
           ON a.project_id = c.project_id AND a.assignment_id = c.assignment_id
         WHERE c.project_id = ? AND (
           c.assignment_id = ? OR c.candidate_id IN (
             SELECT source_id FROM assignment_input_bindings
             WHERE project_id = ? AND assignment_id = ? AND input_kind = 'candidate'
           )
         )
         ORDER BY c.created_at DESC, c.candidate_id DESC LIMIT 1`,
      )
      .get(this.#projectId, assignmentId, this.#projectId, assignmentId) as
      | {
          candidate_id: string;
          commit_sha: string;
          developer_assignment_id: string;
          work_item_id: string;
        }
      | undefined;
    return row
      ? {
          candidateId: row.candidate_id,
          commitSha: row.commit_sha,
          developerAssignmentId: row.developer_assignment_id,
          workItemId: row.work_item_id,
        }
      : undefined;
  }

  assignmentIsContained(assignmentId: string): boolean {
    this.#assertOpen();
    return (
      this.#database
        .prepare(
          `SELECT 1 FROM assignments
           WHERE project_id = ? AND assignment_id = ?
             AND authority_state = 'contained'
             AND containment_proof_ref IS NOT NULL LIMIT 1`,
        )
        .get(this.#projectId, assignmentId) !== undefined
    );
  }

  hasUncontainedAssignmentAuthority(): boolean {
    this.#assertOpen();
    return (
      this.#database
        .prepare(
          `SELECT 1 FROM assignments
           WHERE project_id = ? AND authority_state <> 'contained' LIMIT 1`,
        )
        .get(this.#projectId) !== undefined
    );
  }

  listRuntimeSessions(): readonly RuntimeSessionSummary[] {
    this.#assertOpen();
    return this.#database
      .prepare(
        `SELECT rs.session_id, rs.seat_id, rs.assignment_id, rs.state,
          identity.container_id, identity.endpoint
         FROM runtime_sessions rs
         LEFT JOIN runtime_identities identity ON identity.project_id = rs.project_id
           AND identity.session_id = rs.session_id
           AND identity.observation_id = (
             SELECT latest.observation_id FROM runtime_identities latest
             WHERE latest.project_id = rs.project_id AND latest.session_id = rs.session_id
             ORDER BY latest.observed_at DESC, latest.observation_id DESC LIMIT 1
           )
         WHERE rs.project_id = ?
         ORDER BY rs.started_at, rs.session_id`,
      )
      .all(this.#projectId)
      .map((row) => {
        const session = row as {
          session_id: string;
          seat_id: string;
          assignment_id: string | null;
          state: RuntimeState;
          container_id: string | null;
          endpoint: string | null;
        };
        return {
          sessionId: session.session_id,
          seatId: session.seat_id,
          assignmentId: session.assignment_id,
          state: session.state,
          ...(session.container_id === null
            ? {}
            : { containerId: session.container_id }),
          ...(session.endpoint === null ? {} : { endpoint: session.endpoint }),
        };
      });
  }

  statusSnapshot(): ControllerStatus {
    this.#assertOpen();
    const run = this.#database
      .prepare(
        "SELECT state, state_version FROM run_controls WHERE project_id = ?",
      )
      .get(this.#projectId) as { state: string; state_version: number };
    const supervision = this.#database
      .prepare(
        `SELECT enabled, health, target_epoch, checkpoint_epoch,
          checkpoint_assignment_id, replacement_attempts
         FROM supervision_control WHERE project_id = ?`,
      )
      .get(this.#projectId) as
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
    const roles = this.#database
      .prepare(
        `
      SELECT s.role, s.seat_id, s.state AS seat_state,
        EXISTS(SELECT 1 FROM actors a WHERE a.project_id = s.project_id
          AND a.seat_id = s.seat_id AND a.active = 1 AND a.revoked_at IS NULL) AS actor_active,
        (SELECT rs.state FROM runtime_sessions rs WHERE rs.project_id = s.project_id
          AND rs.seat_id = s.seat_id ORDER BY rs.started_at DESC LIMIT 1) AS session_state,
        (SELECT a.assignment_id FROM assignments a WHERE a.project_id = s.project_id
          AND a.seat_id = s.seat_id AND a.authority_state IN ('active', 'unknown')
          ORDER BY a.created_at DESC LIMIT 1) AS assignment_id
      FROM seats s WHERE s.project_id = ? ORDER BY s.role, s.seat_id
    `,
      )
      .all(this.#projectId) as Array<{
      role: string;
      seat_id: string;
      seat_state: string;
      actor_active: number;
      session_state: string | null;
      assignment_id: string | null;
    }>;
    const work = this.#database
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
      .all(this.#projectId) as Array<{
      work_item_id: string;
      title: string;
      required_role: string;
      state: string;
      owner: string | null;
      blockers: string;
    }>;
    const findings = this.#database
      .prepare(
        `
      SELECT finding_id, affected_work_item_id, affected_seat_id, affected_assignment_id,
        affected_generation, fingerprint, severity, state, evidence_json, requested_correction,
        acknowledgement_deadline, resolution_condition, escalation_route, intervention_count,
        reopened_from_finding_id
      FROM findings WHERE project_id = ? ORDER BY created_at, finding_id
    `,
      )
      .all(this.#projectId) as Array<{
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
    const evidence = this.#database
      .prepare(
        `
      SELECT c.candidate_id, c.commit_sha, c.report_hash, c.evidence_json,
        (SELECT e.artifact_ref FROM candidate_evidence e WHERE e.project_id = c.project_id
          AND e.candidate_id = c.candidate_id ORDER BY e.created_at DESC LIMIT 1) AS evidence_ref
      FROM candidates c WHERE c.project_id = ? ORDER BY c.created_at, c.candidate_id
    `,
      )
      .all(this.#projectId) as Array<{
      candidate_id: string;
      commit_sha: string;
      report_hash: string;
      evidence_json: string | null;
      evidence_ref: string | null;
    }>;
    const verifierEvidenceRows = this.#database
      .prepare(
        `SELECT evidence_id, candidate_id, criterion, passed, artifact_ref,
          observation, exit_status, evidence_hash
         FROM candidate_evidence WHERE project_id = ?
         ORDER BY candidate_id, criterion, evidence_id`,
      )
      .all(this.#projectId) as Array<{
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
    const finalVerificationRows = this.#database
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
      .all(this.#projectId) as Array<{
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
      projectId: this.#projectId,
      run: { state: run.state, stateVersion: run.state_version },
      stateVersion: this.stateVersion,
      inputRevision: this.inputRevision,
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
        sessionState: row.session_state,
        assignmentId: row.assignment_id,
      })),
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
    this.#assertOpen();
    for (const [kind, table, column] of [
      ["work_item", "work_items", "work_item_id"],
      ["assignment", "assignments", "assignment_id"],
      ["candidate", "candidates", "candidate_id"],
      ["finding", "findings", "finding_id"],
      ["recovery", "recovery_attempts", "recovery_id"],
    ] as const) {
      const row = this.#database
        .prepare(
          `SELECT * FROM ${table} WHERE project_id = ? AND ${column} = ?`,
        )
        .get(this.#projectId, id) as Record<string, unknown> | undefined;
      if (!row) continue;
      return { kind, id, record: row };
    }
    throw new ControllerError("inspect target does not exist");
  }

  latestAcceptedWorkEvent(
    workItemId: string,
  ): { readonly eventId: string; readonly sequence: number } | undefined {
    this.#assertOpen();
    const row = this.#database
      .prepare(
        `SELECT event_id, sequence FROM controller_events
         WHERE project_id = ? AND entity_type = 'work_item' AND entity_id = ?
           AND to_state = 'accepted'
         ORDER BY sequence DESC LIMIT 1`,
      )
      .get(this.#projectId, workItemId) as
      { event_id: string; sequence: number } | undefined;
    return row ? { eventId: row.event_id, sequence: row.sequence } : undefined;
  }

  latestCompletedReport(
    workItemId: string,
    assignmentId: string,
  ): CompletedWorkReport | undefined {
    this.#assertOpen();
    const row = this.#database
      .prepare(
        `
        SELECT a.assignment_id, c.command_id, s.role, a.active_generation,
          a.input_revision, a.authority_state, w.state AS work_state,
          r.receipt_hash, r.receipt_json
        FROM assignments a
        JOIN assignment_attempts aa ON aa.project_id = a.project_id
          AND aa.assignment_id = a.assignment_id AND aa.generation = a.active_generation
        JOIN commands c ON c.project_id = aa.project_id AND c.assignment_id = aa.assignment_id
          AND c.attempt = aa.attempt AND c.generation = aa.generation
        JOIN command_receipts r ON r.project_id = c.project_id
          AND r.command_id = c.command_id AND r.receipt_type = 'completed'
        JOIN work_items w ON w.project_id = a.project_id AND w.work_item_id = a.work_item_id
        JOIN seats s ON s.project_id = a.project_id AND s.seat_id = a.seat_id
        WHERE a.project_id = ? AND a.work_item_id = ? AND a.assignment_id = ? AND c.state = 'completed'
        ORDER BY a.active_generation DESC, r.received_at DESC LIMIT 1
      `,
      )
      .get(this.#projectId, workItemId, assignmentId) as
      | {
          assignment_id: string;
          command_id: string;
          role: string;
          active_generation: number;
          input_revision: number;
          authority_state: string;
          work_state: string;
          receipt_hash: string;
          receipt_json: string;
        }
      | undefined;
    if (!row) return undefined;
    const receipt = JSON.parse(row.receipt_json) as Record<string, unknown>;
    if (typeof receipt.reply !== "string") return undefined;
    return {
      assignmentId: row.assignment_id,
      commandId: row.command_id,
      role: row.role,
      generation: row.active_generation,
      inputRevision: row.input_revision,
      authorityState: row.authority_state,
      workState: row.work_state,
      receiptHash: row.receipt_hash,
      reply: receipt.reply,
      receipt,
    };
  }

  reconcile(): { readonly reconciled: number } {
    this.#assertOpen();
    this.#assertWritable();
    return { reconciled: this.#reconcileUncertainAssignments() };
  }

  revokeActiveAssignments(
    context: MutationContext,
    reason: string,
  ): { readonly assignmentIds: readonly string[] } {
    return this.#mutateAsController(
      context,
      "assignment.revoke.active",
      "recovery:write",
      { reason },
      (actor) => {
        if (actor.role !== "controller" || reason.trim().length === 0)
          throw new TransitionAuthorizationError(
            "revocation requires a controller and non-empty reason",
          );
        const rows = this.#database
          .prepare(
            `SELECT a.assignment_id, a.active_generation, a.work_item_id,
              a.state AS assignment_state, a.authority_state,
              at.attempt, at.state AS attempt_state,
              c.command_id, c.state AS command_state,
              w.state AS work_state, w.state_version AS work_version
             FROM assignments a JOIN assignment_attempts at
               ON at.project_id = a.project_id AND at.assignment_id = a.assignment_id
                 AND at.generation = a.active_generation
             JOIN commands c ON c.project_id = a.project_id AND c.assignment_id = a.assignment_id
               AND c.generation = a.active_generation
             JOIN work_items w ON w.project_id = a.project_id AND w.work_item_id = a.work_item_id
             WHERE a.project_id = ? AND a.authority_state IN ('active', 'unknown')
             ORDER BY a.assignment_id`,
          )
          .all(this.#projectId) as Array<{
          assignment_id: string;
          active_generation: number;
          work_item_id: string;
          assignment_state: string;
          authority_state: string;
          attempt: number;
          attempt_state: string;
          command_id: string;
          command_state: string;
          work_state: string;
          work_version: number;
        }>;
        const now = new Date().toISOString();
        const revokedAssignmentIds: string[] = [];
        for (const row of rows) {
          const preserveReportedCompletion =
            row.command_state === "completed" &&
            row.assignment_state === "reported" &&
            row.attempt_state === "reported";
          if (
            row.work_state === "running" ||
            row.work_state === "awaiting_verification"
          )
            this.#database
              .prepare(
                `UPDATE work_items SET state = 'blocked', state_version = state_version + 1
                 WHERE project_id = ? AND work_item_id = ?`,
              )
              .run(this.#projectId, row.work_item_id);
          if (
            row.authority_state === "unknown" &&
            (row.assignment_state === "revoked" || preserveReportedCompletion)
          )
            continue;
          revokedAssignmentIds.push(row.assignment_id);
          if (preserveReportedCompletion) {
            this.#database
              .prepare(
                `UPDATE assignments SET authority_state = 'unknown',
                  state_version = state_version + 1
                 WHERE project_id = ? AND assignment_id = ?`,
              )
              .run(this.#projectId, row.assignment_id);
            this.#database
              .prepare(
                `UPDATE assignment_attempts SET authority_state = 'unknown',
                  state_version = state_version + 1
                 WHERE project_id = ? AND assignment_id = ? AND generation = ?`,
              )
              .run(this.#projectId, row.assignment_id, row.active_generation);
          } else {
            this.#database
              .prepare(
                `UPDATE assignments SET state = 'revoked', authority_state = 'unknown',
                  state_version = state_version + 1, ended_at = COALESCE(ended_at, ?)
                 WHERE project_id = ? AND assignment_id = ?`,
              )
              .run(now, this.#projectId, row.assignment_id);
            this.#database
              .prepare(
                `UPDATE assignment_attempts SET state = 'revoked', authority_state = 'unknown',
                  state_version = state_version + 1, ended_at = COALESCE(ended_at, ?)
                 WHERE project_id = ? AND assignment_id = ? AND generation = ?`,
              )
              .run(
                now,
                this.#projectId,
                row.assignment_id,
                row.active_generation,
              );
          }
          if (row.command_state === "queued") {
            this.#database
              .prepare(
                `UPDATE commands SET state = 'unknown', state_version = state_version + 1,
                  updated_at = ? WHERE project_id = ? AND command_id = ?`,
              )
              .run(now, this.#projectId, row.command_id);
            this.#appendOutboxOutcome(
              row.command_id,
              "unknown",
              canonicalJson({ reason }),
              now,
            );
          }
        }
        const runControl = this.#database
          .prepare("SELECT state FROM run_controls WHERE project_id = ?")
          .get(this.#projectId) as { state: string } | undefined;
        return {
          value: { assignmentIds: revokedAssignmentIds },
          event: {
            entityType: "run_control",
            entityId: this.#projectId,
            stateVersion: 0,
            fromState: runControl?.state ?? "active",
            toState: "revoked",
            details: {
              reason,
              assignmentIds: revokedAssignmentIds,
            },
          },
        };
      },
    );
  }

  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    try {
      this.#database.close();
    } finally {
      this.#lock?.close();
    }
  }

  #reconcileUncertainAssignments(): number {
    this.#database.exec("BEGIN IMMEDIATE");
    let reconciled = 0;
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
        WHERE c.project_id = ? AND c.state IN ('queued', 'attempting', 'acknowledged', 'started', 'completed')
          AND a.authority_state = 'active'
          AND at.state IN ('created', 'dispatched', 'acknowledged', 'running', 'reported')
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
        reconciled++;
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
      const sessions = this.#database
        .prepare(
          `SELECT session_id, state, state_version FROM runtime_sessions
           WHERE project_id = ? AND state IN ('starting', 'ready', 'working', 'stopping')
           ORDER BY session_id`,
        )
        .all(this.#projectId) as Array<{
        session_id: string;
        state: string;
        state_version: number;
      }>;
      for (const session of sessions) {
        if (
          !this.#isTransitionAllowed(
            "runtime_session",
            session.state,
            "unknown",
            controller,
          )
        )
          throw new TransitionAuthorizationError(
            `transition table rejects restart reconciliation of ${session.state} runtime session`,
          );
        const now = new Date().toISOString();
        this.#database
          .prepare(
            `UPDATE runtime_sessions SET state = 'unknown', state_version = state_version + 1
             WHERE project_id = ? AND session_id = ?`,
          )
          .run(this.#projectId, session.session_id);
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
            `INSERT INTO controller_events(project_id, sequence, event_id, entity_type, entity_id,
              from_state, to_state, state_version, actor_id, request_id, input_revision, payload_json, created_at)
             SELECT ?, ?, ?, 'runtime_session', ?, ?, 'unknown', ?, ?, ?, current_input_revision, ?, ?
             FROM projects WHERE project_id = ?`,
          )
          .run(
            this.#projectId,
            sequence,
            randomUUID(),
            session.session_id,
            session.state,
            session.state_version + 1,
            this.#internalActorId,
            `restart-reconcile:session:${session.session_id}`,
            canonicalJson({
              previousState: session.state,
              identityPreserved: true,
            }),
            now,
            this.#projectId,
          );
        reconciled++;
      }
      this.#database.exec("COMMIT");
      return reconciled;
    } catch (error) {
      this.#database.exec("ROLLBACK");
      throw error;
    }
  }

  #assertOpen(): void {
    if (this.#closed) throw new ControllerError("controller is closed");
    this.#lock?.assertHeld();
  }
  #assertWritable(): void {
    if (this.#readOnly)
      throw new ControllerError("controller is open read-only");
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

  #acceptanceCriteria(workItemId?: string): readonly string[] {
    if (workItemId) {
      const workItem = this.#database
        .prepare(
          "SELECT acceptance_criteria_json FROM work_items WHERE project_id = ? AND work_item_id = ?",
        )
        .get(this.#projectId, workItemId) as
        { acceptance_criteria_json: string | null } | undefined;
      if (!workItem)
        throw new CandidateBindingError("candidate work item is missing");
      if (workItem.acceptance_criteria_json)
        return acceptanceCriteriaFromContent(
          JSON.parse(workItem.acceptance_criteria_json),
        );
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
    return acceptanceCriteriaFromContent(JSON.parse(row.content_json));
  }

  #advanceSupervisionEpoch(
    action: string,
    payload: unknown,
    actor: AuthenticatedActor,
  ): void {
    const input =
      payload !== null && typeof payload === "object" && !Array.isArray(payload)
        ? (payload as Record<string, unknown>)
        : {};
    const workRole = (workItemId: unknown): string | undefined => {
      if (typeof workItemId !== "string") return undefined;
      return (
        this.#database
          .prepare(
            "SELECT required_role FROM work_items WHERE project_id = ? AND work_item_id = ?",
          )
          .get(this.#projectId, workItemId) as
          { required_role: string } | undefined
      )?.required_role;
    };
    const isCorrectionWork = (workItemId: unknown): boolean =>
      typeof workItemId === "string" &&
      !!this.#database
        .prepare(
          "SELECT 1 FROM finding_correction_work WHERE project_id = ? AND work_item_id = ?",
        )
        .get(this.#projectId, workItemId);
    let material = false;
    if (action === "dependency.add" && isCorrectionWork(input.workItemId))
      material = false;
    else if (
      action === "project.input.revise" ||
      action === "dependency.add" ||
      action === "dependency.remove" ||
      action === "candidate.submit" ||
      action === "candidate.evidence.record" ||
      action === "candidate.accept" ||
      action === "final_verification.accept"
    )
      material = true;
    else if (
      action === "work.create" &&
      input.requiredRole !== "Supervisor" &&
      !input.findingId
    )
      material = true;
    else if (action === "work.inputs.rebind" || action === "work.report.accept")
      material = workRole(input.workItemId) !== "Supervisor";
    else if (action === "assignment.containment.confirmed") {
      const row = this.#database
        .prepare(
          `SELECT s.role FROM assignments a JOIN seats s
             ON s.project_id = a.project_id AND s.seat_id = a.seat_id
           WHERE a.project_id = ? AND a.assignment_id = ?`,
        )
        .get(this.#projectId, input.assignmentId) as
        { role: string } | undefined;
      material = !!row && row.role !== "Supervisor";
    } else if (action === "finding.transition") {
      material =
        actor.role !== "Supervisor" &&
        input.toState !== "correcting" &&
        [
          "acknowledged",
          "correcting",
          "disputed",
          "resolved",
          "escalated",
        ].includes(String(input.toState));
    }
    if (!material) return;
    this.#database
      .prepare(
        `UPDATE supervision_control SET target_epoch = target_epoch + 1, updated_at = ?
         WHERE project_id = ? AND enabled = 1`,
      )
      .run(new Date().toISOString(), this.#projectId);
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

  #assertSupervisionReady(workItemId: string, allowBootstrapPm = false): void {
    const control = this.#database
      .prepare(
        "SELECT enabled, health, target_epoch, checkpoint_epoch, bootstrap_pm_allowed FROM supervision_control WHERE project_id = ?",
      )
      .get(this.#projectId) as
      | {
          enabled: number;
          health: string;
          target_epoch: number;
          checkpoint_epoch: number | null;
          bootstrap_pm_allowed: number;
        }
      | undefined;
    if (!control?.enabled) return;
    const item = this.#database
      .prepare(
        "SELECT required_role FROM work_items WHERE project_id = ? AND work_item_id = ?",
      )
      .get(this.#projectId, workItemId) as
      { required_role: string } | undefined;
    if (!item) throw new ControllerError("work item does not exist");
    if (item.required_role === "Supervisor") return;
    const bootstrap =
      allowBootstrapPm &&
      item.required_role === "PM" &&
      control.checkpoint_epoch === null &&
      control.bootstrap_pm_allowed === 1 &&
      workItemId ===
        (
          this.#database
            .prepare(
              `SELECT a.work_item_id FROM assignments a
               JOIN work_items w ON w.project_id = a.project_id AND w.work_item_id = a.work_item_id
               WHERE a.project_id = ? AND w.required_role = 'PM'
               ORDER BY a.created_at, a.work_item_id LIMIT 1`,
            )
            .get(this.#projectId) as { work_item_id: string } | undefined
        )?.work_item_id &&
      !this.#database
        .prepare(
          "SELECT 1 FROM work_items WHERE project_id = ? AND required_role = 'PM' AND state = 'accepted' LIMIT 1",
        )
        .get(this.#projectId);
    if (bootstrap) return;
    if (
      control.health !== "healthy" ||
      control.checkpoint_epoch !== control.target_epoch
    )
      throw new MutationConflictError(
        "supervision is degraded or its checkpoint is stale",
      );
    const unresolved = this.#database
      .prepare(
        "SELECT 1 FROM findings WHERE project_id = ? AND state <> 'resolved' LIMIT 1",
      )
      .get(this.#projectId);
    if (!unresolved) return;
    const correction = this.#database
      .prepare(
        `SELECT f.state, f.intervention_count, c.target_assignment_id, c.target_role, c.target_generation,
          a.active_generation, a.authority_state, a.seat_id, 0 AS verifier_parent
         FROM finding_correction_work c
         JOIN findings f ON f.project_id = c.project_id AND f.finding_id = c.finding_id
         JOIN assignments a ON a.project_id = c.project_id AND a.assignment_id = c.target_assignment_id
         WHERE c.project_id = ? AND c.work_item_id = ?
         UNION ALL
         SELECT f.state, f.intervention_count, c.target_assignment_id, c.target_role, c.target_generation,
          a.active_generation, a.authority_state, a.seat_id, 1 AS verifier_parent
         FROM finding_correction_work c
         JOIN findings f ON f.project_id = c.project_id AND f.finding_id = c.finding_id
         JOIN assignments a ON a.project_id = c.project_id AND a.assignment_id = c.target_assignment_id
         JOIN work_items verifier ON verifier.project_id = c.project_id
           AND verifier.work_item_id = c.work_item_id AND verifier.parent_work_item_id = ?
         JOIN assignments verifier_assignment ON verifier_assignment.project_id = c.project_id
           AND verifier_assignment.work_item_id = verifier.work_item_id
         JOIN assignment_input_bindings binding ON binding.project_id = verifier_assignment.project_id
           AND binding.assignment_id = verifier_assignment.assignment_id
           AND binding.input_kind = 'candidate'
         JOIN work_items candidate_parent ON candidate_parent.project_id = c.project_id
           AND candidate_parent.work_item_id = verifier.parent_work_item_id
           AND candidate_parent.required_role = 'Developer'
           AND candidate_parent.state = 'awaiting_verification'
         JOIN candidates candidate ON candidate.project_id = binding.project_id
           AND candidate.candidate_id = binding.source_id
           AND candidate.input_revision = candidate_parent.input_revision
           AND candidate.generation = (
             SELECT MAX(latest_attempt.generation)
             FROM assignments latest
             JOIN assignment_attempts latest_attempt
               ON latest_attempt.project_id = latest.project_id
               AND latest_attempt.assignment_id = latest.assignment_id
             JOIN seats latest_seat ON latest_seat.project_id = latest.project_id
               AND latest_seat.seat_id = latest.seat_id
             WHERE latest.project_id = candidate_parent.project_id
               AND latest.work_item_id = candidate_parent.work_item_id
               AND latest_seat.role = 'Developer'
           )
           AND candidate.assignment_id IN (
             SELECT developer.assignment_id FROM assignments developer
             JOIN seats developer_seat ON developer_seat.project_id = developer.project_id
               AND developer_seat.seat_id = developer.seat_id
             WHERE developer.project_id = c.project_id
               AND developer.work_item_id = verifier.parent_work_item_id
               AND developer_seat.role = 'Developer'
           )
         WHERE c.project_id = ? AND c.target_role = 'Verifier'
           AND f.state = 'correcting'
         UNION ALL
         SELECT f.state, f.intervention_count, c.target_assignment_id, c.target_role, c.target_generation,
          a.active_generation, a.authority_state, a.seat_id, 2 AS verifier_parent
         FROM work_items child
         JOIN finding_correction_work c ON c.project_id = child.project_id
           AND c.work_item_id = child.parent_work_item_id
         JOIN findings f ON f.project_id = c.project_id AND f.finding_id = c.finding_id
         JOIN assignments a ON a.project_id = c.project_id AND a.assignment_id = c.target_assignment_id
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
        this.#projectId,
        workItemId,
        workItemId,
        this.#projectId,
        this.#projectId,
        workItemId,
      ) as
      | {
          state: string;
          intervention_count: number;
          target_assignment_id: string;
          target_role: string;
          target_generation: number;
          active_generation: number;
          authority_state: string;
          seat_id: string;
          verifier_parent: number;
        }
      | undefined;
    const occupied = correction
      ? this.#database
          .prepare(
            "SELECT 1 FROM assignments WHERE project_id = ? AND seat_id = ? AND assignment_id <> ? AND work_item_id <> ? AND authority_state IN ('active', 'unknown') LIMIT 1",
          )
          .get(
            this.#projectId,
            correction.seat_id,
            correction.target_assignment_id,
            workItemId,
          )
      : true;
    if (
      !correction ||
      (correction.verifier_parent === 1
        ? item.required_role !== "Developer" ||
          correction.target_role !== "Verifier" ||
          correction.state !== "correcting"
        : correction.verifier_parent === 2
          ? item.required_role !== "Verifier" ||
            correction.target_role !== "Developer" ||
            correction.state !== "correcting"
          : correction.target_role !== item.required_role) ||
      correction.target_generation !== correction.active_generation ||
      correction.authority_state !== "contained" ||
      correction.intervention_count > 2 ||
      !["reported", "acknowledged", "correcting"].includes(correction.state) ||
      occupied
    )
      throw new MutationConflictError(
        "an unresolved Supervisor finding blocks this acceptance",
      );
  }

  #assertSupervisionRunReady(): void {
    const control = this.#database
      .prepare(
        "SELECT enabled, health, target_epoch, checkpoint_epoch FROM supervision_control WHERE project_id = ?",
      )
      .get(this.#projectId) as
      | {
          enabled: number;
          health: string;
          target_epoch: number;
          checkpoint_epoch: number | null;
        }
      | undefined;
    if (
      control?.enabled &&
      (control.health !== "healthy" ||
        control.checkpoint_epoch !== control.target_epoch)
    )
      throw new MutationConflictError(
        "run cannot complete without a current healthy Supervisor checkpoint",
      );
  }

  #mutate<T>(
    context: MutationContext,
    action: string,
    capability: Capability,
    payload: unknown,
    apply: (actor: AuthenticatedActor) => MutationOutput<T>,
  ): T {
    this.#assertOpen();
    this.#assertWritable();
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
        const value = (
          action === "actor.create"
            ? decryptActorResult(
                existing.result_json,
                context.credential,
                this.#projectId,
                requestHash,
              )
            : JSON.parse(existing.result_json)
        ) as T;
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
      if (
        action === "final_verification.accept" ||
        action === "work.report.accept"
      ) {
        const input = payload as { workItemId?: string };
        const workItemId = input.workItemId ?? "";
        const item = this.#database
          .prepare(
            `SELECT w.required_role,
              EXISTS (
                SELECT 1 FROM finding_correction_work c
                WHERE c.project_id = w.project_id AND c.work_item_id = w.work_item_id
              ) AS is_correction
             FROM work_items w WHERE w.project_id = ? AND w.work_item_id = ?`,
          )
          .get(this.#projectId, workItemId) as
          { required_role: string; is_correction: number } | undefined;
        const isPmCorrectionReport =
          action === "work.report.accept" &&
          item?.required_role === "PM" &&
          item.is_correction === 1;
        if (item?.required_role !== "Supervisor" && !isPmCorrectionReport)
          this.#assertSupervisionReady(
            workItemId,
            action === "work.report.accept" && item?.required_role === "PM",
          );
      }
      if (
        action === "run.transition" &&
        payload !== null &&
        typeof payload === "object" &&
        "toState" in payload &&
        payload.toState === "completed"
      )
        this.#assertSupervisionRunReady();
      const output = apply(actor);
      this.#advanceSupervisionEpoch(action, payload, actor);
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
          action === "actor.create"
            ? encryptActorResult(
                output.value,
                context.credential,
                this.#projectId,
                requestHash,
              )
            : canonicalJson(output.value),
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
