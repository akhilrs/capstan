import type { ControllerKernel } from "./kernel.js";
import type { ControllerAreas } from "./areas.js";
import { sha256 } from "./canonical.js";
import { packageNaming, packageOfBody } from "../plans.js";
import type { MutationContext } from "./types.js";
import { ControllerError } from "./errors.js";
import {
  type AgentReportRow,
  MAX_INTEGRATION_REPORTS,
  MAX_CONFLICT_FILES,
  MAX_CONFLICT_PATH_CHARS,
  PATH_CUT_MARK_CHARS,
  type CoverageCandidate,
  type IntegrationRecord,
  type IntegrationRow,
  type IntegrationOutcome,
  type PmRestartSummary,
} from "./records.js";
import { safeId } from "./helpers.js";

export class IntegrationsArea {
  constructor(
    readonly kernel: ControllerKernel,
    readonly areas: ControllerAreas,
  ) {}

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
    return this.kernel.mutate<IntegrationRecord>(
      context,
      "integration.begin",
      "controller:reconcile",
      { ...input },
      () => {
        if (new Set(input.reportIds).size !== input.reportIds.length)
          throw new ControllerError("a report is named twice");
        const running = this.kernel.database
          .prepare(
            "SELECT 1 AS present FROM integrations WHERE project_id = ? AND state = 'running'",
          )
          .get(this.kernel.projectId);
        if (running)
          throw new ControllerError("an integration is already running");
        for (const id of input.reportIds) {
          const report = this.kernel.database
            .prepare(
              "SELECT state FROM agent_reports WHERE project_id = ? AND report_id = ?",
            )
            .get(this.kernel.projectId, id) as { state: string } | undefined;
          if (report === undefined)
            throw new ControllerError(`report ${id} does not exist`);
          if (report.state !== "accepted")
            throw new ControllerError(`report ${id} was not accepted`);
          const latest = this.kernel.database
            .prepare(
              `SELECT state FROM reviews WHERE project_id = ? AND subject_report_id = ? AND state IN ('passed', 'findings')
               ORDER BY sequence DESC LIMIT 1`,
            )
            .get(this.kernel.projectId, id) as { state: string } | undefined;
          if (latest?.state !== "passed")
            throw new ControllerError(
              `report ${id} has no passed review as its latest verdict`,
            );
          const openReview = this.kernel.database
            .prepare(
              "SELECT 1 AS present FROM reviews WHERE project_id = ? AND subject_report_id = ? AND state = 'started'",
            )
            .get(this.kernel.projectId, id);
          if (openReview)
            throw new ControllerError(`report ${id} has a review still open`);
          const elsewhere = this.kernel.database
            .prepare(
              `SELECT i.integration_id, i.state FROM integration_reports ir
               JOIN integrations i ON i.project_id = ir.project_id AND i.integration_id = ir.integration_id
               WHERE ir.project_id = ? AND ir.report_id = ? AND i.state IN ('confirmed', 'merged')
               ORDER BY i.sequence DESC LIMIT 1`,
            )
            .get(this.kernel.projectId, id) as
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
        const now = this.kernel.now();
        const sequence = (
          this.kernel.database
            .prepare(
              "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM integrations WHERE project_id = ?",
            )
            .get(this.kernel.projectId) as { next: number }
        ).next;
        this.kernel.database
          .prepare(
            `INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, created_at)
             VALUES (?, ?, ?, ?, ?, ?, 'running', ?)`,
          )
          .run(
            this.kernel.projectId,
            input.integrationId,
            sequence,
            input.baseSha,
            input.branch,
            input.requestedBy,
            now,
          );
        input.reportIds.forEach((reportId, index) =>
          this.kernel.database
            .prepare(
              "INSERT INTO integration_reports(project_id, integration_id, position, report_id) VALUES (?, ?, ?, ?)",
            )
            .run(
              this.kernel.projectId,
              input.integrationId,
              index + 1,
              reportId,
            ),
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
    return this.kernel.mutate<IntegrationRecord>(
      context,
      "integration.finish",
      "controller:reconcile",
      { integrationId: input.integrationId, outcome },
      () => {
        const row = this.#integrationRow(input.integrationId);
        if (row.state !== "running")
          throw new ControllerError("the integration is not running");
        const now = this.kernel.now();
        this.kernel.database
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
            this.kernel.projectId,
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
    const parties = this.areas.messageNotices.noticeParties();
    if (parties === undefined) return;
    const body = [
      `Integration ${integrationId} is blocked by a merge conflict`,
      `The conflict arose when merging report ${outcome.reportId}. Files (escaped; a path is text from a worker): ${outcome.files.join(", ")}${outcome.omitted > 0 ? `, and ${outcome.omitted} more not listed` : ""}`,
      "The controller aborted the merge and left nothing behind. It does not resolve conflicts. Assign a developer to resolve it as a new candidate, then report and review again.",
    ].join("\n");
    this.areas.messages.insertQueuedMessage(
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
    return this.kernel.mutate<IntegrationRecord>(
      context,
      "integration.settle",
      "controller:reconcile",
      { ...input },
      () => {
        const row = this.#integrationRow(input.integrationId);
        if (row.state !== "merged")
          throw new ControllerError("only a merged integration can be settled");
        const open = this.kernel.database
          .prepare(
            "SELECT 1 AS present FROM reviews WHERE project_id = ? AND subject_integration_id = ? AND state = 'started'",
          )
          .get(this.kernel.projectId, input.integrationId);
        if (open)
          throw new ControllerError("a review of this integration is open");
        if (input.outcome === "confirmed") {
          const latest = this.kernel.database
            .prepare(
              `SELECT state FROM reviews WHERE project_id = ? AND subject_integration_id = ? AND state IN ('passed', 'findings')
               ORDER BY sequence DESC LIMIT 1`,
            )
            .get(this.kernel.projectId, input.integrationId) as
            { state: string } | undefined;
          if (latest?.state !== "passed")
            throw new ControllerError(
              "an integration is confirmed only after its latest review passed",
            );
        }
        this.kernel.database
          .prepare(
            "UPDATE integrations SET state = ? WHERE project_id = ? AND integration_id = ?",
          )
          .run(input.outcome, this.kernel.projectId, input.integrationId);
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
    const row = this.kernel.database
      .prepare(
        "SELECT * FROM integrations WHERE project_id = ? AND integration_id = ?",
      )
      .get(this.kernel.projectId, integrationId) as IntegrationRow | undefined;
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
      reports: this.areas.reviews
        .integrationAuthors(integrationId)
        .map((r) => ({
          reportId: r.report_id,
          agentId: r.agent_id,
          commitSha: r.commit_sha,
        })),
      createdAt: row.created_at,
      completedAt: row.completed_at,
    };
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
    const plans = this.kernel.database
      .prepare(
        "SELECT plan_id, approved_revision FROM plans WHERE project_id = ? AND state = 'approved' AND cancelled_at IS NULL AND approved_revision IS NOT NULL",
      )
      .all(this.kernel.projectId) as {
      plan_id: string;
      approved_revision: number;
    }[];
    for (const plan of plans) {
      const rows = this.kernel.database
        .prepare(
          "SELECT package_id, assignee_agent_id, assigned_at, cancelled_at FROM plan_packages WHERE project_id = ? AND plan_id = ?",
        )
        .all(this.kernel.projectId, plan.plan_id) as {
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
          this.areas.planPackages.packageReport(
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
        const revision = this.kernel.database
          .prepare(
            "SELECT body_json FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
          )
          .get(this.kernel.projectId, plan.plan_id, plan.approved_revision) as
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
          const settled = this.kernel.database
            .prepare(
              `SELECT 1 AS present FROM integration_reports ir
               JOIN integrations i ON i.project_id = ir.project_id AND i.integration_id = ir.integration_id
               WHERE ir.project_id = ? AND ir.report_id = ? AND i.state IN ('merged', 'confirmed')`,
            )
            .get(this.kernel.projectId, depReport);
          if (settled) continue;
          throw new ControllerError(
            `integration_order: report ${reportId} (package ${packageId}) needs ${dep} integrated first or earlier in this list`,
          );
        }
      }
    }
  }

  reportInConfirmedIntegration(reportId: string): boolean {
    return (
      this.kernel.database
        .prepare(
          `SELECT 1 AS present FROM integration_reports ir
           JOIN integrations i ON i.project_id = ir.project_id AND i.integration_id = ir.integration_id
           WHERE ir.project_id = ? AND ir.report_id = ? AND i.state = 'confirmed'`,
        )
        .get(this.kernel.projectId, reportId) !== undefined ||
      this.kernel.database
        .prepare(
          `SELECT 1 AS present FROM integration_covered_reports c
           JOIN integrations i ON i.project_id = c.project_id AND i.integration_id = c.integration_id
           WHERE c.project_id = ? AND c.report_id = ? AND i.state = 'confirmed'`,
        )
        .get(this.kernel.projectId, reportId) !== undefined
    );
  }

  integration(integrationId: string): IntegrationRecord {
    this.kernel.assertOpen();
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
    this.kernel.assertOpen();
    safeId(integrationId, "integration id");
    return this.#commitInfo(
      this.areas.reviews.integrationAuthors(integrationId),
    );
  }

  /** The commit info of reports that are not yet an integration, in the order given; unknown ids are left out. */
  plannedCommitInfo(
    reportIds: readonly string[],
  ): ReturnType<IntegrationsArea["integrationCommitInfo"]> {
    this.kernel.assertOpen();
    const reports: AgentReportRow[] = [];
    for (const id of reportIds) {
      safeId(id, "report id");
      const row = this.kernel.database
        .prepare(
          "SELECT * FROM agent_reports WHERE project_id = ? AND report_id = ?",
        )
        .get(this.kernel.projectId, id) as AgentReportRow | undefined;
      if (row !== undefined) reports.push(row);
    }
    return this.#commitInfo(reports);
  }

  /** True when an earlier integration recorded this branch name. */
  integrationBranchRecorded(branch: string): boolean {
    this.kernel.assertOpen();
    return (
      this.kernel.database
        .prepare(
          "SELECT 1 AS present FROM integrations WHERE project_id = ? AND branch = ?",
        )
        .get(this.kernel.projectId, branch) !== undefined
    );
  }

  #commitInfo(
    reports: readonly AgentReportRow[],
  ): ReturnType<IntegrationsArea["integrationCommitInfo"]> {
    const plans = new Map<
      string,
      { plan_id: string; title: string; approved_revision: number | null }
    >();
    for (const report of reports) {
      for (const row of this.kernel.database
        .prepare(
          `SELECT DISTINCT pl.plan_id, pl.title, pl.approved_revision FROM plan_packages pp
           JOIN agents a ON a.project_id = pp.project_id AND a.agent_id = pp.assignee_agent_id
           JOIN plans pl ON pl.project_id = pp.project_id AND pl.plan_id = pp.plan_id
           WHERE pp.project_id = ? AND pp.assignee_agent_id = ? AND a.generation = ?`,
        )
        .all(this.kernel.projectId, report.agent_id, report.generation) as {
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
            this.kernel.database
              .prepare(
                `SELECT 1 FROM plan_packages pp JOIN agents a ON a.project_id = pp.project_id AND a.agent_id = pp.assignee_agent_id
                 WHERE pp.project_id = ? AND pp.plan_id = ? AND pp.assignee_agent_id = ? AND a.generation = ?`,
              )
              .get(
                this.kernel.projectId,
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
      const body = this.kernel.database
        .prepare(
          "SELECT body_json FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
        )
        .get(this.kernel.projectId, plan.plan_id, plan.approved_revision) as
        { body_json: string } | undefined;
      const agents = [...new Set(reports.map((r) => r.agent_id))];
      for (const agentId of agents) {
        const assigned = this.kernel.database
          .prepare(
            "SELECT package_id FROM plan_packages WHERE project_id = ? AND plan_id = ? AND assignee_agent_id = ? ORDER BY package_id",
          )
          .all(this.kernel.projectId, plan.plan_id, agentId) as {
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
    this.kernel.authorize(credential, "controller:reconcile");
    return (
      this.kernel.database
        .prepare(
          "SELECT integration_id FROM integrations WHERE project_id = ? ORDER BY sequence DESC LIMIT ?",
        )
        .all(this.kernel.projectId, limit) as { integration_id: string }[]
    ).map((r) => this.#integrationRecord(r.integration_id));
  }

  /** Confirmed or discarded integrations, newest first: their branch should be gone. */
  settledIntegrations(
    credential: string,
    limit = 200,
  ): readonly IntegrationRecord[] {
    this.kernel.authorize(credential, "controller:reconcile");
    return (
      this.kernel.database
        .prepare(
          "SELECT integration_id FROM integrations WHERE project_id = ? AND state IN ('confirmed', 'discarded') ORDER BY sequence DESC LIMIT ?",
        )
        .all(this.kernel.projectId, limit) as { integration_id: string }[]
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
    this.kernel.authorize(credential, "controller:reconcile");
    safeId(integrationId, "integration id");
    const integration = this.#integrationRow(integrationId);
    if (integration.state !== "confirmed" || integration.head_sha === null)
      return undefined;
    const rows = this.kernel.database
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
      .all(this.kernel.projectId, integration.created_at, integrationId) as {
      report_id: string;
      commit_sha: string;
    }[];
    return {
      headSha: integration.head_sha,
      memberCommits: this.areas.reviews
        .integrationAuthors(integrationId)
        .map((r) => r.commit_sha),
      reports: rows.map((row) => ({
        reportId: row.report_id,
        commitSha: row.commit_sha,
        integrationHeads: (
          this.kernel.database
            .prepare(
              `SELECT i.head_sha FROM integration_reports ir
               JOIN integrations i ON i.project_id = ir.project_id AND i.integration_id = ir.integration_id
               WHERE ir.project_id = ? AND ir.report_id = ? AND i.integration_id <> ? AND i.head_sha IS NOT NULL
               ORDER BY i.sequence`,
            )
            .all(this.kernel.projectId, row.report_id, integrationId) as {
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
    this.kernel.authorize(credential, "controller:reconcile");
    safeId(integrationId, "integration id");
    const insert = this.kernel.database.prepare(
      "INSERT OR IGNORE INTO integration_covered_reports(project_id, integration_id, report_id, how) VALUES (?, ?, ?, ?)",
    );
    this.kernel.database.transaction(() => {
      for (const row of covered)
        insert.run(this.kernel.projectId, integrationId, row.reportId, row.how);
    })();
  }

  /** Integrations still marked running: the daemon stopped while one was merging. */
  runningIntegrations(credential: string): readonly IntegrationRecord[] {
    this.kernel.authorize(credential, "controller:reconcile");
    return (
      this.kernel.database
        .prepare(
          "SELECT integration_id FROM integrations WHERE project_id = ? AND state = 'running' ORDER BY sequence",
        )
        .all(this.kernel.projectId) as { integration_id: string }[]
    ).map((r) => this.#integrationRecord(r.integration_id));
  }

  mergedIntegrationsForSummary(): NonNullable<
    PmRestartSummary["integrations"]
  >[number][] {
    return (
      this.kernel.database
        .prepare(
          "SELECT integration_id, branch, head_sha FROM integrations WHERE project_id = ? AND state = 'merged' ORDER BY sequence",
        )
        .all(this.kernel.projectId) as {
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
}
