/** Pane and layout operations: creating, placing, renaming, closing and adopting panes. */
import fs from "node:fs";
import path from "node:path";
import { HOST_KINDS, type HostKind } from "../config/capstan-config.js";
import {
  AgentPaneMismatch,
  InvalidArgumentError,
  PaneGone,
  PaneLost,
  PhaseError,
  UnknownPaneError,
  UnsupportedHostError,
} from "./adapter-errors.js";
import {
  PANE_PATTERN,
  TAB_PATTERN,
  WORKSPACE_PATTERN,
  BRANCH_PATTERN,
  isAgentName,
  requireLabel,
  requireMatch,
} from "./adapter-validate.js";
import { HerdrError, runJson, type HerdrRunner } from "./runner.js";
import type { InputBlocker } from "./screen.js";

export type PaneRole = "PM" | "worker";
export type PanePhase = "fresh" | "prepared" | "started" | "tainted";

export interface PaneEntry {
  readonly role: PaneRole;
  readonly phase: PanePhase;
  readonly kind: string;
  readonly agent?: string;
  readonly worktreePath?: string;
  readonly workspaceId?: string;
}

export interface PaneIdentity {
  readonly terminalId: string | undefined;
  /** The `agent` token, when one was reported for the pane. */
  readonly agent: string | undefined;
  /** The `project` token, when one was reported for the pane. */
  readonly project: string | undefined;
}

export interface PaneLayoutView {
  readonly tabId: string;
  readonly workspaceId: string;
  readonly zoomed: boolean;
  readonly panes: readonly {
    readonly paneId: string;
    readonly width: number;
    readonly height: number;
  }[];
}

/** What the adapter hands its collaborators: the registry, the clock and the Herdr reads they share. */
export interface AdapterCore {
  readonly run: HerdrRunner;
  readonly panes: Map<string, PaneEntry>;
  readonly slug: string | undefined;
  readonly pollMs: number;
  now(): number;
  sleep(ms: number): Promise<void>;
  herdrName(agentId: string): string;
  runChecked(args: readonly string[]): Promise<void>;
  record(value: unknown, label: string): Record<string, unknown>;
  assertTypable(
    paneId: string,
    action: "prepare" | "start" | "send" | "clear" | "dialog",
  ): PaneEntry;
  stateFor(agent: string, paneId: string): Promise<string>;
  agentState(
    name: string,
  ): Promise<{ status: string; paneId: string; kind: string }>;
  agentStateByHerdrName(
    herdrName: string,
  ): Promise<{ status: string; paneId: string; kind: string }>;
  readScreen(
    paneId: string,
    options?: { ansi?: boolean; lines?: number },
  ): Promise<string>;
  readInput(paneId: string): Promise<string | undefined>;
  readInputAndBlocker(
    paneId: string,
  ): Promise<{ text: string | undefined; blocker: InputBlocker }>;
}

export class PaneOperations {
  readonly core: AdapterCore;

  constructor(core: AdapterCore) {
    this.core = core;
  }

