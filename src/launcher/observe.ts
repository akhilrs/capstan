/** Status, observation of panes, prompt relay and re-adoption after a daemon restart. */
import path from "node:path";
import {
  AgentPaneMismatch,
  PaneGone,
  buildAgentEnvironment,
} from "../herdr/adapter.js";
import { workspaceLabel } from "../herdr/naming.js";
import type {
  CaptureOutcome,
  PromptAnswer,
  RelayOutcome,
} from "../herdr/prompt-relay.js";
import { sanitizeScreen } from "../observe.js";
import type { LauncherKernel } from "./kernel.js";
import {
  ADOPT_BUDGET_MS,
  LauncherError,
  type Budget,
  type LauncherStatus,
  type ObserveResult,
} from "./shared.js";

export class ObserveOps {
  constructor(private readonly k: LauncherKernel) {}

  status(): LauncherStatus {
    // A worktree that could not be removed keeps its ledger row, so it is
    // reported from the ledger and survives a daemon restart.
    const leftovers = this.k.core
      .agentPanes(this.k.credential)
      .filter((row) => this.k.core.agentRecord(row.agentId)?.state === "ended")
      .map((row) => ({
        agentId: row.agentId,
        reason:
          row.worktreePath === null
            ? "a record of an ended agent is waiting to be cleaned up"
            : "git could not remove the worktree",
        ...(row.worktreePath === null
          ? {}
          : { worktreePath: row.worktreePath }),
      }));
    // An entry for an agent that has since ended is no longer a problem.
    const blocked = this.k.cleanupFailed.filter(
      (entry) => this.k.core.agentRecord(entry.agentId)?.state === "active",
    );
    return {
      cleanupFailed: [...blocked, ...leftovers],
      orphanPanes: this.k.core.orphanPanes(this.k.credential),
    };
  }

  /** Reads the blocking permission prompt of an active worker; nothing is typed. */
  async capturePrompt(agentId: string): Promise<CaptureOutcome> {
    return await this.k.adapter.capturePrompt(this.k.activePane(agentId));
  }

  /** Types an answer to the worker's prompt when the screen still hashes to `promptSha`; every key sent is logged. */
  async answerPrompt(
    agentId: string,
    input: {
      promptSha: string;
      answer: PromptAnswer;
      beforeType: () => void | Promise<void>;
    },
  ): Promise<RelayOutcome> {
    const paneId = this.k.activePane(agentId);
    return await this.k.adapter.answerPrompt({
      paneId,
      promptSha: input.promptSha,
      answer: input.answer,
      beforeType: input.beforeType,
      log: (entry) => this.k.log("prompt_relay_key", { agentId, ...entry }),
    });
  }

  /**
   * Sends one Esc to a worker that Herdr shows working, so a pause takes hold
   * at once. Needs `[prompt_relay]`, whose key log it shares. True when the Esc
   * was sent.
   */
  async interrupt(agentId: string): Promise<boolean> {
    if (this.k.config.promptRelay?.enabled !== true)
      throw new LauncherError(
        "not_configured",
        "--interrupt needs [prompt_relay] enabled = true in capstan.toml",
      );
    const paneId = this.k.activePane(agentId);
    const outcome = await this.k.adapter.interruptWorking({
      paneId,
      log: (entry) => this.k.log("prompt_relay_key", { agentId, ...entry }),
    });
    return outcome.sent;
  }

  /** The recent screen of an active agent. A read: it does not wait for other launcher operations. */
  async observe(agentId: string, lines: number): Promise<ObserveResult> {
    const agent = this.k.core.agentRecord(agentId);
    if (agent === undefined || agent.state !== "active")
      throw new LauncherError("agent_not_active", "the agent is not active");
    const paneId =
      this.k.adapter.paneForAgent(agentId) ??
      this.k.core
        .agentPanes(this.k.credential)
        .find((row) => row.agentId === agentId)?.paneId ??
      undefined;
    if (paneId === undefined || paneId === null)
      throw new LauncherError("no_pane", "the agent has no pane recorded");
    let screen: string;
    try {
      screen = await this.k.adapter.readScreen(paneId, { lines });
    } catch (error) {
      this.k.log("observe_failed", { agentId, error: String(error) });
      throw new LauncherError(
        "pane_unreadable",
        "the agent's pane could not be read",
      );
    }
    let agentStatus: string | null = null;
    try {
      agentStatus = await this.k.adapter.agentObservation(agentId);
    } catch {
      agentStatus = null;
    }
    return {
      agentId,
      roleName: agent.roleName,
      kind: agent.kind,
      state: agent.state,
      agentStatus,
      text: sanitizeScreen(screen),
    };
  }

  /** The environment an approved Operator command runs in: the setup command's filtered environment, with every CAPSTAN_ variable absent and no `cstan` wrapper directory on PATH. */
  operatorEnvironment(): Record<string, string> {
    const wrapperDirectory = path.resolve(this.k.root, ".capstan", "bin");
    const basePath = this.k.baseEnvironment.PATH;
    const kept = (basePath ?? "")
      .split(":")
      .filter(
        (entry) => entry !== "" && path.resolve(entry) !== wrapperDirectory,
      );
    const environment = buildAgentEnvironment(
      this.k.baseEnvironment,
      {},
      this.k.config.env.pass,
    );
    for (const name of Object.keys(environment))
      if (name.startsWith("CAPSTAN_")) delete environment[name];
    if (kept.length > 0) environment.PATH = kept.join(":");
    else delete environment.PATH;
    return environment;
  }

