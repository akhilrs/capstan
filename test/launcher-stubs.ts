import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { PaneLost } from "../src/herdr/adapter.js";
import { HerdrError } from "../src/herdr/runner.js";
import type { GitRunner, LauncherAdapter } from "../src/launcher.js";

export const SHA = "b".repeat(40);

export interface StartCall {
  name: string;
  kind: string;
  paneId: string;
  args: readonly string[];
  environment: Record<string, string> | undefined;
}

export class StubAdapter implements LauncherAdapter {
  // The driver's part of the adapter, so one stub serves the daemon too.
  async guardedSend() {
    return { sent: false as const, reason: "agent_busy" as const };
  }
  async clearAfterDeferral() {
    return { cleared: false, text: "" };
  }

  readonly calls: string[] = [];
  readonly keeps: number[] = [];
  readonly created: string[] = [];
  readonly metadata: Array<{ target: string; tokens: Record<string, string> }> =
    [];
  readonly labels: string[] = [];
  metadataError: Error | undefined;
  readonly starts: StartCall[] = [];
  readonly entries = new Map<string, { agent?: string }>();
  readonly agentPanes = new Map<string, string>();
  readonly prompts: string[] = [];
  readonly keys: unknown[] = [];
  private counter = 0;
  startStatus: "started" | "blocked_at_startup" = "started";
  startError: Error | undefined;
  startGate: Promise<void> | undefined;
  dialogHandled = true;
  adoptErrors = new Map<string, Error>();
  observation: "idle" | "blocked" | "working" = "idle";
  closeError: Error | undefined;
  worktreeError: Error | undefined;
  runError: Error | undefined;
  dir = mkdtempSync(path.join(tmpdir(), "capstan-launcher-prompts-"));

  // The PM's tab as the layout sees it: sizes in terminal cells.
  pmWorkspace: string | undefined;
  tabPanes: Array<{ paneId: string; width: number; height: number }> = [];
  layoutSize = { width: 200, height: 50 };
  zoomed = false;
  layoutError: Error | undefined;
  placeError: Error | undefined;
  /** Closing a pane that is not there answers pane_not_found, as Herdr does. */
  closeMissingThrows = false;
  strays = new Map<string, Array<{ paneId: string; workspaceId: string }>>();
  private placed = 10;

  async paneLayout(paneId: string) {
    this.calls.push(`layout:${paneId}`);
    if (this.layoutError) throw this.layoutError;
    const workspace = this.pmWorkspace ?? "w1";
    return {
      tabId: `${workspace}:t1`,
      workspaceId: workspace,
      zoomed: this.zoomed,
      panes: this.tabPanes.map((pane) => ({ ...pane })),
    };
  }

  async placePane(input: {
    paneId: string;
    tabId: string;
    targetPaneId: string;
    direction: "right" | "down";
    keep: number;
    worktreePath: string;
  }) {
    this.keeps.push(input.keep);
    this.calls.push(
      `place:${input.paneId}:${input.targetPaneId}:${input.direction}`,
    );
    if (this.placeError) {
      // A lost move: the old pane is gone and no registered pane replaced it.
      if (this.placeError instanceof PaneLost)
        this.entries.delete(input.paneId);
      throw this.placeError;
    }
    const target = this.tabPanes.find((p) => p.paneId === input.targetPaneId)!;
    const workspace = this.pmWorkspace!;
    this.placed += 1;
    const paneId = `${workspace}:p${this.placed}`;
    const kept = (n: number) => Math.floor(n * input.keep);
    const next =
      input.direction === "right"
        ? { width: target.width - kept(target.width), height: target.height }
        : { width: target.width, height: target.height - kept(target.height) };
    if (input.direction === "right") target.width = kept(target.width);
    else target.height = kept(target.height);
    this.tabPanes.push({ paneId, ...next });
    this.entries.delete(input.paneId);
    this.entries.set(paneId, {});
    return { paneId, workspaceId: workspace };
  }