  async createWorktree(input: {
    workspaceId: string;
    branch: string;
    label: string;
    base?: string;
  }): Promise<{
    workspaceId: string;
    paneId: string;
    path: string;
    branch: string;
  }> {
    requireMatch(input.workspaceId, WORKSPACE_PATTERN, "workspace id");
    requireMatch(input.branch, BRANCH_PATTERN, "branch");
    requireLabel(input.label);
    const args = [
      "worktree",
      "create",
      "--workspace",
      input.workspaceId,
      "--branch",
      input.branch,
      "--label",
      input.label,
    ];
    if (input.base !== undefined)
      args.push("--base", requireMatch(input.base, BRANCH_PATTERN, "base"));
    args.push("--no-focus");
    const result = await runJson(this.core.run, args);
    const pane = this.core.record(result.root_pane, "root_pane");
    const workspace = this.core.record(result.workspace, "workspace");
    const worktree = this.core.record(workspace.worktree, "worktree");
    const paneId = requireMatch(pane.pane_id, PANE_PATTERN, "pane id");
    const workspaceId = requireMatch(
      workspace.workspace_id,
      WORKSPACE_PATTERN,
      "workspace id",
    );
    const checkout = worktree.checkout_path;
    if (typeof checkout !== "string" || !path.isAbsolute(checkout))
      throw new HerdrError(
        "bad_output",
        "herdr did not report a checkout path",
      );
    this.core.panes.set(paneId, {
      role: "worker",
      phase: "fresh",
      kind: "shell",
      worktreePath: checkout,
      workspaceId,
    });
    return { workspaceId, paneId, path: checkout, branch: input.branch };
  }
  /** The panes of the tab that holds `paneId`, with their sizes in terminal cells. */
  async paneLayout(paneId: string): Promise<PaneLayoutView> {
    requireMatch(paneId, PANE_PATTERN, "pane id");
    const result = await runJson(this.core.run, [
      "pane",
      "layout",
      "--pane",
      paneId,
    ]);
    const layout = this.core.record(result.layout, "layout");
    if (!Array.isArray(layout.panes))
      throw new HerdrError("bad_output", "herdr did not report layout panes");
    return {
      tabId: requireMatch(layout.tab_id, TAB_PATTERN, "tab id"),
      workspaceId: requireMatch(
        layout.workspace_id,
        WORKSPACE_PATTERN,
        "workspace id",
      ),
      // Anything but an explicit false counts as zoomed: no split on a layout we cannot read.
      zoomed: layout.zoomed !== false,
      panes: layout.panes.map((entry) => {
        const pane = this.core.record(entry, "layout pane");
        const rect = this.core.record(pane.rect, "layout rect");
        return {
          paneId: requireMatch(pane.pane_id, PANE_PATTERN, "pane id"),
          width: typeof rect.width === "number" ? rect.width : Number.NaN,
          height: typeof rect.height === "number" ? rect.height : Number.NaN,
        };
      }),
    };
  }
  async #listPanes(): Promise<
    Array<{
      paneId: string;
      tabId: string;
      workspaceId: string;
      /** undefined when Herdr gave no absolute directory for the pane. */
      cwd: string | undefined;
    }>
  > {
    const result = await runJson(this.core.run, ["pane", "list"]);
    if (!Array.isArray(result.panes))
      throw new HerdrError("bad_output", "herdr did not report panes");
    // A pane whose id this adapter cannot read is skipped: it cannot be ours.
    // A pane without a usable directory is kept, so it still counts as existing.
    return result.panes.flatMap((entry) => {
      try {
        const pane = this.core.record(entry, "pane");
        return [
          {
            paneId: requireMatch(pane.pane_id, PANE_PATTERN, "pane id"),
            tabId: requireMatch(pane.tab_id, TAB_PATTERN, "tab id"),
            workspaceId: requireMatch(
              pane.workspace_id,
              WORKSPACE_PATTERN,
              "workspace id",
            ),
            cwd:
              typeof pane.cwd === "string" && path.isAbsolute(pane.cwd)
                ? pane.cwd
                : undefined,
          },
        ];
      } catch {
        return [];
      }
    });
  }
  /** The panes whose working directory is exactly `directory` (compared as real paths), with their workspaces. */
  async panesAtPath(
    directory: string,
  ): Promise<Array<{ paneId: string; workspaceId: string }>> {
    if (!path.isAbsolute(directory)) return [];
    const wanted = this.#canonical(directory);
    return (await this.#listPanes())
      .filter(
        (pane) =>
          pane.cwd !== undefined && this.#canonical(pane.cwd) === wanted,
      )
      .map((pane) => ({ paneId: pane.paneId, workspaceId: pane.workspaceId }));
  }
  /**
   * Moves a registered worker pane into another tab as a split. Herdr gives
   * the moved pane a new id; the registry follows it. A move that errors is
   * ambiguous (Herdr may have done it), so the panes are listed again: the old
   * pane still there means nothing moved; exactly one new pane at the worktree
   * path means it moved; anything else is an error.
   */
  async placePane(input: {
    paneId: string;
    tabId: string;
    targetPaneId: string;
    direction: "right" | "down";
    /** The fraction of the target pane the target keeps. */
    keep: number;
    worktreePath: string;
  }): Promise<{ paneId: string; workspaceId: string }> {
    requireMatch(input.paneId, PANE_PATTERN, "pane id");
    requireMatch(input.tabId, TAB_PATTERN, "tab id");
    requireMatch(input.targetPaneId, PANE_PATTERN, "target pane id");
    if (!path.isAbsolute(input.worktreePath))
      throw new InvalidArgumentError("worktree path must be absolute");
    if (input.direction !== "right" && input.direction !== "down")
      throw new InvalidArgumentError("split direction is not acceptable");
    if (!(input.keep >= 0.1 && input.keep <= 0.9))
      throw new InvalidArgumentError("split ratio is not acceptable");
    const entry = this.core.panes.get(input.paneId);
    if (entry === undefined)
      throw new UnknownPaneError("pane is not registered");
    if (entry.role !== "worker")
      throw new PhaseError("only a worker pane can be placed");
    const before = new Set((await this.#listPanes()).map((p) => p.paneId));
    let moved: { paneId: string; workspaceId: string };
    try {
      const result = await runJson(this.core.run, [
        "pane",
        "move",
        input.paneId,
        "--tab",
        input.tabId,
        "--split",
        input.direction,
        "--target-pane",
        input.targetPaneId,
        "--ratio",
        String(input.keep),
        "--no-focus",
      ]);
      const pane = this.core.record(
        this.core.record(result.move_result, "move_result").pane,
        "pane",
      );
      moved = {
        paneId: requireMatch(pane.pane_id, PANE_PATTERN, "pane id"),
        workspaceId: requireMatch(
          pane.workspace_id,
          WORKSPACE_PATTERN,
          "workspace id",
        ),
      };
    } catch (error) {
      const after = await this.#listPanes().catch(() => undefined);
      if (after === undefined)
        throw new PaneLost(
          "the pane move failed and the panes could not be listed to check it",
        );
      if (after.some((p) => p.paneId === input.paneId)) throw error;
      const wanted = this.#canonical(input.worktreePath);
      const found = after.filter(
        (p) =>
          !before.has(p.paneId) &&
          p.cwd !== undefined &&
          this.#canonical(p.cwd) === wanted,
      );
      if (found.length !== 1)
        throw new PaneLost(
          "the pane move failed and the pane cannot be found again",
        );
      moved = { paneId: found[0]!.paneId, workspaceId: found[0]!.workspaceId };
    }
    this.core.panes.delete(input.paneId);
    this.core.panes.set(moved.paneId, {
      ...entry,
      workspaceId: moved.workspaceId,
    });
    return moved;
  }
  async createWorkspace(input: {
    cwd: string;
    label: string;
    role: PaneRole;
  }): Promise<{ workspaceId: string; paneId: string; tabId: string }> {
    if (!path.isAbsolute(input.cwd))
      throw new InvalidArgumentError("workspace directory must be absolute");
    requireLabel(input.label);
    const result = await runJson(this.core.run, [
      "workspace",
      "create",
      "--cwd",
      input.cwd,
      "--label",
      input.label,
      "--no-focus",
    ]);
    const pane = this.core.record(result.root_pane, "root_pane");
    const workspace = this.core.record(result.workspace, "workspace");
    const paneId = requireMatch(pane.pane_id, PANE_PATTERN, "pane id");
    const workspaceId = requireMatch(
      workspace.workspace_id,
      WORKSPACE_PATTERN,
      "workspace id",
    );
    const tabId = requireMatch(
      this.core.record(result.tab, "tab").tab_id,
      TAB_PATTERN,
      "tab id",
    );
    this.core.panes.set(paneId, {
      role: input.role,
      phase: "fresh",
      kind: "shell",
      workspaceId,
    });
    return { workspaceId, paneId, tabId };
  }
  /** A new tab with its own root pane inside an existing workspace. */
  async createTab(input: {
    workspaceId: string;
    cwd: string;
    label: string;
    role: PaneRole;
  }): Promise<{ tabId: string; paneId: string }> {
    requireMatch(input.workspaceId, WORKSPACE_PATTERN, "workspace id");
    if (!path.isAbsolute(input.cwd))
      throw new InvalidArgumentError("tab directory must be absolute");
    requireLabel(input.label);
    const result = await runJson(this.core.run, [
      "tab",
      "create",
      "--workspace",
      input.workspaceId,
      "--cwd",
      input.cwd,
      "--label",
      input.label,
      "--no-focus",
    ]);
    const pane = this.core.record(result.root_pane, "root_pane");
    const tab = this.core.record(result.tab, "tab");
    const paneId = requireMatch(pane.pane_id, PANE_PATTERN, "pane id");
    const tabId = requireMatch(tab.tab_id, TAB_PATTERN, "tab id");
    this.core.panes.set(paneId, {
      role: input.role,
      phase: "fresh",
      kind: "shell",
      workspaceId: input.workspaceId,
    });
    return { tabId, paneId };
  }
  async removeWorktree(
    workspaceId: string,
    options: { force?: boolean } = {},
  ): Promise<void> {
    requireMatch(workspaceId, WORKSPACE_PATTERN, "workspace id");
    // A worker placed as a pane shares the PM's workspace id; removing "its" worktree workspace would reach the PM.
    for (const entry of this.core.panes.values())
      if (entry.workspaceId === workspaceId && entry.role === "PM")
        throw new PhaseError("that workspace holds the PM");
    await runJson(this.core.run, [
      "worktree",
      "remove",
      "--workspace",
      workspaceId,
      ...(options.force ? ["--force"] : []),
    ]);
    for (const [paneId, entry] of this.core.panes)
      if (entry.workspaceId === workspaceId && entry.role === "worker")
        this.core.panes.delete(paneId);
  }
  /** Display-only metadata for the operator's sidebar; the caller treats a failure as non-fatal. */
  async reportMetadata(
    target: { readonly paneId: string } | { readonly workspaceId: string },
    tokens: Readonly<Record<string, string>>,
  ): Promise<void> {
    const pane = "paneId" in target;
    const id = pane
      ? requireMatch(target.paneId, PANE_PATTERN, "pane id")
      : requireMatch(target.workspaceId, WORKSPACE_PATTERN, "workspace id");
    const args = [pane ? "pane" : "workspace", "report-metadata", id];
    args.push("--source", "capstan");
    for (const [name, value] of Object.entries(tokens)) {
      requireMatch(name, /^[a-z][a-z0-9_]{0,31}$/, "token name");
      args.push("--token", `${name}=${requireLabel(value)}`);
    }
    await this.core.runChecked(args);
  }
  /** Names a tab; display only. */
  async renameTab(tabId: string, label: string): Promise<void> {
    requireMatch(tabId, TAB_PATTERN, "tab id");
    await this.core.runChecked(["tab", "rename", tabId, requireLabel(label)]);
  }
  /** Gives an existing workspace its current label (an upgraded project still holds the old one). */
  async renameWorkspace(workspaceId: string, label: string): Promise<void> {
    requireMatch(workspaceId, WORKSPACE_PATTERN, "workspace id");
    await this.core.runChecked([
      "workspace",
      "rename",
      workspaceId,
      requireLabel(label),
    ]);
  }
  /**
   * Who a pane belongs to as Herdr shows it now: its terminal id and the
   * `agent` and `project` tokens Capstan reported for it. Undefined when Herdr
   * has no such pane. Herdr reuses short pane ids, so this is what tells an
   * agent's pane from a newer pane that was given the same id.
   */
  async paneIdentity(paneId: string): Promise<PaneIdentity | undefined> {
    requireMatch(paneId, PANE_PATTERN, "pane id");
    let result: Record<string, unknown>;
    try {
      result = await runJson(this.core.run, ["pane", "get", paneId]);
    } catch (error) {
      if (error instanceof HerdrError && /not_found|no_such/.test(error.code))
        return undefined;
      throw error;
    }
    const pane = this.core.record(result.pane, "pane");
    const tokens =
      typeof pane.tokens === "object" &&
      pane.tokens !== null &&
      !Array.isArray(pane.tokens)
        ? (pane.tokens as Record<string, unknown>)
        : {};
    const text = (value: unknown): string | undefined =>
      typeof value === "string" && value !== "" ? value : undefined;
    return {
      terminalId: text(pane.terminal_id),
      agent: text(tokens.agent),
      project: text(tokens.project),
    };
  }
  async closePane(paneId: string): Promise<void> {
    requireMatch(paneId, PANE_PATTERN, "pane id");
    await this.core.runChecked(["pane", "close", paneId]);
    this.core.panes.delete(paneId);
  }
  /**
   * Registers a pane an earlier adapter instance created (a daemon that
   * restarted), after Herdr confirms the pane exists and that the agent name
   * still points at it. Nothing is registered otherwise.
   */
  async adoptPane(input: {
    paneId: string;
    role: PaneRole;
    agent: string;
    workspaceId: string | null;
    worktreePath: string | null;
  }): Promise<void> {
    requireMatch(input.paneId, PANE_PATTERN, "pane id");
    if (!isAgentName(input.agent))
      throw new InvalidArgumentError("agent name is not acceptable");
    if (this.core.panes.has(input.paneId))
      throw new PhaseError("the pane is already registered");
    await this.#assertPaneExists(input.paneId);
    let state: { status: string; paneId: string; kind: string };
    try {
      state = await this.core.agentState(input.agent);
    } catch (error) {
      if (
        this.core.slug === undefined ||
        !(error instanceof HerdrError) ||
        error.code !== "agent_not_found"
      )
        throw error;
      // An agent started before names carried the project is still known by its bare id: name it the new way when it is on the recorded pane.
      const legacy = await this.core.agentStateByHerdrName(input.agent);
      if (legacy.paneId !== input.paneId)
        throw new AgentPaneMismatch(
          "the agent name points at another pane than the recorded one",
        );
      await this.core.runChecked([
        "agent",
        "rename",
        input.paneId,
        this.core.herdrName(input.agent),
      ]);
      state = await this.core.agentState(input.agent);
    }
    if (state.paneId !== input.paneId)
      throw new AgentPaneMismatch(
        "the agent name points at another pane than the recorded one",
      );
    if (!(HOST_KINDS as readonly string[]).includes(state.kind))
      throw new UnsupportedHostError(
        `the agent is a ${state.kind} agent, which this adapter does not drive`,
      );
    this.core.panes.set(input.paneId, {
      role: input.role,
      phase: "started",
      kind: state.kind as HostKind,
      agent: input.agent,
      ...(input.workspaceId === null ? {} : { workspaceId: input.workspaceId }),
      ...(input.worktreePath === null
        ? {}
        : { worktreePath: input.worktreePath }),
    });
  }
  /** Registers the fallback watch pane of an earlier instance; it never takes input from the adapter again. */
  async adoptShellPane(
    paneId: string,
    workspaceId: string | null,
  ): Promise<void> {
    requireMatch(paneId, PANE_PATTERN, "pane id");
    await this.#assertPaneExists(paneId);
    this.core.panes.set(paneId, {
      role: "worker",
      phase: "started",
      kind: "shell",
      ...(workspaceId === null ? {} : { workspaceId }),
    });
  }
  async #assertPaneExists(paneId: string): Promise<void> {
    try {
      await runJson(this.core.run, ["pane", "get", paneId]);
    } catch (error) {
      if (error instanceof HerdrError && /not_found|no_such/.test(error.code))
        throw new PaneGone("Herdr has no such pane");
      throw error;
    }
  }
  #canonical(value: string): string {
    const trim = (text: string): string => text.replace(/\/+$/, "") || "/";
    try {
      return trim(fs.realpathSync(value));
    } catch {
      return trim(path.resolve(value));
    }
  }
}
