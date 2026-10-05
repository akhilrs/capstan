import type { ControllerKernel } from "./kernel.js";
import type { ControllerAreas } from "./areas.js";
import { sha256 } from "./canonical.js";
import { packageOfBody, workPackageMessage } from "../plans.js";
import { type PackageFacts } from "../nexora.js";
import type { MutationContext } from "./types.js";
import { ControllerError, RunPausedError } from "./errors.js";
import {
  MAX_MESSAGE_BYTES,
  type PackageProgress,
  type PlanPackageRecord,
  type PlanCancelResult,
  type PlanSignoffRecord,
} from "./records.js";
import {
  safeId,
  safeText,
  planSignedOffNotice,
  planCancelledNotice,
} from "./helpers.js";

export class PlanPackagesArea {
  constructor(
    readonly kernel: ControllerKernel,
    readonly areas: ControllerAreas,
  ) {}

  /**
   * One line of derived state per package: the latest accepted report of the assignee's current generation decides.
   * A report counts only for the package assigned last at or before the report was accepted, so it never predates the
   * assignment it counts for and never counts for two packages (ties on assigned_at fall back to plan and package id).
   */
  packageReport(
    planId: string,
    packageId: string,
    assigneeAgentId: string | null,
    assignedAt: string | null,
  ): { report_id: string } | undefined {
    if (assigneeAgentId === null || assignedAt === null) return undefined;
    return this.kernel.database
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
        this.kernel.projectId,
        assigneeAgentId,
        assignedAt,
        assignedAt,
        planId,
        packageId,
      ) as { report_id: string } | undefined;
  }

  #packageProgress(
    planId: string,
    packageId: string,
    assigneeAgentId: string | null,
    assignedAt: string | null,
  ): PackageProgress {
    if (assigneeAgentId === null || assignedAt === null) return "unassigned";
    const report = this.packageReport(
      planId,
      packageId,
      assigneeAgentId,
      assignedAt,
    );
    if (report === undefined) return "assigned";
    const integrated = this.kernel.database
      .prepare(
        `SELECT 1 AS present FROM integration_reports ir
         JOIN integrations i ON i.project_id = ir.project_id AND i.integration_id = ir.integration_id
         WHERE ir.project_id = ? AND ir.report_id = ? AND i.state IN ('merged', 'confirmed')`,
      )
      .get(this.kernel.projectId, report.report_id);
    if (integrated) return "integrated";
    const latest = this.kernel.database
      .prepare(
        `SELECT state FROM reviews WHERE project_id = ? AND subject_report_id = ? AND state IN ('passed', 'findings')
         ORDER BY sequence DESC LIMIT 1`,
      )
      .get(this.kernel.projectId, report.report_id) as
      { state: string } | undefined;
    if (latest === undefined) return "reported";
    return latest.state === "passed" ? "reviewed" : "findings";
  }

  planPackages(planId: string): PlanPackageRecord[] {
    return (
      this.kernel.database
        .prepare(
          "SELECT * FROM plan_packages WHERE project_id = ? AND plan_id = ? ORDER BY package_id",
        )
        .all(this.kernel.projectId, planId) as {
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
    return this.kernel.mutate<
      PlanPackageRecord & { readonly unmet: readonly string[] }
    >(context, "plan.assign", "plan:write", { ...input }, (actor) => {
      this.areas.pauses.assertRunNotPaused("plan assign");
      if (actor.role !== "PM" && actor.role !== "operator")
        throw new ControllerError(
          "only the PM or the operator assigns a package",
        );
      const pausedTarget = this.areas.pauses
        .pauseState()
        .agents.find((pause) => pause.agentId === input.agentId);
      if (pausedTarget !== undefined)
        throw new RunPausedError(
          `agent_paused: ${input.agentId} is paused and cannot be assigned a package: ${pausedTarget.reason}`,
        );
      const plan = this.areas.plans.planRow(input.planId);
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
      const pkg = this.kernel.database
        .prepare(
          "SELECT assignee_agent_id, cancelled_at FROM plan_packages WHERE project_id = ? AND plan_id = ? AND package_id = ?",
        )
        .get(this.kernel.projectId, input.planId, input.packageId) as
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
      const agent = this.kernel.agentRow(input.agentId);
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
        const current = this.kernel.agentRow(pkg.assignee_agent_id);
        if (current?.state === "active")
          throw new ControllerError(
            `package ${input.packageId} is already assigned to ${pkg.assignee_agent_id}`,
          );
      }
      const holding = this.kernel.database
        .prepare(
          "SELECT package_id FROM plan_packages WHERE project_id = ? AND plan_id = ? AND assignee_agent_id = ? AND package_id <> ? AND cancelled_at IS NULL",
        )
        .get(
          this.kernel.projectId,
          input.planId,
          agent.agent_id,
          input.packageId,
        ) as { package_id: string } | undefined;
      if (holding !== undefined)
        throw new ControllerError(
          `${agent.agent_id} already holds package ${holding.package_id} of this plan`,
        );
      const revision = this.kernel.database
        .prepare(
          "SELECT body_json FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
        )
        .get(this.kernel.projectId, input.planId, plan.approved_revision) as
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
        const row = this.kernel.database
          .prepare(
            "SELECT assignee_agent_id, assigned_at, cancelled_at FROM plan_packages WHERE project_id = ? AND plan_id = ? AND package_id = ?",
          )
          .get(this.kernel.projectId, input.planId, dep) as
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
      const now = this.kernel.now();
      const messageId = this.areas.messages.insertQueuedMessage(
        this.kernel.controllerActorId(),
        agent,
        task,
        sha256(task),
        now,
      );
      this.kernel.database
        .prepare(
          "UPDATE plan_packages SET assignee_agent_id = ?, assigned_at = ?, assignment_message_id = ? WHERE project_id = ? AND plan_id = ? AND package_id = ?",
        )
        .run(
          agent.agent_id,
          now,
          messageId,
          this.kernel.projectId,
          input.planId,
          input.packageId,
        );
      this.kernel.database
        .prepare(
          "UPDATE plans SET updated_at = ? WHERE project_id = ? AND plan_id = ?",
        )
        .run(now, this.kernel.projectId, input.planId);
      return {
        value: {
          ...this.planPackages(input.planId).find(
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
    return this.kernel.mutate<PlanSignoffRecord>(
      context,
      "plan.signoff",
      "plan:write",
      { ...input, summary },
      (actor) => {
        const agent = this.kernel.agentByActor(actor.actorId);
        const plan = this.areas.plans.planRow(input.planId);
        if (plan === undefined)
          throw new ControllerError(`plan ${input.planId} does not exist`);
        if (agent === undefined || agent.agent_id !== plan.architect_agent_id)
          throw new ControllerError("only the plan's architect signs it off");
        if (plan.state !== "approved")
          throw new ControllerError(
            `plan ${input.planId} is ${plan.state}, not approved`,
          );
        const integration = this.kernel.database
          .prepare(
            "SELECT state, branch, head_sha FROM integrations WHERE project_id = ? AND integration_id = ?",
          )
          .get(this.kernel.projectId, input.integrationId) as
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
        const outside = this.kernel.database
          .prepare(
            `SELECT ir.report_id FROM integration_reports ir
             JOIN agent_reports r ON r.project_id = ir.project_id AND r.report_id = ir.report_id
             JOIN agents a ON a.project_id = r.project_id AND a.agent_id = r.agent_id
             WHERE ir.project_id = ? AND ir.integration_id = ? AND NOT EXISTS (
               SELECT 1 FROM plan_packages pp
               WHERE pp.project_id = ir.project_id AND pp.plan_id = ? AND pp.assignee_agent_id = r.agent_id AND a.generation = r.generation)
             ORDER BY ir.position LIMIT 1`,
          )
          .get(this.kernel.projectId, input.integrationId, input.planId) as
          { report_id: string } | undefined;
        if (outside !== undefined)
          throw new ControllerError(
            `report ${outside.report_id} of the integration is not a package of plan ${input.planId}`,
          );
        const reportCount = this.kernel.database
          .prepare(
            "SELECT COUNT(*) AS n FROM integration_reports WHERE project_id = ? AND integration_id = ?",
          )
          .get(this.kernel.projectId, input.integrationId) as { n: number };
        if (reportCount.n === 0)
          throw new ControllerError(
            `integration ${input.integrationId} has no reports of plan ${input.planId}`,
          );
        const latest = this.kernel.database
          .prepare(
            `SELECT state FROM reviews WHERE project_id = ? AND subject_integration_id = ? AND state IN ('passed', 'findings')
             ORDER BY sequence DESC LIMIT 1`,
          )
          .get(this.kernel.projectId, input.integrationId) as
          { state: string } | undefined;
        if (latest?.state !== "passed")
          throw new ControllerError(
            `integration ${input.integrationId} has no passed review as its latest verdict`,
          );
        const signed = this.kernel.database
          .prepare(
            "SELECT 1 AS present FROM plan_signoffs WHERE project_id = ? AND plan_id = ? AND integration_id = ?",
          )
          .get(this.kernel.projectId, input.planId, input.integrationId);
        if (signed)
          throw new ControllerError(
            `plan ${input.planId} is already signed off for integration ${input.integrationId}`,
          );
        const now = this.kernel.now();
        this.kernel.database
          .prepare(
            `INSERT INTO plan_signoffs(project_id, plan_id, integration_id, architect_agent_id, summary, created_at)
             VALUES (?, ?, ?, ?, ?, ?)`,
          )
          .run(
            this.kernel.projectId,
            input.planId,
            input.integrationId,
            agent.agent_id,
            summary,
            now,
          );
        this.areas.messageNotices.noticeToPm(
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
  packageFacts(
    planId: string,
    row: {
      readonly package_id: string;
      readonly assignee_agent_id: string | null;
      readonly assigned_at: string | null;
      readonly cancelled_at: string | null;
    },
    planCancelled: boolean,
  ): PackageFacts {
    const report = this.packageReport(
      planId,
      row.package_id,
      row.assignee_agent_id,
      row.assigned_at,
    );
    const confirmed =
      report !== undefined &&
      this.areas.integrations.reportInConfirmedIntegration(report.report_id);
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

  planPackageRows(planId: string): {
    package_id: string;
    assignee_agent_id: string | null;
    assigned_at: string | null;
    cancelled_at: string | null;
  }[] {
    return this.kernel.database
      .prepare(
        "SELECT package_id, assignee_agent_id, assigned_at, cancelled_at FROM plan_packages WHERE project_id = ? AND plan_id = ? ORDER BY package_id",
      )
      .all(this.kernel.projectId, planId) as {
      package_id: string;
      assignee_agent_id: string | null;
      assigned_at: string | null;
      cancelled_at: string | null;
    }[];
  }

  /** Splits a package ref `<plan-id>/<package-id>`; the ids themselves are checked against the ledger by the caller. */
  packageRef(refId: string): { planId: string; packageId: string } {
    const parts = refId.split("/");
    if (parts.length !== 2)
      throw new TypeError("a package ref is <plan-id>/<package-id>");
    return {
      planId: safeId(parts[0], "plan id"),
      packageId: safeId(parts[1], "package id"),
    };
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
    return this.kernel.mutate<PlanCancelResult>(
      context,
      "plan.cancel",
      "plan:write",
      { ...input },
      (actor) => {
        if (actor.role !== "operator")
          throw new ControllerError("only the operator cancels a plan");
        const plan = this.areas.plans.planRow(input.planId);
        if (plan === undefined)
          throw new ControllerError(`plan ${input.planId} does not exist`);
        if (plan.cancelled_at !== null)
          throw new ControllerError(
            `${input.packageId === undefined ? "already_cancelled" : "plan_cancelled"}: plan ${input.planId} was cancelled`,
          );
        const now = this.kernel.now();
        let cancelledPackages: string[];
        let reviewId: string | null = null;
        let reviewerAgentId: string | null = null;
        if (input.packageId !== undefined) {
          if (plan.state !== "approved")
            throw new ControllerError(
              `plan_not_approved: plan ${input.planId} is ${plan.state}`,
            );
          const row = this.planPackageRows(input.planId).find(
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
          if (this.packageFacts(input.planId, row, false).confirmed)
            throw new ControllerError(
              `package_confirmed: package ${input.packageId} is already integrated and confirmed`,
            );
          this.kernel.database
            .prepare(
              "UPDATE plan_packages SET cancelled_at = ? WHERE project_id = ? AND plan_id = ? AND package_id = ? AND cancelled_at IS NULL",
            )
            .run(now, this.kernel.projectId, input.planId, input.packageId);
          cancelledPackages = [input.packageId];
        } else {
          if (plan.state === "superseded")
            throw new ControllerError(
              `plan_superseded: plan ${input.planId} was superseded`,
            );
          if (plan.state === "in_review") {
            const open = this.kernel.database
              .prepare(
                "SELECT review_id, reviewer_agent_id FROM reviews WHERE project_id = ? AND subject_plan_id = ? AND state = 'started'",
              )
              .get(this.kernel.projectId, input.planId) as
              { review_id: string; reviewer_agent_id: string } | undefined;
            if (open !== undefined) {
              this.kernel.database
                .prepare(
                  "UPDATE reviews SET state = 'cancelled', failure_reason = 'plan cancelled', completed_at = ? WHERE project_id = ? AND review_id = ? AND state = 'started'",
                )
                .run(now, this.kernel.projectId, open.review_id);
              reviewId = open.review_id;
              reviewerAgentId = open.reviewer_agent_id;
            }
            this.kernel.database
              .prepare(
                "UPDATE plans SET state = 'draft', updated_at = ? WHERE project_id = ? AND plan_id = ?",
              )
              .run(now, this.kernel.projectId, input.planId);
          }
          cancelledPackages = this.planPackageRows(input.planId)
            .filter(
              (row) =>
                row.cancelled_at === null &&
                !this.packageFacts(input.planId, row, false).confirmed,
            )
            .map((row) => row.package_id);
          for (const packageId of cancelledPackages)
            this.kernel.database
              .prepare(
                "UPDATE plan_packages SET cancelled_at = ? WHERE project_id = ? AND plan_id = ? AND package_id = ? AND cancelled_at IS NULL",
              )
              .run(now, this.kernel.projectId, input.planId, packageId);
          this.kernel.database
            .prepare(
              "UPDATE plans SET cancelled_at = ?, updated_at = ? WHERE project_id = ? AND plan_id = ? AND cancelled_at IS NULL",
            )
            .run(now, now, this.kernel.projectId, input.planId);
        }
        const notified = this.areas.plans.queuePlanNotice(
          plan,
          planCancelledNotice(input.planId, input.packageId),
          now,
        );
        const held = this.planPackageRows(input.planId).filter((row) =>
          cancelledPackages.includes(row.package_id),
        );
        for (const row of held) {
          const developer =
            row.assignee_agent_id === null
              ? undefined
              : this.kernel.agentRow(row.assignee_agent_id);
          if (developer?.state !== "active") continue;
          const body = `Stop work on package ${row.package_id} of plan ${input.planId}: it was cancelled by the operator. Do not report it.`;
          this.areas.messages.insertQueuedMessage(
            this.kernel.controllerActorId(),
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
}
