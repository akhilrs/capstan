import { writeFileSync } from "node:fs";
import path from "node:path";
import { tempDir } from "./tmp.js";
import { PaneLost, type PaneIdentity } from "../src/herdr/adapter.js";
import { HerdrError } from "../src/herdr/runner.js";
import type {
  CaptureOutcome,
  PromptAnswer,
  RelayOutcome,
} from "../src/herdr/prompt-relay.js";
import type { GitRunner, LauncherAdapter } from "../src/launcher.js";

export const SHA = "b".repeat(40);

/** The terminal id the stub gives a pane it has no other identity for. */
export const stubTerminalId = (paneId: string): string => `term:${paneId}`;

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
  async wakePm() {
    return { sent: false as const, reason: "pm_not_idle" as const };
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
  /** Workspaces whose tabs cannot be made, as when the workspace is gone. */
  readonly tabErrors = new Set<string>();
  tabFailure: Error | undefined;
  readonly starts: StartCall[] = [];
  readonly entries = new Map<string, { agent?: string }>();
  readonly agentPanes = new Map<string, string>();
  readonly prompts: string[] = [];
  readonly keys: unknown[] = [];
  private counter = 0;
  startStatus: "started" | "blocked_at_startup" = "started";
  startError: Error | undefined;
  /** Errors thrown by the next start attempts, one each, before startError applies. */
  startErrors: Error[] = [];
  startAttempts = 0;
  startGate: Promise<void> | undefined;
  dialogHandled = true;
  adoptErrors = new Map<string, Error>();
  observation: "idle" | "blocked" | "working" = "idle";
  closeError: Error | undefined;
  worktreeError: Error | undefined;
  runError: Error | undefined;
  /** Where created worktrees are said to be; a test that runs a real command there points it at a real directory. */
  worktreeBase = "/tmp/work";
  dir = tempDir("capstan-launcher-prompts-");

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
    if (this.tabFailure) throw this.tabFailure;
    if (this.tabErrors.has(input.workspaceId))
      throw new HerdrError("workspace_not_found", "no such workspace");
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
      path: `${this.worktreeBase}/${input.label.split(" · ").pop()}`,
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
    this.startAttempts += 1;
    const scripted = this.startErrors.shift();
    if (scripted) throw scripted;
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

  captureOutcome: CaptureOutcome = {
    captured: false,
    reason: "prompt_unrecognized",
  };
  answerOutcome: RelayOutcome = { typed: true, keys: ["enter"] };
  readonly answered: Array<{
    paneId: string;
    promptSha: string;
    answer: PromptAnswer;
  }> = [];

  /** Pane ids that are working: an interrupt sends one Esc to these only. */
  readonly workingPanes = new Set<string>();
  readonly interrupts: string[] = [];

  async interruptWorking(input: {
    paneId: string;
    log: (entry: {
      kind: "key";
      pane: string;
      key: string;
      reason: string;
    }) => void | Promise<void>;
  }): Promise<{ readonly sent: boolean }> {
    this.calls.push(`interrupt:${input.paneId}`);
    if (!this.workingPanes.has(input.paneId)) return { sent: false };
    this.interrupts.push("esc");
    await input.log({
      kind: "key",
      pane: input.paneId,
      key: "esc",
      reason: "interrupt a paused worker",
    });
    return { sent: true };
  }

  async capturePrompt(paneId: string): Promise<CaptureOutcome> {
    this.calls.push(`capture:${paneId}`);
    return this.captureOutcome;
  }

  async answerPrompt(input: {
    paneId: string;
    promptSha: string;
    answer: PromptAnswer;
    beforeType: () => void | Promise<void>;
    log: (entry: {
      kind: "key";
      pane: string;
      key: string;
      reason: string;
    }) => void | Promise<void>;
  }): Promise<RelayOutcome> {
    this.calls.push(`answer:${input.paneId}`);
    this.answered.push({
      paneId: input.paneId,
      promptSha: input.promptSha,
      answer: input.answer,
    });
    if (this.answerOutcome.typed) {
      await input.beforeType();
      await input.log({
        kind: "key",
        pane: input.paneId,
        key: "enter",
        reason: "stub",
      });
    }
    return this.answerOutcome;
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

  /** What Herdr says a pane is, per pane id; a pane not listed answers with a terminal id derived from its id and the tokens reported for it. */
  readonly identities = new Map<string, PaneIdentity>();
  /** Pane ids Herdr no longer has. */
  readonly gonePanes = new Set<string>();
  readonly identified: string[] = [];
  async paneIdentity(paneId: string): Promise<PaneIdentity | undefined> {
    this.identified.push(paneId);
    if (
      this.gonePanes.has(paneId) ||
      (this.closeMissingThrows && !this.entries.has(paneId))
    )
      return undefined;
    const listed = this.identities.get(paneId);
    if (listed !== undefined) return listed;
    const tokens = this.metadata.findLast((m) => m.target === paneId)?.tokens;
    return {
      terminalId: stubTerminalId(paneId),
      agent: tokens?.agent,
      project: tokens?.project,
    };
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

  promptError: Error | undefined;
  writePromptFile(text: string): string {
    if (this.promptError) throw this.promptError;
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
  /** What worktreeDirtyCount answers; null means git could not tell. */
  dirty: number | null = 0;
  /** Git's message when removeOk is false. */
  removeStderr = "";
  worktreeDirtyCount() {
    return this.dirty;
  }
  /** Worktree paths whose directories are gone. */
  missing = new Set<string>();
  prunes = 0;
  worktreePresent(worktreePath: string) {
    return !this.missing.has(worktreePath);
  }
  pruneWorktrees() {
    this.prunes += 1;
  }
  /** Names of the calls that matter for ordering, in call order. */
  order: string[] = [];
  /** The recorded branch worktreeRemove was given, per removal. */
  removedWithBranch: Array<string | undefined> = [];
  worktreeRemove(p: string, recordedBranch?: string) {
    this.order.push("remove");
    this.removed.push(p);
    this.removedWithBranch.push(recordedBranch);
    return { removed: this.removeOk, stderr: this.removeStderr };
  }
  deleteBranchIf(branch: string, sha: string) {
    this.deleted.push([branch, sha]);
    if (this.deleteOk && this.tips.get(branch) === sha)
      this.tips.delete(branch);
    return this.deleteOk;
  }
  renames: Array<[string, string]> = [];
  renameOk = true;
  /** Moves the tip to the new name, as git branch -m does. */
  renameBranch(from: string, to: string) {
    if (!this.renameOk) return { renamed: false, stderr: "refused" };
    this.renames.push([from, to]);
    const tip = this.tips.get(from);
    if (tip !== undefined) {
      this.tips.delete(from);
      this.tips.set(to, tip);
    }
    return { renamed: true, stderr: "" };
  }
  refs = new Map<string, string>();
  saveRef(ref: string, sha: string) {
    this.refs.set(ref, sha);
    return true;
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
  /** Every branch worktreeByBranch was asked about. */
  byBranchQueries: string[] = [];
  worktreeByBranch(branch: string) {
    this.byBranchQueries.push(branch);
    if (this.byBranchError) throw this.byBranchError;
    return this.byBranch.get(branch);
  }
}
