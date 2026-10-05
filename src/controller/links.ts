import type { ControllerKernel } from "./kernel.js";
import type { ControllerAreas } from "./areas.js";
import {
  EXTERNAL_REF_KINDS,
  NEXORA_ID_PATTERN,
  isNexoraState,
  wantedPackageState,
  wantedPlanState,
  wantedRequirementState,
  type ExternalRefKind,
  type NexoraState,
} from "../nexora.js";
import type { MutationContext } from "./types.js";
import { ControllerError } from "./errors.js";
import { type ExternalLinkRecord, type ExternalLinkRow } from "./records.js";
import { safeId } from "./helpers.js";

export class LinksArea {
  constructor(
    readonly kernel: ControllerKernel,
    readonly areas: ControllerAreas,
  ) {}

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
      const report = this.kernel.database
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
          this.kernel.projectId,
          this.kernel.projectId,
          link.boundAt,
        ) as { report_id: string } | undefined;
      const verdict =
        report === undefined
          ? undefined
          : (this.kernel.database
              .prepare(
                `SELECT state FROM reviews WHERE project_id = ? AND subject_report_id = ? AND state IN ('passed', 'findings')
                 ORDER BY sequence DESC LIMIT 1`,
              )
              .get(this.kernel.projectId, report.report_id) as
              { state: string } | undefined);
      return wantedRequirementState({
        bound: true,
        syncedState: link.syncedState,
        reportConfirmed:
          report !== undefined &&
          this.areas.integrations.reportInConfirmedIntegration(
            report.report_id,
          ),
        reviewPassed: verdict?.state === "passed",
      });
    }
    const planId =
      refKind === "plan"
        ? refId
        : this.areas.planPackages.packageRef(refId).planId;
    const plan = this.areas.plans.planRow(planId);
    if (plan === undefined)
      throw new ControllerError(`plan ${planId} does not exist`);
    const planCancelled = plan.cancelled_at !== null;
    const rows = this.areas.planPackages.planPackageRows(planId);
    if (refKind === "plan")
      return wantedPlanState({
        state: plan.state,
        cancelled: planCancelled,
        packages: rows.map((row) =>
          this.areas.planPackages.packageFacts(planId, row, planCancelled),
        ),
      });
    const { packageId } = this.areas.planPackages.packageRef(refId);
    const row = rows.find((r) => r.package_id === packageId);
    if (row === undefined)
      throw new ControllerError(`plan ${planId} has no package ${packageId}`);
    return wantedPackageState(
      this.areas.planPackages.packageFacts(planId, row, planCancelled),
    );
  }

  linkRecord(row: ExternalLinkRow): ExternalLinkRecord {
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

  linkRow(refKind: string, refId: string): ExternalLinkRow | undefined {
    return this.kernel.database
      .prepare(
        "SELECT * FROM external_links WHERE project_id = ? AND ref_kind = ? AND ref_id = ? AND system = 'nexora'",
      )
      .get(this.kernel.projectId, refKind, refId) as
      ExternalLinkRow | undefined;
  }

  #assertLinkTarget(refKind: ExternalRefKind, refId: string): void {
    if (refKind === "requirement") return;
    if (refKind === "plan") {
      if (this.areas.plans.planRow(refId) === undefined)
        throw new ControllerError(`plan ${refId} does not exist`);
      return;
    }
    const { planId, packageId } = this.areas.planPackages.packageRef(refId);
    const exists = this.kernel.database
      .prepare(
        "SELECT 1 AS present FROM plan_packages WHERE project_id = ? AND plan_id = ? AND package_id = ?",
      )
      .get(this.kernel.projectId, planId, packageId);
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
    if (input.refKind === "package")
      this.areas.planPackages.packageRef(input.refId);
    else safeId(input.refId, "ref id");
    if (
      typeof input.externalId !== "string" ||
      !NEXORA_ID_PATTERN.test(input.externalId)
    )
      throw new TypeError("the external id must look like PM-47");
    if (input.syncedState !== undefined && !isNexoraState(input.syncedState))
      throw new TypeError("the synced state is not a Nexora status");
    return this.kernel.mutate<ExternalLinkRecord>(
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
        const now = this.kernel.now();
        const existing = this.linkRow(input.refKind, input.refId);
        if (existing === undefined)
          this.kernel.database
            .prepare(
              `INSERT INTO external_links(project_id, ref_kind, ref_id, system, external_id, synced_state, linked_by, linked_at, synced_at)
               VALUES (?, ?, ?, 'nexora', ?, ?, ?, ?, ?)`,
            )
            .run(
              this.kernel.projectId,
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
          this.kernel.database
            .prepare(
              "UPDATE external_links SET synced_state = ?, synced_at = ? WHERE project_id = ? AND ref_kind = ? AND ref_id = ? AND system = 'nexora'",
            )
            .run(
              input.syncedState ?? existing.synced_state,
              now,
              this.kernel.projectId,
              input.refKind,
              input.refId,
            );
        return {
          value: this.linkRecord(this.linkRow(input.refKind, input.refId)!),
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
    return this.kernel.mutate<ExternalLinkRecord>(
      context,
      "link.bind",
      "plan:write",
      { ...input },
      (actor) => {
        if (actor.role !== "PM" && actor.role !== "operator")
          throw new ControllerError(
            "only the PM or the operator binds a requirement",
          );
        const link = this.linkRow("requirement", input.refId);
        if (link === undefined)
          throw new ControllerError(
            `requirement ${input.refId} is not linked; run link first`,
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
        this.kernel.database
          .prepare(
            "UPDATE external_links SET bound_agent_id = ?, bound_at = ? WHERE project_id = ? AND ref_kind = 'requirement' AND ref_id = ? AND system = 'nexora'",
          )
          .run(
            agent.agent_id,
            this.kernel.now(),
            this.kernel.projectId,
            input.refId,
          );
        return {
          value: this.linkRecord(this.linkRow("requirement", input.refId)!),
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
    return this.areas.panes.taskNaming(credential, ref);
  }

  /** Whether the agent has made any report, accepted or not. */
  agentHasReports(agentId: string): boolean {
    return this.areas.panes.agentHasReports(agentId);
  }

  /** The active agent whose recorded branch is `branch`, other than `exceptAgentId`; undefined when none. */
  activeBranchHolder(
    branch: string,
    exceptAgentId?: string,
  ): string | undefined {
    return this.areas.panes.activeBranchHolder(branch, exceptAgentId);
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
    return this.areas.panes.renameAgentBranch(context, input);
  }

  /** Every link with its wanted state and drift flag, oldest first. */
  externalLinks(credential: string): readonly ExternalLinkRecord[] {
    this.kernel.authorize(credential, "plan:read");
    return (
      this.kernel.database
        .prepare(
          "SELECT * FROM external_links WHERE project_id = ? ORDER BY linked_at, ref_kind, ref_id",
        )
        .all(this.kernel.projectId) as ExternalLinkRow[]
    ).map((row) => this.linkRecord(row));
  }

  /** The wanted Nexora status of one ref; null when the ledger has none (an unlinked or unbound requirement, a superseded plan). */
  wantedNexoraState(
    credential: string,
    refKind: ExternalRefKind,
    refId: string,
  ): NexoraState | null {
    this.kernel.authorize(credential, "plan:read");
    if (!(EXTERNAL_REF_KINDS as readonly unknown[]).includes(refKind))
      throw new TypeError(
        `the link kind must be one of ${EXTERNAL_REF_KINDS.join(", ")}`,
      );
    if (refKind === "package") this.areas.planPackages.packageRef(refId);
    else safeId(refId, "ref id");
    const link = this.linkRow(refKind, refId);
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
}
