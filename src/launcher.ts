/**
 * Starts, restarts and re-adopts the agents that live in Herdr panes: the PM,
 * workers spawned into their own git worktrees, and the fallback watch pane.
 * It runs inside the daemon, which owns the adapter, and does one operation at
 * a time. Every core write builds its own context at the moment of the call.
 */
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { randomUUID } from "node:crypto";
import type { CapstanConfig, ResolvedRole } from "./config/capstan-config.js";
import { newContext } from "./context.js";
import type { ControllerCore, PmRestartSummary } from "./controller/core.js";
import type { AgentRecord } from "./controller/types.js";
import {
  AgentPaneMismatch,
  PaneGone,
  buildAgentEnvironment,
  claudeArguments,
  type HerdrAdapter,
} from "./herdr/adapter.js";
import { buildRolePrompt, CSTAN_ALLOW_RULE } from "./prompts.js";
import { DEFAULT_WAIT_TIMEOUT_SECONDS } from "./config/capstan-config.js";

export const STEP_BUDGET_MS = 60_000;
export const CLEANUP_BUDGET_MS = 30_000;
export const ADOPT_BUDGET_MS = 20_000;
export const MAX_WAITING_OPERATIONS = 1;
const START_TIMEOUT_MS = 30_000;
const DIALOG_TIMEOUT_MS = 10_000;
const ID_ATTEMPTS = 3;

/** The adapter methods the launcher uses; tests stub them. */
export type LauncherAdapter = Pick<
  HerdrAdapter,
  | "createWorkspace"
  | "createWorktree"
  | "prepareShell"
  | "startAgent"
  | "answerTrustDialog"
  | "closePane"
  | "adoptPane"
  | "adoptShellPane"
  | "forgetPane"
  | "runInPane"
  | "writePromptFile"
  | "paneForAgent"
  | "paneEntry"
  | "agentObservation"
>;

export interface GitRunner {
  headSha(): string;
  /** Removes a worktree without forcing; false when git refuses (for example untracked files). */
  worktreeRemove(worktreePath: string): boolean;
  /** Atomic compare-and-delete: only when the branch still points at `sha`. */
  deleteBranchIf(branch: string, sha: string): boolean;
  worktreeByBranch(branch: string): string | undefined;
}

export class LauncherError extends Error {
  override readonly name = "LauncherError";
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

export type LaunchState =
  "started" | "running" | "blocked" | "needs_restart" | "failed";

export interface LaunchResult {
  readonly state: LaunchState;
  readonly agentId?: string;
  readonly paneId?: string;
  readonly generation?: number;
  readonly hint?: string;
  readonly reason?: string;
  readonly step?: string;
  /** The long-lived hub workspace (it runs `cstan status --watch`): workers' worktrees hang under it, so a PM pane can be closed and replaced. */
  readonly hub?: "opened" | "present" | "failed";
}

export interface SpawnResult {
  readonly state: "started" | "blocked";
  readonly agentId: string;
  readonly paneId: string;
  readonly worktreePath: string;
  readonly branch: string;
  readonly hint?: string;
}

export interface LauncherStatus {
  readonly cleanupFailed: readonly {
    readonly agentId: string;
    readonly reason: string;
    readonly worktreePath?: string;
  }[];
  readonly orphanPanes: readonly {
    readonly agentId: string;
    readonly paneId: string;
  }[];
}

export interface LauncherOptions {
  readonly core: ControllerCore;
  readonly adapter: LauncherAdapter;
  readonly config: CapstanConfig;
  readonly projectRoot: string;
  readonly cliPath: string;
  readonly socketPath: string;
  /** The project (operator) credential. */
  readonly credential: string;
  readonly nodePath?: string;
  readonly baseEnvironment?: NodeJS.ProcessEnv;
  readonly git?: GitRunner;
  readonly now?: () => number;
  readonly log?: (event: string, details: Record<string, unknown>) => void;
}

interface Budget {
  readonly deadline: number;
  check(step: string): void;
}

function shellQuote(value: string): string {
  return `'${value.replaceAll("'", "'\\''")}'`;
}

export function defaultGit(projectRoot: string): GitRunner {
  const git = (args: string[]) =>
    spawnSync("git", args, {
      cwd: projectRoot,
      encoding: "utf8",
      timeout: 30_000,
    });
  return {
    headSha() {
      const result = git(["rev-parse", "HEAD"]);
      const sha = result.stdout.trim();
      if (result.status !== 0 || !/^[0-9a-f]{40}$/.test(sha))
        throw new LauncherError(
          "git_error",
          "the project has no commit to branch from",
        );
      return sha;
    },
    worktreeRemove: (worktreePath) =>
      git(["worktree", "remove", worktreePath]).status === 0,
    deleteBranchIf: (branch, sha) =>
      git(["update-ref", "-d", `refs/heads/${branch}`, sha]).status === 0,
    worktreeByBranch(branch) {
      const result = git(["worktree", "list", "--porcelain", "-z"]);
      if (result.status !== 0) return undefined;
      let current: string | undefined;
      for (const line of result.stdout.split("\0")) {
        if (line.startsWith("worktree "))
          current = line.slice("worktree ".length);
        if (line === `branch refs/heads/${branch}`) return current;
      }
      return undefined;
    },
  };
}

export class Launcher {
  readonly #core: ControllerCore;
  readonly #adapter: LauncherAdapter;
  readonly #config: CapstanConfig;
  readonly #root: string;
  readonly #cliPath: string;
  readonly #socketPath: string;
  readonly #credential: string;
  readonly #node: string;
  readonly #baseEnvironment: NodeJS.ProcessEnv;
  readonly #git: GitRunner;
  readonly #now: () => number;
  readonly #log: (event: string, details: Record<string, unknown>) => void;
  #tail: Promise<unknown> = Promise.resolve();
  #active = 0;
  #cleanupFailed: LauncherStatus["cleanupFailed"][number][] = [];
  #orphanPanes: LauncherStatus["orphanPanes"][number][] = [];

