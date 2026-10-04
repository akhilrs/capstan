/** The project workspace, the PM start and restart, and the watch tab. */
import type { ResolvedRole } from "../config/capstan-config.js";
import type { PmRestartSummary } from "../controller/core.js";
import { PaneGone, shellQuote } from "../herdr/adapter.js";
import { HerdrError } from "../herdr/runner.js";
import { buildRolePrompt } from "../prompts.js";
import type { LauncherKernel } from "./kernel.js";
import {
  ADOPT_BUDGET_MS,
  LauncherError,
  START_TIMEOUT_MS,
  selfInvocation,
  type Budget,
  type Hub,
  type LaunchResult,
  type LaunchState,
} from "./shared.js";

export class HubOps {
  constructor(private readonly k: LauncherKernel) {}

  async startPm(
    role: ResolvedRole,
    agent: { agentId: string; credential: string },
    budget: Budget,
    hub: Hub,
    summary?: PmRestartSummary,
  ): Promise<LaunchResult> {
    budget.check("the PM prompt");
    const promptText = buildRolePrompt({
      roleName: role.name,
      kind: "PM",
      agentId: agent.agentId,
      waitTimeoutSeconds: this.k.waitSeconds(role),
      rolePrompt: role.promptText,
      workerRoles: this.k.workerRoles(),
      ...(this.k.architectPrompt() === undefined
        ? {}
        : { architect: this.k.architectPrompt()! }),
      ...(this.k.operatorPrompt() === undefined
        ? {}
        : { operator: this.k.operatorPrompt()! }),
      ...(this.k.researcherPrompt() === undefined
        ? {}
        : { researcher: this.k.researcherPrompt()! }),
      ...(this.k.config.promptRelay?.enabled === true
        ? { promptRelay: { enabled: true as const } }
        : {}),
      ...this.k.nexoraPrompt(),
      ...(summary === undefined ? {} : { restartSummary: summary }),
    });
    const promptFile = this.k.adapter.writePromptFile(promptText);
    if (hub.workspaceId === null)
      throw new LauncherError(
        "hub_failed",
        "the project workspace is not open, so the PM has nowhere to start",
      );
    // The hub's root pane is free for the PM only in the call that made the hub; a later start gets a tab of its own.
    const workspace =
      hub.freePmPane !== null
        ? { workspaceId: hub.workspaceId, paneId: hub.freePmPane }
        : {
            workspaceId: hub.workspaceId,
            paneId: (
              await this.k.adapter.createTab({
                workspaceId: hub.workspaceId,
                cwd: this.k.root,
                label: "pm",
                role: "PM",
              })
            ).paneId,
          };
    try {
      budget.check("starting the PM");
      const started = await this.k.adapter.startAgent({
        name: agent.agentId,
        kind: this.k.hostKind(role),
        paneId: workspace.paneId,
        args: this.k.arguments(role, { text: promptText, file: promptFile }),
        environment: this.k.environment(agent.credential),
        timeoutMs: START_TIMEOUT_MS,
      });
      this.k.agentsStarted += 1;
      this.k.core.recordAgentPane(this.k.context(), {
        agentId: agent.agentId,
        workspaceId: workspace.workspaceId,
        paneId: workspace.paneId,
        worktreePath: null,
        branch: null,
        baseSha: null,
      });
      await this.k.describe({
        paneId: workspace.paneId,
        workspaceId: workspace.workspaceId,
        agentId: agent.agentId,
        roleName: role.name,
      });
      return started.status === "started"
        ? { state: "started", agentId: agent.agentId, paneId: workspace.paneId }
        : {
            state: "blocked",
            agentId: agent.agentId,
            paneId: workspace.paneId,
            hint: "answer the trust dialog in the PM pane; the controller never answers it",
          };
    } catch (error) {
      try {
        await this.k.close(workspace.paneId);
      } catch (closeError) {
        this.k.log("pane_not_closed", {
          paneId: workspace.paneId,
          error: String(closeError),
        });
      }
      throw error;
    }
  }

  /** The project workspace carries the project's name, so its PM and the worker worktrees under it read as one group; display only. */
  async labelHub(workspaceId: string | null): Promise<void> {
    if (workspaceId === null) return;
    try {
      await this.k.adapter.renameWorkspace(workspaceId, this.k.project);
      await this.k.adapter.reportMetadata(
        { workspaceId },
        { project: this.k.project },
      );
    } catch (error) {
      this.k.log("describe_failed", { agentId: "watch", error: String(error) });
    }
  }

