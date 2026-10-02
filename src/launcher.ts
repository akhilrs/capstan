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
import type {
  CapstanConfig,
  HostKind,
  ResolvedRole,
} from "./config/capstan-config.js";
import { newContext } from "./context.js";
import type { ControllerCore, PmRestartSummary } from "./controller/core.js";
import type { AgentRecord } from "./controller/types.js";
import {
  AgentPaneMismatch,
  PaneGone,
  PaneLost,
  buildAgentEnvironment,
  shellQuote,
  claudeArguments,
  type HerdrAdapter,
} from "./herdr/adapter.js";
import { codexArguments, ompArguments } from "./herdr/hosts.js";
import { projectDisplayName, workspaceLabel } from "./herdr/naming.js";
import { HerdrError } from "./herdr/runner.js";
import { sanitizeScreen } from "./observe.js";
import { SeedTooLargeError, buildSeed, type SeedBase } from "./seed.js";
import { buildRolePrompt, CSTAN_ALLOW_RULE } from "./prompts.js";
import { choosePlacement, type LayoutPane } from "./layout.js";
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
  | "createTab"
  | "paneLayout"
  | "placePane"
  | "panesAtPath"
  | "prepareShell"
  | "startAgent"
  | "answerTrustDialog"
  | "closePane"
  | "adoptPane"
  | "adoptShellPane"
  | "reportMetadata"
  | "renameWorkspace"
  | "renameTab"
  | "forgetPane"
  | "runInPane"
  | "writePromptFile"
  | "paneForAgent"
  | "paneEntry"
  | "agentObservation"
  | "readScreen"
>;

export interface GitRunner {
  headSha(): string;
  /** Removes a worktree without forcing; false when git refuses (for example untracked files). */
  worktreeRemove(worktreePath: string): boolean;
  /** Atomic compare-and-delete: only when the branch still points at `sha`. */
  deleteBranchIf(branch: string, sha: string): boolean;
  worktreeByBranch(branch: string): string | undefined;
  /** Whether `capstan/...` names a valid branch, checked by git before a worktree is created for it. */
  branchNameValid(branch: string): boolean;
  /** The commit a branch points at, or null when there is no such branch. */
  branchTip(branch: string): string | null;
  /** Whether `sha` (40 lowercase hex characters) resolves as a commit and is an ancestor of one of the refs. */
  reachableCommit(sha: string, from: readonly string[]): boolean;
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
  /** Names listed in `[env] pass` that are not set where the daemon runs, so the agent does not have them. */
  readonly missingEnv?: readonly string[];
  /** Printed by the CLI; names the missing variables and where they have to be set. */
  readonly warning?: string;
}

export interface SpawnResult {
  readonly state: "started" | "blocked";
  readonly agentId: string;
  readonly paneId: string;
  readonly worktreePath: string;
  readonly branch: string;
  /** "pane": the worker is a split in the PM's tab; "tab": it has its own Herdr workspace. */
  readonly placement: "pane" | "tab";
  /** Why a pane-mode spawn stayed a tab. */
  readonly placementNote?: string;
  readonly hint?: string;
  /** Names listed in `[env] pass` that are not set where the daemon runs, so the agent does not have them. */
  readonly missingEnv?: readonly string[];
  /** Printed by the CLI; names the missing variables and where they have to be set. */
  readonly warning?: string;
}

export interface ObserveResult {
  readonly agentId: string;
  readonly roleName: string;
  readonly kind: string;
  readonly state: string;
  /** Herdr's own state for the pane (idle, working, blocked, done, unknown), or null when it could not be read. */
  readonly agentStatus: string | null;
  /** The agent's visible screen as plain text: its own output, not verified. */
  readonly text: string;
}

export interface ReleaseOutcome {
  /** null when no pane was recorded. */
  readonly paneClosed: boolean | null;
  /** null when no worktree was recorded or found. */
  readonly worktreeRemoved: boolean | null;
  /** True while the branch still exists: it holds commits, or its removal was not attempted. null when no branch was recorded. */
  readonly branchKept: boolean | null;
}

export interface ReleaseResult extends ReleaseOutcome {
  readonly state: "released";
  readonly agentId: string;
  readonly branch: string | null;
  readonly cancelledMessageIds: readonly string[];
}

/** What `cstan replace` answers: the new agent (as a spawn answers) and what happened to the old one, or why nothing was started. */
export type ReplaceResult =
  | (SpawnResult & {
      readonly predecessor: string;
      readonly baseSha: string;
      readonly baseSource: "predecessor" | "head";
      /** null when the predecessor was already ended. */
      readonly predecessorWorktreeRemoved: boolean | null;
      readonly cancelledMessageIds: readonly string[];
      /** False when the ledger could not record the replacement; the new agent runs all the same. */
      readonly replacementRecorded: boolean;
    })
  | {
      readonly state: "blocked";
      readonly predecessor: string;
      readonly reason: string;
    };

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
  /** Brings the configured roles into the ledger; the launcher calls it once more when a role is missing or out of date. */
  readonly syncRoles?: () => void;
}

interface Hub {
  readonly status: NonNullable<LaunchResult["hub"]>;
  readonly workspaceId: string | null;
  /** The root pane of a hub made in this call; the first PM takes it. */
  readonly freePmPane: string | null;
}

interface Budget {
  readonly deadline: number;
  check(step: string): void;
}

