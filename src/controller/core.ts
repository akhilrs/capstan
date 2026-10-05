import {} from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import type { Database } from "./sqlite.js";
import { initializeProject } from "./bootstrap.js";
import { createAreas, type ControllerAreas } from "./areas.js";
import { ControllerKernel } from "./kernel.js";
import { authenticateActor, type AuthenticatedActor } from "./auth.js";
import {} from "./canonical.js";
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
import {} from "../operator-policy.js";
import { type FullAutoChange } from "../operator.js";
import {} from "../plans.js";
import { type ExternalRefKind, type NexoraState } from "../nexora.js";
import type {
  AgentInput,
  AgentRecord,
  Capability,
  ControllerOptions,
  Identity,
  MessageInput,
  InputBlocker,
  MessageRecord,
  MessageRejectionRecord,
  MessagingAdvance,
  MutationContext,
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
import {} from "./types.js";
import {
  type CapturedPrompt,
  type PromptAnswer,
} from "../herdr/prompt-relay.js";
import { ControllerError } from "./errors.js";
import {
  type AgentPaneInput,
  type AgentPaneRecord,
  type ReportEvidence,
  type AgentReportRecord,
  type ReportResult,
  type ReviewRecord,
  type PlanTier,
  type PlanRecord,
  type PlanPackageRecord,
  type ExternalLinkRecord,
  type PlanCancelResult,
  type PlanNoticeRef,
  type PlanSignoffRecord,
  type PlanDetail,
  type CoverageCandidate,
  type IntegrationRecord,
  type IntegrationOutcome,
  type AgentFindingRecord,
  type AgentFindingNoticeRecord,
  type AgentSeedData,
  type PmRestartSummary,
  type AgentRow,
  type OperatorClaim,
  type MutationOutput,
  type ActorInput,
  type ReadinessResult,
  type SupervisionActivity,
  type ControllerStatus,
} from "./records.js";
import {} from "./helpers.js";

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
    this.#areas = createAreas(kernel);
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
        initializeProject(database, project);
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
      core.#areas.reconcile.reconcileUncertainAssignments();
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
    return this.#areas.reports.recordAgentReport(context, input);
  }

  /** The accepted report of this exact commit by this agent generation, if there is one. */
  acceptedReportFor(
    agentId: string,
    generation: number,
    commitSha: string,
  ): AgentReportRecord | undefined {
    return this.#areas.reports.acceptedReportFor(
      agentId,
      generation,
      commitSha,
    );
  }

  /** Accepted reports whose PM notice has not been queued yet (no PM was active when they arrived). */
  unannouncedReports(
    credential: string,
    limit = 50,
  ): readonly AgentReportRecord[] {
    return this.#areas.reports.unannouncedReports(credential, limit);
  }

  /** Queues the PM notice for one accepted report, once. */
  announceReport(
    context: MutationContext,
    reportId: string,
  ): { readonly announced: boolean } {
    return this.#areas.reports.announceReport(context, reportId);
  }

  agentReports(credential: string, limit = 20): readonly AgentReportRecord[] {
    return this.#areas.reports.agentReports(credential, limit);
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
    return this.#areas.reviews.checkReviewRequest(subjectId, reviewerRole);
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
    return this.#areas.reviews.beginReview(context, input);
  }

  /** The reviewer's verdict, written once. The PM notice is queued in the same transaction when exactly one PM is active. */
  completeReview(
    context: MutationContext,
    input: { readonly verdict: "pass" | "findings"; readonly text: string },
  ): ReviewRecord {
    return this.#areas.reviews.completeReview(context, input);
  }

  /** Finished reviews whose PM notice has not been queued. */
  unannouncedReviews(credential: string, limit = 50): readonly ReviewRecord[] {
    return this.#areas.reviews.unannouncedReviews(credential, limit);
  }

  announceReview(
    context: MutationContext,
    reviewId: string,
  ): { readonly announced: boolean } {
    return this.#areas.reviews.announceReview(context, reviewId);
  }

  reviews(credential: string, limit = 20): readonly ReviewRecord[] {
    return this.#areas.reviews.reviews(credential, limit);
  }

  /** Finished reviews whose reviewer agent is still active: the reviewer was not released (a crash after the verdict). */
  reviewsToRelease(credential: string): readonly ReviewRecord[] {
    return this.#areas.reviews.reviewsToRelease(credential);
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
    return this.#areas.integrations.beginIntegration(context, input);
  }

  /** Records how a running integration ended. A conflict is also queued to the PM when the operator asked for the integration. */
  finishIntegration(
    context: MutationContext,
    input: {
      readonly integrationId: string;
      readonly outcome: IntegrationOutcome;
    },
  ): IntegrationRecord {
    return this.#areas.integrations.finishIntegration(context, input);
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
    return this.#areas.integrations.settleIntegration(context, input);
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
    return this.#areas.plans.openPlan(context, input);
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
    return this.#areas.plans.submitPlan(context, input);
  }

  /**
   * Plan notices the PM has not received (no PM was active when the plan was approved or ran out of review rounds).
   * A notice counts as sent when a controller message with its leading text exists.
   */
  unannouncedPlanNotices(credential: string): readonly PlanNoticeRef[] {
    return this.#areas.plans.unannouncedPlanNotices(credential);
  }

  /** Queues one missing plan notice to the PM; false when no PM is the sole active one. */
  announcePlanNotice(
    context: MutationContext,
    input: PlanNoticeRef,
  ): { readonly announced: boolean } {
    return this.#areas.plans.announcePlanNotice(context, input);
  }

  /** Review rounds of a plan that finished with a verdict. */
  planReviewRounds(credential: string, planId: string): number {
    return this.#areas.plans.planReviewRounds(credential, planId);
  }

  /**
   * A plan review that could not start (the reviewer did not spawn or the review was refused): the plan goes back to
   * draft and its architect is told why. A plan that is not in review is left as it is.
   */
  abandonPlanReview(
    context: MutationContext,
    input: { readonly planId: string; readonly reason: string },
  ): PlanRecord {
    return this.#areas.plans.abandonPlanReview(context, input);
  }

  /** The plan with its approved revision (the current one while it is not approved), packages with derived progress, and sign-offs. */
  planRecord(credential: string, planId: string): PlanDetail | undefined {
    return this.#areas.plans.planRecord(credential, planId);
  }

  listPlans(credential: string): readonly PlanRecord[] {
    return this.#areas.plans.listPlans(credential);
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
    return this.#areas.planPackages.assignPackage(context, input);
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
    return this.#areas.planPackages.recordSignoff(context, input);
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
    return this.#areas.links.linkExternal(context, input);
  }

  /** Binds the developer whose reports drive a small-tier requirement's wanted state; the requirement must be linked first. */
  bindRequirement(
    context: MutationContext,
    input: { readonly refId: string; readonly agentId: string },
  ): ExternalLinkRecord {
    return this.#areas.links.bindRequirement(context, input);
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
    return this.#areas.links.taskNaming(credential, ref);
  }

  /** Whether the agent has made any report, accepted or not. */
  agentHasReports(agentId: string): boolean {
    return this.#areas.links.agentHasReports(agentId);
  }

  /** The active agent whose recorded branch is `branch`, other than `exceptAgentId`; undefined when none. */
  activeBranchHolder(
    branch: string,
    exceptAgentId?: string,
  ): string | undefined {
    return this.#areas.links.activeBranchHolder(branch, exceptAgentId);
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
    return this.#areas.links.renameAgentBranch(context, input);
  }

  /** Every link with its wanted state and drift flag, oldest first. */
  externalLinks(credential: string): readonly ExternalLinkRecord[] {
    return this.#areas.links.externalLinks(credential);
  }

  /** The wanted Nexora status of one ref; null when the ledger has none (an unlinked or unbound requirement, a superseded plan). */
  wantedNexoraState(
    credential: string,
    refKind: ExternalRefKind,
    refId: string,
  ): NexoraState | null {
    return this.#areas.links.wantedNexoraState(credential, refKind, refId);
  }

  /** The links whose last synced state differs from the wanted one: the work the PM still owes Nexora. */
  syncDrift(credential: string): readonly ExternalLinkRecord[] {
    return this.#areas.links.syncDrift(credential);
  }

  /**
   * The extra line of the `Plan <id> approved` notice when the plan it supersedes was cancelled first: that plan
   * stays cancelled and is not marked superseded. Null otherwise.
   */
  approvalNoticeNote(planId: string): string | null {
    return this.#areas.plans.approvalNoticeNote(planId);
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
    return this.#areas.planPackages.cancelPlan(context, input);
  }

  integration(integrationId: string): IntegrationRecord {
    return this.#areas.integrations.integration(integrationId);
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
    return this.#areas.integrations.integrationCommitInfo(integrationId);
  }

  /** The commit info of reports that are not yet an integration, in the order given; unknown ids are left out. */
  plannedCommitInfo(
    reportIds: readonly string[],
  ): ReturnType<ControllerCore["integrationCommitInfo"]> {
    return this.#areas.integrations.plannedCommitInfo(reportIds);
  }

  /** True when an earlier integration recorded this branch name. */
  integrationBranchRecorded(branch: string): boolean {
    return this.#areas.integrations.integrationBranchRecorded(branch);
  }

  integrations(credential: string, limit = 20): readonly IntegrationRecord[] {
    return this.#areas.integrations.integrations(credential, limit);
  }

  /** Confirmed or discarded integrations, newest first: their branch should be gone. */
  settledIntegrations(
    credential: string,
    limit = 200,
  ): readonly IntegrationRecord[] {
    return this.#areas.integrations.settledIntegrations(credential, limit);
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
    return this.#areas.integrations.coverageCandidates(
      credential,
      integrationId,
    );
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
    return this.#areas.integrations.recordCoveredReports(
      credential,
      integrationId,
      covered,
    );
  }

  /** Integrations still marked running: the daemon stopped while one was merging. */
  runningIntegrations(credential: string): readonly IntegrationRecord[] {
    return this.#areas.integrations.runningIntegrations(credential);
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
   * An agent reads its mail (the PM too): every queued or deferred message becomes sent, in
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
    readonly actionNeeded: number;
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
    return this.#areas.status.readiness(workItemId);
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

  statusSnapshot(): ControllerStatus {
    return this.#areas.status.statusSnapshot();
  }

  inspect(id: string): unknown {
    return this.#areas.status.inspect(id);
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

  #assertOpen(): void {
    this.#kernel.assertOpen();
  }

  /** Grants that have not ended and have not passed their time cap. */
  activeOperatorGrants(): readonly OperatorGrantRecord[] {
    return this.#areas.operatorGrants.activeOperatorGrants();
  }

  /** Grants newest first, ended ones included; `limit` bounds the list. */
  listOperatorGrants(limit = 50): readonly OperatorGrantRecord[] {
    return this.#areas.operatorGrants.listOperatorGrants(limit);
  }

  operatorProposal(proposalId: string): OperatorProposalRecord | undefined {
    return this.#areas.operatorProposals.operatorProposal(proposalId);
  }

  /** Proposals newest first, optionally of some states or of one proposer. */
  listOperatorProposals(
    filter: {
      readonly states?: readonly OperatorProposalState[];
      readonly proposerAgentId?: string;
      readonly limit?: number;
    } = {},
  ): readonly OperatorProposalRecord[] {
    return this.#areas.operatorProposals.listOperatorProposals(filter);
  }

  /** Proposals of an agent that still wait for a decision, a run or the end of a run. */
  pendingOperatorProposalCount(agentId: string): number {
    return this.#areas.operatorProposals.pendingOperatorProposalCount(agentId);
  }

  /** Approved proposals in approval order; the worker takes the first. */
  approvedOperatorProposals(): readonly OperatorProposalRecord[] {
    return this.#areas.operatorProposals.approvedOperatorProposals();
  }

  runningOperatorProposal(): OperatorProposalRecord | undefined {
    return this.#areas.operatorProposals.runningOperatorProposal();
  }

  /** Abandoned runs whose process group has not been confirmed gone. */
  unclearedOperatorOrphans(): readonly {
    readonly proposalId: string;
    readonly pgid: number;
    readonly leaderStart: string | null;
  }[] {
    return this.#areas.operatorRuns.unclearedOperatorOrphans();
  }

  /** What an Operator restart waits for. Read-only. */
  busyIndicators(): {
    readonly startedReviews: number;
    readonly nonTerminalIntegrations: number;
    readonly unackedDeliveries: number;
  } {
    return this.#areas.status.busyIndicators();
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
    return this.#areas.operatorProposals.proposeOperatorAction(context, input);
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
    return this.#areas.operatorProposals.decideOperatorProposal(context, input);
  }

  /** Withdraws a proposal that has not started: the proposer, the PM or the operator may. */
  cancelOperatorProposal(
    context: MutationContext,
    proposalId: string,
  ): OperatorProposalRecord {
    return this.#areas.operatorProposals.cancelOperatorProposal(
      context,
      proposalId,
    );
  }

  /** Open grants (not ended) whose time cap has passed. Read-only. */
  dueOperatorGrantExpiries(
    alsoDue: ReadonlySet<string> = new Set(),
  ): readonly string[] {
    return this.#areas.operatorGrants.dueOperatorGrantExpiries(alsoDue);
  }

  /** Ends every grant that is past its cap; the operator tick calls it. */
  expireOperatorGrants(
    context: MutationContext,
    alsoDue: ReadonlySet<string> = new Set(),
  ): number {
    return this.#areas.operatorGrants.expireOperatorGrants(context, alsoDue);
  }

  /** At startup no grant survives: a controller restart ends every open grant. */
  endOperatorGrantsForRestart(context: MutationContext): number {
    return this.#areas.operatorGrants.endOperatorGrantsForRestart(context);
  }

  /** The PM or the operator CLI ends one grant at once. */
  revokeOperatorGrant(
    context: MutationContext,
    grantId: string,
  ): OperatorGrantRecord {
    return this.#areas.operatorGrants.revokeOperatorGrant(context, grantId);
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
    return this.#areas.operatorGrants.recordOperatorFullAuto(context, input);
  }

  /** An approved proposal that has not started is ended because full auto went off before it began. */
  endFullAutoProposal(
    context: MutationContext,
    proposalId: string,
  ): OperatorProposalRecord {
    return this.#areas.operatorGrants.endFullAutoProposal(context, proposalId);
  }

  /** Proposals that are past their time to live: undecided ones past the proposal limit, approved ones past the approval limit. Read-only. */
  dueOperatorExpiries(limits: {
    readonly proposalTtlMinutes: number;
    readonly approvalTtlMinutes: number;
  }): readonly string[] {
    return this.#areas.operatorProposals.dueOperatorExpiries(limits);
  }

  expireOperatorProposals(
    context: MutationContext,
    limits: {
      readonly proposalTtlMinutes: number;
      readonly approvalTtlMinutes: number;
    },
  ): readonly string[] {
    return this.#areas.operatorProposals.expireOperatorProposals(
      context,
      limits,
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
    return this.#areas.operatorRuns.claimOperatorRun(context, input);
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
    return this.#areas.operatorRuns.recordOperatorRunProcess(context, input);
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
    return this.#areas.operatorRuns.finishOperatorRun(context, input);
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
    return this.#areas.operatorRuns.abandonRunningOperatorRuns(
      context,
      options,
    );
  }

  /** Records that the process group of an abandoned run is gone (or was never ours to signal). */
  clearOperatorOrphan(context: MutationContext, proposalId: string): null {
    return this.#areas.operatorRuns.clearOperatorOrphan(context, proposalId);
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
