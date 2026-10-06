import type { ControllerKernel } from "./kernel.js";
import type { ControllerAreas } from "./areas.js";
import { sha256 } from "./canonical.js";
import type { MutationContext } from "./types.js";
import { ControllerError } from "./errors.js";
import {
  MAX_REVIEW_ROUNDS,
  type PlanTier,
  type PlanRecord,
  type PlanNoticeRef,
  type PlanDetail,
  type PlanRow,
  type PmRestartSummary,
  type AgentRow,
} from "./records.js";
import {
  safeId,
  safeText,
  planNeedsAttentionNotice,
  planSignedOffNotice,
  planApprovedNotice,
  planRecordOf,
  planBodyPackageIds,
} from "./helpers.js";

export class PlansArea {
  constructor(
    readonly kernel: ControllerKernel,
    readonly areas: ControllerAreas,
  ) {}

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
    return this.kernel.mutate<PlanRecord>(
      context,
      "plan.open",
      "plan:write",
      { tier: input.tier, title, supersedesPlanId: supersedes },
      (actor) => {
        if (actor.role !== "PM" && actor.role !== "operator")
          throw new ControllerError("only the PM or the operator opens a plan");
        if (supersedes !== null) {
          const old = this.planRow(supersedes);
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
        const now = this.kernel.now();
        const sequence = (
          this.kernel.database
            .prepare(
              "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM plans WHERE project_id = ?",
            )
            .get(this.kernel.projectId) as { next: number }
        ).next;
        const planId = `plan-${sequence}`;
        this.kernel.database
          .prepare(
            `INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, supersedes_plan_id, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, 'draft', ?, ?, ?, ?)`,
          )
          .run(
            this.kernel.projectId,
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
          value: planRecordOf(this.planRow(planId)!),
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
    return this.kernel.mutate<PlanRecord>(
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
        const agent = this.kernel.agentByActor(actor.actorId);
        if (agent?.kind !== "Developer")
          throw new ControllerError(
            "only an active developer-kind agent submits a plan",
          );
        const plan = this.planRow(input.planId);
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
        const now = this.kernel.now();
        const revision = plan.current_revision + 1;
        this.kernel.database
          .prepare(
            `INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
          )
          .run(
            this.kernel.projectId,
            input.planId,
            revision,
            input.baseSha,
            input.bodyJson,
            sha256(input.bodyJson),
            agent.agent_id,
            actor.actorId,
            now,
          );
        this.kernel.database
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
            this.kernel.projectId,
            input.planId,
          );
        if (!input.review) this.settlePlanApproval(plan, input.bodyJson, now);
        return {
          value: planRecordOf(this.planRow(input.planId)!),
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
  settlePlanApproval(plan: PlanRow, bodyJson: string, now: string): void {
    for (const packageId of planBodyPackageIds(bodyJson))
      this.kernel.database
        .prepare(
          "INSERT INTO plan_packages(project_id, plan_id, package_id) VALUES (?, ?, ?)",
        )
        .run(this.kernel.projectId, plan.plan_id, packageId);
    if (plan.supersedes_plan_id !== null) {
      const old = this.planRow(plan.supersedes_plan_id);
      if (old === undefined || old.cancelled_at === null) {
        if (old?.state !== "approved")
          throw new ControllerError(
            `plan ${plan.supersedes_plan_id} is no longer approved and cannot be superseded`,
          );
        this.kernel.database
          .prepare(
            "UPDATE plans SET state = 'superseded', updated_at = ? WHERE project_id = ? AND plan_id = ?",
          )
          .run(now, this.kernel.projectId, plan.supersedes_plan_id);
      }
    }
    this.areas.messageNotices.noticeToPm(
      planApprovedNotice(plan.plan_id, bodyJson, this.#approvalNote(plan)),
      now,
      true,
    );
  }

  /**
   * Plan notices the PM has not received (no PM was active when the plan was approved or ran out of review rounds).
   * A notice counts as sent when a controller message with its leading text exists.
   */
  unannouncedPlanNotices(credential: string): readonly PlanNoticeRef[] {
    this.kernel.authorize(credential, "controller:reconcile");
    const sent = (planId: string, lead: string): boolean => {
      const text = `Plan ${planId} ${lead}`;
      return (
        this.kernel.database
          .prepare(
            "SELECT 1 AS present FROM messages WHERE project_id = ? AND substr(body, 1, ?) = ?",
          )
          .get(this.kernel.projectId, text.length, text) !== undefined
      );
    };
    const out: PlanNoticeRef[] = [];
    for (const plan of this.kernel.database
      .prepare(
        "SELECT * FROM plans WHERE project_id = ? AND state IN ('approved', 'draft') ORDER BY sequence",
      )
      .all(this.kernel.projectId) as PlanRow[]) {
      if (plan.cancelled_at !== null) continue;
      if (plan.state === "approved") {
        if (!sent(plan.plan_id, "approved"))
          out.push({ planId: plan.plan_id, kind: "approved" });
      } else if (
        this.finishedPlanReviews(plan.plan_id) >= MAX_REVIEW_ROUNDS &&
        !sent(plan.plan_id, "needs attention")
      )
        out.push({ planId: plan.plan_id, kind: "needs_attention" });
    }
    for (const signoff of this.kernel.database
      .prepare(
        `SELECT s.plan_id, s.integration_id FROM plan_signoffs s JOIN plans p
           ON p.project_id = s.project_id AND p.plan_id = s.plan_id
         WHERE s.project_id = ? AND p.cancelled_at IS NULL ORDER BY p.sequence, s.created_at, s.integration_id`,
      )
      .all(this.kernel.projectId) as {
      plan_id: string;
      integration_id: string;
    }[]) {
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
    return this.kernel.mutate(
      context,
      "plan.announce",
      "controller:reconcile",
      { ...input },
      () => {
        const plan = this.planRow(input.planId);
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
          const revision = this.kernel.database
            .prepare(
              "SELECT body_json FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
            )
            .get(
              this.kernel.projectId,
              plan.plan_id,
              plan.approved_revision,
            ) as { body_json: string } | undefined;
          if (plan.state !== "approved" || revision === undefined)
            throw new ControllerError(`plan ${input.planId} is not approved`);
          body = planApprovedNotice(
            plan.plan_id,
            revision.body_json,
            this.#approvalNote(plan),
          );
        } else if (input.kind === "signed_off") {
          const signoff = this.kernel.database
            .prepare(
              `SELECT s.summary, i.branch, i.head_sha, i.state FROM plan_signoffs s JOIN integrations i
                 ON i.project_id = s.project_id AND i.integration_id = s.integration_id
               WHERE s.project_id = ? AND s.plan_id = ? AND s.integration_id = ?`,
            )
            .get(this.kernel.projectId, plan.plan_id, input.integrationId) as
            | {
                summary: string;
                branch: string;
                head_sha: string | null;
                state: string;
              }
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
            signoff.state === "confirmed",
            this.areas.planPackages.signoffExtraReports(
              plan.plan_id,
              input.integrationId,
            ),
          );
        } else body = planNeedsAttentionNotice(plan.plan_id);
        const announced = this.areas.messageNotices.noticeToPm(
          body,
          this.kernel.now(),
          true,
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
    this.kernel.authorize(credential, "plan:read");
    safeId(planId, "plan id");
    return this.finishedPlanReviews(planId);
  }

  finishedPlanReviews(planId: string): number {
    return (
      this.kernel.database
        .prepare(
          "SELECT COUNT(*) AS n FROM reviews WHERE project_id = ? AND subject_plan_id = ? AND state IN ('passed', 'findings')",
        )
        .get(this.kernel.projectId, planId) as { n: number }
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
    return this.kernel.mutate<PlanRecord>(
      context,
      "plan.review_abandon",
      "plan:write",
      { planId: input.planId, reason },
      () => {
        const plan = this.planRow(input.planId);
        if (plan === undefined)
          throw new ControllerError(`plan ${input.planId} does not exist`);
        if (plan.state === "in_review") {
          const open = this.kernel.database
            .prepare(
              "SELECT 1 AS present FROM reviews WHERE project_id = ? AND subject_plan_id = ? AND state = 'started'",
            )
            .get(this.kernel.projectId, input.planId);
          if (open)
            throw new ControllerError(
              `plan ${input.planId} has a review in progress`,
            );
          const now = this.kernel.now();
          this.kernel.database
            .prepare(
              "UPDATE plans SET state = 'draft', updated_at = ? WHERE project_id = ? AND plan_id = ?",
            )
            .run(now, this.kernel.projectId, input.planId);
          const architect =
            plan.architect_agent_id === null
              ? undefined
              : this.kernel.agentRow(plan.architect_agent_id);
          const parties = this.areas.messageNotices.noticeParties();
          if (architect?.state === "active") {
            const body = `Plan ${input.planId} review could not start: ${reason}. The plan is a draft again; submit it again with cstan plan submit.`;
            this.areas.messages.insertQueuedMessage(
              parties?.controllerActorId ?? this.kernel.controllerActorId(),
              architect,
              body,
              sha256(body),
              now,
              false,
            );
          }
        }
        return {
          value: planRecordOf(this.planRow(input.planId)!),
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

  planRow(planId: string): PlanRow | undefined {
    return this.kernel.database
      .prepare("SELECT * FROM plans WHERE project_id = ? AND plan_id = ?")
      .get(this.kernel.projectId, planId) as PlanRow | undefined;
  }

  /** The plan with its approved revision (the current one while it is not approved), packages with derived progress, and sign-offs. */
  planRecord(credential: string, planId: string): PlanDetail | undefined {
    this.kernel.authorize(credential, "plan:read");
    safeId(planId, "plan id");
    const plan = this.planRow(planId);
    if (plan === undefined) return undefined;
    const shown = plan.approved_revision ?? plan.current_revision;
    const revision = this.kernel.database
      .prepare(
        "SELECT * FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
      )
      .get(this.kernel.projectId, planId, shown) as
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
      packages: this.areas.planPackages.planPackages(planId),
      signoffs: (
        this.kernel.database
          .prepare(
            "SELECT * FROM plan_signoffs WHERE project_id = ? AND plan_id = ? ORDER BY created_at, integration_id",
          )
          .all(this.kernel.projectId, planId) as {
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
        extraReports: this.areas.planPackages.signoffExtraReports(
          planId,
          row.integration_id,
        ),
      })),
    };
  }

  listPlans(credential: string): readonly PlanRecord[] {
    this.kernel.authorize(credential, "plan:read");
    return (
      this.kernel.database
        .prepare("SELECT * FROM plans WHERE project_id = ? ORDER BY sequence")
        .all(this.kernel.projectId) as PlanRow[]
    ).map(planRecordOf);
  }

  /**
   * The extra line of the `Plan <id> approved` notice when the plan it supersedes was cancelled first: that plan
   * stays cancelled and is not marked superseded. Null otherwise.
   */
  approvalNoticeNote(planId: string): string | null {
    this.kernel.assertOpen();
    safeId(planId, "plan id");
    const plan = this.planRow(planId);
    return plan === undefined ? null : this.#approvalNote(plan);
  }

  #approvalNote(plan: PlanRow): string | null {
    const old =
      plan.supersedes_plan_id === null
        ? undefined
        : this.planRow(plan.supersedes_plan_id);
    return old !== undefined && old.cancelled_at !== null
      ? `Plan ${old.plan_id} was cancelled before this plan was approved; it was not superseded`
      : null;
  }

  /** Queues a controller notice to the plan's architect (while active) and to the sole active PM; returns the agents told. The caller owns the transaction. */
  queuePlanNotice(
    plan: PlanRow,
    body: string,
    now: string,
    actionNeeded: boolean,
  ): string[] {
    const parties = this.areas.messageNotices.noticeParties();
    if (parties === undefined) return [];
    const recipients: AgentRow[] = [parties.pm];
    const architect =
      plan.architect_agent_id === null
        ? undefined
        : this.kernel.agentRow(plan.architect_agent_id);
    if (architect?.state === "active") recipients.push(architect);
    for (const recipient of recipients)
      this.areas.messages.insertQueuedMessage(
        parties.controllerActorId,
        recipient,
        body,
        sha256(body),
        now,
        actionNeeded,
      );
    return recipients.map((r) => r.agent_id);
  }

  /** Plans a restarted PM still has to follow: not cancelled, not superseded, and not signed off for an integration that is confirmed. */
  openPlansForSummary(): NonNullable<PmRestartSummary["plans"]>[number][] {
    return (
      this.kernel.database
        .prepare(
          `SELECT p.* FROM plans p WHERE p.project_id = ? AND p.cancelled_at IS NULL
           AND p.state IN ('draft', 'in_review', 'approved')
           AND NOT EXISTS (
             SELECT 1 FROM plan_signoffs s JOIN integrations i
               ON i.project_id = s.project_id AND i.integration_id = s.integration_id
             WHERE s.project_id = p.project_id AND s.plan_id = p.plan_id AND i.state = 'confirmed')
           ORDER BY p.sequence`,
        )
        .all(this.kernel.projectId) as PlanRow[]
    ).map((row) => ({
      planId: row.plan_id,
      title: row.title,
      tier: row.tier,
      state: row.state,
      packages: this.areas.planPackages.planPackages(row.plan_id).length,
      signedOff: (
        this.kernel.database
          .prepare(
            "SELECT integration_id FROM plan_signoffs WHERE project_id = ? AND plan_id = ? ORDER BY created_at, integration_id",
          )
          .all(this.kernel.projectId, row.plan_id) as {
          integration_id: string;
        }[]
      ).map((signoff) => signoff.integration_id),
    }));
  }
}
