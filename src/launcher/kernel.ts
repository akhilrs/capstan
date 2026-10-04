/** The state and helpers every launcher operation shares. */
import fs from "node:fs";
import path from "node:path";
import { randomUUID } from "node:crypto";
import {
  DEFAULT_WAIT_TIMEOUT_SECONDS,
  type CapstanConfig,
  type HostKind,
  type ResolvedRole,
} from "../config/capstan-config.js";
import { newContext } from "../context.js";
import type { ControllerCore } from "../controller/core.js";
import type { AgentRecord } from "../controller/types.js";
import { buildAgentEnvironment, claudeArguments } from "../herdr/adapter.js";
import { codexArguments, ompArguments } from "../herdr/hosts.js";
import { projectDisplayName } from "../herdr/naming.js";
import { HerdrError } from "../herdr/runner.js";
import { CSTAN_ALLOW_RULE, type PromptInput } from "../prompts.js";
import { defaultGit } from "./git.js";
import type { HubOps } from "./hub.js";
import type { ObserveOps } from "./observe.js";
import type { ReleaseOps } from "./release.js";
import { runSetupCommand, type SetupOps } from "./setup.js";
import {
  ID_ATTEMPTS,
  LauncherError,
  MAX_SYNC_REASON_CHARS,
  MAX_WAITING_OPERATIONS,
  STEP_BUDGET_MS,
  cstanWrapperScript,
  type Budget,
  type GitRunner,
  type LauncherAdapter,
  type LauncherOptions,
  type LauncherStatus,
  type SetupRunner,
  type TeardownRunner,
} from "./shared.js";
import type { SpawnOps } from "./spawn.js";

export class LauncherKernel {
  readonly core: ControllerCore;
  readonly adapter: LauncherAdapter;
  readonly config: CapstanConfig;
  readonly root: string;
  readonly project: string;
  readonly cliPath: string;
  readonly socketPath: string;
  readonly credential: string;
  readonly node: string;
  readonly baseEnvironment: NodeJS.ProcessEnv;
  /** Predecessors whose replacement is running now. */
  readonly replacing = new Set<string>();
  /** How many agents this launcher has started; an operation that raised it started one. */
  agentsStarted = 0;
  readonly git: GitRunner;
  readonly now: () => number;
  readonly log: (event: string, details: Record<string, unknown>) => void;
  readonly syncRoles: (() => void) | undefined;
  readonly runSetup: SetupRunner;
  readonly sleep: (milliseconds: number) => Promise<void>;
  readonly runTeardown: TeardownRunner;
  tail: Promise<unknown> = Promise.resolve();
  active = 0;
  cleanupFailed: LauncherStatus["cleanupFailed"][number][] = [];
  /** The collaborators, set by the Launcher right after construction. */
  hub!: HubOps;
  releaser!: ReleaseOps;
  spawner!: SpawnOps;
  observer!: ObserveOps;
  setup!: SetupOps;

  constructor(options: LauncherOptions) {
    this.core = options.core;
    this.adapter = options.adapter;
    this.config = options.config;
    this.root = options.projectRoot;
    this.project = projectDisplayName(
      options.config.projectName,
      options.projectRoot,
    );
    this.cliPath = options.cliPath;
    this.socketPath = options.socketPath;
    this.credential = options.credential;
    this.node = options.nodePath ?? process.execPath;
    this.baseEnvironment = options.baseEnvironment ?? process.env;
    this.git = options.git ?? defaultGit(options.projectRoot);
    this.now = options.now ?? Date.now;
    this.log = options.log ?? (() => undefined);
    this.syncRoles = options.syncRoles;
    this.sleep =
      options.sleep ??
      ((milliseconds) =>
        new Promise((resolve) => setTimeout(resolve, milliseconds)));
    this.runSetup =
      options.runSetup ??
      ((command, cwd, timeoutMs) =>
        runSetupCommand(command, cwd, timeoutMs, this.environment(null, true)));
    this.runTeardown = options.runTeardown ?? runSetupCommand;
  }

