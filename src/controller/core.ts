/* eslint-disable max-lines -- temporary: plan-17 lint-guard */
import { randomBytes, randomUUID } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import type { Database } from "./sqlite.js";
import { createAreas, type ControllerAreas } from "./areas.js";
import { ControllerKernel } from "./kernel.js";
import {
  authenticateActor,
  credentialHash,
  type AuthenticatedActor,
} from "./auth.js";
import { canonicalJson, sha256 } from "./canonical.js";
import {
  openDatabase,
  openDatabaseReadOnly,
  resolveDatabasePath,
} from "./database.js";
import {
  type AttentionEpisode,
  type DeferralReason,
  type HerdrState,
  type MessagingTimers,
  type ResolutionDecision,
} from "./messaging.js";
import { ControllerOwnershipError, ProjectLock } from "./ownership.js";
import {
  FULL_AUTO_RULE,
  HASH_PREFIX_CHARS,
  MAX_OPERATOR_COMMAND_BYTES,
  MAX_OPERATOR_REASON_BYTES,
  commandHash,
  normalizeCommand,
  normalizeReason,
  prefixGrantProblem,
  sessionRuleGrantId,
  words,
} from "../operator-policy.js";
import {
  decisionNotice,
  endedWithoutRunNotice,
  expiredNotice,
  fullAutoNotice,
  grantEndedNotice,
  proposalNoticeToPm,
  runResultNotice,
  type FullAutoChange,
} from "../operator.js";
import { packageNaming, packageOfBody, workPackageMessage } from "../plans.js";
import {
  EXTERNAL_REF_KINDS,
  NEXORA_ID_PATTERN,
  isNexoraState,
  wantedPackageState,
  wantedPlanState,
  wantedRequirementState,
  type ExternalRefKind,
  type NexoraState,
  type PackageFacts,
} from "../nexora.js";
import type {
  AgentInput,
  AgentRecord,
  Capability,
  ControllerOptions,
  Identity,
  InitialProject,
  InputKind,
  MessageInput,
  InputBlocker,
  MessageRecord,
  MessageRejectionRecord,
  MessagingAdvance,
  MutationContext,
  OperatorGrantEndReason,
  OperatorGrantKind,
  OperatorGrantRecord,
  OperatorProposalKind,
  OperatorProposalRecord,
  OperatorProposalState,
  PauseRecord,
  PromptRelayRecord,
  PromptRelayState,
  ReplacedAgent,
  RunState,
  RoleDefinition,
  RoleDefinitionInput,
  RoleSyncResult,
  SeatInput,
} from "./types.js";
import { OPERATOR_PROPOSAL_STATES } from "./types.js";
import {
  type CapturedPrompt,
  type PromptAnswer,
} from "../herdr/prompt-relay.js";
import {
  ControllerError,
  RunPausedError,
  TransitionAuthorizationError,
} from "./errors.js";
import {
  MAX_MESSAGE_BYTES,
  RESTART_COMMAND_TEXT,
  MAX_OPERATOR_TAIL_BYTES,
  UNSAFE_TEXT,
  type AgentPaneInput,
  type AgentPaneRecord,
  MAX_REPORT_SUMMARY_BYTES,
  MAX_REJECTED_REPORTS,
  type ReportReason,
  type ReportEvidence,
  type AgentReportRecord,
  type ReportResult,
  type AgentReportRow,
  MAX_REVIEW_TEXT_BYTES,
  MAX_REVIEW_ROUNDS,
  type ReviewRecord,
  type ReviewRow,
  type PlanTier,
  type PackageProgress,
  type PlanRecord,
  type PlanPackageRecord,
  type ExternalLinkRecord,
  type ExternalLinkRow,
  type PlanCancelResult,
  type PlanNoticeRef,
  type PlanSignoffRecord,
  type PlanStatusEntry,
  type PlanDetail,
  type PlanRow,
  MAX_INTEGRATION_REPORTS,
  MAX_CONFLICT_FILES,
  MAX_CONFLICT_PATH_CHARS,
  PATH_CUT_MARK_CHARS,
  type CoverageCandidate,
  type IntegrationRecord,
  type IntegrationRow,
  type IntegrationOutcome,
  type AgentFindingRecord,
  type AgentFindingNoticeRecord,
  type AgentSeedData,
  type PmRestartSummary,
  type AgentRow,
  type OperatorProposalRow,
  type OperatorRunRow,
  type OperatorGrantRow,
  type OperatorClaimRefusal,
  type OperatorClaim,
  type MutationEvent,
  type MutationOutput,
  type ActorInput,
  type ReadinessResult,
  type SupervisionActivity,
  type ControllerStatus,
} from "./records.js";
import {
  safeId,
  safeText,
  reportRecord,
  reportNotice,
  reviewRecord,
  reviewTask,
  reviewNotice,
  planNeedsAttentionNotice,
  planSignedOffNotice,
  planApprovedNotice,
  planRecordOf,
  planBodyPackageIds,
  acceptanceCriteriaFromContent,
} from "./helpers.js";

export {
  ControllerError,
  MutationConflictError,
  RunPausedError,
  IdempotencyConflictError,
  StateVersionConflictError,
  InputRevisionConflictError,
  TransitionAuthorizationError,
  MessageTransitionError,
  CandidateBindingError,
} from "./errors.js";
export {
  MAX_MESSAGE_BYTES,
  RESTART_COMMAND_TEXT,
  MAX_INPUT_CLEAR_BYTES,
  type AgentPaneInput,
  type AgentPaneRecord,
  MAX_REPORT_SUMMARY_BYTES,
  MAX_REJECTED_REPORTS,
  type ReportReason,
  type ReportEvidence,
  type AgentReportRecord,
  type ReportResult,
  MAX_REVIEW_TEXT_BYTES,
  MAX_REVIEW_ROUNDS,
  type ReviewState,
  type ReviewRecord,
  MAX_PLAN_BODY_BYTES,
  MAX_PLAN_PACKAGES,
  type PlanTier,
  type PlanState,
  type PackageProgress,
  type PlanRecord,
  type PlanRevisionRecord,
  type PlanPackageRecord,
  type ExternalLinkRecord,
  type PlanCancelResult,
  type PlanNoticeRef,
  type PlanSignoffRecord,
  type PlanStatusEntry,
  type PlanDetail,
  MAX_INTEGRATION_REPORTS,
  MAX_CONFLICT_FILES,
  MAX_CONFLICT_PATH_CHARS,
  PATH_CUT_MARK_CHARS,
  type CoverageCandidate,
  type IntegrationState,
  type IntegrationRecord,
  type IntegrationOutcome,
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
  LOST_NOTICE_IDS,
  SEED_MESSAGES,
  SEED_REPORTS,
  SEED_FINDINGS,
  type AgentSeedData,
  type PmRestartSummary,
  PROMPT_RELAY_TEXT_MAX_CHARS,
  type OperatorClaimRefusal,
  type OperatorClaim,
  type ActorInput,
  type ReadinessResult,
  type SupervisionActivity,
  type ControllerStatus,
} from "./records.js";
export { oneLineText } from "./helpers.js";

export class ControllerCore {
  readonly #kernel: ControllerKernel;

  readonly #areas: ControllerAreas;

