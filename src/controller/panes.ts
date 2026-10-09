import type { ControllerKernel } from "./kernel.js";
import type { ControllerAreas } from "./areas.js";
import path from "node:path";
import { packageNaming, packageOfBody } from "../plans.js";
import { type MutationContext } from "./types.js";
import { ControllerError } from "./errors.js";
import { type AgentPaneInput, type AgentPaneRecord } from "./records.js";
import { MAX_TASK_TITLE, TASK_REF_PATTERN } from "../task-text.js";
import { safeId } from "./helpers.js";

export class PanesArea {
  constructor(
    readonly kernel: ControllerKernel,
    readonly areas: ControllerAreas,
  ) {}

  recordAgentPane(
    context: MutationContext,
    input: AgentPaneInput,
  ): { readonly recorded: true } {
    safeId(input.agentId, "agent id");
    for (const [value, label] of [
      [input.workspaceId, "workspace id"],
      [input.paneId, "pane id"],
    ] as const)
      if (value !== null) safeId(value, label);
    if (input.terminalId !== undefined) safeId(input.terminalId, "terminal id");
    if (
      input.worktreePath !== null &&
      (!path.isAbsolute(input.worktreePath) ||
        input.worktreePath.length > 1000 ||
        /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u.test(input.worktreePath))
    )
      throw new TypeError("worktree path must be absolute and printable");
    if (
      input.branch !== null &&
      !/^[A-Za-z0-9][A-Za-z0-9._/-]{0,99}$/.test(input.branch)
    )
      throw new TypeError("branch name is not acceptable");
    if (input.baseSha !== null && !/^[0-9a-f]{40}$/.test(input.baseSha))
      throw new TypeError("base sha must be 40 lowercase hex characters");
    if (input.taskRef !== undefined && !TASK_REF_PATTERN.test(input.taskRef))
      throw new TypeError("task ref is not acceptable");
    if (
      input.taskTitle !== undefined &&
      (Array.from(input.taskTitle).length > MAX_TASK_TITLE ||
        input.taskTitle.trim() !== input.taskTitle ||
        input.taskTitle === "" ||
        /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]|\s\s/u.test(input.taskTitle))
    )
      throw new TypeError("task title is not acceptable");
    return this.kernel.mutate(
      context,
      "agent_pane.record",
      "controller:reconcile",
      { ...input },
      () => {
        const agent = this.kernel.agentRow(input.agentId);
        if (agent?.state !== "active")
          throw new ControllerError(
            "a pane is recorded only for an active agent",
          );
        const now = this.kernel.now();
        this.kernel.database
          .prepare(
            `INSERT INTO agent_panes(project_id, agent_id, workspace_id, pane_id, worktree_path, branch, base_sha, generation, created_at, updated_at, task_ref, task_title, terminal_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(project_id, agent_id) DO UPDATE SET
               workspace_id = excluded.workspace_id, pane_id = excluded.pane_id,
               worktree_path = excluded.worktree_path, branch = excluded.branch,
               base_sha = excluded.base_sha, generation = excluded.generation,
               updated_at = excluded.updated_at,
               task_ref = COALESCE(excluded.task_ref, agent_panes.task_ref),
               task_title = COALESCE(excluded.task_title, agent_panes.task_title),
               terminal_id = excluded.terminal_id`,
          )
          .run(
            this.kernel.projectId,
            input.agentId,
            input.workspaceId,
            input.paneId,
            input.worktreePath,
            input.branch,
            input.baseSha,
            agent.generation,
            now,
            now,
            input.taskRef ?? null,
            input.taskTitle ?? null,
            input.terminalId ?? null,
          );
        return {
          value: { recorded: true as const },
          event: {
            entityType: "agent_pane",
            entityId: input.agentId,
            stateVersion: agent.generation,
            details: { paneId: input.paneId, branch: input.branch },
          },
        };
      },
    );
  }

  clearAgentPane(
    context: MutationContext,
    agentId: string,
  ): { readonly cleared: boolean } {
    safeId(agentId, "agent id");
    this.kernel.assertOpen();
    if (
      !this.kernel.hasStoredRequest(context) &&
      this.kernel.database
        .prepare(
          "SELECT 1 AS present FROM agent_panes WHERE project_id = ? AND agent_id = ?",
        )
        .get(this.kernel.projectId, agentId) === undefined
    ) {
      this.kernel.authorize(context.credential, "controller:reconcile");
      return { cleared: false };
    }
    return this.kernel.mutate(
      context,
      "agent_pane.clear",
      "controller:reconcile",
      { agentId },
      () => {
        this.kernel.database
          .prepare(
            "DELETE FROM agent_panes WHERE project_id = ? AND agent_id = ?",
          )
          .run(this.kernel.projectId, agentId);
        return {
          value: { cleared: true },
          event: {
            entityType: "agent_pane",
            entityId: agentId,
            stateVersion: 0,
            details: { cleared: true },
          },
        };
      },
    );
  }

  recordFallbackPane(
    context: MutationContext,
    input: { readonly workspaceId: string; readonly paneId: string },
  ): { readonly recorded: true } {
    safeId(input.workspaceId, "workspace id");
    safeId(input.paneId, "pane id");
    return this.kernel.mutate(
      context,
      "fallback_pane.record",
      "controller:reconcile",
      { ...input },
      () => {
        this.kernel.database
          .prepare(
            `INSERT INTO fallback_panes(project_id, workspace_id, pane_id, created_at) VALUES (?, ?, ?, ?)
             ON CONFLICT(project_id) DO UPDATE SET workspace_id = excluded.workspace_id, pane_id = excluded.pane_id`,
          )
          .run(
            this.kernel.projectId,
            input.workspaceId,
            input.paneId,
            this.kernel.now(),
          );
        return {
          value: { recorded: true as const },
          event: {
            entityType: "fallback_pane",
            entityId: this.kernel.projectId,
            stateVersion: 0,
            details: { paneId: input.paneId },
          },
        };
      },
    );
  }

  clearFallbackPane(context: MutationContext): { readonly cleared: boolean } {
    this.kernel.assertOpen();
    if (
      !this.kernel.hasStoredRequest(context) &&
      this.kernel.database
        .prepare("SELECT 1 AS present FROM fallback_panes WHERE project_id = ?")
        .get(this.kernel.projectId) === undefined
    ) {
      this.kernel.authorize(context.credential, "controller:reconcile");
      return { cleared: false };
    }
    return this.kernel.mutate(
      context,
      "fallback_pane.clear",
      "controller:reconcile",
      {},
      () => {
        this.kernel.database
          .prepare("DELETE FROM fallback_panes WHERE project_id = ?")
          .run(this.kernel.projectId);
        return {
          value: { cleared: true },
          event: {
            entityType: "fallback_pane",
            entityId: this.kernel.projectId,
            stateVersion: 0,
            details: { cleared: true },
          },
        };
      },
    );
  }

  /** A live pane of a replaced PM that could not be closed; kept in the ledger so a daemon restart still knows it. */
  recordOrphanPane(
    context: MutationContext,
    input: {
      readonly paneId: string;
      readonly agentId: string;
      /** Omitted when the pane's terminal id is not known. */
      readonly terminalId?: string;
    },
  ): { readonly recorded: true } {
    safeId(input.paneId, "pane id");
    safeId(input.agentId, "agent id");
    if (input.terminalId !== undefined) safeId(input.terminalId, "terminal id");
    return this.kernel.mutate(
      context,
      "orphan_pane.record",
      "controller:reconcile",
      { ...input },
      () => {
        if (this.kernel.agentRow(input.agentId) === undefined)
          throw new ControllerError("an orphan pane belongs to a known agent");
        this.kernel.database
          .prepare(
            "INSERT OR IGNORE INTO orphan_panes(project_id, pane_id, agent_id, created_at, terminal_id) VALUES (?, ?, ?, ?, ?)",
          )
          .run(
            this.kernel.projectId,
            input.paneId,
            input.agentId,
            this.kernel.now(),
            input.terminalId ?? null,
          );
        return {
          value: { recorded: true as const },
          event: {
            entityType: "orphan_pane",
            entityId: input.paneId,
            stateVersion: 0,
            details: { agentId: input.agentId },
          },
        };
      },
    );
  }

  clearOrphanPane(
    context: MutationContext,
    paneId: string,
  ): { readonly cleared: boolean } {
    safeId(paneId, "pane id");
    this.kernel.assertOpen();
    if (
      !this.kernel.hasStoredRequest(context) &&
      this.kernel.database
        .prepare(
          "SELECT 1 AS present FROM orphan_panes WHERE project_id = ? AND pane_id = ?",
        )
        .get(this.kernel.projectId, paneId) === undefined
    ) {
      this.kernel.authorize(context.credential, "controller:reconcile");
      return { cleared: false };
    }
    return this.kernel.mutate(
      context,
      "orphan_pane.clear",
      "controller:reconcile",
      { paneId },
      () => {
        this.kernel.database
          .prepare(
            "DELETE FROM orphan_panes WHERE project_id = ? AND pane_id = ?",
          )
          .run(this.kernel.projectId, paneId);
        return {
          value: { cleared: true },
          event: {
            entityType: "orphan_pane",
            entityId: paneId,
            stateVersion: 0,
            details: { cleared: true },
          },
        };
      },
    );
  }

  orphanPanes(
    credential: string,
  ): readonly { readonly paneId: string; readonly agentId: string }[] {
    return this.orphanPaneTerminals(credential).map((row) => ({
      paneId: row.paneId,
      agentId: row.agentId,
    }));
  }

  /** The orphan panes with the Herdr terminal id each had when it was recorded (null when it was not known). */
  orphanPaneTerminals(credential: string): readonly {
    readonly paneId: string;
    readonly agentId: string;
    readonly terminalId: string | null;
  }[] {
    this.kernel.authorize(credential, "controller:reconcile");
    return (
      this.kernel.database
        .prepare(
          "SELECT pane_id, agent_id, terminal_id FROM orphan_panes WHERE project_id = ? ORDER BY created_at, pane_id",
        )
        .all(this.kernel.projectId) as Array<{
        pane_id: string;
        agent_id: string;
        terminal_id: string | null;
      }>
    ).map((row) => ({
      paneId: row.pane_id,
      agentId: row.agent_id,
      terminalId: row.terminal_id,
    }));
  }

  /** The Herdr terminal id recorded with an agent's pane; null when there is no row or it was not known. */
  paneTerminalId(credential: string, agentId: string): string | null {
    this.kernel.authorize(credential, "controller:reconcile");
    const row = this.kernel.database
      .prepare(
        "SELECT terminal_id FROM agent_panes WHERE project_id = ? AND agent_id = ?",
      )
      .get(this.kernel.projectId, agentId) as
      { terminal_id: string | null } | undefined;
    return row?.terminal_id ?? null;
  }

  agentPanes(credential: string): readonly AgentPaneRecord[] {
    this.kernel.authorize(credential, "controller:reconcile");
    return (
      this.kernel.database
        .prepare(
          "SELECT agent_id, workspace_id, pane_id, worktree_path, branch, base_sha, generation, task_ref, task_title FROM agent_panes WHERE project_id = ? ORDER BY agent_id",
        )
        .all(this.kernel.projectId) as Array<{
        agent_id: string;
        workspace_id: string | null;
        pane_id: string | null;
        worktree_path: string | null;
        branch: string | null;
        base_sha: string | null;
        generation: number;
        task_ref: string | null;
        task_title: string | null;
      }>
    ).map((row) => ({
      agentId: row.agent_id,
      workspaceId: row.workspace_id,
      paneId: row.pane_id,
      worktreePath: row.worktree_path,
      branch: row.branch,
      baseSha: row.base_sha,
      generation: row.generation,
      taskRef: row.task_ref,
      taskTitle: row.task_title,
    }));
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
    this.kernel.authorize(credential, "plan:read");
    const slash = ref.indexOf("/");
    if (slash < 0) {
      safeId(ref, "requirement ref id");
      const link = this.areas.links.linkRow("requirement", ref);
      return {
        kind: "requirement",
        taskId: link?.external_id ?? ref,
        type: null,
        title: ref,
      };
    }
    const planId = ref.slice(0, slash);
    const packageId = ref.slice(slash + 1);
    safeId(planId, "plan id");
    safeId(packageId, "package id");
    const plan = this.areas.plans.planRow(planId);
    if (plan === undefined)
      throw new ControllerError(`plan ${planId} does not exist`);
    const revision = this.kernel.database
      .prepare(
        "SELECT body_json FROM plan_revisions WHERE project_id = ? AND plan_id = ? AND revision = ?",
      )
      .get(this.kernel.projectId, planId, plan.approved_revision) as
      { body_json: string } | undefined;
    const view =
      revision === undefined
        ? undefined
        : packageOfBody(revision.body_json, packageId);
    if (revision === undefined || view === undefined)
      throw new ControllerError(
        `plan ${planId} has no approved package ${packageId}`,
      );
    const link = this.areas.links.linkRow("package", `${planId}/${packageId}`);
    return {
      kind: "package",
      taskId: link?.external_id ?? `${planId}-${packageId}`,
      type: packageNaming(revision.body_json, packageId).type,
      title: view.title,
    };
  }

  /** Whether the agent has made any report, accepted or not. */
  agentHasReports(agentId: string): boolean {
    this.kernel.assertOpen();
    safeId(agentId, "agent id");
    return (
      this.kernel.database
        .prepare(
          "SELECT 1 AS present FROM agent_reports WHERE project_id = ? AND agent_id = ? LIMIT 1",
        )
        .get(this.kernel.projectId, agentId) !== undefined
    );
  }

  /** The active agent whose recorded branch is `branch`, other than `exceptAgentId`; undefined when none. */
  activeBranchHolder(
    branch: string,
    exceptAgentId?: string,
  ): string | undefined {
    this.kernel.assertOpen();
    const row = this.kernel.database
      .prepare(
        `SELECT p.agent_id FROM agent_panes p JOIN agents a ON a.project_id = p.project_id AND a.agent_id = p.agent_id
         WHERE p.project_id = ? AND p.branch = ? AND a.state = 'active' AND p.agent_id <> ? LIMIT 1`,
      )
      .get(this.kernel.projectId, branch, exceptAgentId ?? "") as
      { agent_id: string } | undefined;
    return row?.agent_id;
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
    safeId(input.agentId, "agent id");
    for (const name of [input.from, input.to])
      if (!/^[A-Za-z0-9][A-Za-z0-9._/-]{0,99}$/.test(name))
        throw new TypeError("branch name is not acceptable");
    return this.kernel.mutate(
      context,
      "agent_pane.rename_branch",
      "controller:reconcile",
      { ...input },
      () => {
        const agent = this.kernel.agentRow(input.agentId);
        if (agent?.state !== "active")
          throw new ControllerError("a branch is renamed for an active agent");
        const row = this.kernel.database
          .prepare(
            "SELECT branch FROM agent_panes WHERE project_id = ? AND agent_id = ?",
          )
          .get(this.kernel.projectId, input.agentId) as
          { branch: string | null } | undefined;
        if (row?.branch !== input.from)
          throw new ControllerError(
            `${input.agentId} does not have the recorded branch ${input.from}`,
          );
        this.kernel.database
          .prepare(
            "UPDATE agent_panes SET branch = ?, updated_at = ? WHERE project_id = ? AND agent_id = ?",
          )
          .run(
            input.to,
            this.kernel.now(),
            this.kernel.projectId,
            input.agentId,
          );
        return {
          value: { renamed: true as const },
          event: {
            entityType: "agent_pane",
            entityId: input.agentId,
            stateVersion: agent.generation,
            toState: "branch_renamed",
            details: { from: input.from, to: input.to },
          },
        };
      },
    );
  }

  fallbackPane(
    credential: string,
  ): { readonly workspaceId: string; readonly paneId: string } | undefined {
    this.kernel.authorize(credential, "controller:reconcile");
    const row = this.kernel.database
      .prepare(
        "SELECT workspace_id, pane_id FROM fallback_panes WHERE project_id = ?",
      )
      .get(this.kernel.projectId) as
      { workspace_id: string; pane_id: string } | undefined;
    return row === undefined
      ? undefined
      : { workspaceId: row.workspace_id, paneId: row.pane_id };
  }
}