  /** How many spawn, release or replace operations are running or waiting; a restart waits until it is zero. */
  inFlightOperations(): number {
    return this.k.active;
  }

  /** Re-registers the panes recorded before a daemon restart; the daemon runs it in the background before it starts the driver. */
  adoptAll(): Promise<void> {
    return this.k.run(() => this.adoptNow(this.k.budget(ADOPT_BUDGET_MS)));
  }

  async adoptNow(budget: Budget): Promise<void> {
    for (const orphan of this.k.core.orphanPaneTerminals(this.k.credential)) {
      try {
        if (orphan.terminalId === null) {
          // Without a terminal id an orphan cannot be told apart from the PM's new pane (same agent token): it is never closed, only dropped once Herdr no longer has it.
          if ((await this.k.adapter.paneIdentity(orphan.paneId)) !== undefined)
            continue;
        } else
          await this.k.closeOwned(orphan.paneId, {
            agentId: orphan.agentId,
            terminalId: orphan.terminalId,
          });
        this.k.core.clearOrphanPane(this.k.context(), orphan.paneId);
      } catch {
        // Still open; it stays listed.
      }
    }
    const rows = this.k.core.agentPanes(this.k.credential);
    const seen = new Set(rows.map((r) => r.agentId));
    for (const row of rows) {
      if (!this.k.within(budget)) {
        this.k.log("adopt_skipped", { agentId: row.agentId });
        continue;
      }
      const agent = this.k.core.agentRecord(row.agentId);
      if (agent === undefined || agent.state !== "active") {
        await this.k.releaser.releaseResources(row.agentId, {
          ...(row.paneId === null ? {} : { paneId: row.paneId }),
          ...(row.branch === null ? {} : { branch: row.branch }),
          ...(row.baseSha === null ? {} : { baseSha: row.baseSha }),
          ...(row.worktreePath === null
            ? {}
            : { worktreePath: row.worktreePath }),
        });
        continue;
      }
      if (row.paneId === null) {
        await this.k.releaser.cleanupAgent(row.agentId, {
          ...(row.branch === null ? {} : { branch: row.branch }),
          ...(row.baseSha === null ? {} : { baseSha: row.baseSha }),
          ...(row.worktreePath === null
            ? {}
            : { worktreePath: row.worktreePath }),
        });
        continue;
      }
      if (this.k.adapter.paneEntry(row.paneId) !== undefined) continue;
      try {
        await this.k.adapter.adoptPane({
          paneId: row.paneId,
          role: agent.kind === "PM" ? "PM" : "worker",
          agent: row.agentId,
          workspaceId: row.workspaceId,
          worktreePath: row.worktreePath,
        });
        await this.k.describe({
          paneId: row.paneId,
          workspaceId: row.workspaceId,
          agentId: row.agentId,
          roleName: agent.roleName,
          // A worker placed as a tab or pane shares the PM's workspace, and a PM in the project workspace shares the hub's name, so only a PM's own old workspace is relabelled here.
          ...(agent.kind === "PM" &&
          row.workspaceId !==
            this.k.core.fallbackPane(this.k.credential)?.workspaceId
            ? { label: workspaceLabel(this.k.project, agent.roleName) }
            : {}),
        });
      } catch (error) {
        if (error instanceof PaneGone || error instanceof AgentPaneMismatch) {
          this.k.log("pane_lost", { agentId: row.agentId });
          // Read before the row is cleared: the pane id may now be another agent's, and only this terminal id tells.
          const terminalId = this.k.core.paneTerminalId(
            this.k.credential,
            row.agentId,
          );
          this.k.core.clearAgentPane(this.k.context(), row.agentId);
          if (agent.kind !== "PM")
            await this.k.releaser.cleanupAgent(
              row.agentId,
              {
                paneId: row.paneId,
                terminalId,
                ...(this.k.releaser.interruptedMove(row.workspaceId)
                  ? { moveMayHaveHappened: true }
                  : {}),
                ...(row.worktreePath === null
                  ? {}
                  : { worktreePath: row.worktreePath }),
                ...(row.branch === null ? {} : { branch: row.branch }),
                ...(row.baseSha === null ? {} : { baseSha: row.baseSha }),
              },
              { lost: "found_dead_at_start", branch: row.branch },
            );
        } else
          this.k.log("adopt_failed", {
            agentId: row.agentId,
            error: String(error),
          });
      }
    }
    for (const agent of this.k.activeAgents())
      if (agent.kind !== "PM" && !seen.has(agent.agentId)) {
        this.k.log("crashed_spawn", { agentId: agent.agentId });
        await this.k.releaser.cleanupAgent(agent.agentId, {});
      }
    const fallback = this.k.core.fallbackPane(this.k.credential);
    if (
      fallback !== undefined &&
      this.k.adapter.paneEntry(fallback.paneId) === undefined
    ) {
      try {
        await this.k.adapter.adoptShellPane(
          fallback.paneId,
          fallback.workspaceId,
        );
        await this.k.hub.labelHub(fallback.workspaceId);
      } catch (error) {
        // A gone watch pane keeps its row: the workspace id in it lets the next hub check make the watch tab again in the same workspace.
        if (error instanceof PaneGone)
          this.k.log("fallback_pane_gone", { paneId: fallback.paneId });
        else this.k.log("fallback_adopt_failed", { error: String(error) });
      }
    }
  }
}