  /** The watch shell in a tab of the project workspace; it stays the recorded fallback pane. */
  async openWatchTab(workspaceId: string): Promise<string> {
    const tab = await this.k.adapter.createTab({
      workspaceId,
      cwd: this.k.root,
      label: "watch",
      role: "worker",
    });
    try {
      await this.k.adapter.prepareShell({
        paneId: tab.paneId,
        environment: this.k.environment(null),
      });
      await this.k.adapter.runInPane(
        tab.paneId,
        `cd ${shellQuote(this.k.root)} && exec ${selfInvocation(this.k.node, this.k.cliPath)} status --watch`,
      );
      this.k.core.recordFallbackPane(this.k.context(), {
        workspaceId,
        paneId: tab.paneId,
      });
    } catch (error) {
      // A tab with no recorded watch pane would be made again at every check.
      try {
        await this.k.close(tab.paneId);
      } catch (closeError) {
        this.k.log("pane_not_closed", {
          paneId: tab.paneId,
          error: String(closeError),
        });
      }
      throw error;
    }
    return tab.paneId;
  }

  async ensureHubWithoutPm(
    budget: Budget,
  ): Promise<NonNullable<LaunchResult["hub"]>> {
    const hub = await this.ensureHub(budget);
    await this.dropUnusedRoot(hub);
    return hub.status;
  }

  /** A hub made while a PM is already running has an empty root pane no PM will take; it goes, and the workspace keeps its watch tab. */
  async dropUnusedRoot(hub: Hub): Promise<void> {
    if (hub.freePmPane === null) return;
    try {
      await this.k.close(hub.freePmPane);
    } catch (error) {
      this.k.log("pane_not_closed", {
        paneId: hub.freePmPane,
        error: String(error),
      });
    }
  }

  /** The project workspace: tab 1 is the PM's, a `watch` tab keeps the workspace open when a PM pane is replaced, and worker worktrees hang under it. */
  async ensureHub(budget: Budget): Promise<Hub> {
    const row = this.k.core.fallbackPane(this.k.credential);
    if (row !== undefined) {
      if (this.k.adapter.paneEntry(row.paneId) !== undefined)
        return {
          status: "present",
          workspaceId: row.workspaceId,
          freePmPane: null,
        };
      // Re-adopt before opening another hub: a second one would hide the
      // worktrees that hang under the first.
      try {
        await this.k.adapter.adoptShellPane(row.paneId, row.workspaceId);
        await this.labelHub(row.workspaceId);
        return {
          status: "present",
          workspaceId: row.workspaceId,
          freePmPane: null,
        };
      } catch (error) {
        if (!(error instanceof PaneGone)) {
          this.k.log("hub_adopt_failed", { error: String(error) });
          return { status: "failed", workspaceId: null, freePmPane: null };
        }
      }
      // The watch pane is gone; its workspace may not be, and closing a PM pane in a workspace with worktree children needs another tab there. Only a workspace that Herdr says is gone is replaced: any other failure leaves the row alone, so no second hub hides the worktrees under the first.
      if (row.workspaceId !== null) {
        try {
          budget.check("opening the watch tab");
          await this.openWatchTab(row.workspaceId);
          return {
            status: "opened",
            workspaceId: row.workspaceId,
            freePmPane: null,
          };
        } catch (error) {
          this.k.log("watch_tab_failed", { error: String(error) });
          if (
            !(error instanceof HerdrError) ||
            error.code !== "workspace_not_found"
          )
            return { status: "failed", workspaceId: null, freePmPane: null };
        }
      }
      this.k.core.clearFallbackPane(this.k.context());
    }
    let workspace:
      { workspaceId: string; paneId: string; tabId: string } | undefined;
    try {
      budget.check("opening the project workspace");
      workspace = await this.k.adapter.createWorkspace({
        cwd: this.k.root,
        label: this.k.project,
        role: "PM",
      });
      await this.labelHub(workspace.workspaceId);
      try {
        await this.k.adapter.renameTab(workspace.tabId, "pm");
      } catch (error) {
        this.k.log("describe_failed", { agentId: "pm", error: String(error) });
      }
      await this.openWatchTab(workspace.workspaceId);
      return {
        status: "opened",
        workspaceId: workspace.workspaceId,
        freePmPane: workspace.paneId,
      };
    } catch (error) {
      this.k.log("fallback_pane_failed", { error: String(error) });
      if (workspace !== undefined)
        try {
          await this.k.close(workspace.paneId);
        } catch (closeError) {
          this.k.log("pane_not_closed", {
            paneId: workspace.paneId,
            error: String(closeError),
          });
        }
      // A configuration fault (a colon in the project path, say) must reach the operator as itself, not as an unopened workspace.
      if (error instanceof LauncherError) throw error;
      return { status: "failed", workspaceId: null, freePmPane: null };
    }
  }