  constructor(options: LauncherOptions) {
    this.#core = options.core;
    this.#adapter = options.adapter;
    this.#config = options.config;
    this.#root = options.projectRoot;
    this.#cliPath = options.cliPath;
    this.#socketPath = options.socketPath;
    this.#credential = options.credential;
    this.#node = options.nodePath ?? process.execPath;
    this.#baseEnvironment = options.baseEnvironment ?? process.env;
    this.#git = options.git ?? defaultGit(options.projectRoot);
    this.#now = options.now ?? Date.now;
    this.#log = options.log ?? (() => undefined);
  }

  status(): LauncherStatus {
    // A worktree that could not be removed keeps its ledger row, so it is
    // reported from the ledger and survives a daemon restart.
    const leftovers = this.#core
      .agentPanes(this.#credential)
      .filter((row) => this.#core.agentRecord(row.agentId)?.state === "ended")
      .map((row) => ({
        agentId: row.agentId,
        reason:
          row.worktreePath === null
            ? "a record of an ended agent is waiting to be cleaned up"
            : "the worktree could not be removed without force",
        ...(row.worktreePath === null
          ? {}
          : { worktreePath: row.worktreePath }),
      }));
    // An entry for an agent that has since ended is no longer a problem.
    const blocked = this.#cleanupFailed.filter(
      (entry) => this.#core.agentRecord(entry.agentId)?.state === "active",
    );
    return {
      cleanupFailed: [...blocked, ...leftovers],
      orphanPanes: [...this.#orphanPanes],
    };
  }

  // ---------------------------------------------------------------- queue

  #run<T>(operation: (budget: Budget) => Promise<T>): Promise<T> {
    // One operation runs, up to MAX_WAITING_OPERATIONS wait behind it.
    if (this.#active >= 1 + MAX_WAITING_OPERATIONS)
      return Promise.reject(
        new LauncherError(
          "busy",
          "another launcher operation is already waiting",
        ),
      );
    this.#active += 1;
    const result = this.#tail
      .then(() => operation(this.#budget(STEP_BUDGET_MS)))
      .finally(() => {
        this.#active -= 1;
      });
    this.#tail = result.catch(() => undefined);
    return result;
  }

  #budget(ms: number): Budget {
    const deadline = this.#now() + ms;
    return {
      deadline,
      check: (step) => {
        if (this.#now() > deadline)
          throw new LauncherError(
            "timeout",
            `the operation ran out of time before ${step}`,
          );
      },
    };
  }

  #context() {
    return newContext(this.#core, this.#credential);
  }

  // ------------------------------------------------------------- identity

  #pmRole(): ResolvedRole {
    const role = this.#config.roles.find((r) => r.kind === "PM");
    if (role === undefined)
      throw new LauncherError(
        "not_configured",
        "the configuration has no PM role",
      );
    return role;
  }

  #waitSeconds(role: ResolvedRole): number {
    return (
      this.#config.hosts.find((h) => h.name === role.host)
        ?.waitTimeoutSeconds ?? DEFAULT_WAIT_TIMEOUT_SECONDS
    );
  }

  #activeAgents(): AgentRecord[] {
    return this.#core.listAgents().filter((a) => a.state === "active");
  }

  #seat(role: ResolvedRole): string {
    const seatId = `${role.name}-seat`;
    const existing = this.#core
      .statusSnapshot()
      .roles.find((entry) => entry.seatId === seatId);
    if (existing === undefined) {
      this.#core.createSeat(this.#context(), {
        seatId,
        name: role.name,
        role: role.kind,
      });
      return seatId;
    }
    if (existing.role !== role.kind)
      throw new LauncherError(
        "seat_kind_mismatch",
        `the seat ${seatId} was created for kind ${existing.role} but the configuration says ${role.kind}`,
      );
    if (existing.seatState === "disabled")
      throw new LauncherError(
        "seat_disabled",
        `the seat ${seatId} is disabled`,
      );
    return seatId;
  }

  #createAgent(role: ResolvedRole): { agentId: string; credential: string } {
    const seatId = this.#seat(role);
    const used = this.#core
      .listAgents()
      .filter((a) => a.roleName === role.name).length;
    let lastError: unknown;
    for (let attempt = 1; attempt <= ID_ATTEMPTS; attempt += 1) {
      const agentId = `${role.name}-${used + attempt}`;
      if (this.#core.agentRecord(agentId) !== undefined) continue;
      // Only the crash window between createActor and registerAgent can leave
      // an active actor on the seat that no active agent owns.
      const owned = new Set(this.#activeAgents().map((a) => a.actorId));
      for (const actorId of this.#core.seatActorIds(this.#credential, seatId))
        if (!owned.has(actorId))
          this.#core.revokeActor(this.#context(), actorId);
      const actor = this.#core.createActor(this.#context(), {
        displayName: agentId,
        role: role.kind,
        seatId,
      });
      try {
        this.#core.registerAgent(this.#context(), {
          agentId,
          roleName: role.name,
          seatId,
          actorId: actor.actorId,
        });
        return { agentId, credential: actor.credential };
      } catch (error) {
        lastError = error;
        try {
          this.#core.revokeActor(this.#context(), actor.actorId);
        } catch (revokeError) {
          this.#log("actor_not_revoked", { error: String(revokeError) });
        }
      }
    }
    throw (
      lastError ??
      new LauncherError("id_conflict", "no free agent id was found")
    );
  }

  // ---------------------------------------------------------- environment

  #ensureWrapper(): string {
    const directory = path.join(this.#root, ".capstan", "bin");
    fs.mkdirSync(directory, { recursive: true, mode: 0o700 });
    fs.chmodSync(directory, 0o700);
    const temporary = path.join(directory, `cstan.tmp-${randomUUID()}`);
    fs.writeFileSync(
      temporary,
      `#!/bin/sh\nexec ${shellQuote(this.#node)} ${shellQuote(this.#cliPath)} "$@"\n`,
      { mode: 0o700 },
    );
    fs.renameSync(temporary, path.join(directory, "cstan"));
    return directory;
  }

  /** The one environment every agent starts with: the allowlist, its token and socket, and `cstan` first on PATH. */
  #environment(token: string | null): Record<string, string> {
    const bin = this.#ensureWrapper();
    const basePath = this.#baseEnvironment.PATH;
    const extras: Record<string, string> = {
      PATH:
        basePath === undefined || basePath === "" ? bin : `${bin}:${basePath}`,
    };
    if (token !== null) {
      extras.CAPSTAN_TOKEN = token;
      extras.CAPSTAN_SOCKET = this.#socketPath;
    }
    return buildAgentEnvironment(this.#baseEnvironment, extras);
  }

  #arguments(role: ResolvedRole, promptFile: string): string[] {
    const allow = role.allow.includes(CSTAN_ALLOW_RULE)
      ? role.allow
      : [...role.allow, CSTAN_ALLOW_RULE];
    return claudeArguments({ ...role, allow }, promptFile);
  }

  // ---------------------------------------------------------------- start

  async #startPm(
    role: ResolvedRole,
    agent: { agentId: string; credential: string },
    budget: Budget,
    summary?: PmRestartSummary,
  ): Promise<LaunchResult> {
    budget.check("the PM prompt");
    const promptFile = this.#adapter.writePromptFile(
      buildRolePrompt({
        roleName: role.name,
        kind: "PM",
        agentId: agent.agentId,
        waitTimeoutSeconds: this.#waitSeconds(role),
        rolePrompt: role.promptText,
        ...(summary === undefined ? {} : { restartSummary: summary }),
      }),
    );
    const workspace = await this.#adapter.createWorkspace({
      cwd: this.#root,
      label: `capstan-${role.name}`,
      role: "PM",
    });
    try {
      budget.check("starting the PM");
      const started = await this.#adapter.startAgent({
        name: agent.agentId,
        kind: "claude",
        paneId: workspace.paneId,
        args: this.#arguments(role, promptFile),
        environment: this.#environment(agent.credential),
        timeoutMs: START_TIMEOUT_MS,
      });
      this.#core.recordAgentPane(this.#context(), {
        agentId: agent.agentId,
        workspaceId: workspace.workspaceId,
        paneId: workspace.paneId,
        worktreePath: null,
        branch: null,
        baseSha: null,
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
        await this.#adapter.closePane(workspace.paneId);
      } catch (closeError) {
        this.#log("pane_not_closed", {
          paneId: workspace.paneId,
          error: String(closeError),
        });
      }
      throw error;
    }
  }

  async #ensureHub(budget: Budget): Promise<NonNullable<LaunchResult["hub"]>> {
    const row = this.#core.fallbackPane(this.#credential);
    if (row !== undefined) {
      if (this.#adapter.paneEntry(row.paneId) !== undefined) return "present";
      // Re-adopt before opening another hub: a second one would hide the
      // worktrees that hang under the first.
      try {
        await this.#adapter.adoptShellPane(row.paneId, row.workspaceId);
        return "present";
      } catch (error) {
        if (!(error instanceof PaneGone)) {
          this.#log("hub_adopt_failed", { error: String(error) });
          return "failed";
        }
        this.#core.clearFallbackPane(this.#context());
      }
    }
    try {
      budget.check("opening the watch pane");
      const workspace = await this.#adapter.createWorkspace({
        cwd: this.#root,
        label: "capstan-watch",
        role: "worker",
      });
      await this.#adapter.prepareShell({
        paneId: workspace.paneId,
        environment: this.#environment(null),
      });
      await this.#adapter.runInPane(
        workspace.paneId,
        `cd ${shellQuote(this.#root)} && exec ${shellQuote(this.#node)} ${shellQuote(this.#cliPath)} status --watch`,
      );
      this.#core.recordFallbackPane(this.#context(), {
        workspaceId: workspace.workspaceId,
        paneId: workspace.paneId,
      });
      return "opened";
    } catch (error) {
      this.#log("fallback_pane_failed", { error: String(error) });
      return "failed";
    }
  }

  // ---------------------------------------------------------- operations

  launchPm(): Promise<LaunchResult> {
    return this.#run(async (budget) => {
      await this.#adoptAll(this.#budget(ADOPT_BUDGET_MS));
      const role = this.#pmRole();
      const active = this.#activeAgents().filter((a) => a.kind === "PM");
      if (active.length > 1)
        throw new LauncherError(
          "pm_exists",
          "more than one PM agent is active",
        );
      const existing = active[0];
      if (existing !== undefined) {
        const paneId = this.#adapter.paneForAgent(existing.agentId);
        if (paneId === undefined)
          return {
            state: "needs_restart",
            agentId: existing.agentId,
            hint: "the PM has no live pane; run cstan pm restart",
          };
        let state: LaunchState = "running";
        try {
          if (
            (await this.#adapter.agentObservation(existing.agentId)) ===
            "blocked"
          )
            state = "blocked";
        } catch (error) {
          this.#log("observe_failed", {
            agentId: existing.agentId,
            error: String(error),
          });
        }
        return {
          state,
          agentId: existing.agentId,
          paneId,
          generation: existing.generation,
          hub: await this.#ensureHub(budget),
        };
      }
      const agent = this.#createAgent(role);
      let result: LaunchResult;
      try {
        result = await this.#startPm(role, agent, budget);
      } catch (error) {
        await this.#cleanupAgent(agent.agentId, {});
        return {
          state: "failed",
          agentId: agent.agentId,
          reason: error instanceof Error ? error.message : String(error),
          step: "start",
        };
      }
      return { ...result, hub: await this.#ensureHub(budget) };
    });
  }

  restartPm(): Promise<LaunchResult> {
    return this.#run(async (budget) => {
      await this.#adoptAll(this.#budget(ADOPT_BUDGET_MS));
      const active = this.#activeAgents().filter((a) => a.kind === "PM");
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
      const role = this.#config.roles.find((r) => r.name === agent.roleName);
      if (role === undefined)
        throw new LauncherError(
          "unknown_role",
          `the configuration has no role ${agent.roleName}`,
        );
      const replaced = this.#core.restartAgentGeneration(
        this.#context(),
        agent.agentId,
      );
      // The recorded pane counts even when adoption did not register it, so a
      // live old PM is closed or listed, never silently dropped.
      const oldPane =
        this.#adapter.paneForAgent(agent.agentId) ??
        this.#core
          .agentPanes(this.#credential)
          .find((r) => r.agentId === agent.agentId)?.paneId ??
        undefined;
      if (oldPane !== undefined) {
        try {
          await this.#adapter.closePane(oldPane);
        } catch (error) {
          if (!this.#orphanPanes.some((o) => o.paneId === oldPane))
            this.#orphanPanes.push({ agentId: agent.agentId, paneId: oldPane });
          this.#adapter.forgetPane(oldPane);
          this.#log("old_pane_not_closed", {
            paneId: oldPane,
            error: String(error),
          });
        }
      }
      this.#core.clearAgentPane(this.#context(), agent.agentId);
      if (!replaced.summary.truncated) {
        const listed = new Set(
          replaced.summary.messages.map((m) => m.messageId),
        );
        if (replaced.cancelledMessageIds.some((id) => !listed.has(id)))
          this.#log("summary_incomplete", { agentId: agent.agentId });
      }
      try {
        const started = await this.#startPm(
          role,
          { agentId: agent.agentId, credential: replaced.credential },
          budget,
          replaced.summary,
        );
        const latest = this.#core
          .pmRestarts(this.#credential, agent.agentId)
          .at(-1);
        if (latest !== undefined)
          this.#core.markPmRestartsConsumed(
            this.#context(),
            agent.agentId,
            latest.sequence,
          );
        return {
          ...started,
          generation: replaced.generation,
          hub: await this.#ensureHub(budget),
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

  spawn(roleName: string): Promise<SpawnResult> {
    return this.#run(async (budget) => {
      await this.#adoptAll(this.#budget(ADOPT_BUDGET_MS));
      const role = this.#config.roles.find((r) => r.name === roleName);
      if (role === undefined)
        throw new LauncherError(
          "unknown_role",
          `the configuration has no role ${roleName}`,
        );
      if (role.kind === "PM")
        throw new LauncherError(
          "kind_not_spawnable",
          "a PM is launched, not spawned",
        );
      const running = this.#activeAgents().find(
        (a) => a.roleName === role.name,
      );
      if (running !== undefined) {
        const failed = this.#cleanupFailed.find(
          (c) => c.agentId === running.agentId,
        );
        throw new LauncherError(
          "role_active",
          `${running.agentId} is already active for role ${role.name}${failed === undefined ? "" : ` (an earlier cleanup failed: ${failed.reason})`}`,
        );
      }
      const pm = this.#activeAgents().find((a) => a.kind === "PM");
      const pmPane =
        pm === undefined
          ? undefined
          : this.#core
              .agentPanes(this.#credential)
              .find((r) => r.agentId === pm.agentId);
      if (pmPane === undefined || pmPane.workspaceId === null)
        throw new LauncherError(
          "pm_not_launched",
          "launch the PM first with cstan start",
        );
      // Herdr refuses to close a pane whose workspace has worktree children, so
      // worktrees hang under the long-lived hub workspace, never under the PM's.
      if ((await this.#ensureHub(budget)) === "failed")
        throw new LauncherError(
          "hub_unavailable",
          "the hub workspace could not be opened",
        );
      const hub = this.#core.fallbackPane(this.#credential);
      if (hub === undefined)
        throw new LauncherError(
          "hub_unavailable",
          "the hub workspace could not be opened",
        );
      budget.check("creating the agent");
      const agent = this.#createAgent(role);
      const branch = `capstan/${agent.agentId}`;
      const info: {
        worktreePath?: string;
        paneId?: string;
        branch: string;
        baseSha?: string;
      } = { branch };
      try {
        const baseSha = this.#git.headSha();
        info.baseSha = baseSha;
        this.#core.recordAgentPane(this.#context(), {
          agentId: agent.agentId,
          workspaceId: null,
          paneId: null,
          worktreePath: null,
          branch,
          baseSha,
        });
        budget.check("creating the worktree");
        const tree = await this.#adapter.createWorktree({
          workspaceId: hub.workspaceId,
          branch,
          label: agent.agentId,
          base: baseSha,
        });
        info.worktreePath = tree.path;
        info.paneId = tree.paneId;
        this.#core.recordAgentPane(this.#context(), {
          agentId: agent.agentId,
          workspaceId: tree.workspaceId,
          paneId: tree.paneId,
          worktreePath: tree.path,
          branch,
          baseSha,
        });
        budget.check("starting the worker");
        const promptFile = this.#adapter.writePromptFile(
          buildRolePrompt({
            roleName: role.name,
            kind: role.kind,
            agentId: agent.agentId,
            waitTimeoutSeconds: this.#waitSeconds(role),
            rolePrompt: role.promptText,
          }),
        );
        const started = await this.#adapter.startAgent({
          name: agent.agentId,
          kind: "claude",
          paneId: tree.paneId,
          args: this.#arguments(role, promptFile),
          environment: this.#environment(agent.credential),
          timeoutMs: START_TIMEOUT_MS,
        });
        if (started.status === "started")
          return {
            state: "started",
            agentId: agent.agentId,
            paneId: tree.paneId,
            worktreePath: tree.path,
            branch,
          };
        const answered = await this.#adapter.answerTrustDialog({
          paneId: tree.paneId,
          timeoutMs: DIALOG_TIMEOUT_MS,
          log: (entry) => this.#log("trust_dialog_key", { ...entry }),
        });
        return {
          state: answered.handled ? "started" : "blocked",
          agentId: agent.agentId,
          paneId: tree.paneId,
          worktreePath: tree.path,
          branch,
          ...(answered.handled
            ? {}
            : {
                hint: `the trust dialog was left alone (${answered.reason}); answer it in the worker pane`,
              }),
        };
      } catch (error) {
        await this.#cleanupAgent(agent.agentId, info);
        throw error;
      }
    });
  }

  // ------------------------------------------------------------- cleanup

  /** endAgent, pane, worktree (never forced), branch (compare-and-delete), row; in that order. */
  async #cleanupAgent(
    agentId: string,
    info: {
      worktreePath?: string;
      paneId?: string;
      branch?: string;
      baseSha?: string;
    },
  ): Promise<void> {
    try {
      this.#core.endAgent(this.#context(), agentId);
    } catch (error) {
      const reason = error instanceof Error ? error.message : String(error);
      this.#cleanupFailed = this.#cleanupFailed.filter(
        (entry) => entry.agentId !== agentId,
      );
      this.#cleanupFailed.push({ agentId, reason });
      this.#log("cleanup_blocked", { agentId, error: String(error) });
      return;
    }
    await this.#releaseResources(agentId, info);
  }

  /**
   * Everything an ended agent still holds: its pane, its worktree (never
   * forced) and its branch (only at the base commit). The pane row goes last
   * and only when the worktree is gone, so a refused removal stays in the
   * ledger and the next start retries it.
   */
  async #releaseResources(
    agentId: string,
    info: {
      worktreePath?: string;
      paneId?: string;
      branch?: string;
      baseSha?: string;
    },
  ): Promise<void> {
    const budget = this.#budget(CLEANUP_BUDGET_MS);
    if (info.paneId !== undefined && this.#within(budget)) {
      try {
        await this.#adapter.closePane(info.paneId);
      } catch (error) {
        this.#log("pane_not_closed", {
          agentId,
          paneId: info.paneId,
          error: String(error),
        });
      }
    }
    let removed = true;
    const worktreePath =
      info.worktreePath ??
      (info.branch === undefined
        ? undefined
        : this.#git.worktreeByBranch(info.branch));
    if (worktreePath !== undefined) {
      removed = this.#git.worktreeRemove(worktreePath);
      if (!removed) {
        this.#log("worktree_kept", { agentId, worktreePath });
        return;
      }
    }
    if (info.branch !== undefined && info.baseSha !== undefined) {
      if (!this.#git.deleteBranchIf(info.branch, info.baseSha))
        this.#log("branch_kept", { agentId, branch: info.branch });
    }
    try {
      this.#core.clearAgentPane(this.#context(), agentId);
    } catch (error) {
      this.#log("pane_row_not_cleared", { agentId, error: String(error) });
    }
  }

  #within(budget: Budget): boolean {
    return this.#now() <= budget.deadline;
  }

  // ---------------------------------------------------------------- adopt

  /** Re-registers the panes recorded before a daemon restart; the daemon runs it in the background before it starts the driver. */
  adoptAll(): Promise<void> {
    return this.#run(() => this.#adoptAll(this.#budget(ADOPT_BUDGET_MS)));
  }

  async #adoptAll(budget: Budget): Promise<void> {
    for (const orphan of [...this.#orphanPanes]) {
      try {
        await this.#adapter.closePane(orphan.paneId);
        this.#orphanPanes = this.#orphanPanes.filter(
          (entry) => entry.paneId !== orphan.paneId,
        );
      } catch {
        // Still open; it stays listed.
      }
    }
    const rows = this.#core.agentPanes(this.#credential);
    const seen = new Set(rows.map((r) => r.agentId));
    for (const row of rows) {
      if (!this.#within(budget)) {
        this.#log("adopt_skipped", { agentId: row.agentId });
        continue;
      }
      const agent = this.#core.agentRecord(row.agentId);
      if (agent === undefined || agent.state !== "active") {
        await this.#releaseResources(row.agentId, {
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
        await this.#cleanupAgent(row.agentId, {
          ...(row.branch === null ? {} : { branch: row.branch }),
          ...(row.baseSha === null ? {} : { baseSha: row.baseSha }),
          ...(row.worktreePath === null
            ? {}
            : { worktreePath: row.worktreePath }),
        });
        continue;
      }
      if (this.#adapter.paneEntry(row.paneId) !== undefined) continue;
      try {
        await this.#adapter.adoptPane({
          paneId: row.paneId,
          role: agent.kind === "PM" ? "PM" : "worker",
          agent: row.agentId,
          workspaceId: row.workspaceId,
          worktreePath: row.worktreePath,
        });
      } catch (error) {
        if (error instanceof PaneGone || error instanceof AgentPaneMismatch) {
          this.#log("pane_lost", { agentId: row.agentId });
          this.#core.clearAgentPane(this.#context(), row.agentId);
          if (agent.kind !== "PM")
            await this.#cleanupAgent(row.agentId, {
              ...(row.worktreePath === null
                ? {}
                : { worktreePath: row.worktreePath }),
              ...(row.branch === null ? {} : { branch: row.branch }),
              ...(row.baseSha === null ? {} : { baseSha: row.baseSha }),
            });
        } else
          this.#log("adopt_failed", {
            agentId: row.agentId,
            error: String(error),
          });
      }
    }
    for (const agent of this.#activeAgents())
      if (agent.kind !== "PM" && !seen.has(agent.agentId)) {
        this.#log("crashed_spawn", { agentId: agent.agentId });
        await this.#cleanupAgent(agent.agentId, {});
      }
    const fallback = this.#core.fallbackPane(this.#credential);
    if (
      fallback !== undefined &&
      this.#adapter.paneEntry(fallback.paneId) === undefined
    ) {
      try {
        await this.#adapter.adoptShellPane(
          fallback.paneId,
          fallback.workspaceId,
        );
      } catch (error) {
        if (error instanceof PaneGone)
          this.#core.clearFallbackPane(this.#context());
        else this.#log("fallback_adopt_failed", { error: String(error) });
      }
    }
  }
}
