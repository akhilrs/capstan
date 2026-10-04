/**
 * Starts, restarts and re-adopts the agents that live in Herdr panes: the PM,
 * workers spawned into their own git worktrees, and the fallback watch pane.
 * It runs inside the daemon, which owns the adapter, and does one operation at
 * a time. Every core write builds its own context at the moment of the call.
 *
 * The work lives in src/launcher/: the Launcher builds one shared kernel (the
 * state, the operation queue and the helpers every operation needs) and the
 * collaborators that take it: hub and PM, spawn, release and replace, setup
 * and teardown, observe and adopt.
 */
import type {
  CaptureOutcome,
  PromptAnswer,
  RelayOutcome,
} from "./herdr/prompt-relay.js";
import { HubOps } from "./launcher/hub.js";
import { LauncherKernel } from "./launcher/kernel.js";
import { ObserveOps } from "./launcher/observe.js";
import { ReleaseOps } from "./launcher/release.js";
import { SetupOps } from "./launcher/setup.js";
import {
  type BranchRenameResult,
  type LaunchResult,
  type LauncherOptions,
  type LauncherStatus,
  type ObserveResult,
  type ReleaseResult,
  type ReplaceResult,
  type SpawnOptions,
  type SpawnResult,
} from "./launcher/shared.js";
import { SpawnOps } from "./launcher/spawn.js";

export {
  ADOPT_BUDGET_MS,
  CLEANUP_BUDGET_MS,
  LauncherError,
  MAX_WAITING_OPERATIONS,
  STEP_BUDGET_MS,
  cstanWrapperScript,
  selfInvocation,
} from "./launcher/shared.js";
export type {
  BranchRenameResult,
  GitRunner,
  LaunchResult,
  LaunchState,
  LauncherAdapter,
  LauncherOptions,
  LauncherStatus,
  ObserveResult,
  ReleaseOutcome,
  ReleaseResult,
  ReplaceResult,
  SetupOutcome,
  SetupRunner,
  SpawnOptions,
  SpawnResult,
  TeardownRunner,
} from "./launcher/shared.js";
export { defaultGit } from "./launcher/git.js";
export { runSetupCommand } from "./launcher/setup.js";

export class Launcher {
  readonly #kernel: LauncherKernel;

  constructor(options: LauncherOptions) {
    const kernel = new LauncherKernel(options);
    kernel.hub = new HubOps(kernel);
    kernel.releaser = new ReleaseOps(kernel);
    kernel.spawner = new SpawnOps(kernel);
    kernel.observer = new ObserveOps(kernel);
    kernel.setup = new SetupOps(kernel);
    this.#kernel = kernel;
  }

  status(): LauncherStatus {
    return this.#kernel.observer.status();
  }

  /** Reads the blocking permission prompt of an active worker; nothing is typed. */
  capturePrompt(agentId: string): Promise<CaptureOutcome> {
    return this.#kernel.observer.capturePrompt(agentId);
  }

  /** Types an answer to the worker's prompt when the screen still hashes to `promptSha`; every key sent is logged. */
  answerPrompt(
    agentId: string,
    input: {
      promptSha: string;
      answer: PromptAnswer;
      beforeType: () => void | Promise<void>;
    },
  ): Promise<RelayOutcome> {
    return this.#kernel.observer.answerPrompt(agentId, input);
  }

  /** Sends one Esc to a worker that Herdr shows working. True when the Esc was sent. */
  interrupt(agentId: string): Promise<boolean> {
    return this.#kernel.observer.interrupt(agentId);
  }

  /** The recent screen of an active agent. A read: it does not wait for other launcher operations. */
  observe(agentId: string, lines: number): Promise<ObserveResult> {
    return this.#kernel.observer.observe(agentId, lines);
  }

  /** The environment an approved Operator command runs in. */
  operatorEnvironment(): Record<string, string> {
    return this.#kernel.observer.operatorEnvironment();
  }

  /** How many spawn, release or replace operations are running or waiting; a restart waits until it is zero. */
  inFlightOperations(): number {
    return this.#kernel.observer.inFlightOperations();
  }

  launchPm(): Promise<LaunchResult> {
    return this.#kernel.hub.launchPm();
  }

  restartPm(): Promise<LaunchResult> {
    return this.#kernel.hub.restartPm();
  }

  /** Replaces a worker: releases it (when it is still active) and starts a new agent of the same role whose prompt carries a seed built from the ledger. */
  replace(agentId: string): Promise<ReplaceResult> {
    return this.#kernel.releaser.replace(agentId);
  }

  /** Ends a worker and frees its pane, worktree and, when it holds no commits, its branch. A branch with commits is kept for the user to merge. */
  release(agentId: string): Promise<ReleaseResult> {
    return this.#kernel.releaser.release(agentId);
  }

  /** Starts a worker of `roleName` in its own worktree; see SpawnOps.spawn for how the branch is named. */
  spawn(roleName: string, options: SpawnOptions = {}): Promise<SpawnResult> {
    return this.#kernel.spawner.spawn(roleName, options);
  }

  /** Gives an assignee's branch the name of its task when it still has no commit after its base and no report; otherwise the branch is kept. */
  renameBranchForTask(
    agentId: string,
    task: string,
  ): Promise<BranchRenameResult> {
    return this.#kernel.spawner.renameBranchForTask(agentId, task);
  }

  /** Re-registers the panes recorded before a daemon restart; the daemon runs it in the background before it starts the driver. */
  adoptAll(): Promise<void> {
    return this.#kernel.observer.adoptAll();
  }
}
