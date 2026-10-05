import type { ControllerKernel } from "./kernel.js";
import type { ControllerAreas } from "./areas.js";
import { randomUUID } from "node:crypto";
import { sha256 } from "./canonical.js";
import type { MutationContext } from "./types.js";
import { ControllerError } from "./errors.js";
import {
  type ReportEvidence,
  type AgentReportRow,
  MAX_REVIEW_TEXT_BYTES,
  MAX_REVIEW_ROUNDS,
  type ReviewRecord,
  type ReviewRow,
  type IntegrationRow,
} from "./records.js";
import {
  safeId,
  reviewRecord,
  reviewTask,
  reviewNotice,
  planNeedsAttentionNotice,
  packageReviewedNotice,
} from "./helpers.js";

export class ReviewsArea {
  constructor(
    readonly kernel: ControllerKernel,
    readonly areas: ControllerAreas,
  ) {}

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
    this.kernel.assertOpen();
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
    const report = this.kernel.database
      .prepare(
        "SELECT * FROM agent_reports WHERE project_id = ? AND report_id = ?",
      )
      .get(this.kernel.projectId, subjectId) as AgentReportRow | undefined;
    const integration =
      report === undefined
        ? (this.kernel.database
            .prepare(
              "SELECT * FROM integrations WHERE project_id = ? AND integration_id = ?",
            )
            .get(this.kernel.projectId, subjectId) as
            IntegrationRow | undefined)
        : undefined;
    const planRow =
      report === undefined && integration === undefined
        ? this.areas.plans.planRow(subjectId)
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
      const revision = this.kernel.database
        .prepare(
          "SELECT * FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
        )
        .get(
          this.kernel.projectId,
          planRow.plan_id,
          planRow.current_revision,
        ) as
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
      authors = this.integrationAuthors(integration!.integration_id);
    }
    if (typeof baseSha !== "string" || !/^[0-9a-f]{40}$/.test(baseSha))
      throw new ControllerError("the subject has no usable base commit");
    const open = this.kernel.database
      .prepare(
        `SELECT 1 AS present FROM reviews WHERE project_id = ? AND ${column} = ? AND state = 'started'`,
      )
      .get(this.kernel.projectId, subjectId);
    if (open)
      throw new ControllerError("a review of this subject is already open");
    const done = (
      this.kernel.database
        .prepare(
          `SELECT COUNT(*) AS n FROM reviews WHERE project_id = ? AND ${column} = ? AND state IN ('passed', 'findings')`,
        )
        .get(this.kernel.projectId, subjectId) as { n: number }
    ).n;
    if (done >= MAX_REVIEW_ROUNDS)
      throw new ControllerError("the review limit for this subject is reached");
    const role = this.kernel.database
      .prepare(
        "SELECT kind FROM role_definitions WHERE project_id = ? AND role_name = ? AND state = 'active'",
      )
      .get(this.kernel.projectId, reviewerRole) as { kind: string } | undefined;
    if (role?.kind !== "Verifier")
      throw new ControllerError(
        "the reviewer role must be an active Verifier role",
      );
    const round = (
      this.kernel.database
        .prepare(
          `SELECT COALESCE(MAX(round), 0) + 1 AS next FROM reviews WHERE project_id = ? AND ${column} = ?`,
        )
        .get(this.kernel.projectId, subjectId) as { next: number }
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
  integrationAuthors(integrationId: string): AgentReportRow[] {
    return this.kernel.database
      .prepare(
        `SELECT r.* FROM integration_reports ir
         JOIN agent_reports r ON r.project_id = ir.project_id AND r.report_id = ir.report_id
         WHERE ir.project_id = ? AND ir.integration_id = ? ORDER BY ir.position`,
      )
      .all(this.kernel.projectId, integrationId) as AgentReportRow[];
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
    return this.kernel.mutate<ReviewRecord>(
      context,
      "review.begin",
      "review:request",
      { ...input },
      (actor) => {
        const checked = this.#reviewChecks(input.subjectId, input.reviewerRole);
        const reviewer = this.kernel.agentRow(input.reviewerAgentId);
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
        const controller = this.kernel.database
          .prepare(
            "SELECT actor_id FROM actors WHERE project_id = ? AND is_internal = 1 AND role = 'controller' AND active = 1 AND revoked_at IS NULL",
          )
          .get(this.kernel.projectId) as { actor_id: string } | undefined;
        if (controller === undefined)
          throw new ControllerError("the controller actor is missing");
        const now = this.kernel.now();
        const reviewId = randomUUID();
        const sequence = (
          this.kernel.database
            .prepare(
              "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM reviews WHERE project_id = ?",
            )
            .get(this.kernel.projectId) as { next: number }
        ).next;
        this.kernel.database
          .prepare(
            `INSERT INTO reviews(project_id, review_id, sequence, round, subject_report_id, subject_integration_id, subject_plan_id, subject_plan_revision,
               commit_sha, base_sha, author_agent_id, author_actor_id, requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, state, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'started', ?)`,
          )
          .run(
            this.kernel.projectId,
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
        const row = this.kernel.database
          .prepare(
            "SELECT * FROM reviews WHERE project_id = ? AND review_id = ?",
          )
          .get(this.kernel.projectId, reviewId) as ReviewRow;
        const task = reviewTask(
          row,
          checked.authors.map((report) => ({ report })),
        );
        this.areas.messages.insertQueuedMessage(
          controller.actor_id,
          reviewer,
          task,
          sha256(task),
          now,
          false,
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
    return this.kernel.mutate<ReviewRecord>(
      context,
      "review.complete",
      "review:submit",
      { verdict: input.verdict, textHash: sha256(input.text) },
      (actor) => {
        // By agent, not by actor: a replaced generation gets a new actor but is the same reviewer.
        const caller = this.kernel.agentByActor(actor.actorId);
        const row =
          caller === undefined
            ? undefined
            : (this.kernel.database
                .prepare(
                  "SELECT * FROM reviews WHERE project_id = ? AND reviewer_agent_id = ? AND state = 'started'",
                )
                .get(this.kernel.projectId, caller.agent_id) as
                ReviewRow | undefined);
        if (row === undefined)
          throw new ControllerError("you have no review in progress");
        const now = this.kernel.now();
        this.kernel.database
          .prepare(
            "UPDATE reviews SET state = ?, verdict_text = ?, completed_at = ? WHERE project_id = ? AND review_id = ?",
          )
          .run(
            input.verdict === "pass" ? "passed" : "findings",
            input.text,
            now,
            this.kernel.projectId,
            row.review_id,
          );
        this.kernel.touchAgent(row.reviewer_agent_id, now);
        if (row.subject_plan_id !== null)
          this.#settlePlanReview(row, input.verdict, now);
        else if (input.verdict === "pass" && row.subject_report_id !== null)
          this.#noticePackageReviewed(row, now);
        this.#announceReview(row.review_id, now);
        const done = this.kernel.database
          .prepare(
            "SELECT * FROM reviews WHERE project_id = ? AND review_id = ?",
          )
          .get(this.kernel.projectId, row.review_id) as ReviewRow;
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
    const plan = this.areas.plans.planRow(row.subject_plan_id!);
    if (plan?.state !== "in_review" || plan.cancelled_at !== null) return;
    if (verdict === "findings") {
      this.kernel.database
        .prepare(
          "UPDATE plans SET state = 'draft', updated_at = ? WHERE project_id = ? AND plan_id = ?",
        )
        .run(now, this.kernel.projectId, plan.plan_id);
      if (
        this.areas.plans.finishedPlanReviews(plan.plan_id) >= MAX_REVIEW_ROUNDS
      )
        this.areas.messageNotices.noticeToPm(
          planNeedsAttentionNotice(plan.plan_id),
          now,
          true,
        );
      return;
    }
    const revision = this.kernel.database
      .prepare(
        "SELECT body_json FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
      )
      .get(this.kernel.projectId, plan.plan_id, row.subject_plan_revision) as {
      body_json: string;
    };
    this.kernel.database
      .prepare(
        "UPDATE plans SET state = 'approved', approved_revision = ?, updated_at = ? WHERE project_id = ? AND plan_id = ?",
      )
      .run(row.subject_plan_revision, now, this.kernel.projectId, plan.plan_id);
    this.areas.plans.settlePlanApproval(plan, revision.body_json, now);
  }

  /**
   * Tells the PM that a package's report passed review, once per report, when the package's plan has a `plan` link:
   * this is the one status change the PM has no other signal for. A cancelled plan or package is not announced.
   */
  #noticePackageReviewed(row: ReviewRow, now: string): void {
    const candidates = this.kernel.database
      .prepare(
        `SELECT p.plan_id, p.package_id, p.assignee_agent_id, p.assigned_at FROM plan_packages p
         JOIN plans pl ON pl.project_id = p.project_id AND pl.plan_id = p.plan_id
         JOIN external_links l ON l.project_id = p.project_id AND l.ref_kind = 'plan' AND l.ref_id = p.plan_id AND l.system = 'nexora'
         WHERE p.project_id = ? AND p.assignee_agent_id = ? AND p.cancelled_at IS NULL AND pl.cancelled_at IS NULL
         ORDER BY p.plan_id, p.package_id`,
      )
      .all(this.kernel.projectId, row.author_agent_id) as {
      plan_id: string;
      package_id: string;
      assignee_agent_id: string;
      assigned_at: string;
    }[];
    const match = candidates.find(
      (c) =>
        this.areas.planPackages.packageReport(
          c.plan_id,
          c.package_id,
          c.assignee_agent_id,
          c.assigned_at,
        )?.report_id === row.subject_report_id,
    );
    if (match === undefined) return;
    const body = packageReviewedNotice(
      match.plan_id,
      match.package_id,
      row.subject_report_id!,
      row.commit_sha,
    );
    const sent = this.kernel.database
      .prepare(
        "SELECT 1 AS present FROM messages WHERE project_id = ? AND body = ?",
      )
      .get(this.kernel.projectId, body);
    if (sent === undefined)
      this.areas.messageNotices.noticeToPm(body, now, true);
  }

  #reviewAuthorIds(row: ReviewRow): string[] {
    if (row.subject_plan_id !== null) return [row.author_agent_id!];
    return row.subject_integration_id === null
      ? [row.author_agent_id!]
      : this.integrationAuthors(row.subject_integration_id).map(
          (r) => r.agent_id,
        );
  }

  /** Queues the notice for a finished review that has none, to the agent that requested it (the architect) and to the PM when the requester is the PM, the operator or no longer active; false when no PM is the sole active one. The caller owns the transaction. */
  #announceReview(reviewId: string, now: string): boolean {
    const row = this.kernel.database
      .prepare(
        "SELECT * FROM reviews WHERE project_id = ? AND review_id = ? AND state IN ('passed', 'findings') AND notified_message_id IS NULL",
      )
      .get(this.kernel.projectId, reviewId) as ReviewRow | undefined;
    if (row === undefined) return false;
    const requester = this.kernel.agentByActor(row.requested_by_actor_id);
    const parties = this.areas.messageNotices.noticeParties();
    const recipient =
      requester?.state === "active" && requester.kind === "Developer"
        ? requester
        : parties?.pm;
    if (recipient === undefined) return false;
    const body = reviewNotice(row, this.#reviewAuthorIds(row));
    const messageId = this.areas.messages.insertQueuedMessage(
      parties?.controllerActorId ?? this.kernel.controllerActorId(),
      recipient,
      body,
      sha256(body),
      now,
      false,
    );
    this.kernel.database
      .prepare(
        "UPDATE reviews SET notified_message_id = ? WHERE project_id = ? AND review_id = ? AND notified_message_id IS NULL",
      )
      .run(messageId, this.kernel.projectId, reviewId);
    return true;
  }

  /** Finished reviews whose PM notice has not been queued. */
  unannouncedReviews(credential: string, limit = 50): readonly ReviewRecord[] {
    this.kernel.authorize(credential, "controller:reconcile");
    return (
      this.kernel.database
        .prepare(
          "SELECT * FROM reviews WHERE project_id = ? AND state IN ('passed', 'findings') AND notified_message_id IS NULL ORDER BY sequence LIMIT ?",
        )
        .all(this.kernel.projectId, limit) as ReviewRow[]
    ).map(reviewRecord);
  }

  announceReview(
    context: MutationContext,
    reviewId: string,
  ): { readonly announced: boolean } {
    safeId(reviewId, "review id");
    return this.kernel.mutate(
      context,
      "review.announce",
      "controller:reconcile",
      { reviewId },
      () => {
        const announced = this.#announceReview(reviewId, this.kernel.now());
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
    this.kernel.authorize(credential, "controller:reconcile");
    return (
      this.kernel.database
        .prepare(
          "SELECT * FROM reviews WHERE project_id = ? ORDER BY sequence DESC LIMIT ?",
        )
        .all(this.kernel.projectId, limit) as ReviewRow[]
    ).map(reviewRecord);
  }

  /** Finished reviews whose reviewer agent is still active: the reviewer was not released (a crash after the verdict). */
  reviewsToRelease(credential: string): readonly ReviewRecord[] {
    this.kernel.authorize(credential, "controller:reconcile");
    return (
      this.kernel.database
        .prepare(
          `SELECT r.* FROM reviews r JOIN agents a ON a.project_id = r.project_id AND a.agent_id = r.reviewer_agent_id
           WHERE r.project_id = ? AND r.state IN ('passed', 'findings') AND a.state = 'active' ORDER BY r.sequence`,
        )
        .all(this.kernel.projectId) as ReviewRow[]
    ).map(reviewRecord);
  }
}