  run<T>(operation: (budget: Budget) => Promise<T>): Promise<T> {
    // One operation runs, up to MAX_WAITING_OPERATIONS wait behind it.
    if (this.active >= 1 + MAX_WAITING_OPERATIONS)
      return Promise.reject(
        new LauncherError(
          "busy",
          "another launcher operation is already waiting",
        ),
      );
    this.active += 1;
    const result = this.tail
      .then(() => operation(this.budget(STEP_BUDGET_MS)))
      .finally(() => {
        this.active -= 1;
      });
    this.tail = result.catch(() => undefined);
    return result;
  }

  budget(ms: number): Budget {
    let deadline = this.now() + ms;
    return {
      get deadline() {
        return deadline;
      },
      extend: (extra) => {
        deadline += extra;
      },
      check: (step) => {
        if (this.now() > deadline)
          throw new LauncherError(
            "timeout",
            `the operation ran out of time before ${step}`,
          );
      },
    };
  }

  context() {
    return newContext(this.core, this.credential);
  }

  pmRole(): ResolvedRole {
    const role = this.config.roles.find((r) => r.kind === "PM");
    if (role === undefined)
      throw new LauncherError(
        "not_configured",
        "the configuration has no PM role",
      );
    return role;
  }

  waitSeconds(role: ResolvedRole): number {
    return (
      this.config.hosts.find((h) => h.name === role.host)?.waitTimeoutSeconds ??
      DEFAULT_WAIT_TIMEOUT_SECONDS
    );
  }

  workerRoles(): { name: string; kind: string }[] {
    return this.config.roles
      .filter((role) => role.kind !== "PM")
      .map((role) => ({ name: role.name, kind: role.kind }));
  }

  /** The architect settings a prompt needs; undefined while the Architect is disabled, so no prompt changes. */
  architectPrompt(): PromptInput["architect"] {
    const architect = this.config.architect;
    return architect?.enabled === true
      ? {
          role: architect.role,
          highRiskTriggers: architect.highRiskTriggers,
        }
      : undefined;
  }

  /** The researcher settings a prompt needs; undefined while the Researcher is disabled, so no prompt changes. */
  researcherPrompt(): PromptInput["researcher"] {
    const researcher = this.config.researcher;
    return researcher?.enabled === true
      ? {
          role: researcher.role,
          outputDir: researcher.outputDir,
          userAgent: researcher.userAgent,
        }
      : undefined;
  }

  /** The operator settings a prompt needs; undefined while the Operator is disabled, so no prompt changes. */
  operatorPrompt(): PromptInput["operator"] {
    const operator = this.config.operator;
    return operator?.enabled === true
      ? { role: operator.role, autoApprove: operator.autoApprove }
      : undefined;
  }

  isOperatorRole(name: string, kind: string): boolean {
    const operator = this.config.operator;
    return (
      operator?.configured === true &&
      kind === "Developer" &&
      name === operator.role
    );
  }

  nexoraPrompt(): Pick<PromptInput, "nexora"> {
    const nexora = this.config.nexora;
    return nexora === undefined || nexora.track === "never"
      ? {}
      : {
          nexora: { track: nexora.track, defaultAction: nexora.defaultAction },
        };
  }

  isArchitectRole(name: string, kind: string): boolean {
    const architect = this.config.architect;
    return (
      architect?.enabled === true &&
      kind === "Developer" &&
      name === architect.role
    );
  }

  isResearcherRole(name: string, kind: string): boolean {
    const researcher = this.config.researcher;
    return (
      researcher?.enabled === true &&
      kind === "Developer" &&
      name === researcher.role
    );
  }

  activeAgents(): AgentRecord[] {
    return this.core.listAgents().filter((a) => a.state === "active");
  }