  async panesAtPath(directory: string) {
    this.calls.push(`panes-at:${directory}`);
    return this.strays.get(directory) ?? [];
  }

  async createWorkspace(input: {
    cwd: string;
    label: string;
    role: "PM" | "worker";
  }) {
    this.counter += 1;
    const paneId = `w${this.counter}:p1`;
    this.calls.push(`workspace:${input.label}:${input.role}`);
    this.entries.set(paneId, {});
    if (input.role === "PM") {
      this.pmWorkspace = `w${this.counter}`;
      this.tabPanes = [{ paneId, ...this.layoutSize }];
    }
    return {
      workspaceId: `w${this.counter}`,
      paneId,
      tabId: `w${this.counter}:t1`,
    };
  }

  async createTab(input: {
    workspaceId: string;
    cwd: string;
    label: string;
    role: "PM" | "worker";
  }) {
    this.counter += 1;
    const paneId = `${input.workspaceId}:p${this.counter + 100}`;
    this.calls.push(`tab:${input.workspaceId}:${input.label}:${input.role}`);
    this.entries.set(paneId, {});
    if (input.role === "PM") this.tabPanes = [{ paneId, ...this.layoutSize }];
    return { tabId: `${input.workspaceId}:t${this.counter}`, paneId };
  }

  async createWorktree(input: {
    workspaceId: string;
    branch: string;
    label: string;
    base?: string;
  }) {
    if (this.worktreeError) throw this.worktreeError;
    this.counter += 1;
    const paneId = `w${this.counter}:p1`;
    this.created.push(input.label);
    this.calls.push(`worktree:${input.branch}:${input.base}`);
    this.worktreeParents.push(input.workspaceId);
    this.entries.set(paneId, {});
    return {
      workspaceId: `w${this.counter}`,
      paneId,
      path: `/tmp/work/${input.label.split(" · ").pop()}`,
      branch: input.branch,
    };
  }

  async prepareShell(input: {
    paneId: string;
    environment: Readonly<Record<string, string>>;
  }) {
    this.calls.push(`prepare:${input.paneId}`);
    this.lastShellEnvironment = { ...input.environment };
  }
  lastShellEnvironment: Record<string, string> | undefined;
  readonly worktreeParents: string[] = [];

  async startAgent(input: {
    name: string;
    kind: string;
    paneId: string;
    args: readonly string[];
    environment?: Readonly<Record<string, string>>;
  }) {
    await this.startGate;
    if (this.startError) throw this.startError;
    this.starts.push({
      name: input.name,
      kind: input.kind,
      paneId: input.paneId,
      args: input.args,
      environment:
        input.environment === undefined ? undefined : { ...input.environment },
    });
    this.entries.set(input.paneId, { agent: input.name });
    this.agentPanes.set(input.name, input.paneId);
    this.calls.push(`start:${input.name}`);
    return { status: this.startStatus };
  }

  async answerTrustDialog(input: {
    paneId: string;
    log: (entry: {
      kind: "key";
      pane: string;
      key: string;
      reason: string;
    }) => void | Promise<void>;
  }) {
    this.calls.push(`dialog:${input.paneId}`);
    if (!this.dialogHandled)
      return { handled: false as const, reason: "path_mismatch" };
    for (const key of ["down", "enter"]) {
      const entry = {
        kind: "key" as const,
        pane: input.paneId,
        key,
        reason: "trust",
      };
      this.keys.push(entry);
      await input.log(entry);
    }
    return { handled: true as const, keys: ["down", "enter"] };
  }

  async reportMetadata(
    target: { paneId: string } | { workspaceId: string },
    tokens: Readonly<Record<string, string>>,
  ) {
    if (this.metadataError) throw this.metadataError;
    this.metadata.push({
      target: "paneId" in target ? target.paneId : target.workspaceId,
      tokens: { ...tokens },
    });
  }

  async renameTab(tabId: string, label: string) {
    this.labels.push(`${tabId}:${label}`);
  }