  private constructor(kernel: ControllerKernel) {
    this.#kernel = kernel;
    this.#areas = createAreas(kernel, {
      statusSnapshot: () => this.statusSnapshot(),
      linkRecord: (row) => this.#linkRecord(row),
      linkRow: (refKind, refId) => this.#linkRow(refKind, refId),
      planRow: (planId) => this.#planRow(planId),
      openPlansForSummary: () => this.#openPlansForSummary(),
      mergedIntegrationsForSummary: () => this.#mergedIntegrationsForSummary(),
      cancelUnstartedOperatorProposalsOf: (agentId, now) =>
        this.#cancelUnstartedOperatorProposalsOf(agentId, now),
      endOperatorGrantsOf: (agentId) => this.#endOperatorGrantsOf(agentId),
    });
  }

  get #database(): Database {
    return this.#kernel.database;
  }

  get #projectId(): string {
    return this.#kernel.projectId;
  }

  get #internalActorId(): string {
    return this.#kernel.internalActorId;
  }

  get #closed(): boolean {
    return this.#kernel.closed;
  }

  set #closed(value: boolean) {
    this.#kernel.closed = value;
  }

  get #lock(): ProjectLock | undefined {
    return this.#kernel.lock;
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
    let database: Database | undefined;
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
      database = await openDatabase(
        databasePath,
        options.keepMigrationBackups === undefined
          ? {}
          : { keepMigrationBackups: options.keepMigrationBackups },
      );
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
      const core = new ControllerCore(
        new ControllerKernel(
          database,
          lock,
          project.projectId,
          internalActor.actor_id,
          false,
          options.clock ?? (() => new Date()),
        ),
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
    let database: Database | undefined;
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
      return new ControllerCore(
        new ControllerKernel(
          database,
          undefined,
          project.projectId,
          internalActor.actor_id,
          true,
          options.clock ?? (() => new Date()),
        ),
      );
    } catch (error) {
      database?.close();
      throw error;
    }
  }

  static #initializeProject(database: Database, project: InitialProject): void {
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
    return this.#kernel.stateVersion;
  }

  get inputRevision(): number {
    return this.#kernel.inputRevision;
  }

  createActor(
    context: MutationContext,
    input: ActorInput,
  ): { readonly actorId: string; readonly credential: string } {
    return this.#areas.actors.createActor(context, input);
  }

  revokeActor(
    context: MutationContext,
    actorId: string,
  ): { readonly revoked: true } {
    return this.#areas.actors.revokeActor(context, actorId);
  }

  createSeat(
    context: MutationContext,
    input: SeatInput,
  ): { readonly seatId: string } {
    return this.#areas.actors.createSeat(context, input);
  }

  roleDefinitions(): readonly RoleDefinition[] {
    return this.#areas.actors.roleDefinitions();
  }

  roleKind(roleName: string): RoleDefinition["kind"] {
    return this.#areas.actors.roleKind(roleName);
  }

  syncRoleDefinitions(
    context: MutationContext,
    desired: readonly RoleDefinitionInput[],
  ): RoleSyncResult {
    return this.#areas.actors.syncRoleDefinitions(context, desired);
  }

  registerAgent(context: MutationContext, input: AgentInput): AgentRecord {
    return this.#areas.agents.registerAgent(context, input);
  }

  identify(credential: string): Identity {
    return this.#areas.agents.identify(credential);
  }

  listAgents(): readonly AgentRecord[] {
    return this.#areas.agents.listAgents();
  }

  agentRecord(agentId: string): AgentRecord | undefined {
    return this.#areas.agents.agentRecord(agentId);
  }

  recordAgentObservation(
    context: MutationContext,
    agentId: string,
    state: HerdrState,
  ): { readonly recorded: boolean } {
    return this.#areas.agents.recordAgentObservation(context, agentId, state);
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
    return this.#areas.agents.endAgent(context, agentId, options);
  }

  replaceAgentGeneration(
    context: MutationContext,
    agentId: string,
  ): ReplacedAgent {
    return this.#areas.agents.replaceAgentGeneration(context, agentId);
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
    return this.#areas.agents.restartAgentGeneration(context, agentId);
  }

  /** Marks every restart summary of an agent up to a sequence as used by a started PM. */
  markPmRestartsConsumed(
    context: MutationContext,
    agentId: string,
    upToSequence: number,
  ): { readonly marked: number } {
    return this.#areas.agents.markPmRestartsConsumed(
      context,
      agentId,
      upToSequence,
    );
  }

  recordAgentPane(
    context: MutationContext,
    input: AgentPaneInput,
  ): { readonly recorded: true } {
    return this.#areas.panes.recordAgentPane(context, input);
  }

  clearAgentPane(
    context: MutationContext,
    agentId: string,
  ): { readonly cleared: boolean } {
    return this.#areas.panes.clearAgentPane(context, agentId);
  }

  recordFallbackPane(
    context: MutationContext,
    input: { readonly workspaceId: string; readonly paneId: string },
  ): { readonly recorded: true } {
    return this.#areas.panes.recordFallbackPane(context, input);
  }

  clearFallbackPane(context: MutationContext): { readonly cleared: boolean } {
    return this.#areas.panes.clearFallbackPane(context);
  }

  /** A live pane of a replaced PM that could not be closed; kept in the ledger so a daemon restart still knows it. */
  recordOrphanPane(
    context: MutationContext,
    input: { readonly paneId: string; readonly agentId: string },
  ): { readonly recorded: true } {
    return this.#areas.panes.recordOrphanPane(context, input);
  }

  clearOrphanPane(
    context: MutationContext,
    paneId: string,
  ): { readonly cleared: boolean } {
    return this.#areas.panes.clearOrphanPane(context, paneId);
  }

  orphanPanes(
    credential: string,
  ): readonly { readonly paneId: string; readonly agentId: string }[] {
    return this.#areas.panes.orphanPanes(credential);
  }

  agentPanes(credential: string): readonly AgentPaneRecord[] {
    return this.#areas.panes.agentPanes(credential);
  }

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
    return this.#mutate<ReportResult>(
      context,
      "report.record",
      "report:submit",
      {
        commitSha: input.commitSha,
        summaryHash: sha256(input.summary),
        evidence,
      },
      (actor) => {
        const agent = this.#agentByActor(actor.actorId);
        if (agent === undefined)
          throw new ControllerError("only an active agent can report");
        const existing = this.#database
          .prepare(
            "SELECT * FROM agent_reports WHERE project_id = ? AND agent_id = ? AND generation = ? AND commit_sha = ? AND state = 'accepted'",
          )
          .get(
            this.#projectId,
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
        const pane = this.#database
          .prepare(
            "SELECT branch, base_sha FROM agent_panes WHERE project_id = ? AND agent_id = ?",
          )
          .get(this.#projectId, agent.agent_id) as
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
            this.#database
              .prepare(
                "SELECT COUNT(*) AS n FROM agent_reports WHERE project_id = ? AND agent_id = ? AND generation = ? AND state = 'rejected'",
              )
              .get(this.#projectId, agent.agent_id, agent.generation) as {
              n: number;
            }
          ).n;
          // Only a report that would be rejected is refused at the cap: a correct report always gets through.
          if (rejected >= MAX_REJECTED_REPORTS)
            throw new ControllerError(
              "report limit reached for this agent generation",
            );
        }
        const now = this.#now();
        const reportId = randomUUID();
        const sequence = (
          this.#database
            .prepare(
              "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM agent_reports WHERE project_id = ?",
            )
            .get(this.#projectId) as { next: number }
        ).next;
        this.#database
          .prepare(
            `INSERT INTO agent_reports(project_id, report_id, sequence, agent_id, generation, actor_id, commit_sha, branch, summary, state, reason, evidence_json, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
          )
          .run(
            this.#projectId,
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
        this.#kernel.touchAgent(agent.agent_id, now);
        if (reason === null)
          this.#announceReport(reportId, agent.role_name, now);
        const row = this.#database
          .prepare(
            "SELECT * FROM agent_reports WHERE project_id = ? AND report_id = ?",
          )
          .get(this.#projectId, reportId) as AgentReportRow;
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
    const row = this.#database
      .prepare(
        `SELECT p.plan_id, p.package_id, pl.architect_agent_id FROM plan_packages p
         JOIN plans pl ON pl.project_id = p.project_id AND pl.plan_id = p.plan_id
         WHERE p.project_id = ? AND p.assignee_agent_id = ? AND pl.state = 'approved' AND pl.architect_agent_id IS NOT NULL
         ORDER BY p.assigned_at DESC, pl.sequence DESC LIMIT 1`,
      )
      .get(this.#projectId, agentId) as
      | { plan_id: string; package_id: string; architect_agent_id: string }
      | undefined;
    if (row === undefined) return undefined;
    const architect = this.#agentRow(row.architect_agent_id);
    return architect?.state === "active"
      ? { architect, planId: row.plan_id, packageId: row.package_id }
      : undefined;
  }

  /** Queues the notice for an accepted report that has none yet, to the plan's architect for a package report and to the PM otherwise; false when no recipient is active or there is no controller actor. The caller owns the transaction. */
  #announceReport(reportId: string, roleName: string, now: string): boolean {
    const row = this.#database
      .prepare(
        "SELECT * FROM agent_reports WHERE project_id = ? AND report_id = ? AND state = 'accepted' AND notified_message_id IS NULL",
      )
      .get(this.#projectId, reportId) as AgentReportRow | undefined;
    if (row === undefined) return false;
    const controller = this.#database
      .prepare(
        "SELECT actor_id FROM actors WHERE project_id = ? AND is_internal = 1 AND role = 'controller' AND active = 1 AND revoked_at IS NULL",
      )
      .get(this.#projectId) as { actor_id: string } | undefined;
    if (controller === undefined) return false;
    const route = this.#packageRoute(row.agent_id);
    let recipient: AgentRow | undefined = route?.architect;
    if (recipient === undefined) {
      const pms = this.#database
        .prepare(
          "SELECT * FROM agents WHERE project_id = ? AND kind = 'PM' AND state = 'active'",
        )
        .all(this.#projectId) as AgentRow[];
      if (pms.length !== 1) return false;
      recipient = pms[0]!;
    }
    const body =
      route === undefined || recipient !== route.architect
        ? reportNotice(row, roleName)
        : `${reportNotice(row, roleName)}\nWork package: ${route.planId}/${route.packageId}`;
    const messageId = this.#areas.messages.insertQueuedMessage(
      controller.actor_id,
      recipient,
      body,
      sha256(body),
      now,
    );
    this.#database
      .prepare(
        "UPDATE agent_reports SET notified_message_id = ? WHERE project_id = ? AND report_id = ? AND notified_message_id IS NULL",
      )
      .run(messageId, this.#projectId, reportId);
    return true;
  }

  /** The accepted report of this exact commit by this agent generation, if there is one. */
  acceptedReportFor(
    agentId: string,
    generation: number,
    commitSha: string,
  ): AgentReportRecord | undefined {
    this.#assertOpen();
    const row = this.#database
      .prepare(
        "SELECT * FROM agent_reports WHERE project_id = ? AND agent_id = ? AND generation = ? AND commit_sha = ? AND state = 'accepted'",
      )
      .get(this.#projectId, agentId, generation, commitSha) as
      AgentReportRow | undefined;
    return row === undefined ? undefined : reportRecord(row);
  }

  /** Accepted reports whose PM notice has not been queued yet (no PM was active when they arrived). */
  unannouncedReports(
    credential: string,
    limit = 50,
  ): readonly AgentReportRecord[] {
    this.#kernel.authorize(credential, "controller:reconcile");
    return (
      this.#database
        .prepare(
          "SELECT * FROM agent_reports WHERE project_id = ? AND state = 'accepted' AND notified_message_id IS NULL ORDER BY sequence LIMIT ?",
        )
        .all(this.#projectId, limit) as AgentReportRow[]
    ).map(reportRecord);
  }

  /** Queues the PM notice for one accepted report, once. */
  announceReport(
    context: MutationContext,
    reportId: string,
  ): { readonly announced: boolean } {
    safeId(reportId, "report id");
    return this.#mutate(
      context,
      "report.announce",
      "controller:reconcile",
      { reportId },
      () => {
        const row = this.#database
          .prepare(
            "SELECT a.role_name AS role_name FROM agent_reports r JOIN agents a ON a.project_id = r.project_id AND a.agent_id = r.agent_id WHERE r.project_id = ? AND r.report_id = ?",
          )
          .get(this.#projectId, reportId) as { role_name: string } | undefined;
        const announced =
          row !== undefined &&
          this.#announceReport(reportId, row.role_name, this.#now());
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
    this.#kernel.authorize(credential, "controller:reconcile");
    return (
      this.#database
        .prepare(
          "SELECT * FROM agent_reports WHERE project_id = ? ORDER BY sequence DESC LIMIT ?",
        )
        .all(this.#projectId, limit) as AgentReportRow[]
    ).map(reportRecord);
  }

  /** What a review of this report or integration would need, or why it cannot start. A read; beginReview checks again inside its transaction. */
  checkReviewRequest(
    subjectId: string,
    reviewerRole: string,
  ): {
    readonly commitSha: string;
    readonly baseSha: string;
    readonly round: number;
    readonly authorAgentIds: readonly string[];
  } {
    this.#assertOpen();
    safeId(subjectId, "report or integration id");
    safeId(reviewerRole, "reviewer role");
    const checked = this.#reviewChecks(subjectId, reviewerRole);
    return {
      commitSha: checked.commitSha,
      baseSha: checked.baseSha,
      round: checked.round,
      authorAgentIds:
        checked.plan === undefined
          ? checked.authors.map((a) => a.agent_id)
          : [checked.plan.authorAgentId],
    };
  }

  #reviewChecks(
    subjectId: string,
    reviewerRole: string,
  ): {
    readonly subject: "report" | "integration" | "plan";
    readonly commitSha: string;
    readonly baseSha: string;
    readonly round: number;
    readonly authors: readonly AgentReportRow[];
    readonly plan?: {
      readonly revision: number;
      readonly bodyJson: string;
      readonly authorAgentId: string;
      readonly authorActorId: string;
    };
  } {
    const report = this.#database
      .prepare(
        "SELECT * FROM agent_reports WHERE project_id = ? AND report_id = ?",
      )
      .get(this.#projectId, subjectId) as AgentReportRow | undefined;
    const integration =
      report === undefined
        ? (this.#database
            .prepare(
              "SELECT * FROM integrations WHERE project_id = ? AND integration_id = ?",
            )
            .get(this.#projectId, subjectId) as IntegrationRow | undefined)
        : undefined;
    const planRow =
      report === undefined && integration === undefined
        ? this.#planRow(subjectId)
        : undefined;
    if (
      report === undefined &&
      integration === undefined &&
      planRow === undefined
    )
      throw new ControllerError("the report or integration does not exist");
    const column =
      planRow !== undefined
        ? "subject_plan_id"
        : report === undefined
          ? "subject_integration_id"
          : "subject_report_id";
    let baseSha: unknown;
    let commitSha: string;
    let authors: readonly AgentReportRow[] = [];
    let plan:
      | {
          revision: number;
          bodyJson: string;
          authorAgentId: string;
          authorActorId: string;
        }
      | undefined;
    if (planRow !== undefined) {
      if (planRow.state !== "in_review")
        throw new ControllerError(
          `plan ${planRow.plan_id} is ${planRow.state}; only a plan in review can be reviewed`,
        );
      const revision = this.#database
        .prepare(
          "SELECT * FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
        )
        .get(this.#projectId, planRow.plan_id, planRow.current_revision) as
        | {
            base_sha: string;
            body_json: string;
            author_agent_id: string;
            author_actor_id: string;
          }
        | undefined;
      if (revision === undefined)
        throw new ControllerError("the plan has no revision to review");
      baseSha = revision.base_sha;
      commitSha = revision.base_sha;
      plan = {
        revision: planRow.current_revision,
        bodyJson: revision.body_json,
        authorAgentId: revision.author_agent_id,
        authorActorId: revision.author_actor_id,
      };
    } else if (report !== undefined) {
      if (report.state !== "accepted")
        throw new ControllerError("only an accepted report can be reviewed");
      try {
        baseSha = (JSON.parse(report.evidence_json) as ReportEvidence).baseSha;
      } catch {
        baseSha = undefined;
      }
      commitSha = report.commit_sha;
      authors = [report];
    } else {
      if (integration!.state !== "merged")
        throw new ControllerError(
          "only a merged, unconfirmed integration can be reviewed",
        );
      baseSha = integration!.base_sha;
      commitSha = integration!.head_sha!;
      authors = this.#integrationAuthors(integration!.integration_id);
    }
    if (typeof baseSha !== "string" || !/^[0-9a-f]{40}$/.test(baseSha))
      throw new ControllerError("the subject has no usable base commit");
    const open = this.#database
      .prepare(
        `SELECT 1 AS present FROM reviews WHERE project_id = ? AND ${column} = ? AND state = 'started'`,
      )
      .get(this.#projectId, subjectId);
    if (open)
      throw new ControllerError("a review of this subject is already open");
    const done = (
      this.#database
        .prepare(
          `SELECT COUNT(*) AS n FROM reviews WHERE project_id = ? AND ${column} = ? AND state IN ('passed', 'findings')`,
        )
        .get(this.#projectId, subjectId) as { n: number }
    ).n;
    if (done >= MAX_REVIEW_ROUNDS)
      throw new ControllerError("the review limit for this subject is reached");
    const role = this.#database
      .prepare(
        "SELECT kind FROM role_definitions WHERE project_id = ? AND role_name = ? AND state = 'active'",
      )
      .get(this.#projectId, reviewerRole) as { kind: string } | undefined;
    if (role?.kind !== "Verifier")
      throw new ControllerError(
        "the reviewer role must be an active Verifier role",
      );
    const round = (
      this.#database
        .prepare(
          `SELECT COALESCE(MAX(round), 0) + 1 AS next FROM reviews WHERE project_id = ? AND ${column} = ?`,
        )
        .get(this.#projectId, subjectId) as { next: number }
    ).next;
    return {
      subject:
        plan !== undefined
          ? "plan"
          : report === undefined
            ? "integration"
            : "report",
      commitSha,
      baseSha,
      round,
      authors,
      ...(plan === undefined ? {} : { plan }),
    };
  }

  /** The reports an integration merged, in merge order. */
  #integrationAuthors(integrationId: string): AgentReportRow[] {
    return this.#database
      .prepare(
        `SELECT r.* FROM integration_reports ir
         JOIN agent_reports r ON r.project_id = ir.project_id AND r.report_id = ir.report_id
         WHERE ir.project_id = ? AND ir.integration_id = ? ORDER BY ir.position`,
      )
      .all(this.#projectId, integrationId) as AgentReportRow[];
  }

  /**
   * Starts a review of an accepted report with an already spawned reviewer: the
   * review row (state started), the independence check and the task message
   * to the reviewer are one transaction. The reviewer must be a new agent: its
   * actor and agent id must differ from the author's.
   */
  beginReview(
    context: MutationContext,
    input: {
      readonly subjectId: string;
      readonly reviewerRole: string;
      readonly reviewerAgentId: string;
    },
  ): ReviewRecord {
    safeId(input.subjectId, "report or integration id");
    safeId(input.reviewerRole, "reviewer role");
    safeId(input.reviewerAgentId, "reviewer agent id");
    return this.#mutate<ReviewRecord>(
      context,
      "review.begin",
      "review:request",
      { ...input },
      (actor) => {
        const checked = this.#reviewChecks(input.subjectId, input.reviewerRole);
        const reviewer = this.#agentRow(input.reviewerAgentId);
        if (
          reviewer?.state !== "active" ||
          reviewer.kind !== "Verifier" ||
          reviewer.role_name !== input.reviewerRole
        )
          throw new ControllerError(
            "the reviewer agent is not an active agent of the requested Verifier role",
          );
        const authorParties =
          checked.plan === undefined
            ? checked.authors.map((a) => ({
                agent_id: a.agent_id,
                actor_id: a.actor_id,
              }))
            : [
                {
                  agent_id: checked.plan.authorAgentId,
                  actor_id: checked.plan.authorActorId,
                },
              ];
        if (
          authorParties.some(
            (author) =>
              reviewer.actor_id === author.actor_id ||
              reviewer.agent_id === author.agent_id,
          )
        )
          throw new ControllerError("a reviewer cannot be an author");
        const single =
          checked.subject === "integration" ? null : authorParties[0]!;
        const controller = this.#database
          .prepare(
            "SELECT actor_id FROM actors WHERE project_id = ? AND is_internal = 1 AND role = 'controller' AND active = 1 AND revoked_at IS NULL",
          )
          .get(this.#projectId) as { actor_id: string } | undefined;
        if (controller === undefined)
          throw new ControllerError("the controller actor is missing");
        const now = this.#now();
        const reviewId = randomUUID();
        const sequence = (
          this.#database
            .prepare(
              "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM reviews WHERE project_id = ?",
            )
            .get(this.#projectId) as { next: number }
        ).next;
        this.#database
          .prepare(
            `INSERT INTO reviews(project_id, review_id, sequence, round, subject_report_id, subject_integration_id, subject_plan_id, subject_plan_revision,
               commit_sha, base_sha, author_agent_id, author_actor_id, requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, state, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'started', ?)`,
          )
          .run(
            this.#projectId,
            reviewId,
            sequence,
            checked.round,
            checked.subject === "report" ? input.subjectId : null,
            checked.subject === "integration" ? input.subjectId : null,
            checked.plan === undefined ? null : input.subjectId,
            checked.plan?.revision ?? null,
            checked.commitSha,
            checked.baseSha,
            single?.agent_id ?? null,
            single?.actor_id ?? null,
            actor.actorId,
            input.reviewerRole,
            reviewer.agent_id,
            reviewer.actor_id,
            now,
          );
        const row = this.#database
          .prepare(
            "SELECT * FROM reviews WHERE project_id = ? AND review_id = ?",
          )
          .get(this.#projectId, reviewId) as ReviewRow;
        const task = reviewTask(
          row,
          checked.authors.map((report) => ({ report })),
        );
        this.#areas.messages.insertQueuedMessage(
          controller.actor_id,
          reviewer,
          task,
          sha256(task),
          now,
        );
        return {
          value: reviewRecord(row),
          event: {
            entityType: "review",
            entityId: reviewId,
            stateVersion: 0,
            toState: "started",
            details: {
              subjectId: input.subjectId,
              round: checked.round,
              reviewerAgentId: reviewer.agent_id,
              authorAgentIds: authorParties.map((a) => a.agent_id),
            },
          },
        };
      },
    );
  }

  /** The reviewer's verdict, written once. The PM notice is queued in the same transaction when exactly one PM is active. */
  completeReview(
    context: MutationContext,
    input: { readonly verdict: "pass" | "findings"; readonly text: string },
  ): ReviewRecord {
    if (input.verdict !== "pass" && input.verdict !== "findings")
      throw new TypeError("the verdict must be pass or findings");
    if (
      typeof input.text !== "string" ||
      !input.text.isWellFormed() ||
      input.text.trim() === "" ||
      /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}\p{Noncharacter_Code_Point}]/u.test(
        input.text.replaceAll("\n", ""),
      ) ||
      Buffer.byteLength(input.text, "utf8") > MAX_REVIEW_TEXT_BYTES
    )
      throw new TypeError(
        `the review text must be printable text of at most ${MAX_REVIEW_TEXT_BYTES} bytes`,
      );
    return this.#mutate<ReviewRecord>(
      context,
      "review.complete",
      "review:submit",
      { verdict: input.verdict, textHash: sha256(input.text) },
      (actor) => {
        // By agent, not by actor: a replaced generation gets a new actor but is the same reviewer.
        const caller = this.#agentByActor(actor.actorId);
        const row =
          caller === undefined
            ? undefined
            : (this.#database
                .prepare(
                  "SELECT * FROM reviews WHERE project_id = ? AND reviewer_agent_id = ? AND state = 'started'",
                )
                .get(this.#projectId, caller.agent_id) as
                ReviewRow | undefined);
        if (row === undefined)
          throw new ControllerError("you have no review in progress");
        const now = this.#now();
        this.#database
          .prepare(
            "UPDATE reviews SET state = ?, verdict_text = ?, completed_at = ? WHERE project_id = ? AND review_id = ?",
          )
          .run(
            input.verdict === "pass" ? "passed" : "findings",
            input.text,
            now,
            this.#projectId,
            row.review_id,
          );
        this.#kernel.touchAgent(row.reviewer_agent_id, now);
        if (row.subject_plan_id !== null)
          this.#settlePlanReview(row, input.verdict, now);
        else if (input.verdict === "pass" && row.subject_report_id !== null)
          this.#noticePackageReviewed(row, now);
        this.#announceReview(row.review_id, now);
        const done = this.#database
          .prepare(
            "SELECT * FROM reviews WHERE project_id = ? AND review_id = ?",
          )
          .get(this.#projectId, row.review_id) as ReviewRow;
        return {
          value: reviewRecord(done),
          event: {
            entityType: "review",
            entityId: row.review_id,
            stateVersion: 0,
            fromState: "started",
            toState: done.state,
            details: {
              subjectId:
                row.subject_report_id ??
                row.subject_integration_id ??
                row.subject_plan_id,
              round: row.round,
            },
          },
        };
      },
    );
  }

  /** A finished plan review moves its plan: pass approves it (package rows, superseded plan), findings send it back to draft. A plan that is no longer in review is left as it is. The caller owns the transaction. */
  #settlePlanReview(
    row: ReviewRow,
    verdict: "pass" | "findings",
    now: string,
  ): void {
    const plan = this.#planRow(row.subject_plan_id!);
    if (plan?.state !== "in_review" || plan.cancelled_at !== null) return;
    if (verdict === "findings") {
      this.#database
        .prepare(
          "UPDATE plans SET state = 'draft', updated_at = ? WHERE project_id = ? AND plan_id = ?",
        )
        .run(now, this.#projectId, plan.plan_id);
      if (this.#finishedPlanReviews(plan.plan_id) >= MAX_REVIEW_ROUNDS)
        this.#areas.messageNotices.noticeToPm(
          planNeedsAttentionNotice(plan.plan_id),
          now,
        );
      return;
    }
    const revision = this.#database
      .prepare(
        "SELECT body_json FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
      )
      .get(this.#projectId, plan.plan_id, row.subject_plan_revision) as {
      body_json: string;
    };
    this.#database
      .prepare(
        "UPDATE plans SET state = 'approved', approved_revision = ?, updated_at = ? WHERE project_id = ? AND plan_id = ?",
      )
      .run(row.subject_plan_revision, now, this.#projectId, plan.plan_id);
    this.#settlePlanApproval(plan, revision.body_json, now);
  }

  /**
   * Tells the PM that a package's report passed review, once per report, when the package's plan has a `plan` link:
   * this is the one status change the PM has no other signal for. A cancelled plan or package is not announced.
   */
  #noticePackageReviewed(row: ReviewRow, now: string): void {
    const candidates = this.#database
      .prepare(
        `SELECT p.plan_id, p.package_id, p.assignee_agent_id, p.assigned_at FROM plan_packages p
         JOIN plans pl ON pl.project_id = p.project_id AND pl.plan_id = p.plan_id
         JOIN external_links l ON l.project_id = p.project_id AND l.ref_kind = 'plan' AND l.ref_id = p.plan_id AND l.system = 'nexora'
         WHERE p.project_id = ? AND p.assignee_agent_id = ? AND p.cancelled_at IS NULL AND pl.cancelled_at IS NULL
         ORDER BY p.plan_id, p.package_id`,
      )
      .all(this.#projectId, row.author_agent_id) as {
      plan_id: string;
      package_id: string;
      assignee_agent_id: string;
      assigned_at: string;
    }[];
    const match = candidates.find(
      (c) =>
        this.#packageReport(
          c.plan_id,
          c.package_id,
          c.assignee_agent_id,
          c.assigned_at,
        )?.report_id === row.subject_report_id,
    );
    if (match === undefined) return;
    const body = [
      `Plan ${match.plan_id} package ${match.package_id} reviewed`,
      `Report: ${row.subject_report_id}`,
      `Commit: ${row.commit_sha}`,
    ].join("\n");
    const sent = this.#database
      .prepare(
        "SELECT 1 AS present FROM messages WHERE project_id = ? AND body = ?",
      )
      .get(this.#projectId, body);
    if (sent === undefined) this.#areas.messageNotices.noticeToPm(body, now);
  }

  #reviewAuthorIds(row: ReviewRow): string[] {
    if (row.subject_plan_id !== null) return [row.author_agent_id!];
    return row.subject_integration_id === null
      ? [row.author_agent_id!]
      : this.#integrationAuthors(row.subject_integration_id).map(
          (r) => r.agent_id,
        );
  }

  /** Queues the notice for a finished review that has none, to the agent that requested it (the architect) and to the PM when the requester is the PM, the operator or no longer active; false when no PM is the sole active one. The caller owns the transaction. */
  #announceReview(reviewId: string, now: string): boolean {
    const row = this.#database
      .prepare(
        "SELECT * FROM reviews WHERE project_id = ? AND review_id = ? AND state IN ('passed', 'findings') AND notified_message_id IS NULL",
      )
      .get(this.#projectId, reviewId) as ReviewRow | undefined;
    if (row === undefined) return false;
    const requester = this.#agentByActor(row.requested_by_actor_id);
    const parties = this.#areas.messageNotices.noticeParties();
    const recipient =
      requester?.state === "active" && requester.kind === "Developer"
        ? requester
        : parties?.pm;
    if (recipient === undefined) return false;
    const body = reviewNotice(row, this.#reviewAuthorIds(row));
    const messageId = this.#areas.messages.insertQueuedMessage(
      parties?.controllerActorId ?? this.#kernel.controllerActorId(),
      recipient,
      body,
      sha256(body),
      now,
    );
    this.#database
      .prepare(
        "UPDATE reviews SET notified_message_id = ? WHERE project_id = ? AND review_id = ? AND notified_message_id IS NULL",
      )
      .run(messageId, this.#projectId, reviewId);
    return true;
  }

  /** Finished reviews whose PM notice has not been queued. */
  unannouncedReviews(credential: string, limit = 50): readonly ReviewRecord[] {
    this.#kernel.authorize(credential, "controller:reconcile");
    return (
      this.#database
        .prepare(
          "SELECT * FROM reviews WHERE project_id = ? AND state IN ('passed', 'findings') AND notified_message_id IS NULL ORDER BY sequence LIMIT ?",
        )
        .all(this.#projectId, limit) as ReviewRow[]
    ).map(reviewRecord);
  }

  announceReview(
    context: MutationContext,
    reviewId: string,
  ): { readonly announced: boolean } {
    safeId(reviewId, "review id");
    return this.#mutate(
      context,
      "review.announce",
      "controller:reconcile",
      { reviewId },
      () => {
        const announced = this.#announceReview(reviewId, this.#now());
        return {
          value: { announced },
          event: {
            entityType: "review",
            entityId: reviewId,
            stateVersion: 0,
            details: { announced },
          },
        };
      },
    );
  }

  reviews(credential: string, limit = 20): readonly ReviewRecord[] {
    this.#kernel.authorize(credential, "controller:reconcile");
    return (
      this.#database
        .prepare(
          "SELECT * FROM reviews WHERE project_id = ? ORDER BY sequence DESC LIMIT ?",
        )
        .all(this.#projectId, limit) as ReviewRow[]
    ).map(reviewRecord);
  }

  /** Finished reviews whose reviewer agent is still active: the reviewer was not released (a crash after the verdict). */
  reviewsToRelease(credential: string): readonly ReviewRecord[] {
    this.#kernel.authorize(credential, "controller:reconcile");
    return (
      this.#database
        .prepare(
          `SELECT r.* FROM reviews r JOIN agents a ON a.project_id = r.project_id AND a.agent_id = r.reviewer_agent_id
           WHERE r.project_id = ? AND r.state IN ('passed', 'findings') AND a.state = 'active' ORDER BY r.sequence`,
        )
        .all(this.#projectId) as ReviewRow[]
    ).map(reviewRecord);
  }

  /**
   * Starts an integration: records the base, the merge order and the branch the controller will
   * create. Every report must be accepted and its
   * latest finished review must be a pass; only one integration runs at a time.
   */
  beginIntegration(
    context: MutationContext,
    input: {
      readonly integrationId: string;
      readonly reportIds: readonly string[];
      readonly baseSha: string;
      readonly branch: string;
      readonly requestedBy: string;
    },
  ): IntegrationRecord {
    safeId(input.integrationId, "integration id");
    safeId(input.requestedBy, "requester");
    if (!/^[0-9a-f]{40}$/.test(input.baseSha))
      throw new TypeError("the base commit must be a full lowercase sha1");
    if (
      !Array.isArray(input.reportIds) ||
      input.reportIds.length < 1 ||
      input.reportIds.length > MAX_INTEGRATION_REPORTS
    )
      throw new TypeError(
        `an integration takes 1 to ${MAX_INTEGRATION_REPORTS} reports`,
      );
    for (const id of input.reportIds) safeId(id, "report id");
    return this.#mutate<IntegrationRecord>(
      context,
      "integration.begin",
      "controller:reconcile",
      { ...input },
      () => {
        if (new Set(input.reportIds).size !== input.reportIds.length)
          throw new ControllerError("a report is named twice");
        const running = this.#database
          .prepare(
            "SELECT 1 AS present FROM integrations WHERE project_id = ? AND state = 'running'",
          )
          .get(this.#projectId);
        if (running)
          throw new ControllerError("an integration is already running");
        for (const id of input.reportIds) {
          const report = this.#database
            .prepare(
              "SELECT state FROM agent_reports WHERE project_id = ? AND report_id = ?",
            )
            .get(this.#projectId, id) as { state: string } | undefined;
          if (report === undefined)
            throw new ControllerError(`report ${id} does not exist`);
          if (report.state !== "accepted")
            throw new ControllerError(`report ${id} was not accepted`);
          const latest = this.#database
            .prepare(
              `SELECT state FROM reviews WHERE project_id = ? AND subject_report_id = ? AND state IN ('passed', 'findings')
               ORDER BY sequence DESC LIMIT 1`,
            )
            .get(this.#projectId, id) as { state: string } | undefined;
          if (latest?.state !== "passed")
            throw new ControllerError(
              `report ${id} has no passed review as its latest verdict`,
            );
          const openReview = this.#database
            .prepare(
              "SELECT 1 AS present FROM reviews WHERE project_id = ? AND subject_report_id = ? AND state = 'started'",
            )
            .get(this.#projectId, id);
          if (openReview)
            throw new ControllerError(`report ${id} has a review still open`);
          const elsewhere = this.#database
            .prepare(
              `SELECT i.integration_id, i.state FROM integration_reports ir
               JOIN integrations i ON i.project_id = ir.project_id AND i.integration_id = ir.integration_id
               WHERE ir.project_id = ? AND ir.report_id = ? AND i.state IN ('confirmed', 'merged')
               ORDER BY i.sequence DESC LIMIT 1`,
            )
            .get(this.#projectId, id) as
            { integration_id: string; state: string } | undefined;
          if (elsewhere?.state === "confirmed")
            throw new ControllerError(
              `report ${id} was already integrated and confirmed in ${elsewhere.integration_id}`,
            );
          if (elsewhere !== undefined)
            throw new ControllerError(
              `report ${id} is already in the unsettled integration ${elsewhere.integration_id}; confirm or discard that one first`,
            );
        }
        this.#checkIntegrationOrder(input.reportIds);
        const now = this.#now();
        const sequence = (
          this.#database
            .prepare(
              "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM integrations WHERE project_id = ?",
            )
            .get(this.#projectId) as { next: number }
        ).next;
        this.#database
          .prepare(
            `INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, created_at)
             VALUES (?, ?, ?, ?, ?, ?, 'running', ?)`,
          )
          .run(
            this.#projectId,
            input.integrationId,
            sequence,
            input.baseSha,
            input.branch,
            input.requestedBy,
            now,
          );
        input.reportIds.forEach((reportId, index) =>
          this.#database
            .prepare(
              "INSERT INTO integration_reports(project_id, integration_id, position, report_id) VALUES (?, ?, ?, ?)",
            )
            .run(this.#projectId, input.integrationId, index + 1, reportId),
        );
        return {
          value: this.#integrationRecord(input.integrationId),
          event: {
            entityType: "integration",
            entityId: input.integrationId,
            stateVersion: 0,
            toState: "running",
            details: {
              baseSha: input.baseSha,
              reportIds: [...input.reportIds],
              requestedBy: input.requestedBy,
            },
          },
        };
      },
    );
  }

  /** Records how a running integration ended. A conflict is also queued to the PM when the operator asked for the integration. */
  finishIntegration(
    context: MutationContext,
    input: {
      readonly integrationId: string;
      readonly outcome: IntegrationOutcome;
    },
  ): IntegrationRecord {
    safeId(input.integrationId, "integration id");
    const outcome = input.outcome;
    if (outcome.kind === "merged" && !/^[0-9a-f]{40}$/.test(outcome.headSha))
      throw new TypeError(
        "the integrated commit must be a full lowercase sha1",
      );
    if (
      outcome.kind === "conflicted" &&
      (outcome.files.length < 1 ||
        outcome.files.length > MAX_CONFLICT_FILES ||
        !Number.isInteger(outcome.omitted) ||
        outcome.omitted < 0 ||
        outcome.files.some(
          (file) =>
            !/^[\x20-\x7e]+$/.test(file) ||
            file.length > MAX_CONFLICT_PATH_CHARS + PATH_CUT_MARK_CHARS,
        ))
    )
      throw new TypeError(
        "the conflict files must be a short list of printable paths",
      );
    if (
      outcome.kind === "failed" &&
      !/^[\x20-\x7e]{1,300}$/.test(outcome.reason)
    )
      throw new TypeError("the failure reason must be short printable text");
    return this.#mutate<IntegrationRecord>(
      context,
      "integration.finish",
      "controller:reconcile",
      { integrationId: input.integrationId, outcome },
      () => {
        const row = this.#integrationRow(input.integrationId);
        if (row.state !== "running")
          throw new ControllerError("the integration is not running");
        const now = this.#now();
        this.#database
          .prepare(
            `UPDATE integrations SET state = ?, head_sha = ?, conflict_report_id = ?, conflict_files_json = ?, conflict_files_omitted = ?, failure_reason = ?, completed_at = ?
             WHERE project_id = ? AND integration_id = ?`,
          )
          .run(
            outcome.kind,
            outcome.kind === "merged" ? outcome.headSha : null,
            outcome.kind === "conflicted" ? outcome.reportId : null,
            outcome.kind === "conflicted"
              ? JSON.stringify(outcome.files)
              : null,
            outcome.kind === "conflicted" ? outcome.omitted : null,
            outcome.kind === "failed" ? outcome.reason : null,
            now,
            this.#projectId,
            input.integrationId,
          );
        if (outcome.kind === "conflicted" && row.requested_by === "operator")
          this.#noticeConflict(input.integrationId, outcome, now);
        return {
          value: this.#integrationRecord(input.integrationId),
          event: {
            entityType: "integration",
            entityId: input.integrationId,
            stateVersion: 0,
            fromState: "running",
            toState: outcome.kind,
            details:
              outcome.kind === "conflicted"
                ? {
                    reportId: outcome.reportId,
                    files: [...outcome.files],
                    omitted: outcome.omitted,
                  }
                : outcome.kind === "failed"
                  ? { reason: outcome.reason }
                  : { headSha: outcome.headSha },
          },
        };
      },
    );
  }

  #noticeConflict(
    integrationId: string,
    outcome: Extract<IntegrationOutcome, { kind: "conflicted" }>,
    now: string,
  ): void {
    const parties = this.#areas.messageNotices.noticeParties();
    if (parties === undefined) return;
    const body = [
      `Integration ${integrationId} is blocked by a merge conflict`,
      `The conflict arose when merging report ${outcome.reportId}. Files (escaped; a path is text from a worker): ${outcome.files.join(", ")}${outcome.omitted > 0 ? `, and ${outcome.omitted} more not listed` : ""}`,
      "The controller aborted the merge and left nothing behind. It does not resolve conflicts. Assign a developer to resolve it as a new candidate, then report and review again.",
    ].join("\n");
    this.#areas.messages.insertQueuedMessage(
      parties.controllerActorId,
      parties.pm,
      body,
      sha256(body),
      now,
    );
  }

  /**
   * Ends a merged integration: confirmed (accepted; needs a passed review and no
   * open one) or discarded. The caller removes the branch afterwards.
   */
  settleIntegration(
    context: MutationContext,
    input: {
      readonly integrationId: string;
      readonly outcome: "confirmed" | "discarded";
    },
  ): IntegrationRecord {
    safeId(input.integrationId, "integration id");
    if (input.outcome !== "confirmed" && input.outcome !== "discarded")
      throw new TypeError("the outcome must be confirmed or discarded");
    return this.#mutate<IntegrationRecord>(
      context,
      "integration.settle",
      "controller:reconcile",
      { ...input },
      () => {
        const row = this.#integrationRow(input.integrationId);
        if (row.state !== "merged")
          throw new ControllerError("only a merged integration can be settled");
        const open = this.#database
          .prepare(
            "SELECT 1 AS present FROM reviews WHERE project_id = ? AND subject_integration_id = ? AND state = 'started'",
          )
          .get(this.#projectId, input.integrationId);
        if (open)
          throw new ControllerError("a review of this integration is open");
        if (input.outcome === "confirmed") {
          const latest = this.#database
            .prepare(
              `SELECT state FROM reviews WHERE project_id = ? AND subject_integration_id = ? AND state IN ('passed', 'findings')
               ORDER BY sequence DESC LIMIT 1`,
            )
            .get(this.#projectId, input.integrationId) as
            { state: string } | undefined;
          if (latest?.state !== "passed")
            throw new ControllerError(
              "an integration is confirmed only after its latest review passed",
            );
        }
        this.#database
          .prepare(
            "UPDATE integrations SET state = ? WHERE project_id = ? AND integration_id = ?",
          )
          .run(input.outcome, this.#projectId, input.integrationId);
        return {
          value: this.#integrationRecord(input.integrationId),
          event: {
            entityType: "integration",
            entityId: input.integrationId,
            stateVersion: 0,
            fromState: "merged",
            toState: input.outcome,
            details: {},
          },
        };
      },
    );
  }

  #integrationRow(integrationId: string): IntegrationRow {
    const row = this.#database
      .prepare(
        "SELECT * FROM integrations WHERE project_id = ? AND integration_id = ?",
      )
      .get(this.#projectId, integrationId) as IntegrationRow | undefined;
    if (row === undefined)
      throw new ControllerError("the integration does not exist");
    return row;
  }

  #integrationRecord(integrationId: string): IntegrationRecord {
    const row = this.#integrationRow(integrationId);
    return {
      integrationId: row.integration_id,
      sequence: row.sequence,
      baseSha: row.base_sha,
      branch: row.branch,
      requestedBy: row.requested_by,
      state: row.state,
      headSha: row.head_sha,
      conflictReportId: row.conflict_report_id,
      conflictFilesOmitted: row.conflict_files_omitted,
      conflictFiles:
        row.conflict_files_json === null
          ? null
          : (JSON.parse(row.conflict_files_json) as string[]),
      failureReason: row.failure_reason,
      reports: this.#integrationAuthors(integrationId).map((r) => ({
        reportId: r.report_id,
        agentId: r.agent_id,
        commitSha: r.commit_sha,
      })),
      createdAt: row.created_at,
      completedAt: row.completed_at,
    };
  }

  /** Opens a draft plan; only the PM or the operator may. `supersedesPlanId` must name an approved plan. */
  openPlan(
    context: MutationContext,
    input: {
      readonly tier: PlanTier;
      readonly title: string;
      readonly supersedesPlanId?: string;
    },
  ): PlanRecord {
    if (input.tier !== "normal" && input.tier !== "high_risk")
      throw new TypeError("the plan tier must be normal or high_risk");
    const title = safeText(input.title, "plan title", 200, false);
    const supersedes =
      input.supersedesPlanId === undefined
        ? null
        : safeId(input.supersedesPlanId, "superseded plan id");
    return this.#mutate<PlanRecord>(
      context,
      "plan.open",
      "plan:write",
      { tier: input.tier, title, supersedesPlanId: supersedes },
      (actor) => {
        if (actor.role !== "PM" && actor.role !== "operator")
          throw new ControllerError("only the PM or the operator opens a plan");
        if (supersedes !== null) {
          const old = this.#planRow(supersedes);
          if (old === undefined)
            throw new ControllerError(`plan ${supersedes} does not exist`);
          if (old.cancelled_at !== null)
            throw new ControllerError(
              `plan_cancelled: ${supersedes} was cancelled; open a fresh plan without naming it`,
            );
          if (old.state !== "approved")
            throw new ControllerError(
              `plan ${supersedes} is ${old.state}; only an approved plan can be superseded`,
            );
        }
        const now = this.#now();
        const sequence = (
          this.#database
            .prepare(
              "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM plans WHERE project_id = ?",
            )
            .get(this.#projectId) as { next: number }
        ).next;
        const planId = `plan-${sequence}`;
        this.#database
          .prepare(
            `INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, supersedes_plan_id, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, 'draft', ?, ?, ?, ?)`,
          )
          .run(
            this.#projectId,
            planId,
            sequence,
            title,
            input.tier,
            actor.actorId,
            supersedes,
            now,
            now,
          );
        return {
          value: planRecordOf(this.#planRow(planId)!),
          event: {
            entityType: "plan",
            entityId: planId,
            stateVersion: 0,
            toState: "draft",
            details: { tier: input.tier, supersedesPlanId: supersedes },
          },
        };
      },
    );
  }

  /**
   * Stores the next revision of a draft plan, written by an active Developer-kind agent (the designated
   * Architect check belongs to the command layer; the first submitter is recorded as the plan's architect
   * and no other agent may submit afterwards). `bodyJson` is the canonical text of a body that `src/plans.ts`
   * validated. With `review` the plan moves to in_review, otherwise it is approved at once.
   */
  submitPlan(
    context: MutationContext,
    input: {
      readonly planId: string;
      readonly bodyJson: string;
      readonly baseSha: string;
      readonly review: boolean;
    },
  ): PlanRecord {
    safeId(input.planId, "plan id");
    if (!/^[0-9a-f]{40}$/.test(input.baseSha))
      throw new TypeError("the base commit must be a full lowercase sha1");
    planBodyPackageIds(input.bodyJson);
    return this.#mutate<PlanRecord>(
      context,
      "plan.submit",
      "plan:write",
      {
        planId: input.planId,
        bodySha: sha256(input.bodyJson),
        baseSha: input.baseSha,
        review: input.review,
      },
      (actor) => {
        const agent = this.#agentByActor(actor.actorId);
        if (agent?.kind !== "Developer")
          throw new ControllerError(
            "only an active developer-kind agent submits a plan",
          );
        const plan = this.#planRow(input.planId);
        if (plan === undefined)
          throw new ControllerError(`plan ${input.planId} does not exist`);
        if (plan.cancelled_at !== null)
          throw new ControllerError(
            `plan_cancelled: plan ${input.planId} was cancelled`,
          );
        if (plan.state !== "draft")
          throw new ControllerError(
            `plan ${input.planId} is ${plan.state}, not a draft`,
          );
        if (
          plan.architect_agent_id !== null &&
          plan.architect_agent_id !== agent.agent_id
        )
          throw new ControllerError(
            `plan ${input.planId} belongs to the architect ${plan.architect_agent_id}`,
          );
        const now = this.#now();
        const revision = plan.current_revision + 1;
        this.#database
          .prepare(
            `INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
          )
          .run(
            this.#projectId,
            input.planId,
            revision,
            input.baseSha,
            input.bodyJson,
            sha256(input.bodyJson),
            agent.agent_id,
            actor.actorId,
            now,
          );
        this.#database
          .prepare(
            `UPDATE plans SET current_revision = ?, architect_agent_id = ?, state = ?, approved_revision = ?, updated_at = ?
             WHERE project_id = ? AND plan_id = ?`,
          )
          .run(
            revision,
            agent.agent_id,
            input.review ? "in_review" : "approved",
            input.review ? null : revision,
            now,
            this.#projectId,
            input.planId,
          );
        if (!input.review) this.#settlePlanApproval(plan, input.bodyJson, now);
        return {
          value: planRecordOf(this.#planRow(input.planId)!),
          event: {
            entityType: "plan",
            entityId: input.planId,
            stateVersion: 0,
            fromState: "draft",
            toState: input.review ? "in_review" : "approved",
            details: { revision, agentId: agent.agent_id },
          },
        };
      },
    );
  }

  /** On approval: one package row per package of the approved body, and the superseded plan is retired. The caller owns the transaction and has already moved the plan to approved. */
  #settlePlanApproval(plan: PlanRow, bodyJson: string, now: string): void {
    for (const packageId of planBodyPackageIds(bodyJson))
      this.#database
        .prepare(
          "INSERT INTO plan_packages(project_id, plan_id, package_id) VALUES (?, ?, ?)",
        )
        .run(this.#projectId, plan.plan_id, packageId);
    if (plan.supersedes_plan_id !== null) {
      const old = this.#planRow(plan.supersedes_plan_id);
      if (old === undefined || old.cancelled_at === null) {
        if (old?.state !== "approved")
          throw new ControllerError(
            `plan ${plan.supersedes_plan_id} is no longer approved and cannot be superseded`,
          );
        this.#database
          .prepare(
            "UPDATE plans SET state = 'superseded', updated_at = ? WHERE project_id = ? AND plan_id = ?",
          )
          .run(now, this.#projectId, plan.supersedes_plan_id);
      }
    }
    this.#areas.messageNotices.noticeToPm(
      planApprovedNotice(plan.plan_id, bodyJson, this.#approvalNote(plan)),
      now,
    );
  }

  /**
   * Plan notices the PM has not received (no PM was active when the plan was approved or ran out of review rounds).
   * A notice counts as sent when a controller message with its leading text exists.
   */
  unannouncedPlanNotices(credential: string): readonly PlanNoticeRef[] {
    this.#kernel.authorize(credential, "controller:reconcile");
    const sent = (planId: string, lead: string): boolean => {
      const text = `Plan ${planId} ${lead}`;
      return (
        this.#database
          .prepare(
            "SELECT 1 AS present FROM messages WHERE project_id = ? AND substr(body, 1, ?) = ?",
          )
          .get(this.#projectId, text.length, text) !== undefined
      );
    };
    const out: PlanNoticeRef[] = [];
    for (const plan of this.#database
      .prepare(
        "SELECT * FROM plans WHERE project_id = ? AND state IN ('approved', 'draft') ORDER BY sequence",
      )
      .all(this.#projectId) as PlanRow[]) {
      if (plan.cancelled_at !== null) continue;
      if (plan.state === "approved") {
        if (!sent(plan.plan_id, "approved"))
          out.push({ planId: plan.plan_id, kind: "approved" });
      } else if (
        this.#finishedPlanReviews(plan.plan_id) >= MAX_REVIEW_ROUNDS &&
        !sent(plan.plan_id, "needs attention")
      )
        out.push({ planId: plan.plan_id, kind: "needs_attention" });
    }
    for (const signoff of this.#database
      .prepare(
        `SELECT s.plan_id, s.integration_id FROM plan_signoffs s JOIN plans p
           ON p.project_id = s.project_id AND p.plan_id = s.plan_id
         WHERE s.project_id = ? AND p.cancelled_at IS NULL ORDER BY p.sequence, s.created_at, s.integration_id`,
      )
      .all(this.#projectId) as { plan_id: string; integration_id: string }[]) {
      if (
        !sent(
          signoff.plan_id,
          `signed off. Integration ${signoff.integration_id} `,
        )
      )
        out.push({
          planId: signoff.plan_id,
          kind: "signed_off",
          integrationId: signoff.integration_id,
        });
    }
    return out;
  }

  /** Queues one missing plan notice to the PM; false when no PM is the sole active one. */
  announcePlanNotice(
    context: MutationContext,
    input: PlanNoticeRef,
  ): { readonly announced: boolean } {
    safeId(input.planId, "plan id");
    if (input.kind === "signed_off")
      safeId(input.integrationId, "integration id");
    return this.#mutate(
      context,
      "plan.announce",
      "controller:reconcile",
      { ...input },
      () => {
        const plan = this.#planRow(input.planId);
        if (plan === undefined)
          throw new ControllerError(`plan ${input.planId} does not exist`);
        if (plan.cancelled_at !== null)
          return {
            value: { announced: false },
            event: {
              entityType: "plan",
              entityId: input.planId,
              stateVersion: 0,
              details: { announced: false, kind: input.kind },
            },
          };
        let body: string;
        if (input.kind === "approved") {
          const revision = this.#database
            .prepare(
              "SELECT body_json FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
            )
            .get(this.#projectId, plan.plan_id, plan.approved_revision) as
            { body_json: string } | undefined;
          if (plan.state !== "approved" || revision === undefined)
            throw new ControllerError(`plan ${input.planId} is not approved`);
          body = planApprovedNotice(
            plan.plan_id,
            revision.body_json,
            this.#approvalNote(plan),
          );
        } else if (input.kind === "signed_off") {
          const signoff = this.#database
            .prepare(
              `SELECT s.summary, i.branch, i.head_sha FROM plan_signoffs s JOIN integrations i
                 ON i.project_id = s.project_id AND i.integration_id = s.integration_id
               WHERE s.project_id = ? AND s.plan_id = ? AND s.integration_id = ?`,
            )
            .get(this.#projectId, plan.plan_id, input.integrationId) as
            | { summary: string; branch: string; head_sha: string | null }
            | undefined;
          if (signoff === undefined)
            throw new ControllerError(
              `plan ${input.planId} is not signed off for integration ${input.integrationId}`,
            );
          body = planSignedOffNotice(
            plan.plan_id,
            input.integrationId,
            signoff.branch,
            signoff.head_sha,
            signoff.summary,
          );
        } else body = planNeedsAttentionNotice(plan.plan_id);
        const announced = this.#areas.messageNotices.noticeToPm(
          body,
          this.#now(),
        );
        return {
          value: { announced },
          event: {
            entityType: "plan",
            entityId: input.planId,
            stateVersion: 0,
            details: { announced, kind: input.kind },
          },
        };
      },
    );
  }

  /** Review rounds of a plan that finished with a verdict. */
  planReviewRounds(credential: string, planId: string): number {
    this.#kernel.authorize(credential, "plan:read");
    safeId(planId, "plan id");
    return this.#finishedPlanReviews(planId);
  }

  #finishedPlanReviews(planId: string): number {
    return (
      this.#database
        .prepare(
          "SELECT COUNT(*) AS n FROM reviews WHERE project_id = ? AND subject_plan_id = ? AND state IN ('passed', 'findings')",
        )
        .get(this.#projectId, planId) as { n: number }
    ).n;
  }

  /**
   * A plan review that could not start (the reviewer did not spawn or the review was refused): the plan goes back to
   * draft and its architect is told why. A plan that is not in review is left as it is.
   */
  abandonPlanReview(
    context: MutationContext,
    input: { readonly planId: string; readonly reason: string },
  ): PlanRecord {
    safeId(input.planId, "plan id");
    const reason = safeText(input.reason, "reason", 500, false);
    return this.#mutate<PlanRecord>(
      context,
      "plan.review_abandon",
      "plan:write",
      { planId: input.planId, reason },
      () => {
        const plan = this.#planRow(input.planId);
        if (plan === undefined)
          throw new ControllerError(`plan ${input.planId} does not exist`);
        if (plan.state === "in_review") {
          const open = this.#database
            .prepare(
              "SELECT 1 AS present FROM reviews WHERE project_id = ? AND subject_plan_id = ? AND state = 'started'",
            )
            .get(this.#projectId, input.planId);
          if (open)
            throw new ControllerError(
              `plan ${input.planId} has a review in progress`,
            );
          const now = this.#now();
          this.#database
            .prepare(
              "UPDATE plans SET state = 'draft', updated_at = ? WHERE project_id = ? AND plan_id = ?",
            )
            .run(now, this.#projectId, input.planId);
          const architect =
            plan.architect_agent_id === null
              ? undefined
              : this.#agentRow(plan.architect_agent_id);
          const parties = this.#areas.messageNotices.noticeParties();
          if (architect?.state === "active") {
            const body = `Plan ${input.planId} review could not start: ${reason}. The plan is a draft again; submit it again with cstan plan submit.`;
            this.#areas.messages.insertQueuedMessage(
              parties?.controllerActorId ?? this.#kernel.controllerActorId(),
              architect,
              body,
              sha256(body),
              now,
            );
          }
        }
        return {
          value: planRecordOf(this.#planRow(input.planId)!),
          event: {
            entityType: "plan",
            entityId: input.planId,
            stateVersion: 0,
            toState: "draft",
            details: { reason },
          },
        };
      },
    );
  }

  #planRow(planId: string): PlanRow | undefined {
    return this.#database
      .prepare("SELECT * FROM plans WHERE project_id = ? AND plan_id = ?")
      .get(this.#projectId, planId) as PlanRow | undefined;
  }

  /**
   * One line of derived state per package: the latest accepted report of the assignee's current generation decides.
   * A report counts only for the package assigned last at or before the report was accepted, so it never predates the
   * assignment it counts for and never counts for two packages (ties on assigned_at fall back to plan and package id).
   */
  #packageReport(
    planId: string,
    packageId: string,
    assigneeAgentId: string | null,
    assignedAt: string | null,
  ): { report_id: string } | undefined {
    if (assigneeAgentId === null || assignedAt === null) return undefined;
    return this.#database
      .prepare(
        `SELECT r.report_id FROM agent_reports r
         JOIN agents a ON a.project_id = r.project_id AND a.agent_id = r.agent_id AND a.generation = r.generation
         WHERE r.project_id = ? AND r.agent_id = ? AND r.state = 'accepted' AND r.created_at >= ?
           AND NOT EXISTS (
             SELECT 1 FROM plan_packages other
             WHERE other.project_id = r.project_id AND other.assignee_agent_id = r.agent_id
               AND other.assigned_at <= r.created_at
               AND (other.assigned_at, other.plan_id, other.package_id) > (?, ?, ?))
         ORDER BY r.sequence DESC LIMIT 1`,
      )
      .get(
        this.#projectId,
        assigneeAgentId,
        assignedAt,
        assignedAt,
        planId,
        packageId,
      ) as { report_id: string } | undefined;
  }

  /**
   * A plan package's report may only be integrated after the reports of its depends_on packages: those must already be
   * in a merged or confirmed integration, or come earlier in this list. Reports outside plans are not checked. Only
   * approved, uncancelled plans count, cancelled packages are not checked, and a cancelled dependency is skipped: this
   * check asks whether anything must merge first, and for it nothing will. A dependency that is not cancelled but has no
   * report yet (unassigned, or assigned and unreported) is refused, since it may still need to merge first. (The assign
   * gate asks a different question, whether the interface will exist, so there a cancelled dependency stays unmet.)
   */
  #checkIntegrationOrder(reportIds: readonly string[]): void {
    const position = new Map(reportIds.map((id, index) => [id, index]));
    const plans = this.#database
      .prepare(
        "SELECT plan_id, approved_revision FROM plans WHERE project_id = ? AND state = 'approved' AND cancelled_at IS NULL AND approved_revision IS NOT NULL",
      )
      .all(this.#projectId) as { plan_id: string; approved_revision: number }[];
    for (const plan of plans) {
      const rows = this.#database
        .prepare(
          "SELECT package_id, assignee_agent_id, assigned_at, cancelled_at FROM plan_packages WHERE project_id = ? AND plan_id = ?",
        )
        .all(this.#projectId, plan.plan_id) as {
        package_id: string;
        assignee_agent_id: string | null;
        assigned_at: string | null;
        cancelled_at: string | null;
      }[];
      const cancelled = new Set(
        rows.filter((r) => r.cancelled_at !== null).map((r) => r.package_id),
      );
      const reports = new Map(
        rows.map((row) => [
          row.package_id,
          this.#packageReport(
            plan.plan_id,
            row.package_id,
            row.assignee_agent_id,
            row.assigned_at,
          )?.report_id,
        ]),
      );
      for (const [packageId, reportId] of reports) {
        const at = reportId === undefined ? undefined : position.get(reportId);
        if (reportId === undefined || at === undefined) continue;
        if (cancelled.has(packageId)) continue;
        const revision = this.#database
          .prepare(
            "SELECT body_json FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
          )
          .get(this.#projectId, plan.plan_id, plan.approved_revision) as
          { body_json: string } | undefined;
        const view =
          revision === undefined
            ? undefined
            : packageOfBody(revision.body_json, packageId);
        for (const dep of view?.dependsOn ?? []) {
          if (cancelled.has(dep)) continue;
          const depReport = reports.get(dep);
          if (depReport === undefined)
            throw new ControllerError(
              `integration_order: report ${reportId} (package ${packageId}) needs ${dep}, which has no report yet`,
            );
          const earlier = position.get(depReport);
          if (earlier !== undefined && earlier < at) continue;
          const settled = this.#database
            .prepare(
              `SELECT 1 AS present FROM integration_reports ir
               JOIN integrations i ON i.project_id = ir.project_id AND i.integration_id = ir.integration_id
               WHERE ir.project_id = ? AND ir.report_id = ? AND i.state IN ('merged', 'confirmed')`,
            )
            .get(this.#projectId, depReport);
          if (settled) continue;
          throw new ControllerError(
            `integration_order: report ${reportId} (package ${packageId}) needs ${dep} integrated first or earlier in this list`,
          );
        }
      }
    }
  }

  #packageProgress(
    planId: string,
    packageId: string,
    assigneeAgentId: string | null,
    assignedAt: string | null,
  ): PackageProgress {
    if (assigneeAgentId === null || assignedAt === null) return "unassigned";
    const report = this.#packageReport(
      planId,
      packageId,
      assigneeAgentId,
      assignedAt,
    );
    if (report === undefined) return "assigned";
    const integrated = this.#database
      .prepare(
        `SELECT 1 AS present FROM integration_reports ir
         JOIN integrations i ON i.project_id = ir.project_id AND i.integration_id = ir.integration_id
         WHERE ir.project_id = ? AND ir.report_id = ? AND i.state IN ('merged', 'confirmed')`,
      )
      .get(this.#projectId, report.report_id);
    if (integrated) return "integrated";
    const latest = this.#database
      .prepare(
        `SELECT state FROM reviews WHERE project_id = ? AND subject_report_id = ? AND state IN ('passed', 'findings')
         ORDER BY sequence DESC LIMIT 1`,
      )
      .get(this.#projectId, report.report_id) as { state: string } | undefined;
    if (latest === undefined) return "reported";
    return latest.state === "passed" ? "reviewed" : "findings";
  }

  #planPackages(planId: string): PlanPackageRecord[] {
    return (
      this.#database
        .prepare(
          "SELECT * FROM plan_packages WHERE project_id = ? AND plan_id = ? ORDER BY package_id",
        )
        .all(this.#projectId, planId) as {
        package_id: string;
        assignee_agent_id: string | null;
        assigned_at: string | null;
        assignment_message_id: string | null;
        cancelled_at: string | null;
      }[]
    ).map((row) => ({
      packageId: row.package_id,
      assigneeAgentId: row.assignee_agent_id,
      assignedAt: row.assigned_at,
      assignmentMessageId: row.assignment_message_id,
      cancelledAt: row.cancelled_at,
      progress: this.#packageProgress(
        planId,
        row.package_id,
        row.assignee_agent_id,
        row.assigned_at,
      ),
    }));
  }

  /** The plan with its approved revision (the current one while it is not approved), packages with derived progress, and sign-offs. */
  planRecord(credential: string, planId: string): PlanDetail | undefined {
    this.#kernel.authorize(credential, "plan:read");
    safeId(planId, "plan id");
    const plan = this.#planRow(planId);
    if (plan === undefined) return undefined;
    const shown = plan.approved_revision ?? plan.current_revision;
    const revision = this.#database
      .prepare(
        "SELECT * FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
      )
      .get(this.#projectId, planId, shown) as
      | {
          revision: number;
          base_sha: string;
          body_json: string;
          body_sha: string;
          author_agent_id: string;
          created_at: string;
        }
      | undefined;
    return {
      plan: planRecordOf(plan),
      revision:
        revision === undefined
          ? null
          : {
              revision: revision.revision,
              baseSha: revision.base_sha,
              bodyJson: revision.body_json,
              bodySha: revision.body_sha,
              authorAgentId: revision.author_agent_id,
              createdAt: revision.created_at,
            },
      packages: this.#planPackages(planId),
      signoffs: (
        this.#database
          .prepare(
            "SELECT * FROM plan_signoffs WHERE project_id = ? AND plan_id = ? ORDER BY created_at, integration_id",
          )
          .all(this.#projectId, planId) as {
          integration_id: string;
          architect_agent_id: string;
          summary: string;
          created_at: string;
        }[]
      ).map((row) => ({
        integrationId: row.integration_id,
        architectAgentId: row.architect_agent_id,
        summary: row.summary,
        createdAt: row.created_at,
      })),
    };
  }

  listPlans(credential: string): readonly PlanRecord[] {
    this.#kernel.authorize(credential, "plan:read");
    return (
      this.#database
        .prepare("SELECT * FROM plans WHERE project_id = ? ORDER BY sequence")
        .all(this.#projectId) as PlanRow[]
    ).map(planRecordOf);
  }

  /**
   * Binds a package of an approved plan to an active Developer-kind agent other than the plan's architect.
   * A package is bound once: it can be bound again only when its assignee is no longer active. An agent
   * holds at most one package of a plan, so a report maps to exactly one package.
   */
  assignPackage(
    context: MutationContext,
    input: {
      readonly planId: string;
      readonly packageId: string;
      readonly agentId: string;
      readonly early?: string;
    },
  ): PlanPackageRecord & { readonly unmet: readonly string[] } {
    safeId(input.planId, "plan id");
    safeId(input.packageId, "package id");
    safeId(input.agentId, "agent id");
    const early =
      input.early === undefined
        ? undefined
        : safeText(input.early, "early reason", 500, false);
    return this.#mutate<
      PlanPackageRecord & { readonly unmet: readonly string[] }
    >(context, "plan.assign", "plan:write", { ...input }, (actor) => {
      this.assertRunNotPaused("plan assign");
      if (actor.role !== "PM" && actor.role !== "operator")
        throw new ControllerError(
          "only the PM or the operator assigns a package",
        );
      const pausedTarget = this.pauseState().agents.find(
        (pause) => pause.agentId === input.agentId,
      );
      if (pausedTarget !== undefined)
        throw new RunPausedError(
          `agent_paused: ${input.agentId} is paused and cannot be assigned a package: ${pausedTarget.reason}`,
        );
      const plan = this.#planRow(input.planId);
      if (plan === undefined)
        throw new ControllerError(`plan ${input.planId} does not exist`);
      if (plan.cancelled_at !== null)
        throw new ControllerError(
          `plan_cancelled: plan ${input.planId} was cancelled`,
        );
      if (plan.state !== "approved")
        throw new ControllerError(
          `plan ${input.planId} is ${plan.state}; packages are assigned once it is approved`,
        );
      const pkg = this.#database
        .prepare(
          "SELECT assignee_agent_id, cancelled_at FROM plan_packages WHERE project_id = ? AND plan_id = ? AND package_id = ?",
        )
        .get(this.#projectId, input.planId, input.packageId) as
        | { assignee_agent_id: string | null; cancelled_at: string | null }
        | undefined;
      if (pkg === undefined)
        throw new ControllerError(
          `plan ${input.planId} has no package ${input.packageId}`,
        );
      if (pkg.cancelled_at !== null)
        throw new ControllerError(
          `package_cancelled: package ${input.packageId} of plan ${input.planId} was cancelled`,
        );
      const agent = this.#agentRow(input.agentId);
      if (
        agent === undefined ||
        agent.state !== "active" ||
        agent.kind !== "Developer"
      )
        throw new ControllerError(
          `${input.agentId} is not an active developer-kind agent`,
        );
      if (agent.agent_id === plan.architect_agent_id)
        throw new ControllerError("the architect cannot hold a package");
      if (pkg.assignee_agent_id !== null) {
        const current = this.#agentRow(pkg.assignee_agent_id);
        if (current?.state === "active")
          throw new ControllerError(
            `package ${input.packageId} is already assigned to ${pkg.assignee_agent_id}`,
          );
      }
      const holding = this.#database
        .prepare(
          "SELECT package_id FROM plan_packages WHERE project_id = ? AND plan_id = ? AND assignee_agent_id = ? AND package_id <> ? AND cancelled_at IS NULL",
        )
        .get(this.#projectId, input.planId, agent.agent_id, input.packageId) as
        { package_id: string } | undefined;
      if (holding !== undefined)
        throw new ControllerError(
          `${agent.agent_id} already holds package ${holding.package_id} of this plan`,
        );
      const revision = this.#database
        .prepare(
          "SELECT body_json FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
        )
        .get(this.#projectId, input.planId, plan.approved_revision) as
        { body_json: string } | undefined;
      const view =
        revision === undefined
          ? undefined
          : packageOfBody(revision.body_json, input.packageId);
      if (view === undefined || plan.architect_agent_id === null)
        throw new ControllerError(
          `plan ${input.planId} has no readable text for package ${input.packageId}`,
        );
      // The gate asks whether the interface to build against will exist: a cancelled dependency stays unmet, so the PM
      // decides with --early. (Integration order asks whether anything must merge first; nothing will for a cancelled one.)
      const unmet = view.dependsOn.flatMap((dep) => {
        const row = this.#database
          .prepare(
            "SELECT assignee_agent_id, assigned_at, cancelled_at FROM plan_packages WHERE project_id = ? AND plan_id = ? AND package_id = ?",
          )
          .get(this.#projectId, input.planId, dep) as
          | {
              assignee_agent_id: string | null;
              assigned_at: string | null;
              cancelled_at: string | null;
            }
          | undefined;
        if (row === undefined) return [`${dep} (unknown)`];
        if (row.cancelled_at !== null) return [`${dep} (cancelled)`];
        const state = this.#packageProgress(
          input.planId,
          dep,
          row.assignee_agent_id,
          row.assigned_at,
        );
        return state === "reviewed" || state === "integrated"
          ? []
          : [`${dep} (${state})`];
      });
      if (unmet.length > 0 && early === undefined)
        throw new ControllerError(
          `dependencies_unmet: package ${input.packageId} depends on ${unmet.join(", ")}; assign them first, or pass --early "<reason>" to assign anyway`,
        );
      const task = workPackageMessage(
        input.planId,
        input.packageId,
        plan.architect_agent_id,
        view,
        unmet,
      );
      if (Buffer.byteLength(task, "utf8") > MAX_MESSAGE_BYTES)
        throw new ControllerError(
          `package ${input.packageId} is too large to send as one message`,
        );
      const now = this.#now();
      const messageId = this.#areas.messages.insertQueuedMessage(
        this.#kernel.controllerActorId(),
        agent,
        task,
        sha256(task),
        now,
      );
      this.#database
        .prepare(
          "UPDATE plan_packages SET assignee_agent_id = ?, assigned_at = ?, assignment_message_id = ? WHERE project_id = ? AND plan_id = ? AND package_id = ?",
        )
        .run(
          agent.agent_id,
          now,
          messageId,
          this.#projectId,
          input.planId,
          input.packageId,
        );
      this.#database
        .prepare(
          "UPDATE plans SET updated_at = ? WHERE project_id = ? AND plan_id = ?",
        )
        .run(now, this.#projectId, input.planId);
      return {
        value: {
          ...this.#planPackages(input.planId).find(
            (p) => p.packageId === input.packageId,
          )!,
          unmet: early === undefined ? [] : unmet,
        },
        event: {
          entityType: "plan",
          entityId: input.planId,
          stateVersion: 0,
          details: {
            packageId: input.packageId,
            agentId: agent.agent_id,
            ...(early !== undefined && unmet.length > 0
              ? { early, unmet }
              : {}),
          },
        },
      };
    });
  }

  /**
   * The plan's architect signs off a merged integration whose reports all belong to the plan's packages
   * and whose latest finished review passed. One sign-off per plan and integration.
   */
  recordSignoff(
    context: MutationContext,
    input: {
      readonly planId: string;
      readonly integrationId: string;
      readonly summary: string;
    },
  ): PlanSignoffRecord {
    safeId(input.planId, "plan id");
    safeId(input.integrationId, "integration id");
    const summary = safeText(input.summary, "sign-off summary", 1000, true);
    return this.#mutate<PlanSignoffRecord>(
      context,
      "plan.signoff",
      "plan:write",
      { ...input, summary },
      (actor) => {
        const agent = this.#agentByActor(actor.actorId);
        const plan = this.#planRow(input.planId);
        if (plan === undefined)
          throw new ControllerError(`plan ${input.planId} does not exist`);
        if (agent === undefined || agent.agent_id !== plan.architect_agent_id)
          throw new ControllerError("only the plan's architect signs it off");
        if (plan.state !== "approved")
          throw new ControllerError(
            `plan ${input.planId} is ${plan.state}, not approved`,
          );
        const integration = this.#database
          .prepare(
            "SELECT state, branch, head_sha FROM integrations WHERE project_id = ? AND integration_id = ?",
          )
          .get(this.#projectId, input.integrationId) as
          | { state: string; branch: string; head_sha: string | null }
          | undefined;
        if (integration === undefined)
          throw new ControllerError(
            `integration ${input.integrationId} does not exist`,
          );
        if (integration.state !== "merged")
          throw new ControllerError(
            `integration ${input.integrationId} is ${integration.state}, not merged`,
          );
        const outside = this.#database
          .prepare(
            `SELECT ir.report_id FROM integration_reports ir
             JOIN agent_reports r ON r.project_id = ir.project_id AND r.report_id = ir.report_id
             JOIN agents a ON a.project_id = r.project_id AND a.agent_id = r.agent_id
             WHERE ir.project_id = ? AND ir.integration_id = ? AND NOT EXISTS (
               SELECT 1 FROM plan_packages pp
               WHERE pp.project_id = ir.project_id AND pp.plan_id = ? AND pp.assignee_agent_id = r.agent_id AND a.generation = r.generation)
             ORDER BY ir.position LIMIT 1`,
          )
          .get(this.#projectId, input.integrationId, input.planId) as
          { report_id: string } | undefined;
        if (outside !== undefined)
          throw new ControllerError(
            `report ${outside.report_id} of the integration is not a package of plan ${input.planId}`,
          );
        const reportCount = this.#database
          .prepare(
            "SELECT COUNT(*) AS n FROM integration_reports WHERE project_id = ? AND integration_id = ?",
          )
          .get(this.#projectId, input.integrationId) as { n: number };
        if (reportCount.n === 0)
          throw new ControllerError(
            `integration ${input.integrationId} has no reports of plan ${input.planId}`,
          );
        const latest = this.#database
          .prepare(
            `SELECT state FROM reviews WHERE project_id = ? AND subject_integration_id = ? AND state IN ('passed', 'findings')
             ORDER BY sequence DESC LIMIT 1`,
          )
          .get(this.#projectId, input.integrationId) as
          { state: string } | undefined;
        if (latest?.state !== "passed")
          throw new ControllerError(
            `integration ${input.integrationId} has no passed review as its latest verdict`,
          );
        const signed = this.#database
          .prepare(
            "SELECT 1 AS present FROM plan_signoffs WHERE project_id = ? AND plan_id = ? AND integration_id = ?",
          )
          .get(this.#projectId, input.planId, input.integrationId);
        if (signed)
          throw new ControllerError(
            `plan ${input.planId} is already signed off for integration ${input.integrationId}`,
          );
        const now = this.#now();
        this.#database
          .prepare(
            `INSERT INTO plan_signoffs(project_id, plan_id, integration_id, architect_agent_id, summary, created_at)
             VALUES (?, ?, ?, ?, ?, ?)`,
          )
          .run(
            this.#projectId,
            input.planId,
            input.integrationId,
            agent.agent_id,
            summary,
            now,
          );
        this.#areas.messageNotices.noticeToPm(
          planSignedOffNotice(
            input.planId,
            input.integrationId,
            integration.branch,
            integration.head_sha,
            summary,
          ),
          now,
        );
        return {
          value: {
            integrationId: input.integrationId,
            architectAgentId: agent.agent_id,
            summary,
            createdAt: now,
          },
          event: {
            entityType: "plan",
            entityId: input.planId,
            stateVersion: 0,
            details: { integrationId: input.integrationId },
          },
        };
      },
    );
  }

  // ------------------------------------------------------------ Nexora links and cancellation

  /** The facts `wantedPackageState` needs for one package row; `planCancelled` is the plan's flag. */
  #packageFacts(
    planId: string,
    row: {
      readonly package_id: string;
      readonly assignee_agent_id: string | null;
      readonly assigned_at: string | null;
      readonly cancelled_at: string | null;
    },
    planCancelled: boolean,
  ): PackageFacts {
    const report = this.#packageReport(
      planId,
      row.package_id,
      row.assignee_agent_id,
      row.assigned_at,
    );
    const confirmed =
      report !== undefined &&
      this.#reportInConfirmedIntegration(report.report_id);
    return {
      progress: this.#packageProgress(
        planId,
        row.package_id,
        row.assignee_agent_id,
        row.assigned_at,
      ),
      cancelled: planCancelled || row.cancelled_at !== null,
      confirmed,
    };
  }

  #reportInConfirmedIntegration(reportId: string): boolean {
    return (
      this.#database
        .prepare(
          `SELECT 1 AS present FROM integration_reports ir
           JOIN integrations i ON i.project_id = ir.project_id AND i.integration_id = ir.integration_id
           WHERE ir.project_id = ? AND ir.report_id = ? AND i.state = 'confirmed'`,
        )
        .get(this.#projectId, reportId) !== undefined ||
      this.#database
        .prepare(
          `SELECT 1 AS present FROM integration_covered_reports c
           JOIN integrations i ON i.project_id = c.project_id AND i.integration_id = c.integration_id
           WHERE c.project_id = ? AND c.report_id = ? AND i.state = 'confirmed'`,
        )
        .get(this.#projectId, reportId) !== undefined
    );
  }

  #planPackageRows(planId: string): {
    package_id: string;
    assignee_agent_id: string | null;
    assigned_at: string | null;
    cancelled_at: string | null;
  }[] {
    return this.#database
      .prepare(
        "SELECT package_id, assignee_agent_id, assigned_at, cancelled_at FROM plan_packages WHERE project_id = ? AND plan_id = ? ORDER BY package_id",
      )
      .all(this.#projectId, planId) as {
      package_id: string;
      assignee_agent_id: string | null;
      assigned_at: string | null;
      cancelled_at: string | null;
    }[];
  }

  /** Splits a package ref `<plan-id>/<package-id>`; the ids themselves are checked against the ledger by the caller. */
  #packageRef(refId: string): { planId: string; packageId: string } {
    const parts = refId.split("/");
    if (parts.length !== 2)
      throw new TypeError("a package ref is <plan-id>/<package-id>");
    return {
      planId: safeId(parts[0], "plan id"),
      packageId: safeId(parts[1], "package id"),
    };
  }

  /** The wanted Nexora status of a linked item, derived from ledger facts and never stored; null when there is none (unbound requirement, superseded plan). */
  #wantedState(
    refKind: ExternalRefKind,
    refId: string,
    link:
      | {
          syncedState: NexoraState;
          boundAgentId: string | null;
          boundAt: string | null;
        }
      | undefined,
  ): NexoraState | null {
    if (refKind === "requirement") {
      if (
        link === undefined ||
        link.boundAgentId === null ||
        link.boundAt === null
      )
        return null;
      // The bound agent and the agents it replaced count, but only reports made since the binding.
      const report = this.#database
        .prepare(
          `WITH RECURSIVE chain(agent_id) AS (
             SELECT ?
             UNION
             SELECT e.entity_id FROM controller_events e JOIN chain c
               ON json_extract(e.payload_json, '$.details.successorId') = c.agent_id
             WHERE e.project_id = ? AND e.entity_type = 'agent' AND e.to_state = 'replaced')
           SELECT r.report_id FROM agent_reports r
           WHERE r.project_id = ? AND r.agent_id IN (SELECT agent_id FROM chain)
             AND r.state = 'accepted' AND r.created_at >= ?
           ORDER BY r.sequence DESC LIMIT 1`,
        )
        .get(
          link.boundAgentId,
          this.#projectId,
          this.#projectId,
          link.boundAt,
        ) as { report_id: string } | undefined;
      const verdict =
        report === undefined
          ? undefined
          : (this.#database
              .prepare(
                `SELECT state FROM reviews WHERE project_id = ? AND subject_report_id = ? AND state IN ('passed', 'findings')
                 ORDER BY sequence DESC LIMIT 1`,
              )
              .get(this.#projectId, report.report_id) as
              { state: string } | undefined);
      return wantedRequirementState({
        bound: true,
        syncedState: link.syncedState,
        reportConfirmed:
          report !== undefined &&
          this.#reportInConfirmedIntegration(report.report_id),
        reviewPassed: verdict?.state === "passed",
      });
    }
    const planId = refKind === "plan" ? refId : this.#packageRef(refId).planId;
    const plan = this.#planRow(planId);
    if (plan === undefined)
      throw new ControllerError(`plan ${planId} does not exist`);
    const planCancelled = plan.cancelled_at !== null;
    const rows = this.#planPackageRows(planId);
    if (refKind === "plan")
      return wantedPlanState({
        state: plan.state,
        cancelled: planCancelled,
        packages: rows.map((row) =>
          this.#packageFacts(planId, row, planCancelled),
        ),
      });
    const { packageId } = this.#packageRef(refId);
    const row = rows.find((r) => r.package_id === packageId);
    if (row === undefined)
      throw new ControllerError(`plan ${planId} has no package ${packageId}`);
    return wantedPackageState(this.#packageFacts(planId, row, planCancelled));
  }

  #linkRecord(row: ExternalLinkRow): ExternalLinkRecord {
    const wanted = this.#wantedState(row.ref_kind, row.ref_id, {
      syncedState: row.synced_state,
      boundAgentId: row.bound_agent_id,
      boundAt: row.bound_at,
    });
    return {
      refKind: row.ref_kind,
      refId: row.ref_id,
      system: row.system,
      externalId: row.external_id,
      syncedState: row.synced_state,
      boundAgentId: row.bound_agent_id,
      boundAt: row.bound_at,
      linkedBy: row.linked_by,
      linkedAt: row.linked_at,
      syncedAt: row.synced_at,
      wanted,
      drift: wanted !== null && wanted !== row.synced_state,
    };
  }

  #linkRow(refKind: string, refId: string): ExternalLinkRow | undefined {
    return this.#database
      .prepare(
        "SELECT * FROM external_links WHERE project_id = ? AND ref_kind = ? AND ref_id = ? AND system = 'nexora'",
      )
      .get(this.#projectId, refKind, refId) as ExternalLinkRow | undefined;
  }

  #assertLinkTarget(refKind: ExternalRefKind, refId: string): void {
    if (refKind === "requirement") return;
    if (refKind === "plan") {
      if (this.#planRow(refId) === undefined)
        throw new ControllerError(`plan ${refId} does not exist`);
      return;
    }
    const { planId, packageId } = this.#packageRef(refId);
    const exists = this.#database
      .prepare(
        "SELECT 1 AS present FROM plan_packages WHERE project_id = ? AND plan_id = ? AND package_id = ?",
      )
      .get(this.#projectId, planId, packageId);
    if (exists === undefined)
      throw new ControllerError(`plan ${planId} has no package ${packageId}`);
  }

  /**
   * Records what the PM wrote to Nexora for a requirement, a plan or a package. A new row starts at `todo` (what Nexora
   * creates) unless a state is given; an existing row only moves its synced state, and a different external id for it is
   * refused as `link_conflict` before the trigger can abort.
   */
  linkExternal(
    context: MutationContext,
    input: {
      readonly refKind: ExternalRefKind;
      readonly refId: string;
      readonly externalId: string;
      readonly syncedState?: NexoraState;
    },
  ): ExternalLinkRecord {
    if (!(EXTERNAL_REF_KINDS as readonly unknown[]).includes(input.refKind))
      throw new TypeError(
        `the link kind must be one of ${EXTERNAL_REF_KINDS.join(", ")}`,
      );
    if (input.refKind === "package") this.#packageRef(input.refId);
    else safeId(input.refId, "ref id");
    if (
      typeof input.externalId !== "string" ||
      !NEXORA_ID_PATTERN.test(input.externalId)
    )
      throw new TypeError("the external id must look like PM-47");
    if (input.syncedState !== undefined && !isNexoraState(input.syncedState))
      throw new TypeError("the synced state is not a Nexora status");
    return this.#mutate<ExternalLinkRecord>(
      context,
      "link.external",
      "plan:write",
      { ...input },
      (actor) => {
        if (actor.role !== "PM" && actor.role !== "operator")
          throw new ControllerError(
            "only the PM or the operator records a link",
          );
        this.#assertLinkTarget(input.refKind, input.refId);
        const now = this.#now();
        const existing = this.#linkRow(input.refKind, input.refId);
        if (existing === undefined)
          this.#database
            .prepare(
              `INSERT INTO external_links(project_id, ref_kind, ref_id, system, external_id, synced_state, linked_by, linked_at, synced_at)
               VALUES (?, ?, ?, 'nexora', ?, ?, ?, ?, ?)`,
            )
            .run(
              this.#projectId,
              input.refKind,
              input.refId,
              input.externalId,
              input.syncedState ?? "todo",
              actor.actorId,
              now,
              now,
            );
        else if (existing.external_id !== input.externalId)
          throw new ControllerError(
            `link_conflict: ${input.refKind} ${input.refId} is linked to ${existing.external_id}, not ${input.externalId}`,
          );
        else
          this.#database
            .prepare(
              "UPDATE external_links SET synced_state = ?, synced_at = ? WHERE project_id = ? AND ref_kind = ? AND ref_id = ? AND system = 'nexora'",
            )
            .run(
              input.syncedState ?? existing.synced_state,
              now,
              this.#projectId,
              input.refKind,
              input.refId,
            );
        return {
          value: this.#linkRecord(this.#linkRow(input.refKind, input.refId)!),
          event: {
            entityType: "external_link",
            entityId: `${input.refKind}:${input.refId}`,
            stateVersion: 0,
            details: {
              externalId: input.externalId,
              syncedState: input.syncedState ?? null,
            },
          },
        };
      },
    );
  }

  /** Binds the developer whose reports drive a small-tier requirement's wanted state; the requirement must be linked first. */
  bindRequirement(
    context: MutationContext,
    input: { readonly refId: string; readonly agentId: string },
  ): ExternalLinkRecord {
    safeId(input.refId, "requirement ref id");
    safeId(input.agentId, "agent id");
    return this.#mutate<ExternalLinkRecord>(
      context,
      "link.bind",
      "plan:write",
      { ...input },
      (actor) => {
        if (actor.role !== "PM" && actor.role !== "operator")
          throw new ControllerError(
            "only the PM or the operator binds a requirement",
          );
        const link = this.#linkRow("requirement", input.refId);
        if (link === undefined)
          throw new ControllerError(
            `requirement ${input.refId} is not linked; run link first`,
          );
        const agent = this.#agentRow(input.agentId);
        if (
          agent === undefined ||
          agent.state !== "active" ||
          agent.kind !== "Developer"
        )
          throw new ControllerError(
            `${input.agentId} is not an active developer-kind agent`,
          );
        this.#database
          .prepare(
            "UPDATE external_links SET bound_agent_id = ?, bound_at = ? WHERE project_id = ? AND ref_kind = 'requirement' AND ref_id = ? AND system = 'nexora'",
          )
          .run(agent.agent_id, this.#now(), this.#projectId, input.refId);
        return {
          value: this.#linkRecord(this.#linkRow("requirement", input.refId)!),
          event: {
            entityType: "external_link",
            entityId: `requirement:${input.refId}`,
            stateVersion: 0,
            details: { boundAgentId: agent.agent_id },
          },
        };
      },
    );
  }

  /**
   * The naming keys of a task a worker branch is named after: a `<plan-id>/<package-id>` package of a plan, or a
   * requirement ref id. The task id is the linked Nexora id, else `<plan-id>-<package-id>`, else the ref id.
   */
  taskNaming(
    credential: string,
    ref: string,
  ): {
    readonly kind: "package" | "requirement";
    readonly taskId: string;
    readonly type: string | null;
    readonly title: string;
  } {
    return this.#areas.panes.taskNaming(credential, ref);
  }

  /** Whether the agent has made any report, accepted or not. */
  agentHasReports(agentId: string): boolean {
    return this.#areas.panes.agentHasReports(agentId);
  }

  /** The active agent whose recorded branch is `branch`, other than `exceptAgentId`; undefined when none. */
  activeBranchHolder(
    branch: string,
    exceptAgentId?: string,
  ): string | undefined {
    return this.#areas.panes.activeBranchHolder(branch, exceptAgentId);
  }

  /** Records that an active agent's branch was renamed in git: the pane row now names `to`. */
  renameAgentBranch(
    context: MutationContext,
    input: {
      readonly agentId: string;
      readonly from: string;
      readonly to: string;
    },
  ): { readonly renamed: true } {
    return this.#areas.panes.renameAgentBranch(context, input);
  }

  /** Every link with its wanted state and drift flag, oldest first. */
  externalLinks(credential: string): readonly ExternalLinkRecord[] {
    this.#kernel.authorize(credential, "plan:read");
    return (
      this.#database
        .prepare(
          "SELECT * FROM external_links WHERE project_id = ? ORDER BY linked_at, ref_kind, ref_id",
        )
        .all(this.#projectId) as ExternalLinkRow[]
    ).map((row) => this.#linkRecord(row));
  }

  /** The wanted Nexora status of one ref; null when the ledger has none (an unlinked or unbound requirement, a superseded plan). */
  wantedNexoraState(
    credential: string,
    refKind: ExternalRefKind,
    refId: string,
  ): NexoraState | null {
    this.#kernel.authorize(credential, "plan:read");
    if (!(EXTERNAL_REF_KINDS as readonly unknown[]).includes(refKind))
      throw new TypeError(
        `the link kind must be one of ${EXTERNAL_REF_KINDS.join(", ")}`,
      );
    if (refKind === "package") this.#packageRef(refId);
    else safeId(refId, "ref id");
    const link = this.#linkRow(refKind, refId);
    return this.#wantedState(
      refKind,
      refId,
      link === undefined
        ? undefined
        : {
            syncedState: link.synced_state,
            boundAgentId: link.bound_agent_id,
            boundAt: link.bound_at,
          },
    );
  }

  /** The links whose last synced state differs from the wanted one: the work the PM still owes Nexora. */
  syncDrift(credential: string): readonly ExternalLinkRecord[] {
    return this.externalLinks(credential).filter((link) => link.drift);
  }

  /**
   * The extra line of the `Plan <id> approved` notice when the plan it supersedes was cancelled first: that plan
   * stays cancelled and is not marked superseded. Null otherwise.
   */
  approvalNoticeNote(planId: string): string | null {
    this.#assertOpen();
    safeId(planId, "plan id");
    const plan = this.#planRow(planId);
    return plan === undefined ? null : this.#approvalNote(plan);
  }

  #approvalNote(plan: PlanRow): string | null {
    const old =
      plan.supersedes_plan_id === null
        ? undefined
        : this.#planRow(plan.supersedes_plan_id);
    return old !== undefined && old.cancelled_at !== null
      ? `Plan ${old.plan_id} was cancelled before this plan was approved; it was not superseded`
      : null;
  }

  /**
   * Records the operator's decision to cancel a plan or one package of an approved plan. The guards run first so a
   * request fails with a named reason (`plan_cancelled`, `already_cancelled`, `plan_superseded`, `plan_not_approved`,
   * `package_confirmed`), never with a trigger abort. A plan in review loses its open review: the review is cancelled
   * and `reviewerAgentId` names the reviewer the caller still has to release. The Architect and the PM are told.
   */
  cancelPlan(
    context: MutationContext,
    input: { readonly planId: string; readonly packageId?: string },
  ): PlanCancelResult {
    safeId(input.planId, "plan id");
    if (input.packageId !== undefined) safeId(input.packageId, "package id");
    return this.#mutate<PlanCancelResult>(
      context,
      "plan.cancel",
      "plan:write",
      { ...input },
      (actor) => {
        if (actor.role !== "operator")
          throw new ControllerError("only the operator cancels a plan");
        const plan = this.#planRow(input.planId);
        if (plan === undefined)
          throw new ControllerError(`plan ${input.planId} does not exist`);
        if (plan.cancelled_at !== null)
          throw new ControllerError(
            `${input.packageId === undefined ? "already_cancelled" : "plan_cancelled"}: plan ${input.planId} was cancelled`,
          );
        const now = this.#now();
        let cancelledPackages: string[];
        let reviewId: string | null = null;
        let reviewerAgentId: string | null = null;
        if (input.packageId !== undefined) {
          if (plan.state !== "approved")
            throw new ControllerError(
              `plan_not_approved: plan ${input.planId} is ${plan.state}`,
            );
          const row = this.#planPackageRows(input.planId).find(
            (r) => r.package_id === input.packageId,
          );
          if (row === undefined)
            throw new ControllerError(
              `plan ${input.planId} has no package ${input.packageId}`,
            );
          if (row.cancelled_at !== null)
            throw new ControllerError(
              `already_cancelled: package ${input.packageId} was cancelled`,
            );
          if (this.#packageFacts(input.planId, row, false).confirmed)
            throw new ControllerError(
              `package_confirmed: package ${input.packageId} is already integrated and confirmed`,
            );
          this.#database
            .prepare(
              "UPDATE plan_packages SET cancelled_at = ? WHERE project_id = ? AND plan_id = ? AND package_id = ? AND cancelled_at IS NULL",
            )
            .run(now, this.#projectId, input.planId, input.packageId);
          cancelledPackages = [input.packageId];
        } else {
          if (plan.state === "superseded")
            throw new ControllerError(
              `plan_superseded: plan ${input.planId} was superseded`,
            );
          if (plan.state === "in_review") {
            const open = this.#database
              .prepare(
                "SELECT review_id, reviewer_agent_id FROM reviews WHERE project_id = ? AND subject_plan_id = ? AND state = 'started'",
              )
              .get(this.#projectId, input.planId) as
              { review_id: string; reviewer_agent_id: string } | undefined;
            if (open !== undefined) {
              this.#database
                .prepare(
                  "UPDATE reviews SET state = 'cancelled', failure_reason = 'plan cancelled', completed_at = ? WHERE project_id = ? AND review_id = ? AND state = 'started'",
                )
                .run(now, this.#projectId, open.review_id);
              reviewId = open.review_id;
              reviewerAgentId = open.reviewer_agent_id;
            }
            this.#database
              .prepare(
                "UPDATE plans SET state = 'draft', updated_at = ? WHERE project_id = ? AND plan_id = ?",
              )
              .run(now, this.#projectId, input.planId);
          }
          cancelledPackages = this.#planPackageRows(input.planId)
            .filter(
              (row) =>
                row.cancelled_at === null &&
                !this.#packageFacts(input.planId, row, false).confirmed,
            )
            .map((row) => row.package_id);
          for (const packageId of cancelledPackages)
            this.#database
              .prepare(
                "UPDATE plan_packages SET cancelled_at = ? WHERE project_id = ? AND plan_id = ? AND package_id = ? AND cancelled_at IS NULL",
              )
              .run(now, this.#projectId, input.planId, packageId);
          this.#database
            .prepare(
              "UPDATE plans SET cancelled_at = ?, updated_at = ? WHERE project_id = ? AND plan_id = ? AND cancelled_at IS NULL",
            )
            .run(now, now, this.#projectId, input.planId);
        }
        const subject =
          input.packageId === undefined
            ? `Plan ${input.planId} cancelled`
            : `Plan ${input.planId} package ${input.packageId} cancelled`;
        const notified = this.#queuePlanNotice(
          plan,
          `${subject} by the operator.`,
          now,
        );
        const held = this.#planPackageRows(input.planId).filter((row) =>
          cancelledPackages.includes(row.package_id),
        );
        for (const row of held) {
          const developer =
            row.assignee_agent_id === null
              ? undefined
              : this.#agentRow(row.assignee_agent_id);
          if (developer?.state !== "active") continue;
          const body = `Stop work on package ${row.package_id} of plan ${input.planId}: it was cancelled by the operator. Do not report it.`;
          this.#areas.messages.insertQueuedMessage(
            this.#kernel.controllerActorId(),
            developer,
            body,
            sha256(body),
            now,
          );
          if (!notified.includes(developer.agent_id))
            notified.push(developer.agent_id);
        }
        return {
          value: {
            planId: input.planId,
            packageId: input.packageId ?? null,
            cancelledPackages,
            reviewId,
            reviewerAgentId,
            notified,
          },
          event: {
            entityType: "plan",
            entityId: input.planId,
            stateVersion: 0,
            details: {
              packageId: input.packageId ?? null,
              cancelledPackages,
            },
          },
        };
      },
    );
  }

  /** Queues a controller notice to the plan's architect (while active) and to the sole active PM; returns the agents told. The caller owns the transaction. */
  #queuePlanNotice(plan: PlanRow, body: string, now: string): string[] {
    const parties = this.#areas.messageNotices.noticeParties();
    if (parties === undefined) return [];
    const recipients: AgentRow[] = [parties.pm];
    const architect =
      plan.architect_agent_id === null
        ? undefined
        : this.#agentRow(plan.architect_agent_id);
    if (architect?.state === "active") recipients.push(architect);
    for (const recipient of recipients)
      this.#areas.messages.insertQueuedMessage(
        parties.controllerActorId,
        recipient,
        body,
        sha256(body),
        now,
      );
    return recipients.map((r) => r.agent_id);
  }

  integration(integrationId: string): IntegrationRecord {
    this.#assertOpen();
    safeId(integrationId, "integration id");
    return this.#integrationRecord(integrationId);
  }

  /** What the squash commit of an integration says: the plan and its packages when every report is a package of one plan, and the reports in merge order. */
  integrationCommitInfo(integrationId: string): {
    readonly planId: string | null;
    readonly planTitle: string | null;
    readonly packages: readonly {
      readonly packageId: string;
      readonly type: string | null;
      readonly scope: string | null;
      readonly breaking: boolean;
    }[];
    readonly reports: readonly {
      readonly reportId: string;
      readonly agentId: string;
      readonly summary: string;
      readonly branch: string | null;
    }[];
  } {
    this.#assertOpen();
    safeId(integrationId, "integration id");
    return this.#commitInfo(this.#integrationAuthors(integrationId));
  }

  /** The commit info of reports that are not yet an integration, in the order given; unknown ids are left out. */
  plannedCommitInfo(
    reportIds: readonly string[],
  ): ReturnType<ControllerCore["integrationCommitInfo"]> {
    this.#assertOpen();
    const reports: AgentReportRow[] = [];
    for (const id of reportIds) {
      safeId(id, "report id");
      const row = this.#database
        .prepare(
          "SELECT * FROM agent_reports WHERE project_id = ? AND report_id = ?",
        )
        .get(this.#projectId, id) as AgentReportRow | undefined;
      if (row !== undefined) reports.push(row);
    }
    return this.#commitInfo(reports);
  }

  /** True when an earlier integration recorded this branch name. */
  integrationBranchRecorded(branch: string): boolean {
    this.#assertOpen();
    return (
      this.#database
        .prepare(
          "SELECT 1 AS present FROM integrations WHERE project_id = ? AND branch = ?",
        )
        .get(this.#projectId, branch) !== undefined
    );
  }

  #commitInfo(
    reports: readonly AgentReportRow[],
  ): ReturnType<ControllerCore["integrationCommitInfo"]> {
    const plans = new Map<
      string,
      { plan_id: string; title: string; approved_revision: number | null }
    >();
    for (const report of reports) {
      for (const row of this.#database
        .prepare(
          `SELECT DISTINCT pl.plan_id, pl.title, pl.approved_revision FROM plan_packages pp
           JOIN agents a ON a.project_id = pp.project_id AND a.agent_id = pp.assignee_agent_id
           JOIN plans pl ON pl.project_id = pp.project_id AND pl.plan_id = pp.plan_id
           WHERE pp.project_id = ? AND pp.assignee_agent_id = ? AND a.generation = ?`,
        )
        .all(this.#projectId, report.agent_id, report.generation) as {
        plan_id: string;
        title: string;
        approved_revision: number | null;
      }[])
        plans.set(row.plan_id, row);
    }
    const common = [...plans.values()].filter(
      (plan) =>
        !reports.some(
          (report) =>
            this.#database
              .prepare(
                `SELECT 1 FROM plan_packages pp JOIN agents a ON a.project_id = pp.project_id AND a.agent_id = pp.assignee_agent_id
                 WHERE pp.project_id = ? AND pp.plan_id = ? AND pp.assignee_agent_id = ? AND a.generation = ?`,
              )
              .get(
                this.#projectId,
                plan.plan_id,
                report.agent_id,
                report.generation,
              ) === undefined,
        ),
    );
    const plan = reports.length > 0 && common.length === 1 ? common[0]! : null;
    const packages: {
      packageId: string;
      type: string | null;
      scope: string | null;
      breaking: boolean;
    }[] = [];
    if (plan !== null && plan.approved_revision !== null) {
      const body = this.#database
        .prepare(
          "SELECT body_json FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
        )
        .get(this.#projectId, plan.plan_id, plan.approved_revision) as
        { body_json: string } | undefined;
      const agents = [...new Set(reports.map((r) => r.agent_id))];
      for (const agentId of agents) {
        const assigned = this.#database
          .prepare(
            "SELECT package_id FROM plan_packages WHERE project_id = ? AND plan_id = ? AND assignee_agent_id = ? ORDER BY package_id",
          )
          .all(this.#projectId, plan.plan_id, agentId) as {
          package_id: string;
        }[];
        for (const row of assigned) {
          if (packages.some((p) => p.packageId === row.package_id)) continue;
          packages.push({
            packageId: row.package_id,
            ...(body === undefined
              ? { type: null, scope: null, breaking: false }
              : packageNaming(body.body_json, row.package_id)),
          });
        }
      }
    }
    return {
      planId: plan?.plan_id ?? null,
      planTitle: plan?.title ?? null,
      packages,
      reports: reports.map((report) => ({
        reportId: report.report_id,
        agentId: report.agent_id,
        summary: report.summary,
        branch: report.branch,
      })),
    };
  }

  integrations(credential: string, limit = 20): readonly IntegrationRecord[] {
    this.#kernel.authorize(credential, "controller:reconcile");
    return (
      this.#database
        .prepare(
          "SELECT integration_id FROM integrations WHERE project_id = ? ORDER BY sequence DESC LIMIT ?",
        )
        .all(this.#projectId, limit) as { integration_id: string }[]
    ).map((r) => this.#integrationRecord(r.integration_id));
  }

  /** Confirmed or discarded integrations, newest first: their branch should be gone. */
  settledIntegrations(
    credential: string,
    limit = 200,
  ): readonly IntegrationRecord[] {
    this.#kernel.authorize(credential, "controller:reconcile");
    return (
      this.#database
        .prepare(
          "SELECT integration_id FROM integrations WHERE project_id = ? AND state IN ('confirmed', 'discarded') ORDER BY sequence DESC LIMIT ?",
        )
        .all(this.#projectId, limit) as { integration_id: string }[]
    ).map((r) => this.#integrationRecord(r.integration_id));
  }

  /**
   * Reports a confirmed integration may cover without having merged them: the latest accepted report of each agent
   * generation that existed when the integration began, is not a member of any confirmed integration and has no
   * coverage row for this integration yet. Empty for an integration that is not confirmed.
   */
  coverageCandidates(
    credential: string,
    integrationId: string,
  ):
    | {
        readonly headSha: string;
        readonly memberCommits: readonly string[];
        readonly reports: readonly CoverageCandidate[];
      }
    | undefined {
    this.#kernel.authorize(credential, "controller:reconcile");
    safeId(integrationId, "integration id");
    const integration = this.#integrationRow(integrationId);
    if (integration.state !== "confirmed" || integration.head_sha === null)
      return undefined;
    const rows = this.#database
      .prepare(
        `SELECT r.report_id, r.commit_sha FROM agent_reports r
         WHERE r.project_id = ? AND r.state = 'accepted' AND r.created_at <= ?
           AND NOT EXISTS (
             SELECT 1 FROM agent_reports later
             WHERE later.project_id = r.project_id AND later.agent_id = r.agent_id
               AND later.generation = r.generation AND later.state = 'accepted' AND later.sequence > r.sequence)
           AND NOT EXISTS (
             SELECT 1 FROM integration_reports ir
             JOIN integrations i ON i.project_id = ir.project_id AND i.integration_id = ir.integration_id
             WHERE ir.project_id = r.project_id AND ir.report_id = r.report_id AND i.state = 'confirmed')
           AND NOT EXISTS (
             SELECT 1 FROM integration_covered_reports c
             WHERE c.project_id = r.project_id AND c.integration_id = ? AND c.report_id = r.report_id)
         ORDER BY r.sequence`,
      )
      .all(this.#projectId, integration.created_at, integrationId) as {
      report_id: string;
      commit_sha: string;
    }[];
    return {
      headSha: integration.head_sha,
      memberCommits: this.#integrationAuthors(integrationId).map(
        (r) => r.commit_sha,
      ),
      reports: rows.map((row) => ({
        reportId: row.report_id,
        commitSha: row.commit_sha,
        integrationHeads: (
          this.#database
            .prepare(
              `SELECT i.head_sha FROM integration_reports ir
               JOIN integrations i ON i.project_id = ir.project_id AND i.integration_id = ir.integration_id
               WHERE ir.project_id = ? AND ir.report_id = ? AND i.integration_id <> ? AND i.head_sha IS NOT NULL
               ORDER BY i.sequence`,
            )
            .all(this.#projectId, row.report_id, integrationId) as {
            head_sha: string;
          }[]
        ).map((head) => head.head_sha),
      })),
    };
  }

  /** Stores which reports a confirmed integration covers; storing the same row again changes nothing. */
  recordCoveredReports(
    credential: string,
    integrationId: string,
    covered: readonly {
      readonly reportId: string;
      readonly how: "ancestor" | "tree" | "merge" | "integration";
    }[],
  ): void {
    this.#kernel.authorize(credential, "controller:reconcile");
    safeId(integrationId, "integration id");
    const insert = this.#database.prepare(
      "INSERT OR IGNORE INTO integration_covered_reports(project_id, integration_id, report_id, how) VALUES (?, ?, ?, ?)",
    );
    this.#database.transaction(() => {
      for (const row of covered)
        insert.run(this.#projectId, integrationId, row.reportId, row.how);
    })();
  }

  /** Integrations still marked running: the daemon stopped while one was merging. */
  runningIntegrations(credential: string): readonly IntegrationRecord[] {
    this.#kernel.authorize(credential, "controller:reconcile");
    return (
      this.#database
        .prepare(
          "SELECT integration_id FROM integrations WHERE project_id = ? AND state = 'running' ORDER BY sequence",
        )
        .all(this.#projectId) as { integration_id: string }[]
    ).map((r) => this.#integrationRecord(r.integration_id));
  }

  // -------------------------------------------------------------- recovery

  /**
   * An active agent whose pane is gone (the driver saw Herdr not find it three
   * times in a row). Written once per agent and generation; the agent stays
   * active, nothing is cancelled and nothing is replaced.
   */
  recordAgentLost(
    context: MutationContext,
    input: { readonly agentId: string },
  ): { readonly recorded: boolean } {
    return this.#areas.actors.recordAgentLost(context, input);
  }

  /** Whether the ledger holds a replacement of this agent. */
  isAgentReplaced(agentId: string): boolean {
    return this.#areas.actors.isAgentReplaced(agentId);
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
    return this.#areas.actors.recordAgentReplaced(context, input);
  }

  /** What a replacement of this agent is seeded with, newest entries only, in ledger order. A read; works for an ended agent. */
  agentSeed(agentId: string): AgentSeedData {
    return this.#areas.actors.agentSeed(agentId);
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
    return this.#areas.findings.raiseFinding(context, input);
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
    return this.#areas.findings.checkFinding(context, input);
  }

  /** Finding notices that are still waiting for a PM, oldest first. */
  unannouncedFindingNotices(
    credential: string,
    limit = 50,
  ): readonly AgentFindingNoticeRecord[] {
    return this.#areas.findings.unannouncedFindingNotices(credential, limit);
  }

  announceFindingNotice(
    context: MutationContext,
    noticeId: string,
  ): { readonly announced: boolean } {
    return this.#areas.findings.announceFindingNotice(context, noticeId);
  }

  /** Refuses a caller whose actor lacks the capability to observe agents; a revoked grant stops observation. */
  assertCanObserve(credential: string): void {
    return this.#areas.findings.assertCanObserve(credential);
  }

  /** Findings, newest first, with their deliveries and checks. */
  findings(credential: string, limit = 20): readonly AgentFindingRecord[] {
    return this.#areas.findings.findings(credential, limit);
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
    return this.#areas.findings.sweepFindings(
      newContext,
      deadlineSeconds,
      onError,
    );
  }

  fallbackPane(
    credential: string,
  ): { readonly workspaceId: string; readonly paneId: string } | undefined {
    return this.#areas.panes.fallbackPane(credential);
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
    return this.#areas.agents.pmRestarts(credential, agentId);
  }

  /** The active actors of a seat, so a crash between creating an actor and registering its agent can be undone. */
  seatActorIds(credential: string, seatId: string): readonly string[] {
    return this.#areas.actors.seatActorIds(credential, seatId);
  }

  enqueueMessage(
    context: MutationContext,
    input: MessageInput,
  ): { readonly messageId: string } {
    return this.#areas.messages.enqueueMessage(context, input);
  }

  message(messageId: string): MessageRecord | undefined {
    return this.#areas.messages.message(messageId);
  }

  messagesFor(agentId: string): readonly MessageRecord[] {
    return this.#areas.messages.messagesFor(agentId);
  }

  /** Unresolved messages of every agent: notified ones first (the operator's bell depends on them), then oldest first by sequence. */
  unresolvedMessages(
    credential: string,
    limit: number,
  ): {
    readonly messages: readonly MessageRecord[];
    readonly truncated: boolean;
  } {
    return this.#areas.messages.unresolvedMessages(credential, limit);
  }

  /** The newest input clears, without their text. */
  inputClears(
    credential: string,
    limit: number,
  ): readonly {
    readonly clearId: string;
    readonly messageId: string;
    readonly recordedAt: string;
  }[] {
    return this.#areas.messages.inputClears(credential, limit);
  }

  /** Wait rows that are still open; only a controller may list them. */
  openWaits(
    credential: string,
  ): readonly { readonly waitId: string; readonly agentId: string }[] {
    return this.#areas.messageNotices.openWaits(credential);
  }

  /** Who an actor is, for showing a message's sender; an actor of an earlier generation has no agent id. */
  senderOf(actorId: string): {
    readonly role: string;
    readonly agentId: string | null;
  } {
    return this.#areas.messages.senderOf(actorId);
  }

  messageRejections(): readonly MessageRejectionRecord[] {
    return this.#areas.messages.messageRejections();
  }

  agentInbox(credential: string, agentId?: string): readonly MessageRecord[] {
    return this.#areas.messages.agentInbox(credential, agentId);
  }

  pullMessage(context: MutationContext): {
    readonly message: MessageRecord | null;
  } {
    return this.#areas.messages.pullMessage(context);
  }

  /**
   * A worker reads its mail: every queued or deferred message becomes sent, in
   * sequence order, one ledger event each. It stops at the first expired or
   * failed message, which is the PM's to resolve, and pulls nothing while the
   * agent's delivery is paused. A message already sent or unacked is kept.
   */
  pullPending(credential: string): readonly MessageRecord[] {
    return this.#areas.messages.pullPending(credential);
  }

  /** What waits for the caller: its queued, deferred, sent and unacked messages. Read-only; nothing is written. */
  unreadSummary(credential: string): {
    readonly count: number;
    readonly oldestQueuedAt: string | null;
    readonly messageIds: readonly string[];
  } {
    return this.#areas.messages.unreadSummary(credential);
  }

  recordDeferral(
    context: MutationContext,
    messageId: string,
    reason: DeferralReason,
  ): MessageRecord {
    return this.#areas.messages.recordDeferral(context, messageId, reason);
  }

  recordInputClear(
    context: MutationContext,
    messageId: string,
    text: string,
  ): { readonly clearId: string } {
    return this.#areas.messages.recordInputClear(context, messageId, text);
  }

  recordSent(context: MutationContext, messageId: string): MessageRecord {
    return this.#areas.messages.recordSent(context, messageId);
  }

  recordFailure(
    context: MutationContext,
    messageId: string,
    reason: string,
  ): MessageRecord {
    return this.#areas.messages.recordFailure(context, messageId, reason);
  }

  recordNotification(
    context: MutationContext,
    messageId: string,
  ): MessageRecord {
    return this.#areas.messageNotices.recordNotification(context, messageId);
  }

  ackMessage(context: MutationContext, messageId: string): MessageRecord {
    return this.#areas.messages.ackMessage(context, messageId);
  }

  resolveMessage(
    context: MutationContext,
    messageId: string,
    decision: ResolutionDecision,
    note?: string,
  ): MessageRecord {
    return this.#areas.messages.resolveMessage(
      context,
      messageId,
      decision,
      note,
    );
  }

  beginWait(context: MutationContext): { readonly waitId: string } {
    return this.#areas.messageNotices.beginWait(context);
  }

  endWait(
    context: MutationContext,
    waitId: string,
  ): { readonly ended: boolean } {
    return this.#areas.messageNotices.endWait(context, waitId);
  }

  endWaitAsController(
    context: MutationContext,
    waitId: string,
  ): { readonly ended: boolean } {
    return this.#areas.messageNotices.endWaitAsController(context, waitId);
  }

  advanceMessaging(
    context: MutationContext,
    timers: MessagingTimers,
  ): MessagingAdvance {
    return this.#areas.messageNotices.advanceMessaging(context, timers);
  }

  /**
   * Tells the PM about stuck worker messages that have no notice yet: those that
   * went wrong while no PM was active, and any that were stuck before the
   * notices existed. Nothing is written while there is none.
   */
  queueMissingDeliveryNotices(context: MutationContext): {
    readonly queued: number;
  } {
    return this.#areas.messageNotices.queueMissingDeliveryNotices(context);
  }

  /** Tells the PM, once per deferred head message, that a dialog covers an agent's input box so messages to it wait. */
  queueInputBlockedNotice(
    context: MutationContext,
    input: {
      readonly agentId: string;
      readonly messageId: string;
      readonly blocker: InputBlocker;
    },
  ): { readonly queued: boolean } {
    return this.#areas.messageNotices.queueInputBlockedNotice(context, input);
  }

  /** Tells the PM, once per episode, of workers that have been stalled (working without activity) or blocked at a dialog for the stall time. */
  queueAttentionNotices(
    context: MutationContext,
    episodes: readonly AttentionEpisode[],
  ): { readonly queued: number } {
    return this.#areas.messageNotices.queueAttentionNotices(context, episodes);
  }

  /** Records a wake line before it is typed, so a crash after the record never types it twice. */
  recordPmWake(
    context: MutationContext,
    messageId: string,
  ): { readonly wakes: number } {
    return this.#areas.messageNotices.recordPmWake(context, messageId);
  }

  /**
   * Queues the routine check for the one active Supervisor when a worker is
   * active and the last check is at least `intervalSeconds` old. A check that
   * is still open when the next is due is cancelled and replaced, so a
   * Supervisor's queue is never blocked by its own checks.
   */
  queueSupervisionCheck(
    context: MutationContext,
    intervalSeconds: number,
  ): { readonly queued: boolean; readonly cancelled: number } {
    return this.#areas.messageNotices.queueSupervisionCheck(
      context,
      intervalSeconds,
    );
  }

  #now(): string {
    return this.#kernel.now();
  }

  #agentRow(agentId: string): AgentRow | undefined {
    return this.#kernel.agentRow(agentId);
  }

  #agentByActor(actorId: string): AgentRow | undefined {
    return this.#kernel.agentByActor(actorId);
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

  transitionRun(
    context: MutationContext,
    toState: RunState,
    reason?: string,
  ): { readonly state: RunState } {
    return this.#areas.pauses.transitionRun(context, toState, reason);
  }

  /** What is paused now: the run (while its state is paused) and each paused agent. */
  pauseState(): {
    readonly run: PauseRecord | null;
    readonly agents: readonly PauseRecord[];
  } {
    return this.#areas.pauses.pauseState();
  }

  /** True when messages to the agent are held: it is paused, or the run is paused and the agent is not the PM (which coordinates and must see notices). */
  isDeliveryPaused(agentId: string): boolean {
    return this.#areas.pauses.isDeliveryPaused(agentId);
  }

  /** Refuses a step that must not start while the run is paused. */
  assertRunNotPaused(action: string): void {
    return this.#areas.pauses.assertRunNotPaused(action);
  }

  pauseAgent(
    context: MutationContext,
    input: { readonly agentId: string; readonly reason: string },
  ): PauseRecord {
    return this.#areas.pauses.pauseAgent(context, input);
  }

  resumeAgent(
    context: MutationContext,
    input: { readonly agentId: string; readonly reason: string },
  ): PauseRecord {
    return this.#areas.pauses.resumeAgent(context, input);
  }

  /** Why supervision is degraded: the reason of the newest degraded event, or null when healthy or when that event carried none. */
  supervisionReason(credential: string): string | null {
    return this.#areas.findings.supervisionReason(credential);
  }

  /** What the live supervision loop is doing now: its active Supervisor, the newest routine check it queued and the open findings. */
  supervisionActivity(): SupervisionActivity {
    return this.#areas.findings.supervisionActivity();
  }

  #planStatusEntries(): PlanStatusEntry[] {
    return (
      this.#database
        .prepare("SELECT * FROM plans WHERE project_id = ? ORDER BY sequence")
        .all(this.#projectId) as PlanRow[]
    ).map((row) => {
      const packages: Record<string, number> = {};
      for (const entry of this.#planPackages(row.plan_id)) {
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
          this.#database
            .prepare(
              "SELECT integration_id, created_at FROM plan_signoffs WHERE project_id = ? AND plan_id = ? ORDER BY created_at, integration_id",
            )
            .all(this.#projectId, row.plan_id) as {
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

  /** Plans a restarted PM still has to follow: not cancelled, not superseded, and not signed off for an integration that is confirmed. */
  #openPlansForSummary(): NonNullable<PmRestartSummary["plans"]>[number][] {
    return (
      this.#database
        .prepare(
          `SELECT p.* FROM plans p WHERE p.project_id = ? AND p.cancelled_at IS NULL
           AND p.state IN ('draft', 'in_review', 'approved')
           AND NOT EXISTS (
             SELECT 1 FROM plan_signoffs s JOIN integrations i
               ON i.project_id = s.project_id AND i.integration_id = s.integration_id
             WHERE s.project_id = p.project_id AND s.plan_id = p.plan_id AND i.state = 'confirmed')
           ORDER BY p.sequence`,
        )
        .all(this.#projectId) as PlanRow[]
    ).map((row) => ({
      planId: row.plan_id,
      title: row.title,
      tier: row.tier,
      state: row.state,
      packages: this.#planPackages(row.plan_id).length,
      signedOff: (
        this.#database
          .prepare(
            "SELECT integration_id FROM plan_signoffs WHERE project_id = ? AND plan_id = ? ORDER BY created_at, integration_id",
          )
          .all(this.#projectId, row.plan_id) as { integration_id: string }[]
      ).map((signoff) => signoff.integration_id),
    }));
  }

  #mergedIntegrationsForSummary(): NonNullable<
    PmRestartSummary["integrations"]
  >[number][] {
    return (
      this.#database
        .prepare(
          "SELECT integration_id, branch, head_sha FROM integrations WHERE project_id = ? AND state = 'merged' ORDER BY sequence",
        )
        .all(this.#projectId) as {
        integration_id: string;
        branch: string;
        head_sha: string | null;
      }[]
    ).map((row) => ({
      integrationId: row.integration_id,
      branch: row.branch,
      headSha: row.head_sha,
    }));
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
      pause: this.pauseState(),
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
    this.#assertOpen();
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
      const controller = this.#kernel.internalPrincipal();
      for (const row of rows) {
        reconciled++;
        const preserveReportedCompletion =
          row.command_state === "completed" &&
          row.assignment_state === "reported" &&
          row.attempt_state === "reported";
        if (
          row.command_state !== "completed" &&
          !this.#kernel.isTransitionAllowed(
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
          !this.#kernel.isTransitionAllowed(
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
          !this.#kernel.isTransitionAllowed(
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
      return reconciled;
    } catch (error) {
      this.#database.exec("ROLLBACK");
      throw error;
    }
  }

  #assertOpen(): void {
    this.#kernel.assertOpen();
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

  #operatorRow(proposalId: string): OperatorProposalRow | undefined {
    return this.#database
      .prepare(
        "SELECT * FROM operator_proposals WHERE project_id = ? AND proposal_id = ?",
      )
      .get(this.#projectId, proposalId) as OperatorProposalRow | undefined;
  }

  #operatorRecord(row: OperatorProposalRow): OperatorProposalRecord {
    const run = this.#database
      .prepare(
        "SELECT * FROM operator_runs WHERE project_id = ? AND proposal_id = ?",
      )
      .get(this.#projectId, row.proposal_id) as OperatorRunRow | undefined;
    return {
      proposalId: row.proposal_id,
      sequence: row.sequence,
      kind: row.kind,
      command: row.command,
      commandSha: row.command_sha,
      reason: row.reason,
      forceRestart: row.force_restart === 1,
      proposerAgentId: row.proposer_agent_id,
      proposerActorId: row.proposer_actor_id,
      state: row.state,
      autoRule: row.auto_rule,
      decidedByActorId: row.decided_by_actor_id,
      decidedAt: row.decided_at,
      decisionNote: row.decision_note,
      createdAt: row.created_at,
      updatedAt: row.updated_at,
      run:
        run === undefined
          ? null
          : {
              proposalId: run.proposal_id,
              startedAt: run.started_at,
              finishedAt: run.finished_at,
              status: run.status,
              exitCode: run.exit_code,
              durationMs: run.duration_ms,
              outputTail: run.output_tail,
              outputTruncated: run.output_truncated === 1,
              notifiedMessageId: run.notified_message_id,
              pgid: run.pgid,
              leaderStart: run.leader_start,
              orphanClearedAt: run.orphan_cleared_at,
              fullAuto: run.full_auto === 1,
            },
      sessionGrant: this.#grantOfProposal(row.proposal_id),
    };
  }

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

  #grantOfProposal(proposalId: string): OperatorGrantRecord | null {
    const row = this.#database
      .prepare(
        "SELECT * FROM operator_grants WHERE project_id = ? AND source_proposal_id = ?",
      )
      .get(this.#projectId, proposalId) as OperatorGrantRow | undefined;
    return row === undefined ? null : this.#grantRecord(row);
  }

  #grantRow(grantId: string): OperatorGrantRow | undefined {
    return this.#database
      .prepare(
        "SELECT * FROM operator_grants WHERE project_id = ? AND grant_id = ?",
      )
      .get(this.#projectId, grantId) as OperatorGrantRow | undefined;
  }

  /** Grants that have not ended and have not passed their time cap. */
  activeOperatorGrants(): readonly OperatorGrantRecord[] {
    this.#assertOpen();
    const now = this.#now();
    return (
      this.#database
        .prepare(
          "SELECT * FROM operator_grants WHERE project_id = ? AND ended_reason IS NULL ORDER BY sequence",
        )
        .all(this.#projectId) as OperatorGrantRow[]
    )
      .filter((row) => Date.parse(row.expires_at) > Date.parse(now))
      .map((row) => this.#grantRecord(row));
  }

  /** Grants newest first, ended ones included; `limit` bounds the list. */
  listOperatorGrants(limit = 50): readonly OperatorGrantRecord[] {
    this.#assertOpen();
    return (
      this.#database
        .prepare(
          "SELECT * FROM operator_grants WHERE project_id = ? ORDER BY sequence DESC LIMIT ?",
        )
        .all(this.#projectId, limit) as OperatorGrantRow[]
    ).map((row) => this.#grantRecord(row));
  }

  operatorProposal(proposalId: string): OperatorProposalRecord | undefined {
    this.#assertOpen();
    safeId(proposalId, "proposal id");
    const row = this.#operatorRow(proposalId);
    return row === undefined ? undefined : this.#operatorRecord(row);
  }

  /** Proposals newest first, optionally of some states or of one proposer. */
  listOperatorProposals(
    filter: {
      readonly states?: readonly OperatorProposalState[];
      readonly proposerAgentId?: string;
      readonly limit?: number;
    } = {},
  ): readonly OperatorProposalRecord[] {
    this.#assertOpen();
    const states = filter.states ?? OPERATOR_PROPOSAL_STATES;
    const rows = this.#database
      .prepare(
        `SELECT * FROM operator_proposals WHERE project_id = ?
           AND state IN (${states.map(() => "?").join(", ")})
           AND (? IS NULL OR proposer_agent_id = ?)
         ORDER BY sequence DESC LIMIT ?`,
      )
      .all(
        this.#projectId,
        ...states,
        filter.proposerAgentId ?? null,
        filter.proposerAgentId ?? null,
        filter.limit ?? 50,
      ) as OperatorProposalRow[];
    return rows.map((row) => this.#operatorRecord(row));
  }

  /** Proposals of an agent that still wait for a decision, a run or the end of a run. */
  pendingOperatorProposalCount(agentId: string): number {
    this.#assertOpen();
    safeId(agentId, "agent id");
    return (
      this.#database
        .prepare(
          "SELECT COUNT(*) AS n FROM operator_proposals WHERE project_id = ? AND proposer_agent_id = ? AND state IN ('proposed', 'approved', 'running')",
        )
        .get(this.#projectId, agentId) as { n: number }
    ).n;
  }

  /** Approved proposals in approval order; the worker takes the first. */
  approvedOperatorProposals(): readonly OperatorProposalRecord[] {
    this.#assertOpen();
    return (
      this.#database
        .prepare(
          "SELECT * FROM operator_proposals WHERE project_id = ? AND state = 'approved' ORDER BY decided_at, sequence",
        )
        .all(this.#projectId) as OperatorProposalRow[]
    ).map((row) => this.#operatorRecord(row));
  }

  runningOperatorProposal(): OperatorProposalRecord | undefined {
    this.#assertOpen();
    const row = this.#database
      .prepare(
        "SELECT * FROM operator_proposals WHERE project_id = ? AND state = 'running' ORDER BY sequence LIMIT 1",
      )
      .get(this.#projectId) as OperatorProposalRow | undefined;
    return row === undefined ? undefined : this.#operatorRecord(row);
  }

  /** Abandoned runs whose process group has not been confirmed gone. */
  unclearedOperatorOrphans(): readonly {
    readonly proposalId: string;
    readonly pgid: number;
    readonly leaderStart: string | null;
  }[] {
    this.#assertOpen();
    return (
      this.#database
        .prepare(
          "SELECT proposal_id, pgid, leader_start FROM operator_runs WHERE project_id = ? AND status = 'abandoned' AND pgid IS NOT NULL AND orphan_cleared_at IS NULL ORDER BY started_at",
        )
        .all(this.#projectId) as {
        proposal_id: string;
        pgid: number;
        leader_start: string | null;
      }[]
    ).map((row) => ({
      proposalId: row.proposal_id,
      pgid: row.pgid,
      leaderStart: row.leader_start,
    }));
  }

  /** What an Operator restart waits for. Read-only. */
  busyIndicators(): {
    readonly startedReviews: number;
    readonly nonTerminalIntegrations: number;
    readonly unackedDeliveries: number;
  } {
    this.#assertOpen();
    const count = (sql: string): number =>
      (this.#database.prepare(sql).get(this.#projectId) as { n: number }).n;
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

  #operatorEvent(
    proposalId: string,
    fromState: string | undefined,
    toState: string,
    details: Record<string, unknown> = {},
  ): MutationEvent {
    return {
      entityType: "operator_proposal",
      entityId: proposalId,
      stateVersion: 0,
      ...(fromState === undefined ? {} : { fromState }),
      toState,
      details,
    };
  }

  #setOperatorState(
    row: OperatorProposalRow,
    state: OperatorProposalState,
    now: string,
  ): void {
    this.#database
      .prepare(
        "UPDATE operator_proposals SET state = ?, updated_at = ? WHERE project_id = ? AND proposal_id = ?",
      )
      .run(state, now, this.#projectId, row.proposal_id);
  }

  #operatorRunning(): OperatorProposalRow | undefined {
    return this.#database
      .prepare(
        "SELECT * FROM operator_proposals WHERE project_id = ? AND state = 'running' ORDER BY sequence LIMIT 1",
      )
      .get(this.#projectId) as OperatorProposalRow | undefined;
  }

  /**
   * Records a proposal from the Operator agent. The command-layer checks that the caller is the designated
   * Operator; here the caller must be an active Developer-kind agent. `autoRule` is the rule the controller
   * matched for an auto-approved command; the row then starts approved.
   */
  proposeOperatorAction(
    context: MutationContext,
    input: {
      readonly kind: OperatorProposalKind;
      readonly command: string;
      readonly reason: string;
      readonly forceRestart?: boolean;
      readonly autoRule?: string | null;
      readonly maxPending: number;
    },
  ): OperatorProposalRecord {
    if (input.kind !== "command" && input.kind !== "restart")
      throw new TypeError("the proposal kind must be command or restart");
    const forceRestart = input.forceRestart === true;
    if (forceRestart && input.kind !== "restart")
      throw new TypeError("only a restart proposal can carry force");
    const autoRule = input.autoRule ?? null;
    if (
      autoRule !== null &&
      input.kind !== "command" &&
      autoRule !== FULL_AUTO_RULE
    )
      throw new TypeError(
        "only a command, or any proposal under full auto, can be auto-approved",
      );
    if (!Number.isInteger(input.maxPending) || input.maxPending < 1)
      throw new TypeError("the pending limit must be a positive integer");
    const commandText =
      input.kind === "restart" ? RESTART_COMMAND_TEXT : input.command;
    const command = normalizeCommand(commandText);
    if (!command.ok)
      throw new TypeError(
        `the command must be non-empty printable ASCII of at most ${MAX_OPERATOR_COMMAND_BYTES} bytes (${command.code})`,
      );
    const reason = normalizeReason(input.reason);
    if (!reason.ok)
      throw new TypeError(
        `the reason must be non-empty printable ASCII of at most ${MAX_OPERATOR_REASON_BYTES} bytes (${reason.code})`,
      );
    const sha = commandHash({
      kind: input.kind,
      command: command.text,
      forceRestart,
    });
    const requestedGrantId = sessionRuleGrantId(autoRule);
    return this.#mutate<OperatorProposalRecord>(
      context,
      "operator.propose",
      "operator:propose",
      {
        kind: input.kind,
        commandSha: sha,
        reasonSha: sha256(reason.text),
        autoRule,
      },
      (actor) => {
        // A session grant that ended between the match and this write no longer approves anything.
        const grant =
          requestedGrantId === undefined
            ? undefined
            : this.activeOperatorGrants().find(
                (candidate) => candidate.grantId === requestedGrantId,
              );
        const effectiveRule =
          requestedGrantId !== undefined && grant === undefined
            ? null
            : (input.autoRule ?? null);
        const agent = this.#agentByActor(actor.actorId);
        if (agent?.kind !== "Developer")
          throw new ControllerError(
            "only an active developer-kind agent proposes an operator action",
          );
        const parties = this.#areas.messageNotices.noticeParties();
        if (parties === undefined && effectiveRule === null)
          throw new ControllerError(
            "no_pm: no PM is active to decide this proposal",
          );
        if (
          this.pendingOperatorProposalCount(agent.agent_id) >= input.maxPending
        )
          throw new ControllerError(
            `pending_limit: ${agent.agent_id} already has ${input.maxPending} proposals that wait for a decision or a run`,
          );
        const now = this.#now();
        const sequence = (
          this.#database
            .prepare(
              "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM operator_proposals WHERE project_id = ?",
            )
            .get(this.#projectId) as { next: number }
        ).next;
        const proposalId = `op-${sequence}`;
        this.#database
          .prepare(
            `INSERT INTO operator_proposals(project_id, proposal_id, sequence, kind, command, command_sha, reason, force_restart,
               proposer_agent_id, proposer_actor_id, state, auto_rule, decided_at, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
          )
          .run(
            this.#projectId,
            proposalId,
            sequence,
            input.kind,
            command.text,
            sha,
            reason.text,
            forceRestart ? 1 : 0,
            agent.agent_id,
            actor.actorId,
            effectiveRule === null ? "proposed" : "approved",
            effectiveRule,
            effectiveRule === null ? null : now,
            now,
            now,
          );
        const record = this.#operatorRecord(this.#operatorRow(proposalId)!);
        this.#areas.messageNotices.noticeToPm(proposalNoticeToPm(record), now);
        const fullAuto = effectiveRule === FULL_AUTO_RULE;
        return {
          value: record,
          event: this.#operatorEvent(proposalId, undefined, record.state, {
            kind: input.kind,
            autoRule: effectiveRule,
            agentId: agent.agent_id,
            ...(fullAuto ? { fullAuto: true } : {}),
          }),
          ...(grant === undefined
            ? {}
            : {
                extraEvents: [
                  {
                    entityType: "operator_grant",
                    entityId: grant.grantId,
                    stateVersion: 0,
                    toState: "used",
                    details: {
                      event: "operator.grant_used",
                      proposalId,
                      commandSha: sha,
                    },
                  },
                ],
              }),
        };
      },
    );
  }

  /**
   * Records the PM's decision. Approval is bound to the exact text: `hash` must be a prefix of at least 12
   * characters of the stored hash, and the force flag is part of that hash. The operator CLI identity may
   * deny but never approve, and no approval is taken while a run is in progress or a stray process group of
   * an abandoned run may still be alive.
   */
  decideOperatorProposal(
    context: MutationContext,
    input: {
      readonly proposalId: string;
      readonly decision: "approve" | "deny";
      readonly hash?: string;
      readonly note?: string;
      readonly proposalTtlMinutes: number;
      /** Approve and allow the same text, or the same leading words, again until the grant ends. */
      readonly session?: {
        readonly kind: OperatorGrantKind;
        readonly text?: string;
        readonly maxMinutes: number;
      };
    },
  ): OperatorProposalRecord {
    safeId(input.proposalId, "proposal id");
    if (input.decision !== "approve" && input.decision !== "deny")
      throw new TypeError("the decision must be approve or deny");
    if (input.session !== undefined && input.decision !== "approve")
      throw new TypeError("a session grant comes only with an approval");
    const note =
      input.note === undefined || input.note === ""
        ? null
        : safeText(input.note, "decision note", 1024, true);
    if (note !== null && Buffer.byteLength(note, "utf8") > 1024)
      throw new TypeError("the decision note must be at most 1024 bytes");
    return this.#mutate<OperatorProposalRecord>(
      context,
      "operator.decide",
      "operator:decide",
      {
        proposalId: input.proposalId,
        decision: input.decision,
        hash: input.hash ?? null,
        note,
        session: input.session ?? null,
      },
      (actor) => {
        const agent = this.#agentByActor(actor.actorId);
        const isPm = agent?.kind === "PM";
        if (actor.role === "operator") {
          if (input.decision === "approve")
            throw new ControllerError(
              "approve_requires_pm: only an active PM agent approves an operator proposal; the operator may deny, cancel and show",
            );
        } else if (!isPm)
          throw new ControllerError(
            "approve_requires_pm: only an active PM agent decides an operator proposal",
          );
        const row = this.#operatorRow(input.proposalId);
        if (row === undefined)
          throw new ControllerError(
            `unknown_proposal: proposal ${input.proposalId} does not exist`,
          );
        if (row.state !== "proposed")
          throw new ControllerError(
            `proposal_not_open: proposal ${row.proposal_id} is ${row.state}`,
          );
        const now = this.#now();
        if (this.#isOlderThan(row.created_at, input.proposalTtlMinutes, now))
          throw new ControllerError(
            `proposal_expired: proposal ${row.proposal_id} waited longer than ${input.proposalTtlMinutes} minutes for a decision`,
          );
        if (input.decision === "approve") {
          if (
            agent?.agent_id === row.proposer_agent_id ||
            actor.actorId === row.proposer_actor_id
          )
            throw new ControllerError(
              "self_approval: the proposer cannot approve its own proposal",
            );
          if (this.#operatorRunning() !== undefined)
            throw new ControllerError(
              "run_in_progress: an operator run is in progress; decide again when it ends",
            );
          if (this.unclearedOperatorOrphans().length > 0)
            throw new ControllerError(
              "orphan_running: a process of an abandoned operator run may still be alive; decide again when the controller has cleared it",
            );
          const hash = input.hash;
          if (
            typeof hash !== "string" ||
            !/^[0-9a-f]{12,64}$/.test(hash) ||
            !row.command_sha.startsWith(hash)
          )
            throw new ControllerError(
              `hash_mismatch: --hash must be at least ${HASH_PREFIX_CHARS} hex characters of the hash in the proposal notice for this exact text`,
            );
        }
        const state: OperatorProposalState =
          input.decision === "approve" ? "approved" : "denied";
        let grant: OperatorGrantRow | undefined;
        if (input.session !== undefined) {
          if (row.kind !== "command")
            throw new ControllerError(
              "session_not_for_restart: a restart is never granted for the session",
            );
          const text =
            input.session.kind === "exact" ? row.command : input.session.text;
          if (text === undefined)
            throw new ControllerError(
              "session_prefix_missing: --session prefix needs the leading words",
            );
          const problem =
            input.session.kind === "exact" ? null : prefixGrantProblem(text);
          if (problem !== null)
            throw new ControllerError(
              `session_prefix_refused: the prefix ${problem}`,
            );
          const own = words(row.command);
          if (
            input.session.kind === "prefix" &&
            !words(text).every((token, index) => own[index] === token)
          )
            throw new ControllerError(
              "session_prefix_mismatch: the prefix must be the start of the command being approved",
            );
          const sequence = (
            this.#database
              .prepare(
                "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM operator_grants WHERE project_id = ?",
              )
              .get(this.#projectId) as { next: number }
          ).next;
          const expiresAt = new Date(
            Date.parse(now) + input.session.maxMinutes * 60_000,
          ).toISOString();
          this.#database
            .prepare(
              `INSERT INTO operator_grants(project_id, grant_id, sequence, kind, text, command_sha, created_by, source_proposal_id, created_at, expires_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
            )
            .run(
              this.#projectId,
              `grant-${sequence}`,
              sequence,
              input.session.kind,
              text,
              sha256(text),
              actor.actorId,
              row.proposal_id,
              now,
              expiresAt,
            );
          grant = this.#grantRow(`grant-${sequence}`);
        }
        this.#database
          .prepare(
            `UPDATE operator_proposals SET state = ?, decided_by_actor_id = ?, decided_at = ?, decision_note = ?, updated_at = ?
             WHERE project_id = ? AND proposal_id = ?`,
          )
          .run(
            state,
            actor.actorId,
            now,
            note,
            now,
            this.#projectId,
            row.proposal_id,
          );
        const record = this.#operatorRecord(
          this.#operatorRow(row.proposal_id)!,
        );
        this.#areas.messageNotices.noticeToAgent(
          row.proposer_agent_id,
          decisionNotice(record, state),
          now,
        );
        return {
          value: record,
          event: this.#operatorEvent(row.proposal_id, "proposed", state, {
            decidedBy: actor.actorId,
          }),
          ...(grant === undefined
            ? {}
            : {
                extraEvents: [
                  {
                    entityType: "operator_grant",
                    entityId: grant.grant_id,
                    stateVersion: 0,
                    toState: "created",
                    details: {
                      event: "operator.grant_created",
                      kind: grant.kind,
                      text: grant.text,
                      expiresAt: grant.expires_at,
                      proposalId: row.proposal_id,
                    },
                  },
                ],
              }),
        };
      },
    );
  }

  /** Withdraws a proposal that has not started: the proposer, the PM or the operator may. */
  cancelOperatorProposal(
    context: MutationContext,
    proposalId: string,
  ): OperatorProposalRecord {
    safeId(proposalId, "proposal id");
    return this.#mutate<OperatorProposalRecord>(
      context,
      "operator.cancel",
      "operator:read",
      { proposalId },
      (actor) => {
        const agent = this.#agentByActor(actor.actorId);
        const row = this.#operatorRow(proposalId);
        if (row === undefined)
          throw new ControllerError(
            `unknown_proposal: proposal ${proposalId} does not exist`,
          );
        const isProposer = agent?.agent_id === row.proposer_agent_id;
        if (actor.role !== "operator" && agent?.kind !== "PM" && !isProposer)
          throw new ControllerError(
            "only the proposer, the PM or the operator cancels a proposal",
          );
        if (row.state !== "proposed" && row.state !== "approved")
          throw new ControllerError(
            `not_cancellable: proposal ${proposalId} is ${row.state}; only a proposal that has not started can be cancelled`,
          );
        const now = this.#now();
        this.#setOperatorState(row, "cancelled", now);
        const record = this.#operatorRecord(this.#operatorRow(proposalId)!);
        if (!isProposer)
          this.#areas.messageNotices.noticeToAgent(
            row.proposer_agent_id,
            endedWithoutRunNotice(record, "was cancelled; it was not run."),
            now,
          );
        return {
          value: record,
          event: this.#operatorEvent(proposalId, row.state, "cancelled"),
        };
      },
    );
  }

  /** The proposals of an agent that has not started, cancelled because the agent ends or is replaced. The caller owns the transaction. */
  #cancelUnstartedOperatorProposalsOf(agentId: string, now: string): void {
    for (const row of this.#database
      .prepare(
        "SELECT * FROM operator_proposals WHERE project_id = ? AND proposer_agent_id = ? AND state IN ('proposed', 'approved')",
      )
      .all(this.#projectId, agentId) as OperatorProposalRow[])
      this.#setOperatorState(row, "cancelled", now);
  }

  /** The session grants approved for an agent's proposals end when the agent is released or replaced. The caller owns the transaction. */
  #endOperatorGrantsOf(agentId: string): void {
    this.#database
      .prepare(
        `UPDATE operator_grants SET ended_reason = 'released'
         WHERE project_id = ? AND ended_reason IS NULL AND source_proposal_id IN
           (SELECT proposal_id FROM operator_proposals WHERE project_id = ? AND proposer_agent_id = ?)`,
      )
      .run(this.#projectId, this.#projectId, agentId);
  }

  /** Open grants (not ended) whose time cap has passed. Read-only. */
  dueOperatorGrantExpiries(
    alsoDue: ReadonlySet<string> = new Set(),
  ): readonly string[] {
    this.#assertOpen();
    const now = Date.parse(this.#now());
    return (
      this.#database
        .prepare(
          "SELECT * FROM operator_grants WHERE project_id = ? AND ended_reason IS NULL ORDER BY sequence",
        )
        .all(this.#projectId) as OperatorGrantRow[]
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
    this.#database
      .prepare(
        "UPDATE operator_grants SET ended_reason = ?, revoked_at = ? WHERE project_id = ? AND grant_id = ? AND ended_reason IS NULL",
      )
      .run(
        reason,
        reason === "revoked" ? now : null,
        this.#projectId,
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
    const source = this.#operatorRow(row.source_proposal_id);
    if (source === undefined) return;
    this.#areas.messageNotices.noticeToAgent(
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
    return this.#mutate<number>(
      context,
      "operator.grant_expire",
      "controller:reconcile",
      {},
      () => {
        const now = this.#now();
        const due = new Set(this.dueOperatorGrantExpiries(alsoDue));
        const events: MutationEvent[] = [];
        for (const row of this.#database
          .prepare(
            "SELECT * FROM operator_grants WHERE project_id = ? AND ended_reason IS NULL ORDER BY sequence",
          )
          .all(this.#projectId) as OperatorGrantRow[]) {
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
    return this.#mutate<number>(
      context,
      "operator.grant_restart",
      "controller:reconcile",
      {},
      () => {
        const now = this.#now();
        const events: MutationEvent[] = [];
        for (const row of this.#database
          .prepare(
            "SELECT * FROM operator_grants WHERE project_id = ? AND ended_reason IS NULL ORDER BY sequence",
          )
          .all(this.#projectId) as OperatorGrantRow[])
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
    return this.#mutate<OperatorGrantRecord>(
      context,
      "operator.grant_revoke",
      "operator:decide",
      { grantId },
      (actor) => {
        const agent = this.#agentByActor(actor.actorId);
        if (actor.role !== "operator" && agent?.kind !== "PM")
          throw new ControllerError(
            "only the PM or the operator revokes a grant",
          );
        const row = this.#grantRow(grantId);
        if (row === undefined)
          throw new ControllerError(
            `unknown_grant: grant ${grantId} does not exist`,
          );
        if (
          row.ended_reason !== null ||
          Date.parse(row.expires_at) <= Date.parse(this.#now())
        )
          throw new ControllerError(
            `grant_not_active: grant ${grantId} has already ended`,
          );
        const now = this.#now();
        const event = this.#endGrantRow(row, "revoked", now);
        this.#noticeGrantEnded(row, "revoked", now);
        return {
          value: this.#grantRecord(this.#grantRow(grantId)!),
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
    return this.#mutate<null>(
      context,
      `operator.full_auto_${input.change}`,
      byController ? "controller:reconcile" : "operator:decide",
      {
        change: input.change,
        minutes: input.minutes ?? null,
        askedUser,
      },
      (actor) => {
        const agent = this.#agentByActor(actor.actorId);
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
        const now = this.#now();
        if (input.change !== "startup") {
          const body = fullAutoNotice(input.change, input.minutes ?? null);
          this.#areas.messageNotices.noticeToPm(body, now);
          for (const operator of this.#database
            .prepare(
              "SELECT * FROM agents WHERE project_id = ? AND role_name = ? AND state = 'active'",
            )
            .all(this.#projectId, input.operatorRole) as AgentRow[])
            this.#areas.messageNotices.noticeToAgent(
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
    return this.#mutate<OperatorProposalRecord>(
      context,
      "operator.full_auto_refuse",
      "controller:reconcile",
      { proposalId },
      () => {
        const row = this.#operatorRow(proposalId);
        if (row?.state !== "approved" || row.auto_rule !== FULL_AUTO_RULE)
          throw new ControllerError(
            `proposal ${proposalId} is not waiting under full auto`,
          );
        const now = this.#now();
        this.#setOperatorState(row, "cancelled", now);
        const record = this.#operatorRecord(this.#operatorRow(proposalId)!);
        const body = endedWithoutRunNotice(
          record,
          "was not run: full auto ended before it started. Propose it again if it is still needed.",
        );
        this.#areas.messageNotices.noticeToAgent(
          row.proposer_agent_id,
          body,
          now,
        );
        this.#areas.messageNotices.noticeToPm(body, now);
        return {
          value: record,
          event: this.#operatorEvent(proposalId, "approved", "cancelled", {
            reason: "full_auto_ended",
          }),
        };
      },
    );
  }

  #isOlderThan(createdAt: string, minutes: number, nowIso: string): boolean {
    return Date.parse(createdAt) + minutes * 60_000 <= Date.parse(nowIso);
  }

  /** Proposals that are past their time to live: undecided ones past the proposal limit, approved ones past the approval limit. Read-only. */
  dueOperatorExpiries(limits: {
    readonly proposalTtlMinutes: number;
    readonly approvalTtlMinutes: number;
  }): readonly string[] {
    this.#assertOpen();
    const now = this.#now();
    return (
      this.#database
        .prepare(
          "SELECT * FROM operator_proposals WHERE project_id = ? AND state IN ('proposed', 'approved') ORDER BY sequence",
        )
        .all(this.#projectId) as OperatorProposalRow[]
    )
      .filter((row) => this.#operatorIsStale(row, limits, now))
      .map((row) => row.proposal_id);
  }

  #operatorIsStale(
    row: OperatorProposalRow,
    limits: {
      readonly proposalTtlMinutes: number;
      readonly approvalTtlMinutes: number;
    },
    now: string,
  ): boolean {
    if (row.state === "proposed")
      return this.#isOlderThan(row.created_at, limits.proposalTtlMinutes, now);
    return (
      this.#isOlderThan(row.created_at, limits.proposalTtlMinutes, now) ||
      (row.decided_at !== null &&
        this.#isOlderThan(row.decided_at, limits.approvalTtlMinutes, now))
    );
  }

  /** Marks a stale row expired and tells whoever waits: the Operator always, the PM too when an approval lapsed. The caller owns the transaction. */
  #expireOperatorRow(row: OperatorProposalRow, now: string): void {
    const behind = this.#operatorRunning()?.proposal_id ?? null;
    this.#setOperatorState(row, "expired", now);
    const record = this.#operatorRecord(this.#operatorRow(row.proposal_id)!);
    const body = expiredNotice(record, behind);
    this.#areas.messageNotices.noticeToAgent(row.proposer_agent_id, body, now);
    if (row.state === "approved")
      this.#areas.messageNotices.noticeToPm(body, now);
  }

  expireOperatorProposals(
    context: MutationContext,
    limits: {
      readonly proposalTtlMinutes: number;
      readonly approvalTtlMinutes: number;
    },
  ): readonly string[] {
    return this.#mutate<readonly string[]>(
      context,
      "operator.expire",
      "controller:reconcile",
      { ...limits },
      () => {
        const now = this.#now();
        const expired: string[] = [];
        for (const row of this.#database
          .prepare(
            "SELECT * FROM operator_proposals WHERE project_id = ? AND state IN ('proposed', 'approved') ORDER BY sequence",
          )
          .all(this.#projectId) as OperatorProposalRow[])
          if (this.#operatorIsStale(row, limits, now)) {
            this.#expireOperatorRow(row, now);
            expired.push(row.proposal_id);
          }
        return {
          value: expired,
          event: {
            entityType: "operator_proposal",
            entityId: expired[0] ?? "none",
            stateVersion: 0,
            details: { expired },
          },
        };
      },
    );
  }

  /**
   * The only way a proposal starts to run: approved -> running and the run row in one transaction, so one
   * approval executes at most once and concurrent claims have one winner. A row that is past its time to live
   * becomes expired and is not claimed.
   */
  claimOperatorRun(
    context: MutationContext,
    input: {
      readonly proposalId: string;
      readonly proposalTtlMinutes: number;
      readonly approvalTtlMinutes: number;
    },
  ): OperatorClaim {
    safeId(input.proposalId, "proposal id");
    return this.#mutate<OperatorClaim>(
      context,
      "operator.claim",
      "controller:reconcile",
      { ...input },
      () => {
        const row = this.#operatorRow(input.proposalId);
        const refuse = (
          reason: OperatorClaimRefusal,
        ): MutationOutput<OperatorClaim> => ({
          value: { claimed: false, reason },
          event: {
            entityType: "operator_proposal",
            entityId: input.proposalId,
            stateVersion: 0,
            details: { claimed: false, reason },
          },
        });
        if (row === undefined || row.state !== "approved")
          return refuse("not_approved");
        const now = this.#now();
        // A one-off decision of the PM still runs while the run is paused; a run approved by an
        // auto rule, a session-grant match or full auto does not, and the refusal is recorded.
        const paused = this.pauseState().run;
        if (paused !== null && row.auto_rule !== null) {
          const detail = `run_paused: running ${row.proposal_id} is refused while the run is paused: ${paused.reason}`;
          this.#setOperatorState(row, "cancelled", now);
          this.#areas.messageNotices.noticeToPm(
            `Operator proposal ${row.proposal_id} was approved automatically but not run: the run is paused (${paused.reason}). It is cancelled; the Operator may propose it again after resume.`,
            now,
          );
          return {
            value: { claimed: false, reason: "run_paused", detail },
            event: this.#operatorEvent(
              row.proposal_id,
              "approved",
              "cancelled",
              {
                claimed: false,
                reason: "run_paused",
                error: detail,
              },
            ),
          };
        }
        if (this.#operatorIsStale(row, input, now)) {
          this.#expireOperatorRow(row, now);
          return refuse("expired");
        }
        if (this.#operatorRunning() !== undefined) return refuse("busy");
        this.#setOperatorState(row, "running", now);
        this.#database
          .prepare(
            "INSERT INTO operator_runs(project_id, proposal_id, started_at, status, full_auto) VALUES (?, ?, ?, 'running', ?)",
          )
          .run(
            this.#projectId,
            row.proposal_id,
            now,
            row.auto_rule === FULL_AUTO_RULE ? 1 : 0,
          );
        return {
          value: {
            claimed: true,
            proposal: this.#operatorRecord(this.#operatorRow(row.proposal_id)!),
          },
          event: this.#operatorEvent(row.proposal_id, "approved", "running", {
            ...(row.auto_rule === FULL_AUTO_RULE ? { fullAuto: true } : {}),
          }),
        };
      },
    );
  }

  /** The process group of a run that started, so a later start can find a stray one. */
  recordOperatorRunProcess(
    context: MutationContext,
    input: {
      readonly proposalId: string;
      readonly pgid: number;
      readonly leaderStart: string | null;
    },
  ): null {
    safeId(input.proposalId, "proposal id");
    if (!Number.isInteger(input.pgid) || input.pgid <= 1)
      throw new TypeError("the process group must be an integer above 1");
    return this.#mutate<null>(
      context,
      "operator.process",
      "controller:reconcile",
      { ...input },
      () => {
        const changed = this.#database
          .prepare(
            "UPDATE operator_runs SET pgid = ?, leader_start = ? WHERE project_id = ? AND proposal_id = ? AND status = 'running' AND pgid IS NULL",
          )
          .run(
            input.pgid,
            input.leaderStart,
            this.#projectId,
            input.proposalId,
          ).changes;
        if (changed !== 1)
          throw new ControllerError(
            `no running run of ${input.proposalId} can record a process`,
          );
        return {
          value: null,
          event: {
            entityType: "operator_proposal",
            entityId: input.proposalId,
            stateVersion: 0,
            details: { pgid: input.pgid },
          },
        };
      },
    );
  }

  /** Ends a run with its result, tells the Operator (and the PM for an auto-approved command) and moves the proposal to its final state. */
  finishOperatorRun(
    context: MutationContext,
    input: {
      readonly proposalId: string;
      readonly status: "ok" | "failed" | "timeout" | "error";
      readonly exitCode: number | null;
      readonly durationMs: number;
      readonly outputTail: string;
      readonly truncated: boolean;
    },
  ): OperatorProposalRecord {
    safeId(input.proposalId, "proposal id");
    if (Buffer.byteLength(input.outputTail, "utf8") > MAX_OPERATOR_TAIL_BYTES)
      throw new TypeError(
        `the output tail must be at most ${MAX_OPERATOR_TAIL_BYTES} bytes`,
      );
    return this.#mutate<OperatorProposalRecord>(
      context,
      "operator.finish",
      "controller:reconcile",
      {
        proposalId: input.proposalId,
        status: input.status,
        exitCode: input.exitCode,
        durationMs: input.durationMs,
        tailSha: sha256(input.outputTail),
      },
      () => {
        const row = this.#operatorRow(input.proposalId);
        if (row?.state !== "running")
          throw new ControllerError(
            `proposal ${input.proposalId} is not running`,
          );
        const now = this.#now();
        this.#database
          .prepare(
            `UPDATE operator_runs SET status = ?, finished_at = ?, exit_code = ?, duration_ms = ?, output_tail = ?, output_truncated = ?
             WHERE project_id = ? AND proposal_id = ? AND status = 'running'`,
          )
          .run(
            input.status,
            now,
            input.exitCode,
            Math.max(0, Math.round(input.durationMs)),
            input.outputTail,
            input.truncated ? 1 : 0,
            this.#projectId,
            input.proposalId,
          );
        const state: OperatorProposalState =
          input.status === "ok"
            ? "finished"
            : input.status === "timeout"
              ? "timeout"
              : "failed";
        this.#setOperatorState(row, state, now);
        const record = this.#operatorRecord(
          this.#operatorRow(row.proposal_id)!,
        );
        this.#announceOperatorRun(record, now);
        return {
          value: this.#operatorRecord(this.#operatorRow(row.proposal_id)!),
          event: this.#operatorEvent(row.proposal_id, "running", state, {
            status: input.status,
            exitCode: input.exitCode,
            ...(row.auto_rule === FULL_AUTO_RULE ? { fullAuto: true } : {}),
          }),
        };
      },
    );
  }

  /** Sends the result of an ended run once. The caller owns the transaction. */
  #announceOperatorRun(record: OperatorProposalRecord, now: string): void {
    if (record.run === null) return;
    const body = runResultNotice(record, record.run);
    const messageId = this.#areas.messageNotices.noticeToAgent(
      record.proposerAgentId,
      body,
      now,
    );
    if (record.autoRule !== null)
      this.#areas.messageNotices.noticeToPm(body, now);
    if (messageId !== null)
      this.#database
        .prepare(
          "UPDATE operator_runs SET notified_message_id = ? WHERE project_id = ? AND proposal_id = ? AND notified_message_id IS NULL",
        )
        .run(messageId, this.#projectId, record.proposalId);
  }

  /**
   * At startup: a run that was running when the controller stopped is abandoned and never run again. A
   * restart row whose plan file exists is left alone when `skipRestartsWithPlan` says so; its result is
   * ingested by the restart recovery. Returns the abandoned runs that recorded a process group.
   */
  abandonRunningOperatorRuns(
    context: MutationContext,
    options: {
      readonly skipRestartsWithPlan?: (proposalId: string) => boolean;
    } = {},
  ): readonly {
    readonly proposalId: string;
    readonly pgid: number | null;
    readonly leaderStart: string | null;
  }[] {
    return this.#mutate(
      context,
      "operator.abandon",
      "controller:reconcile",
      {},
      () => {
        const now = this.#now();
        const abandoned: {
          proposalId: string;
          pgid: number | null;
          leaderStart: string | null;
        }[] = [];
        for (const row of this.#database
          .prepare(
            "SELECT * FROM operator_proposals WHERE project_id = ? AND state = 'running' ORDER BY sequence",
          )
          .all(this.#projectId) as OperatorProposalRow[]) {
          if (
            row.kind === "restart" &&
            options.skipRestartsWithPlan?.(row.proposal_id) === true
          )
            continue;
          const run = this.#database
            .prepare(
              "SELECT pgid, leader_start, started_at FROM operator_runs WHERE project_id = ? AND proposal_id = ?",
            )
            .get(this.#projectId, row.proposal_id) as
            | {
                pgid: number | null;
                leader_start: string | null;
                started_at: string;
              }
            | undefined;
          this.#database
            .prepare(
              `UPDATE operator_runs SET status = 'abandoned', finished_at = ?, duration_ms = ?
               WHERE project_id = ? AND proposal_id = ? AND status = 'running'`,
            )
            .run(
              now,
              Math.max(0, Date.parse(now) - Date.parse(run?.started_at ?? now)),
              this.#projectId,
              row.proposal_id,
            );
          this.#setOperatorState(row, "abandoned", now);
          this.#announceOperatorRun(
            this.#operatorRecord(this.#operatorRow(row.proposal_id)!),
            now,
          );
          abandoned.push({
            proposalId: row.proposal_id,
            pgid: run?.pgid ?? null,
            leaderStart: run?.leader_start ?? null,
          });
        }
        return {
          value: abandoned,
          event: {
            entityType: "operator_proposal",
            entityId: abandoned[0]?.proposalId ?? "none",
            stateVersion: 0,
            details: { abandoned: abandoned.map((a) => a.proposalId) },
          },
        };
      },
    );
  }

  /** Records that the process group of an abandoned run is gone (or was never ours to signal). */
  clearOperatorOrphan(context: MutationContext, proposalId: string): null {
    safeId(proposalId, "proposal id");
    return this.#mutate<null>(
      context,
      "operator.orphan_clear",
      "controller:reconcile",
      { proposalId },
      () => {
        this.#database
          .prepare(
            "UPDATE operator_runs SET orphan_cleared_at = ? WHERE project_id = ? AND proposal_id = ? AND status = 'abandoned' AND orphan_cleared_at IS NULL",
          )
          .run(this.#now(), this.#projectId, proposalId);
        return {
          value: null,
          event: {
            entityType: "operator_proposal",
            entityId: proposalId,
            stateVersion: 0,
            details: { orphanCleared: true },
          },
        };
      },
    );
  }

  // ------------------------------------------------------------ prompt relay

  /** Set once by the daemon from `[prompt_relay]`; off, no notice or status view mentions the relay. */
  configurePromptRelay(config: {
    readonly enabled: boolean;
    readonly captureTtlSeconds: number;
  }): void {
    this.#areas.promptRelay.configure(config);
  }

  get promptRelayEnabled(): boolean {
    return this.#areas.promptRelay.enabled;
  }

  /** Read-only: the stored relay when `answer` may be typed to it, else a ControllerError. Nothing is changed. */
  checkPromptAnswer(
    relayId: string,
    hash: unknown,
    answer: PromptAnswer,
  ): PromptRelayRecord {
    return this.#areas.promptRelay.checkPromptAnswer(relayId, hash, answer);
  }

  promptRelay(relayId: string): PromptRelayRecord | undefined {
    return this.#areas.promptRelay.promptRelay(relayId);
  }

  /** Relays newest first. */
  listPromptRelays(limit = 20): readonly PromptRelayRecord[] {
    return this.#areas.promptRelay.listPromptRelays(limit);
  }

  /** The status view: open captures and the last answers. */
  promptRelayStatus(): {
    readonly enabled: boolean;
    readonly openCaptures: readonly {
      readonly relayId: string;
      readonly agentId: string;
      readonly hash12: string;
      readonly state: PromptRelayState;
      readonly expiresAt: string;
    }[];
    readonly lastAnswers: readonly {
      readonly relayId: string;
      readonly agentId: string;
      readonly hash12: string;
      readonly state: PromptRelayState;
      readonly answer: PromptRelayRecord["answer"];
      readonly widensPermissions: boolean;
      readonly actorId: string | null;
      readonly at: string | null;
    }[];
  } {
    return this.#areas.promptRelay.promptRelayStatus();
  }

  /**
   * Records what the PM was shown. An older captured row of the same agent becomes expired (superseded) in
   * the same transaction; a row that is being typed refuses a new capture.
   */
  recordPromptCapture(
    context: MutationContext,
    input: { readonly prompt: CapturedPrompt },
  ): PromptRelayRecord {
    return this.#areas.promptRelay.recordPromptCapture(context, input);
  }

  /** The target of the launcher's beforeType: captured -> typing, with the answer, in one transaction that re-checks state, expiry and hash. */
  beginPromptAnswer(
    context: MutationContext,
    input: {
      readonly relayId: string;
      readonly hash: string;
      readonly answer: PromptAnswer;
    },
  ): PromptRelayRecord {
    return this.#areas.promptRelay.beginPromptAnswer(context, input);
  }

  /** A launcher refusal before any key was typed: captured -> refused, with the answer that was asked for. */
  refusePromptAnswer(
    context: MutationContext,
    input: {
      readonly relayId: string;
      readonly answer: PromptAnswer;
      readonly reason: string;
    },
  ): PromptRelayRecord {
    return this.#areas.promptRelay.refusePromptAnswer(context, input);
  }

  /**
   * Ends a row that is typing, as the controller's own bookkeeping so it still works when the PM was released
   * meanwhile: typed -> answered; refused with no key sent -> refused; any key sent -> failed.
   */
  finishPromptAnswer(
    context: MutationContext,
    input: {
      readonly relayId: string;
      readonly outcome:
        | { readonly typed: true; readonly keys: readonly string[] }
        | {
            readonly typed: false;
            readonly reason: string;
            readonly keys: readonly string[];
            /** Set when keys may have been sent that are not in `keys`. */
            readonly failed?: boolean;
          };
    },
  ): PromptRelayRecord {
    return this.#areas.promptRelay.finishPromptAnswer(context, input);
  }

  /** Read-only: whether a capture has passed its time to live. */
  hasExpiredPromptCaptures(): boolean {
    return this.#areas.promptRelay.hasExpiredPromptCaptures();
  }

  /** Captures that passed their time to live become expired. */
  expirePromptCaptures(context: MutationContext): readonly string[] {
    return this.#areas.promptRelay.expirePromptCaptures(context);
  }

  /** Startup reconciliation: a row left typing by a daemon that stopped is failed (interrupted). */
  failInterruptedPromptRelays(context: MutationContext): readonly string[] {
    return this.#areas.promptRelay.failInterruptedPromptRelays(context);
  }

  #mutate<T>(
    context: MutationContext,
    action: string,
    capability: Capability,
    payload: unknown,
    apply: (actor: AuthenticatedActor) => MutationOutput<T>,
  ): T {
    return this.#kernel.mutate(context, action, capability, payload, apply);
  }
}