/** Escape sequences (CSI with any parameter bytes, OSC, DCS and the other string forms even when unterminated, the 8-bit C1 forms, and two-byte escapes), control and format characters, lone surrogates, line separators and runs of blanks are removed or become one space. The text is cut at a grapheme boundary and kept within `maxLength` UTF-16 units, so combining marks cannot stretch it; a first grapheme longer than that leaves nothing. */
function oneLine(text: string, maxLength: number): string {
  const clean = text
    .replace(
      /(?:\u001b\]|\u009d)[^\u0007\u001b\u009c]*(?:\u0007|\u001b\\|\u009c)?/g,
      "",
    )
    .replace(
      /(?:\u001b[PX^_]|[\u0090\u0098\u009e\u009f])[^\u001b\u009c]*(?:\u001b\\|\u009c)?/g,
      "",
    )
    .replace(/(?:\u001b\[|\u009b)[0-?]*[ -/]*[@-~]/g, "")
    .replace(/\u001b[ -/]*[0-~]/g, "")
    .replace(/\p{Cs}/gu, "")
    .replace(/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}\s]+/gu, " ")
    .trim();
  let result = "";
  for (const part of new Intl.Segmenter(undefined, {
    granularity: "grapheme",
  }).segment(clean)) {
    if (result.length + part.segment.length > maxLength) break;
    result += part.segment;
  }
  return result.trim();
}