  async renameWorkspace(workspaceId: string, label: string) {
    this.labels.push(`${workspaceId}:${label}`);
  }

  async closePane(paneId: string) {
    this.calls.push(`close:${paneId}`);
    if (this.closeError) throw this.closeError;
    if (this.closeMissingThrows && !this.entries.has(paneId))
      throw new HerdrError("pane_not_found", "no such pane");
    this.entries.delete(paneId);
    this.tabPanes = this.tabPanes.filter((p) => p.paneId !== paneId);
    for (const [agent, pane] of this.agentPanes)
      if (pane === paneId) this.agentPanes.delete(agent);
  }

  async adoptPane(input: { paneId: string; agent: string }) {
    const error = this.adoptErrors.get(input.paneId);
    if (error) throw error;
    this.calls.push(`adopt:${input.paneId}`);
    this.entries.set(input.paneId, { agent: input.agent });
    this.agentPanes.set(input.agent, input.paneId);
  }

  forgetPane(paneId: string): void {
    this.calls.push(`forget:${paneId}`);
    this.entries.delete(paneId);
    for (const [agent, pane] of this.agentPanes)
      if (pane === paneId) this.agentPanes.delete(agent);
  }

  async adoptShellPane(paneId: string) {
    const error = this.adoptErrors.get(paneId);
    if (error) throw error;
    this.calls.push(`adopt-shell:${paneId}`);
    this.entries.set(paneId, {});
  }

  async runInPane(paneId: string, command: string) {
    if (this.runError) throw this.runError;
    this.calls.push(`run:${paneId}:${command}`);
  }

  writePromptFile(text: string): string {
    this.prompts.push(text);
    const file = path.join(this.dir, `prompt-${this.prompts.length}.md`);
    writeFileSync(file, text, { mode: 0o600 });
    return file;
  }

  paneForAgent(agentId: string): string | undefined {
    return this.agentPanes.get(agentId);
  }

  paneEntry(paneId: string) {
    return this.entries.get(paneId) as ReturnType<LauncherAdapter["paneEntry"]>;
  }

  /** What readScreen answers, per pane; a pane in `unreadablePanes` throws. */
  readonly screens = new Map<string, string>();
  readonly unreadablePanes = new Set<string>();
  readonly screenReads: Array<{ paneId: string; lines: number | undefined }> =
    [];
  async readScreen(
    paneId: string,
    options: { ansi?: boolean; lines?: number } = {},
  ): Promise<string> {
    this.screenReads.push({ paneId, lines: options.lines });
    if (this.unreadablePanes.has(paneId)) throw new Error("pane read failed");
    return this.screens.get(paneId) ?? "";
  }

  async agentObservation() {
    return this.observation;
  }
}

export class StubGit implements GitRunner {
  head = SHA;
  removed: string[] = [];
  deleted: Array<[string, string]> = [];
  removeOk = true;
  deleteOk = true;
  byBranch = new Map<string, string>();
  headSha() {
    return this.head;
  }
  worktreeRemove(p: string) {
    this.removed.push(p);
    return this.removeOk;
  }
  deleteBranchIf(branch: string, sha: string) {
    this.deleted.push([branch, sha]);
    return this.deleteOk;
  }
  branchNamesValid = true;
  branchNameValid() {
    return this.branchNamesValid;
  }
  /** Tips by branch name for branchTip. */
  tips = new Map<string, string>();
  branchTip(branch: string) {
    return this.tips.get(branch) ?? null;
  }
  /** Commits reachableCommit accepts; any 40-hex string when `reachableAll` is true. */
  reachable = new Set<string>();
  reachableAll = false;
  reachableCommit(sha: string) {
    return this.reachableAll || this.reachable.has(sha);
  }
  byBranchError: Error | undefined;
  worktreeByBranch(branch: string) {
    if (this.byBranchError) throw this.byBranchError;
    return this.byBranch.get(branch);
  }
}
