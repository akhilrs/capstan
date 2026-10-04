/** Constants, types and the error shared by the launcher and its collaborators. */
import type { CapstanConfig } from "../config/capstan-config.js";
import type { ControllerCore } from "../controller/core.js";
import { shellQuote, type HerdrAdapter } from "../herdr/adapter.js";
import { isSea } from "../sea.js";

export const STEP_BUDGET_MS = 60_000;
export const CLEANUP_BUDGET_MS = 30_000;
export const ADOPT_BUDGET_MS = 20_000;
export const MAX_WAITING_OPERATIONS = 1;
export const START_TIMEOUT_MS = 30_000;
export const START_ATTEMPTS = 3;
export const START_RETRY_DELAY_MS = 1_500;
export const DIALOG_TIMEOUT_MS = 10_000;
export const ID_ATTEMPTS = 3;

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
  | "capturePrompt"
  | "answerPrompt"
  | "interruptWorking"
>;

export interface GitRunner {
  headSha(): string;
  /** Removes a worktree. Only a worktree of a legacy `capstan/` branch or of `recordedBranch` (the branch the ledger records for its agent) is forced (it may hold untracked files such as installed dependencies); any other is removed without force. `stderr` is git's message when it refuses. */
  worktreeRemove(
    worktreePath: string,
    recordedBranch?: string,
  ): {
    readonly removed: boolean;
    readonly stderr: string;
  };
  /** How many files are untracked or modified in the worktree; null when git cannot tell. */
  worktreeDirtyCount(worktreePath: string): number | null;
  /** Atomic compare-and-delete: only when the branch still points at `sha`. */
  deleteBranchIf(branch: string, sha: string): boolean;
  worktreeByBranch(branch: string): string | undefined;
  /** Whether `branch` names a valid branch, checked by git before a worktree is created for it. */
  branchNameValid(branch: string): boolean;
  /** `git branch -m` from the project root; the branch's worktree follows. `stderr` is git's message when it refuses. */
  renameBranch(
    from: string,
    to: string,
  ): { readonly renamed: boolean; readonly stderr: string };
  /** Points `ref` (a full ref name) at `sha`. */
  saveRef(ref: string, sha: string): boolean;
  /** The commit a branch points at, or null when there is no such branch. */
  branchTip(branch: string): string | null;
  /** Whether `sha` (40 lowercase hex characters) resolves as a commit and is an ancestor of one of the refs. */
  reachableCommit(sha: string, from: readonly string[]): boolean;
}

/** Shell words that run this CLI: the binary itself under SEA, otherwise node plus the CLI file. */
export function selfInvocation(node: string, cliPath: string): string {
  if (isSea()) return shellQuote(process.execPath);
  return `${shellQuote(node)} ${shellQuote(cliPath)}`;
}

/** The per-agent `cstan` wrapper script put first on every agent's PATH. */
export function cstanWrapperScript(node: string, cliPath: string): string {
  return `#!/bin/sh\nexec ${selfInvocation(node, cliPath)} "$@"\n`;
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

export interface SpawnOptions {
  readonly baseSha?: string;
  readonly seed?: string;
  /** `<plan-id>/<package-id>` or a requirement ref id: the branch is named after it. */
  readonly task?: string;
  readonly type?: string;
  readonly title?: string;
  /** The report or integration a reviewer looks at. */
  readonly reviewTarget?: string;
  /** Continue this branch name (a replacement's). */
  readonly branch?: string;
}

/** What renaming an assignee's branch did; `note` is the line to print when the branch was kept. */
export interface BranchRenameResult {
  readonly branch: string | null;
  readonly renamed: boolean;
  readonly note?: string;
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
      /** Where the predecessor's unreported commits were saved, when it had any. */
      readonly keptRef?: string;
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
  /** Runs the worktree setup command; tests stub it. */
  readonly runSetup?: SetupRunner;
  /** Waits between attempts to start a worker; tests stub it. */
  readonly sleep?: (milliseconds: number) => Promise<void>;
  /** Runs the worktree teardown command; tests stub it. */
  readonly runTeardown?: TeardownRunner;
}

export interface Hub {
  readonly status: NonNullable<LaunchResult["hub"]>;
  readonly workspaceId: string | null;
  /** The root pane of a hub made in this call; the first PM takes it. */
  readonly freePmPane: string | null;
}

export interface Budget {
  readonly deadline: number;
  check(step: string): void;
  /** Moves the deadline later, for time the operation spent on a step that has its own deadline. */
  extend(ms: number): void;
}

export type SetupOutcome =
  | { readonly status: "ok" }
  | {
      readonly status: "failed";
      readonly exitCode: number | null;
      readonly output: string;
    }
  | { readonly status: "timeout" };

export type SetupRunner = (
  command: string,
  cwd: string,
  timeoutMs: number,
) => Promise<SetupOutcome>;

/** Runs the worktree teardown command with the environment it is given; tests stub it. */
export type TeardownRunner = (
  command: string,
  cwd: string,
  timeoutMs: number,
  environment: NodeJS.ProcessEnv,
) => Promise<SetupOutcome>;

/** Keeps a sync error to one short line in the refusal. */
export const MAX_SYNC_REASON_CHARS = 200;
export const MAX_NOTE_LENGTH = 200;