/** Keeps a sync error to one short line in the refusal. */
const MAX_SYNC_REASON_CHARS = 200;
const MAX_NOTE_LENGTH = 200;

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
      const sha = typeof result.stdout === "string" ? result.stdout.trim() : "";
      if (/^[0-9a-f]{64}$/.test(sha))
        throw new LauncherError(
          "git_error",
          "this repository uses SHA-256 object names; only SHA-1 repositories are supported",
        );
      if (result.status !== 0 || !/^[0-9a-f]{40}$/.test(sha))
        throw new LauncherError(
          "git_error",
          "the project has no commit to branch from",
        );
      return sha;
    },
    worktreeRemove: (worktreePath) =>
      git(["worktree", "remove", worktreePath]).status === 0,
    branchTip(branch) {
      const result = git([
        "rev-parse",
        "--verify",
        "--quiet",
        `refs/heads/${branch}^{commit}`,
      ]);
      const sha = typeof result.stdout === "string" ? result.stdout.trim() : "";
      return result.status === 0 && /^[0-9a-f]{40}$/.test(sha) ? sha : null;
    },
    reachableCommit(sha, from) {
      if (!/^[0-9a-f]{40}$/.test(sha)) return false;
      // `--end-of-options` came with git 2.24; an older git must not be mistaken for "no such commit".
      const version = /git version (\d+)\.(\d+)/.exec(
        String(git(["--version"]).stdout ?? ""),
      );
      if (
        version === null ||
        Number(version[1]) < 2 ||
        (Number(version[1]) === 2 && Number(version[2]) < 24)
      )
        throw new LauncherError(
          "old_git",
          "git 2.24 or newer is needed to check a commit",
        );
      const verify = git([
        "rev-parse",
        "--verify",
        "--end-of-options",
        `${sha}^{commit}`,
      ]);
      if (verify.status !== 0) return false;
      return from.some(
        (ref) => git(["merge-base", "--is-ancestor", sha, ref]).status === 0,
      );
    },
    branchNameValid: (branch) =>
      git(["check-ref-format", `refs/heads/${branch}`]).status === 0,
    deleteBranchIf: (branch, sha) =>
      git(["update-ref", "-d", `refs/heads/${branch}`, sha]).status === 0,
    worktreeByBranch(branch) {
      const result = git(["worktree", "list", "--porcelain", "-z"]);
      // A failing git (or one too old for -z) must never read as "no worktree".
      if (result.status !== 0 || typeof result.stdout !== "string")
        throw new LauncherError(
          "git_error",
          "git could not list the worktrees",
        );
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
  readonly #project: string;
  readonly #cliPath: string;
  readonly #socketPath: string;
  readonly #credential: string;
  readonly #node: string;
  readonly #baseEnvironment: NodeJS.ProcessEnv;
  /** Predecessors whose replacement is running now. */
  readonly #replacing = new Set<string>();
  /** How many agents this launcher has started; an operation that raised it started one. */
  #agentsStarted = 0;
  readonly #git: GitRunner;
  readonly #now: () => number;
  readonly #log: (event: string, details: Record<string, unknown>) => void;
  readonly #syncRoles: (() => void) | undefined;
  #tail: Promise<unknown> = Promise.resolve();
  #active = 0;
  #cleanupFailed: LauncherStatus["cleanupFailed"][number][] = [];

  constructor(options: LauncherOptions) {
    this.#core = options.core;
    this.#adapter = options.adapter;
    this.#config = options.config;
    this.#root = options.projectRoot;
    this.#project = projectDisplayName(
      options.config.projectName,
      options.projectRoot,
    );
    this.#cliPath = options.cliPath;
    this.#socketPath = options.socketPath;
    this.#credential = options.credential;
    this.#node = options.nodePath ?? process.execPath;
    this.#baseEnvironment = options.baseEnvironment ?? process.env;
    this.#git = options.git ?? defaultGit(options.projectRoot);
    this.#now = options.now ?? Date.now;
    this.#log = options.log ?? (() => undefined);
    this.#syncRoles = options.syncRoles;
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
      orphanPanes: this.#core.orphanPanes(this.#credential),
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

  #workerRoles(): { name: string; kind: string }[] {
    return this.#config.roles
      .filter((role) => role.kind !== "PM")
      .map((role) => ({ name: role.name, kind: role.kind }));
  }

  #activeAgents(): AgentRecord[] {
    return this.#core.listAgents().filter((a) => a.state === "active");
  }

  /** The first of the role's seats that no active agent holds; an extra seat has the id `<role>-seat-<n>` and the display name `<role>.<n>`, which no role name can equal (role names allow no dot), so it never collides with another role's seat; the core allows one active agent per seat, so each concurrent worker needs its own. A disabled seat, or one made for another kind, is an operator-visible error and stops the walk even when a later seat is free; it is not something to route around. */
  #seat(role: ResolvedRole): string {
    const held = new Set(this.#activeAgents().map((a) => a.seatId));
    const seats = this.#core.statusSnapshot().roles;
    const attempts = role.kind === "PM" ? 1 : this.#config.limits.maxWorkers;
    for (let number = 1; number <= attempts; number += 1) {
      const seatId =
        number === 1 ? `${role.name}-seat` : `${role.name}-seat-${number}`;
      const existing = seats.find((entry) => entry.seatId === seatId);
      if (existing === undefined) {
        this.#core.createSeat(this.#context(), {
          seatId,
          name: number === 1 ? role.name : `${role.name}.${number}`,
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
      if (!held.has(seatId)) return seatId;
    }
    throw new LauncherError(
      "role_active",
      `every seat of the role ${role.name} is held by an active agent`,
    );
  }

  #roleIsSynced(role: ResolvedRole): boolean {
    const definition = this.#core
      .roleDefinitions()
      .find((candidate) => candidate.name === role.name);
    return (
      definition?.state === "active" &&
      definition.kind === role.kind &&
      definition.host === role.host &&
      definition.configHash === role.configHash
    );
  }

  #assertRoleSynced(role: ResolvedRole): void {
    if (this.#roleIsSynced(role)) return;
    let reason = "";
    try {
      this.#syncRoles?.();
    } catch (error) {
      const text = (error instanceof Error ? error.message : String(error))
        .replace(/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}\s]+/gu, " ")
        .trim();
      reason = ` (${Array.from(text).slice(0, MAX_SYNC_REASON_CHARS).join("")})`;
    }
    if (this.#roleIsSynced(role)) return;
    throw new LauncherError(
      "role_not_synced",
      `the role ${role.name} is not synced into the controller${reason}; run cstan stop, then cstan start, and look for role_sync_failed in the daemon log if it persists`,
    );
  }

  #createAgent(role: ResolvedRole): { agentId: string; credential: string } {
    this.#assertRoleSynced(role);
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

  /** The recent screen of an active agent. A read: it does not wait for other launcher operations. */
  async observe(agentId: string, lines: number): Promise<ObserveResult> {
    const agent = this.#core.agentRecord(agentId);
    if (agent === undefined || agent.state !== "active")
      throw new LauncherError("agent_not_active", "the agent is not active");
    const paneId =
      this.#adapter.paneForAgent(agentId) ??
      this.#core
        .agentPanes(this.#credential)
        .find((row) => row.agentId === agentId)?.paneId ??
      undefined;
    if (paneId === undefined || paneId === null)
      throw new LauncherError("no_pane", "the agent has no pane recorded");
    let screen: string;
    try {
      screen = await this.#adapter.readScreen(paneId, { lines });
    } catch (error) {
      this.#log("observe_failed", { agentId, error: String(error) });
      throw new LauncherError(
        "pane_unreadable",
        "the agent's pane could not be read",
      );
    }
    let agentStatus: string | null = null;
    try {
      agentStatus = await this.#adapter.agentObservation(agentId);
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

  /** Names from `[env] pass` that are not set (or are empty) in the daemon's environment: an agent started now would not have them. */
  #missingPassEnvironment(): string[] {
    return this.#config.env.pass.filter(
      (name) =>
        this.#baseEnvironment[name] === undefined ||
        this.#baseEnvironment[name] === "",
    );
  }

  /**
   * Runs an operation like `#run`. If it started an agent while a listed
   * variable was unset, the answer carries the names and a warning and the log
   * gets one entry. The count is read inside the operation, so overlapping
   * operations cannot see each other's starts.
   */
  #runStarting<T extends object>(
    operation: (budget: Budget) => Promise<T>,
  ): Promise<
    T & { readonly missingEnv?: readonly string[]; readonly warning?: string }
  > {
    return this.#run(async (budget) => {
      const before = this.#agentsStarted;
      const result = await operation(budget);
      return this.#agentsStarted > before
        ? this.#withMissingEnvironment(result)
        : result;
    });
  }

  #withMissingEnvironment<T extends object>(
    result: T,
  ): T & {
    readonly missingEnv?: readonly string[];
    readonly warning?: string;
  } {
    const missing = this.#missingPassEnvironment();
    if (missing.length === 0) return result;
    this.#log("env_pass_missing", { names: missing });
    return {
      ...result,
      missingEnv: missing,
      warning: `${missing.join(", ")} ${missing.length === 1 ? "is" : "are"} listed in [env] pass but not set where the daemon was started, so this agent does not have ${missing.length === 1 ? "it" : "them"}; set ${missing.length === 1 ? "it" : "them"} in the shell that runs cstan start (or its profile file), then restart the daemon`,
    };
  }

  /** The one environment every agent starts with: the allowlist, its token and socket, and `cstan` first on PATH. */
  #environment(token: string | null): Record<string, string> {
    if (this.#root.includes(":"))
      throw new LauncherError(
        "unsupported_root",
        "a project path with a colon cannot be put on PATH, so cstan would not be found",
      );
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
    return buildAgentEnvironment(
      this.#baseEnvironment,
      extras,
      token === null ? [] : this.#config.env.pass,
    );
  }

  #hostKind(role: ResolvedRole): HostKind {
    const host = this.#config.hosts.find((h) => h.name === role.host);
    if (host === undefined)
      throw new LauncherError(
        "unknown_host",
        `the configuration has no host ${role.host}`,
      );
    return host.kind;
  }

  #arguments(
    role: ResolvedRole,
    prompt: { readonly text: string; readonly file: string },
    worktreePath?: string,
  ): string[] {
    const kind = this.#hostKind(role);
    if (kind === "codex")
      return codexArguments(role, prompt.text, worktreePath);
    if (kind === "omp") return ompArguments(role, prompt.file);
    const allow = role.allow.includes(CSTAN_ALLOW_RULE)
      ? role.allow
      : [...role.allow, CSTAN_ALLOW_RULE];
    return claudeArguments({ ...role, allow }, prompt.file);
  }

  /** Tells Herdr which project, role and agent a pane belongs to, for the operator's sidebar; display only, so a failure is logged and never stops a start. */
  async #describe(input: {
    paneId: string;
    workspaceId: string | null;
    agentId: string;
    roleName: string;
    label?: string;
  }): Promise<void> {
    try {
      if (input.label !== undefined && input.workspaceId !== null)
        await this.#adapter.renameWorkspace(input.workspaceId, input.label);
      await this.#adapter.reportMetadata(
        { paneId: input.paneId },
        { project: this.#project, role: input.roleName, agent: input.agentId },
      );
      if (input.workspaceId !== null)
        await this.#adapter.reportMetadata(
          { workspaceId: input.workspaceId },
          { project: this.#project },
        );
    } catch (error) {
      this.#log("describe_failed", {
        agentId: input.agentId,
        error: String(error),
      });
    }
  }

  // ---------------------------------------------------------------- start

  async #startPm(
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
      waitTimeoutSeconds: this.#waitSeconds(role),
      rolePrompt: role.promptText,
      workerRoles: this.#workerRoles(),
      ...(summary === undefined ? {} : { restartSummary: summary }),
    });
    const promptFile = this.#adapter.writePromptFile(promptText);
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
              await this.#adapter.createTab({
                workspaceId: hub.workspaceId,
                cwd: this.#root,
                label: "pm",
                role: "PM",
              })
            ).paneId,
          };
    try {
      budget.check("starting the PM");
      const started = await this.#adapter.startAgent({
        name: agent.agentId,
        kind: this.#hostKind(role),
        paneId: workspace.paneId,
        args: this.#arguments(role, { text: promptText, file: promptFile }),
        environment: this.#environment(agent.credential),
        timeoutMs: START_TIMEOUT_MS,
      });
      this.#agentsStarted += 1;
      this.#core.recordAgentPane(this.#context(), {
        agentId: agent.agentId,
        workspaceId: workspace.workspaceId,
        paneId: workspace.paneId,
        worktreePath: null,
        branch: null,
        baseSha: null,
      });
      await this.#describe({
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
        await this.#close(workspace.paneId);
      } catch (closeError) {
        this.#log("pane_not_closed", {
          paneId: workspace.paneId,
          error: String(closeError),
        });
      }
      throw error;
    }
  }

  /** The project workspace carries the project's name, so its PM and the worker worktrees under it read as one group; display only. */
  async #labelHub(workspaceId: string | null): Promise<void> {
    if (workspaceId === null) return;
    try {
      await this.#adapter.renameWorkspace(workspaceId, this.#project);
      await this.#adapter.reportMetadata(
        { workspaceId },
        { project: this.#project },
      );
    } catch (error) {
      this.#log("describe_failed", { agentId: "watch", error: String(error) });
    }
  }

  /** The watch shell in a tab of the project workspace; it stays the recorded fallback pane. */
  async #openWatchTab(workspaceId: string): Promise<string> {
    const tab = await this.#adapter.createTab({
      workspaceId,
      cwd: this.#root,
      label: "watch",
      role: "worker",
    });
    try {
      await this.#adapter.prepareShell({
        paneId: tab.paneId,
        environment: this.#environment(null),
      });
      await this.#adapter.runInPane(
        tab.paneId,
        `cd ${shellQuote(this.#root)} && exec ${shellQuote(this.#node)} ${shellQuote(this.#cliPath)} status --watch`,
      );
      this.#core.recordFallbackPane(this.#context(), {
        workspaceId,
        paneId: tab.paneId,
      });
    } catch (error) {
      // A tab with no recorded watch pane would be made again at every check.
      try {
        await this.#close(tab.paneId);
      } catch (closeError) {
        this.#log("pane_not_closed", {
          paneId: tab.paneId,
          error: String(closeError),
        });
      }
      throw error;
    }
    return tab.paneId;
  }

  async #ensureHubWithoutPm(
    budget: Budget,
  ): Promise<NonNullable<LaunchResult["hub"]>> {
    const hub = await this.#ensureHub(budget);
    await this.#dropUnusedRoot(hub);
    return hub.status;
  }

  /** A hub made while a PM is already running has an empty root pane no PM will take; it goes, and the workspace keeps its watch tab. */
  async #dropUnusedRoot(hub: Hub): Promise<void> {
    if (hub.freePmPane === null) return;
    try {
      await this.#close(hub.freePmPane);
    } catch (error) {
      this.#log("pane_not_closed", {
        paneId: hub.freePmPane,
        error: String(error),
      });
    }
  }

  /** The project workspace: tab 1 is the PM's, a `watch` tab keeps the workspace open when a PM pane is replaced, and worker worktrees hang under it. */
  async #ensureHub(budget: Budget): Promise<Hub> {
    const row = this.#core.fallbackPane(this.#credential);
    if (row !== undefined) {
      if (this.#adapter.paneEntry(row.paneId) !== undefined)
        return {
          status: "present",
          workspaceId: row.workspaceId,
          freePmPane: null,
        };
      // Re-adopt before opening another hub: a second one would hide the
      // worktrees that hang under the first.
      try {
        await this.#adapter.adoptShellPane(row.paneId, row.workspaceId);
        await this.#labelHub(row.workspaceId);
        return {
          status: "present",
          workspaceId: row.workspaceId,
          freePmPane: null,
        };
      } catch (error) {
        if (!(error instanceof PaneGone)) {
          this.#log("hub_adopt_failed", { error: String(error) });
          return { status: "failed", workspaceId: null, freePmPane: null };
        }
      }
      // The watch pane is gone; its workspace may not be, and closing a PM pane in a workspace with worktree children needs another tab there. Only a workspace that Herdr says is gone is replaced: any other failure leaves the row alone, so no second hub hides the worktrees under the first.
      if (row.workspaceId !== null) {
        try {
          budget.check("opening the watch tab");
          await this.#openWatchTab(row.workspaceId);
          return {
            status: "opened",
            workspaceId: row.workspaceId,
            freePmPane: null,
          };
        } catch (error) {
          this.#log("watch_tab_failed", { error: String(error) });
          if (
            !(error instanceof HerdrError) ||
            error.code !== "workspace_not_found"
          )
            return { status: "failed", workspaceId: null, freePmPane: null };
        }
      }
      this.#core.clearFallbackPane(this.#context());
    }
    let workspace:
      { workspaceId: string; paneId: string; tabId: string } | undefined;
    try {
      budget.check("opening the project workspace");
      workspace = await this.#adapter.createWorkspace({
        cwd: this.#root,
        label: this.#project,
        role: "PM",
      });
      await this.#labelHub(workspace.workspaceId);
      try {
        await this.#adapter.renameTab(workspace.tabId, "pm");
      } catch (error) {
        this.#log("describe_failed", { agentId: "pm", error: String(error) });
      }
      await this.#openWatchTab(workspace.workspaceId);
      return {
        status: "opened",
        workspaceId: workspace.workspaceId,
        freePmPane: workspace.paneId,
      };
    } catch (error) {
      this.#log("fallback_pane_failed", { error: String(error) });
      if (workspace !== undefined)
        try {
          await this.#close(workspace.paneId);
        } catch (closeError) {
          this.#log("pane_not_closed", {
            paneId: workspace.paneId,
            error: String(closeError),
          });
        }
      // A configuration fault (a colon in the project path, say) must reach the operator as itself, not as an unopened workspace.
      if (error instanceof LauncherError) throw error;
      return { status: "failed", workspaceId: null, freePmPane: null };
    }
  }

  // ---------------------------------------------------------- operations

  launchPm(): Promise<LaunchResult> {
    return this.#runStarting(async (budget) => {
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
          hub: await this.#ensureHubWithoutPm(budget),
        };
      }
      const agent = this.#createAgent(role);
      let result: LaunchResult;
      let hubStatus: NonNullable<LaunchResult["hub"]> = "failed";
      try {
        const hub = await this.#ensureHub(budget);
        hubStatus = hub.status;
        if (hub.status === "failed")
          throw new LauncherError(
            "hub_unavailable",
            "the project workspace could not be opened; check the Herdr session named in capstan.toml",
          );
        result = await this.#startPm(role, agent, budget, hub);
      } catch (error) {
        await this.#cleanupAgent(agent.agentId, {});
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

  /**
   * Replaces a worker: releases it (when it is still active) and starts a new
   * agent of the same role whose prompt carries a seed built from the ledger.
   * Nothing the predecessor was asked to do is sent again.
   */
  async replace(agentId: string): Promise<ReplaceResult> {
    const agent = this.#core.agentRecord(agentId);
    if (agent === undefined)
      throw new LauncherError("unknown_agent", `there is no agent ${agentId}`);
    if (agent.kind === "PM")
      throw new LauncherError(
        "kind_not_replaceable",
        "the PM is restarted with cstan pm restart, not replaced",
      );
    if (this.#replacing.has(agentId))
      throw new LauncherError(
        "replace_running",
        `a replacement of ${agentId} is already running`,
      );
    if (this.#core.isAgentReplaced(agentId))
      throw new LauncherError(
        "already_replaced",
        `${agentId} was already replaced`,
      );
    if (this.#core.runningIntegrations(this.#credential).length > 0)
      throw new LauncherError(
        "integration_running",
        "an integration is running; replace after it finished",
      );
    const role = this.#config.roles.find((r) => r.name === agent.roleName);
    if (role === undefined)
      throw new LauncherError(
        "unknown_role",
        `the configuration has no role ${agent.roleName}`,
      );
    this.#assertRoleSynced(role);
    const data = this.#core.agentSeed(agentId);
    const tip = data.branch === null ? null : this.#git.branchTip(data.branch);
    const reachableFrom = [
      ...(tip === null ? [] : [`refs/heads/${data.branch}`]),
      "HEAD",
    ];
    const base: SeedBase =
      data.lastAcceptedCommit !== null &&
      this.#git.reachableCommit(data.lastAcceptedCommit, reachableFrom)
        ? { sha: data.lastAcceptedCommit, source: "predecessor" }
        : { sha: this.#git.headSha(), source: "head" };
    let seed: string;
    try {
      seed = buildSeed(data, base, tip);
    } catch (error) {
      if (error instanceof SeedTooLargeError)
        throw new LauncherError("seed_too_large", error.message);
      throw error;
    }
    this.#replacing.add(agentId);
    try {
      let released: ReleaseResult | undefined;
      if (agent.state === "active") {
        released = await this.release(agentId);
        if (released.paneClosed === false)
          return {
            state: "blocked",
            predecessor: agentId,
            reason: `${agentId} was released but its pane is still open: close it in Herdr, then run cstan replace ${agentId} again (it works on an ended agent and starts the replacement with the seed)`,
          };
      }
      let spawned: SpawnResult;
      try {
        spawned = await this.spawn(agent.roleName, {
          baseSha: base.sha,
          seed,
        });
      } catch (error) {
        throw new LauncherError(
          "replacement_not_started",
          `${agentId} was ${released === undefined ? "already ended" : "released"} but the replacement could not start (${error instanceof Error ? error.message : String(error)}); run cstan replace ${agentId} again`,
        );
      }
      let replacementRecorded = true;
      try {
        this.#core.recordAgentReplaced(this.#context(), {
          predecessorId: agentId,
          successorId: spawned.agentId,
        });
      } catch (error) {
        replacementRecorded = false;
        this.#log("replacement_not_recorded", {
          predecessor: agentId,
          successor: spawned.agentId,
          error: String(error),
        });
      }
      return {
        ...spawned,
        predecessor: agentId,
        baseSha: base.sha,
        baseSource: base.source,
        predecessorWorktreeRemoved: released?.worktreeRemoved ?? null,
        cancelledMessageIds: released?.cancelledMessageIds ?? [],
        replacementRecorded,
      };
    } finally {
      this.#replacing.delete(agentId);
    }
  }

  /** Ends a worker and frees its pane, worktree and, when it holds no commits, its branch. A branch with commits is kept for the user to merge. */
  release(agentId: string): Promise<ReleaseResult> {
    return this.#run(async () => {
      await this.#adoptAll(this.#budget(ADOPT_BUDGET_MS));
      const agent = this.#core.agentRecord(agentId);
      if (agent === undefined)
        throw new LauncherError(
          "unknown_agent",
          `there is no agent ${agentId}`,
        );
      if (agent.kind === "PM")
        throw new LauncherError(
          "kind_not_releasable",
          "the PM is restarted with cstan pm restart, not released",
        );
      if (agent.state !== "active")
        throw new LauncherError(
          "agent_not_active",
          `${agentId} is not active (it was released, ended or never started); nothing more to release`,
        );
      const row = this.#core
        .agentPanes(this.#credential)
        .find((candidate) => candidate.agentId === agentId);
      const outcome = await this.#cleanupAgent(agentId, {
        ...(row?.paneId == null ? {} : { paneId: row.paneId }),
        ...(row?.branch == null ? {} : { branch: row.branch }),
        ...(row?.baseSha == null ? {} : { baseSha: row.baseSha }),
        ...(row?.worktreePath == null
          ? {}
          : { worktreePath: row.worktreePath }),
      });
      if (!outcome.ended)
        throw new LauncherError(
          "release_blocked",
          `${agentId} could not be released: ${outcome.reason}`,
        );
      return {
        state: "released",
        agentId,
        branch: row?.branch ?? null,
        paneClosed: outcome.paneClosed,
        worktreeRemoved: outcome.worktreeRemoved,
        branchKept: outcome.branchKept,
        cancelledMessageIds: outcome.cancelledMessageIds,
      };
    });
  }

  restartPm(): Promise<LaunchResult> {
    return this.#runStarting(async (budget) => {
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
      // The project workspace must exist, with its watch tab, before the old PM pane is closed: Herdr refuses to close the last pane of a workspace that has worktree children.
      const hub = await this.#ensureHub(budget);
      if (hub.status === "failed")
        throw new LauncherError(
          "hub_unavailable",
          "the project workspace could not be opened",
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
          await this.#close(oldPane);
        } catch (error) {
          this.#core.recordOrphanPane(this.#context(), {
            agentId: agent.agentId,
            paneId: oldPane,
          });
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
          hub,
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

  /** `baseSha` makes the worker's worktree and branch start at that commit (a review) instead of the project's HEAD. */
  spawn(
    roleName: string,
    options: { readonly baseSha?: string; readonly seed?: string } = {},
  ): Promise<SpawnResult> {
    if (
      options.baseSha !== undefined &&
      !/^[0-9a-f]{40}$/.test(options.baseSha)
    )
      return Promise.reject(
        new LauncherError(
          "invalid_base",
          "the base commit must be a full lowercase id",
        ),
      );
    return this.#runStarting(async (budget) => {
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
      this.#assertRoleSynced(role);
      const workers = this.#activeAgents().filter((a) => a.kind !== "PM");
      const limit = this.#config.limits.maxWorkers;
      if (workers.length >= limit) {
        const stuck = workers
          .map((a) => ({
            id: a.agentId,
            failure: this.#cleanupFailed.find((c) => c.agentId === a.agentId),
          }))
          .filter((entry) => entry.failure !== undefined)
          .map((entry) => `${entry.id}: ${entry.failure!.reason}`);
        throw new LauncherError(
          "worker_limit",
          `${workers.length} of ${limit} workers are active (${workers.map((a) => a.agentId).join(", ")}); release one with cstan release <agent-id>${stuck.length === 0 ? "" : `; a cleanup failed for ${stuck.join("; ")}`}`,
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
      if ((await this.#ensureHubWithoutPm(budget)) === "failed")
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
      const generation = this.#core.agentRecord(agent.agentId)?.generation ?? 1;
      const branch = `capstan/${agent.agentId}-g${generation}`;
      const info: {
        worktreePath?: string;
        paneId?: string;
        branch: string;
        baseSha?: string;
        moveMayHaveHappened?: boolean;
      } = { branch };
      try {
        if (!this.#git.branchNameValid(branch))
          throw new LauncherError(
            "invalid_branch",
            `git does not accept the branch name ${branch}`,
          );
        const baseSha = options.baseSha ?? this.#git.headSha();
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
          label: workspaceLabel(this.#project, agent.agentId),
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
        let paneId = tree.paneId;
        let placement: SpawnResult["placement"] = "tab";
        let placementNote: string | undefined;
        if (this.#config.layout.spawn === "pane") {
          budget.check("placing the worker pane");
          info.moveMayHaveHappened = true;
          const outcome =
            pmPane.paneId === null
              ? { note: "the PM has no recorded pane" }
              : await this.#placeWorkerPane(
                  tree.paneId,
                  tree.path,
                  pmPane.paneId,
                );
          if ("placed" in outcome) {
            paneId = outcome.placed.paneId;
            placement = "pane";
            info.paneId = paneId;
            this.#core.recordAgentPane(this.#context(), {
              agentId: agent.agentId,
              workspaceId: outcome.placed.workspaceId,
              paneId,
              worktreePath: tree.path,
              branch,
              baseSha,
            });
            info.moveMayHaveHappened = false;
          } else {
            placementNote = outcome.note;
            info.moveMayHaveHappened = false;
          }
        }
        const where = {
          placement,
          ...(placementNote === undefined ? {} : { placementNote }),
        };
        budget.check("starting the worker");
        const promptText = buildRolePrompt({
          roleName: role.name,
          kind: role.kind,
          agentId: agent.agentId,
          waitTimeoutSeconds: this.#waitSeconds(role),
          rolePrompt: role.promptText,
          ...(options.seed === undefined
            ? {}
            : { replacementSeed: options.seed }),
        });
        const promptFile = this.#adapter.writePromptFile(promptText);
        const started = await this.#adapter.startAgent({
          name: agent.agentId,
          kind: this.#hostKind(role),
          paneId,
          args: this.#arguments(
            role,
            { text: promptText, file: promptFile },
            tree.path,
          ),
          environment: this.#environment(agent.credential),
          timeoutMs: START_TIMEOUT_MS,
        });
        this.#agentsStarted += 1;
        // The worker's workspace is gone once its pane is placed, and the PM's and the watch workspace already carry the project token.
        await this.#describe({
          paneId,
          workspaceId: null,
          agentId: agent.agentId,
          roleName: role.name,
        });
        if (started.status === "started")
          return {
            state: "started",
            agentId: agent.agentId,
            paneId,
            worktreePath: tree.path,
            branch,
            ...where,
          };
        const answered = await this.#adapter.answerTrustDialog({
          paneId,
          timeoutMs: DIALOG_TIMEOUT_MS,
          log: (entry) => this.#log("trust_dialog_key", { ...entry }),
        });
        return {
          state: answered.handled ? "started" : "blocked",
          agentId: agent.agentId,
          paneId,
          worktreePath: tree.path,
          branch,
          ...where,
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

  // ----------------------------------------------------------- placement

  /** Splits a new worker pane into the PM's tab, or says why it stays a tab. A pane that was lost in the move is an error, not a fallback. */
  async #placeWorkerPane(
    paneId: string,
    worktreePath: string,
    pmPaneId: string,
  ): Promise<
    | { readonly placed: { paneId: string; workspaceId: string } }
    | { readonly note: string }
  > {
    const layoutConfig = this.#config.layout;
    try {
      const layout = await this.#adapter.paneLayout(pmPaneId);
      if (layout.zoomed) return { note: "the PM's tab has a zoomed pane" };
      const live = new Set(
        this.#core
          .agentPanes(this.#credential)
          .filter((row) => {
            const agent = this.#core.agentRecord(row.agentId);
            return (
              row.paneId !== null &&
              agent?.state === "active" &&
              (agent.kind === "PM" ||
                !this.#cleanupFailed.some((c) => c.agentId === row.agentId)) &&
              this.#adapter.paneEntry(row.paneId) !== undefined
            );
          })
          .map((row) => row.paneId!),
      );
      const candidates: LayoutPane[] = layout.panes.filter((pane) =>
        live.has(pane.paneId),
      );
      const choice = choosePlacement(candidates, pmPaneId, {
        pmWidthPercent: layoutConfig.pmWidthPercent,
        minColumns: layoutConfig.minPaneColumns,
        minRows: layoutConfig.minPaneRows,
      });
      if (choice === undefined)
        return {
          note: `no pane has room for a split of at least ${layoutConfig.minPaneColumns} columns by ${layoutConfig.minPaneRows} rows`,
        };
      const placed = await this.#adapter.placePane({
        paneId,
        tabId: layout.tabId,
        targetPaneId: choice.targetPaneId,
        direction: choice.direction,
        keep: choice.keep,
        worktreePath,
      });
      return { placed };
    } catch (error) {
      if (error instanceof PaneLost) throw error;
      this.#log("placement_failed", { paneId, error: String(error) });
      const reason = oneLine(
        error instanceof Error ? error.message : String(error),
        MAX_NOTE_LENGTH,
      );
      return {
        note: `the pane could not be placed${reason === "" ? "" : ` (${reason})`}`,
      };
    }
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
      /** True only when a pane move was started and its result never reached the ledger. */
      moveMayHaveHappened?: boolean;
    },
    options: {
      readonly lost?: "found_dead_at_start";
      readonly branch?: string | null;
    } = {},
  ): Promise<
    | { readonly ended: false; readonly reason: string }
    | ({
        readonly ended: true;
        readonly cancelledMessageIds: readonly string[];
      } & ReleaseOutcome)
  > {
    let cancelledMessageIds: readonly string[];
    try {
      cancelledMessageIds = this.#core.endAgent(
        this.#context(),
        agentId,
        options,
      ).cancelledMessageIds;
    } catch (error) {
      const reason = error instanceof Error ? error.message : String(error);
      this.#cleanupFailed = this.#cleanupFailed.filter(
        (entry) => entry.agentId !== agentId,
      );
      this.#cleanupFailed.push({ agentId, reason });
      this.#log("cleanup_blocked", { agentId, error: String(error) });
      return { ended: false, reason };
    }
    this.#cleanupFailed = this.#cleanupFailed.filter(
      (entry) => entry.agentId !== agentId,
    );
    return {
      ended: true,
      cancelledMessageIds,
      ...(await this.#releaseResources(agentId, info)),
    };
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
      /** True only when a pane move was started and its result never reached the ledger. */
      moveMayHaveHappened?: boolean;
    },
  ): Promise<ReleaseOutcome> {
    const budget = this.#budget(CLEANUP_BUDGET_MS);
    let paneClosed: boolean | null = null;
    if (info.paneId !== undefined) {
      paneClosed = false;
      if (this.#within(budget)) {
        try {
          const existed = await this.#close(info.paneId);
          paneClosed = true;
          if (
            !existed &&
            info.worktreePath !== undefined &&
            info.moveMayHaveHappened === true
          )
            await this.#closeMovedPane(agentId, info.worktreePath, info.paneId);
        } catch (error) {
          this.#log("pane_not_closed", {
            agentId,
            paneId: info.paneId,
            error: String(error),
          });
        }
      }
      // A pane that is still open keeps its row, worktree and branch, so the
      // next start can find and close it instead of leaving it untracked.
      if (!paneClosed)
        return {
          paneClosed,
          worktreeRemoved: info.worktreePath === undefined ? null : false,
          branchKept: info.branch === undefined ? null : true,
        };
    }
    let worktreePath = info.worktreePath;
    if (worktreePath === undefined && info.branch !== undefined) {
      try {
        worktreePath = this.#git.worktreeByBranch(info.branch);
      } catch (error) {
        // Unknown is not "none": keep the branch and the row for the next start.
        this.#log("worktree_unknown", { agentId, error: String(error) });
        return {
          paneClosed,
          worktreeRemoved: false,
          branchKept: info.branch === undefined ? null : true,
        };
      }
    }
    let worktreeRemoved: boolean | null = null;
    if (worktreePath !== undefined) {
      worktreeRemoved = this.#git.worktreeRemove(worktreePath);
      if (!worktreeRemoved) {
        this.#log("worktree_kept", { agentId, worktreePath });
        return {
          paneClosed,
          worktreeRemoved,
          branchKept: info.branch === undefined ? null : true,
        };
      }
    }
    let branchKept: boolean | null = info.branch === undefined ? null : true;
    if (info.branch !== undefined && info.baseSha !== undefined) {
      branchKept = !this.#git.deleteBranchIf(info.branch, info.baseSha);
      if (branchKept)
        this.#log("branch_kept", { agentId, branch: info.branch });
    }
    try {
      this.#core.clearAgentPane(this.#context(), agentId);
    } catch (error) {
      this.#log("pane_row_not_cleared", { agentId, error: String(error) });
    }
    return { paneClosed, worktreeRemoved, branchKept };
  }

  /** Closes a pane; a pane Herdr no longer knows counts as closed. Returns false when the pane was already gone. */
  async #close(paneId: string): Promise<boolean> {
    try {
      await this.#adapter.closePane(paneId);
      return true;
    } catch (error) {
      if (error instanceof HerdrError && /not_found|no_such/.test(error.code)) {
        this.#adapter.forgetPane(paneId);
        return false;
      }
      throw error;
    }
  }

  /**
   * Called only when a split placement was interrupted. Close the one
   * unregistered pane in the PM's workspace whose directory is the agent's
   * worktree; with none or several, leave everything alone. A normal release
   * never looks. Known limit: if the worker's pane died and the operator opened
   * a shell in the PM's workspace at exactly that worktree path, an interrupted
   * move cannot be told apart from it.
   */
  /** In pane mode a row whose pane is gone and whose workspace is not the PM's may hold the old id of a pane that was moved but never recorded. */
  #interruptedMove(workspaceId: string | null): boolean {
    if (this.#config.layout.spawn !== "pane" || workspaceId === null)
      return false;
    const pm = this.#activeAgents().find((agent) => agent.kind === "PM");
    const pmWorkspace = this.#core
      .agentPanes(this.#credential)
      .find((row) => row.agentId === pm?.agentId)?.workspaceId;
    return pmWorkspace !== undefined && pmWorkspace !== workspaceId;
  }

  async #closeMovedPane(
    agentId: string,
    worktreePath: string,
    recordedPaneId: string,
  ): Promise<void> {
    try {
      // A moved pane lands in the PM's workspace, so a pane anywhere else is the operator's.
      const pm = this.#activeAgents().find((agent) => agent.kind === "PM");
      const pmWorkspace = this.#core
        .agentPanes(this.#credential)
        .find((row) => row.agentId === pm?.agentId)?.workspaceId;
      if (pmWorkspace === undefined || pmWorkspace === null) return;
      const strays = (await this.#adapter.panesAtPath(worktreePath)).filter(
        (pane) =>
          pane.paneId !== recordedPaneId &&
          pane.workspaceId === pmWorkspace &&
          this.#adapter.paneEntry(pane.paneId) === undefined,
      );
      if (strays.length !== 1) return;
      await this.#close(strays[0]!.paneId);
      this.#log("moved_pane_closed", { agentId, paneId: strays[0]!.paneId });
    } catch (error) {
      this.#log("moved_pane_not_closed", { agentId, error: String(error) });
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
    for (const orphan of this.#core.orphanPanes(this.#credential)) {
      try {
        await this.#close(orphan.paneId);
        this.#core.clearOrphanPane(this.#context(), orphan.paneId);
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
        await this.#describe({
          paneId: row.paneId,
          workspaceId: row.workspaceId,
          agentId: row.agentId,
          roleName: agent.roleName,
          // A worker placed as a tab or pane shares the PM's workspace, and a PM in the project workspace shares the hub's name, so only a PM's own old workspace is relabelled here.
          ...(agent.kind === "PM" &&
          row.workspaceId !==
            this.#core.fallbackPane(this.#credential)?.workspaceId
            ? { label: workspaceLabel(this.#project, agent.roleName) }
            : {}),
        });
      } catch (error) {
        if (error instanceof PaneGone || error instanceof AgentPaneMismatch) {
          this.#log("pane_lost", { agentId: row.agentId });
          this.#core.clearAgentPane(this.#context(), row.agentId);
          if (agent.kind !== "PM")
            await this.#cleanupAgent(
              row.agentId,
              {
                paneId: row.paneId,
                ...(this.#interruptedMove(row.workspaceId)
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
        await this.#labelHub(fallback.workspaceId);
      } catch (error) {
        // A gone watch pane keeps its row: the workspace id in it lets the next hub check make the watch tab again in the same workspace.
        if (error instanceof PaneGone)
          this.#log("fallback_pane_gone", { paneId: fallback.paneId });
        else this.#log("fallback_adopt_failed", { error: String(error) });
      }
    }
  }
}