  launchPm(): Promise<LaunchResult> {
    return this.k.runStarting(async (budget) => {
      await this.k.observer.adoptNow(this.k.budget(ADOPT_BUDGET_MS));
      const role = this.k.pmRole();
      const active = this.k.activeAgents().filter((a) => a.kind === "PM");
      if (active.length > 1)
        throw new LauncherError(
          "pm_exists",
          "more than one PM agent is active",
        );
      const existing = active[0];
      if (existing !== undefined) {
        const paneId = this.k.adapter.paneForAgent(existing.agentId);
        if (paneId === undefined)
          return {
            state: "needs_restart",
            agentId: existing.agentId,
            hint: "the PM has no live pane; run cstan pm restart",
          };
        let state: LaunchState = "running";
        try {
          if (
            (await this.k.adapter.agentObservation(existing.agentId)) ===
            "blocked"
          )
            state = "blocked";
        } catch (error) {
          this.k.log("observe_failed", {
            agentId: existing.agentId,
            error: String(error),
          });
        }
        return {
          state,
          agentId: existing.agentId,
          paneId,
          generation: existing.generation,
          hub: await this.ensureHubWithoutPm(budget),
        };
      }
      const agent = this.k.createAgent(role);
      let result: LaunchResult;
      let hubStatus: NonNullable<LaunchResult["hub"]> = "failed";
      let madeHub: Hub | undefined;
      try {
        const hub = await this.ensureHub(budget);
        madeHub = hub;
        hubStatus = hub.status;
        if (hub.status === "failed")
          throw new LauncherError(
            "hub_unavailable",
            "the project workspace could not be opened; check the Herdr session named in capstan.toml",
          );
        result = await this.startPm(role, agent, budget, hub);
      } catch (error) {
        await this.k.releaser.cleanupAgent(agent.agentId, {});
        // A start that failed before it took the new workspace's root pane leaves it empty.
        if (
          madeHub?.freePmPane != null &&
          this.k.adapter.paneEntry(madeHub.freePmPane) !== undefined
        )
          await this.dropUnusedRoot(madeHub);
        return {
          state: "failed",
          agentId: agent.agentId,
          reason: error instanceof Error ? error.message : String(error),
          step: "start",
        };
      }
      return { ...result, hub: hubStatus };
    });
  }

  restartPm(): Promise<LaunchResult> {
    return this.k.runStarting(async (budget) => {
      await this.k.observer.adoptNow(this.k.budget(ADOPT_BUDGET_MS));
      const active = this.k.activeAgents().filter((a) => a.kind === "PM");
      if (active.length === 0)
        throw new LauncherError(
          "no_pm",
          "no PM agent is active; run cstan start",
        );
      if (active.length > 1)
        throw new LauncherError(
          "pm_ambiguous",
          "more than one PM agent is active",
        );
      const agent = active[0]!;
      const role = this.k.config.roles.find((r) => r.name === agent.roleName);
      if (role === undefined)
        throw new LauncherError(
          "unknown_role",
          `the configuration has no role ${agent.roleName}`,
        );
      // The project workspace must exist, with its watch tab, before the old PM pane is closed: Herdr refuses to close the last pane of a workspace that has worktree children.
      const hub = await this.ensureHub(budget);
      if (hub.status === "failed")
        throw new LauncherError(
          "hub_unavailable",
          "the project workspace could not be opened",
        );
      const replaced = this.k.core.restartAgentGeneration(
        this.k.context(),
        agent.agentId,
      );
      // The recorded pane counts even when adoption did not register it, so a
      // live old PM is closed or listed, never silently dropped.
      const oldPane =
        this.k.adapter.paneForAgent(agent.agentId) ??
        this.k.core
          .agentPanes(this.k.credential)
          .find((r) => r.agentId === agent.agentId)?.paneId ??
        undefined;
      if (oldPane !== undefined) {
        try {
          await this.k.close(oldPane);
        } catch (error) {
          this.k.core.recordOrphanPane(this.k.context(), {
            agentId: agent.agentId,
            paneId: oldPane,
          });
          this.k.adapter.forgetPane(oldPane);
          this.k.log("old_pane_not_closed", {
            paneId: oldPane,
            error: String(error),
          });
        }
      }
      this.k.core.clearAgentPane(this.k.context(), agent.agentId);
      if (!replaced.summary.truncated) {
        const listed = new Set(
          replaced.summary.messages.map((m) => m.messageId),
        );
        if (replaced.cancelledMessageIds.some((id) => !listed.has(id)))
          this.k.log("summary_incomplete", { agentId: agent.agentId });
      }
      try {
        const started = await this.startPm(
          role,
          { agentId: agent.agentId, credential: replaced.credential },
          budget,
          hub,
          replaced.summary,
        );
        const latest = this.k.core
          .pmRestarts(this.k.credential, agent.agentId)
          .at(-1);
        if (latest !== undefined)
          this.k.core.markPmRestartsConsumed(
            this.k.context(),
            agent.agentId,
            latest.sequence,
          );
        return {
          ...started,
          generation: replaced.generation,
          hub: hub.status,
        };
      } catch (error) {
        return {
          state: "failed",
          agentId: agent.agentId,
          generation: replaced.generation,
          step: "start",
          reason: error instanceof Error ? error.message : String(error),
          hint: "the generation was replaced and its summary is recorded; run cstan pm restart again",
        };
      }
    });
  }
}
