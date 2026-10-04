/** Runs the worktree setup and teardown commands. */
import { runCommand } from "../command-runner.js";
import {
  DEFAULT_WORKTREE_TEARDOWN_TIMEOUT_SECONDS,
  type ResolvedWorktree,
} from "../config/capstan-config.js";
import type { LauncherKernel } from "./kernel.js";
import { LauncherError, type Budget, type SetupOutcome } from "./shared.js";
import { oneLine } from "./text.js";

export const SETUP_OUTPUT_CHARS = 2000;

/** Runs `command` through `sh -c` in its own process group; the whole group is killed on timeout. Its output is kept only as a capped tail. */
export async function runSetupCommand(
  command: string,
  cwd: string,
  timeoutMs: number,
  environment: NodeJS.ProcessEnv,
): Promise<SetupOutcome> {
  const result = await runCommand({
    command,
    cwd,
    timeoutMs,
    environment,
    outputTailBytes: SETUP_OUTPUT_CHARS * 2,
  });
  if (result.status === "ok") return { status: "ok" };
  if (result.status === "timeout") return { status: "timeout" };
  return {
    status: "failed",
    exitCode: result.exitCode,
    output: result.outputTail,
  };
}

export class SetupOps {
  constructor(private readonly k: LauncherKernel) {}

  /** Runs the configured setup command in a new worktree. Its time is not taken from the operation's budget; a failure or timeout throws, so the caller's cleanup removes the agent. */
  async setupWorktree(
    config: ResolvedWorktree,
    agentId: string,
    worktreePath: string,
    budget: Budget,
  ): Promise<void> {
    const setup = config.setup;
    if (setup === undefined) return;
    budget.check("running the worktree setup");
    const started = this.k.now();
    let outcome: SetupOutcome;
    try {
      outcome = await this.k.runSetup(
        setup,
        worktreePath,
        config.setupTimeoutSeconds * 1000,
      );
    } finally {
      budget.extend(Math.max(0, this.k.now() - started));
    }
    if (outcome.status === "ok") return;
    const command = oneLine(setup, 200);
    if (outcome.status === "timeout")
      throw new LauncherError(
        "worktree_setup_failed",
        `the setup of ${agentId} (${command}) timed out after ${config.setupTimeoutSeconds}s`,
      );
    const tail = oneLine(outcome.output, SETUP_OUTPUT_CHARS);
    throw new LauncherError(
      "worktree_setup_failed",
      `the setup of ${agentId} (${command}) failed with exit code ${outcome.exitCode ?? "none"}${tail === "" ? "" : `: ${tail}`}`,
    );
  }

  /** Runs the configured teardown command in the project root just before a worktree is removed. A failure or timeout is logged and never stops the removal. */
  async teardownWorktree(
    agentId: string,
    worktreePath: string,
    budget: Budget,
  ): Promise<void> {
    const config = this.k.config.worktree;
    const teardown = config?.teardown;
    if (config === undefined || teardown === undefined) return;
    const timeoutSeconds =
      config.teardownTimeoutSeconds ??
      DEFAULT_WORKTREE_TEARDOWN_TIMEOUT_SECONDS;
    this.k.log("teardown_started", { agentId });
    const started = this.k.now();
    let outcome: SetupOutcome;
    try {
      outcome = await this.k.runTeardown(
        teardown,
        this.k.root,
        timeoutSeconds * 1000,
        {
          ...this.k.environment(null, true),
          CAPSTAN_WORKTREE_PATH: worktreePath,
          CAPSTAN_AGENT_ID: agentId,
        },
      );
    } catch (error) {
      outcome = {
        status: "failed",
        exitCode: null,
        output: error instanceof Error ? error.message : String(error),
      };
    } finally {
      budget.extend(Math.max(0, this.k.now() - started));
    }
    if (outcome.status === "ok") return;
    this.k.log("teardown_failed", {
      agentId,
      exit: outcome.status === "timeout" ? "timeout" : outcome.exitCode,
      output:
        outcome.status === "failed"
          ? oneLine(outcome.output, SETUP_OUTPUT_CHARS)
          : "",
    });
  }
}