  /** The first of the role's seats that no active agent holds; an extra seat has the id `<role>-seat-<n>` and the display name `<role>.<n>`, which no role name can equal (role names allow no dot), so it never collides with another role's seat; the core allows one active agent per seat, so each concurrent worker needs its own. A disabled seat, or one made for another kind, is an operator-visible error and stops the walk even when a later seat is free; it is not something to route around. */
  seat(role: ResolvedRole): string {
    const held = new Set(this.activeAgents().map((a) => a.seatId));
    const seats = this.core.statusSnapshot().roles;
    const attempts = role.kind === "PM" ? 1 : this.config.limits.maxWorkers;
    for (let number = 1; number <= attempts; number += 1) {
      const seatId =
        number === 1 ? `${role.name}-seat` : `${role.name}-seat-${number}`;
      const existing = seats.find((entry) => entry.seatId === seatId);
      if (existing === undefined) {
        this.core.createSeat(this.context(), {
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

  roleIsSynced(role: ResolvedRole): boolean {
    const definition = this.core
      .roleDefinitions()
      .find((candidate) => candidate.name === role.name);
    return (
      definition?.state === "active" &&
      definition.kind === role.kind &&
      definition.host === role.host &&
      definition.configHash === role.configHash
    );
  }

  assertRoleSynced(role: ResolvedRole): void {
    if (this.roleIsSynced(role)) return;
    let reason = "";
    try {
      this.syncRoles?.();
    } catch (error) {
      const text = (error instanceof Error ? error.message : String(error))
        .replace(/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}\s]+/gu, " ")
        .trim();
      reason = ` (${Array.from(text).slice(0, MAX_SYNC_REASON_CHARS).join("")})`;
    }
    if (this.roleIsSynced(role)) return;
    throw new LauncherError(
      "role_not_synced",
      `the role ${role.name} is not synced into the controller${reason}; run cstan stop, then cstan start, and look for role_sync_failed in the daemon log if it persists`,
    );
  }

  createAgent(role: ResolvedRole): { agentId: string; credential: string } {
    this.assertRoleSynced(role);
    const seatId = this.seat(role);
    const used = this.core
      .listAgents()
      .filter((a) => a.roleName === role.name).length;
    let lastError: unknown;
    for (let attempt = 1; attempt <= ID_ATTEMPTS; attempt += 1) {
      const agentId = `${role.name}-${used + attempt}`;
      if (this.core.agentRecord(agentId) !== undefined) continue;
      // Only the crash window between createActor and registerAgent can leave
      // an active actor on the seat that no active agent owns.
      const owned = new Set(this.activeAgents().map((a) => a.actorId));
      for (const actorId of this.core.seatActorIds(this.credential, seatId))
        if (!owned.has(actorId)) this.core.revokeActor(this.context(), actorId);
      const actor = this.core.createActor(this.context(), {
        displayName: agentId,
        role: role.kind,
        seatId,
      });
      try {
        this.core.registerAgent(this.context(), {
          agentId,
          roleName: role.name,
          seatId,
          actorId: actor.actorId,
        });
        return { agentId, credential: actor.credential };
      } catch (error) {
        lastError = error;
        try {
          this.core.revokeActor(this.context(), actor.actorId);
        } catch (revokeError) {
          this.log("actor_not_revoked", { error: String(revokeError) });
        }
      }
    }
    throw (
      lastError ??
      new LauncherError("id_conflict", "no free agent id was found")
    );
  }

  ensureWrapper(): string {
    const directory = path.join(this.root, ".capstan", "bin");
    fs.mkdirSync(directory, { recursive: true, mode: 0o700 });
    fs.chmodSync(directory, 0o700);
    const temporary = path.join(directory, `cstan.tmp-${randomUUID()}`);
    fs.writeFileSync(temporary, cstanWrapperScript(this.node, this.cliPath), {
      mode: 0o700,
    });
    fs.renameSync(temporary, path.join(directory, "cstan"));
    return directory;
  }

  /** The pane of an active agent, or a LauncherError. */
  activePane(agentId: string): string {
    const agent = this.core.agentRecord(agentId);
    if (agent === undefined || agent.state !== "active")
      throw new LauncherError("agent_not_active", "the agent is not active");
    const paneId =
      this.adapter.paneForAgent(agentId) ??
      this.core
        .agentPanes(this.credential)
        .find((row) => row.agentId === agentId)?.paneId ??
      undefined;
    if (paneId === undefined || paneId === null)
      throw new LauncherError("no_pane", "the agent has no pane recorded");
    return paneId;
  }

  /** Names from `[env] pass` that are not set (or are empty) in the daemon's environment: an agent started now would not have them. */
  missingPassEnvironment(): string[] {
    return this.config.env.pass.filter(
      (name) =>
        this.baseEnvironment[name] === undefined ||
        this.baseEnvironment[name] === "",
    );
  }

  /**
   * Runs an operation like `#run`. If it started an agent while a listed
   * variable was unset, the answer carries the names and a warning and the log
   * gets one entry. The count is read inside the operation, so overlapping
   * operations cannot see each other's starts.
   */
  runStarting<T extends object>(
    operation: (budget: Budget) => Promise<T>,
  ): Promise<
    T & { readonly missingEnv?: readonly string[]; readonly warning?: string }
  > {
    return this.run(async (budget) => {
      const before = this.agentsStarted;
      const result = await operation(budget);
      return this.agentsStarted > before
        ? this.withMissingEnvironment(result)
        : result;
    });
  }

  withMissingEnvironment<T extends object>(
    result: T,
  ): T & {
    readonly missingEnv?: readonly string[];
    readonly warning?: string;
  } {
    const missing = this.missingPassEnvironment();
    if (missing.length === 0) return result;
    this.log("env_pass_missing", { names: missing });
    return {
      ...result,
      missingEnv: missing,
      warning: `${missing.join(", ")} ${missing.length === 1 ? "is" : "are"} listed in [env] pass but not set where the daemon was started, so this agent does not have ${missing.length === 1 ? "it" : "them"}; set ${missing.length === 1 ? "it" : "them"} in the shell that runs cstan start (or its profile file), then restart the daemon`,
    };
  }

  /** The one environment every agent starts with: the allowlist, its token and socket, and `cstan` first on PATH. */
  environment(
    token: string | null,
    includePass = token !== null,
  ): Record<string, string> {
    if (this.root.includes(":"))
      throw new LauncherError(
        "unsupported_root",
        "a project path with a colon cannot be put on PATH, so cstan would not be found",
      );
    const bin = this.ensureWrapper();
    const basePath = this.baseEnvironment.PATH;
    const extras: Record<string, string> = {
      PATH:
        basePath === undefined || basePath === "" ? bin : `${bin}:${basePath}`,
    };
    if (token !== null) {
      extras.CAPSTAN_TOKEN = token;
      extras.CAPSTAN_SOCKET = this.socketPath;
    }
    return buildAgentEnvironment(
      this.baseEnvironment,
      extras,
      includePass ? this.config.env.pass : [],
    );
  }

  hostKind(role: ResolvedRole): HostKind {
    const host = this.config.hosts.find((h) => h.name === role.host);
    if (host === undefined)
      throw new LauncherError(
        "unknown_host",
        `the configuration has no host ${role.host}`,
      );
    return host.kind;
  }

  arguments(
    role: ResolvedRole,
    prompt: { readonly text: string; readonly file: string },
    worktreePath?: string,
  ): string[] {
    const kind = this.hostKind(role);
    if (kind === "codex")
      return codexArguments(role, prompt.text, worktreePath);
    if (kind === "omp") return ompArguments(role, prompt.file);
    const allow = role.allow.includes(CSTAN_ALLOW_RULE)
      ? role.allow
      : [...role.allow, CSTAN_ALLOW_RULE];
    return claudeArguments({ ...role, allow }, prompt.file);
  }

  /** Tells Herdr which project, role and agent a pane belongs to, for the operator's sidebar; display only, so a failure is logged and never stops a start. */
  async describe(input: {
    paneId: string;
    workspaceId: string | null;
    agentId: string;
    roleName: string;
    label?: string;
  }): Promise<void> {
    try {
      if (input.label !== undefined && input.workspaceId !== null)
        await this.adapter.renameWorkspace(input.workspaceId, input.label);
      await this.adapter.reportMetadata(
        { paneId: input.paneId },
        { project: this.project, role: input.roleName, agent: input.agentId },
      );
      if (input.workspaceId !== null)
        await this.adapter.reportMetadata(
          { workspaceId: input.workspaceId },
          { project: this.project },
        );
    } catch (error) {
      this.log("describe_failed", {
        agentId: input.agentId,
        error: String(error),
      });
    }
  }

  /** Closes a pane; a pane Herdr no longer knows counts as closed. Returns false when the pane was already gone. */
  async close(paneId: string): Promise<boolean> {
    try {
      await this.adapter.closePane(paneId);
      return true;
    } catch (error) {
      if (error instanceof HerdrError && /not_found|no_such/.test(error.code)) {
        this.adapter.forgetPane(paneId);
        return false;
      }
      throw error;
    }
  }

  within(budget: Budget): boolean {
    return this.now() <= budget.deadline;
  }
}
