/** Release, replace and the cleanup of an agent's pane, worktree and branch. */
import { SeedTooLargeError, buildSeed, type SeedBase } from "../seed.js";
import type { LauncherKernel } from "./kernel.js";
import {
  ADOPT_BUDGET_MS,
  CLEANUP_BUDGET_MS,
  LauncherError,
  type ReleaseOutcome,
  type ReleaseResult,
  type ReplaceResult,
  type SpawnResult,
} from "./shared.js";

/** Git's answer for a path that is not (or no longer) one of its worktrees. */
const NOT_A_WORKTREE = /is not a working tree/;

export class ReleaseOps {
  constructor(private readonly k: LauncherKernel) {}

  /**
   * Replaces a worker: releases it (when it is still active) and starts a new
   * agent of the same role whose prompt carries a seed built from the ledger.
   * Nothing the predecessor was asked to do is sent again.
   */
  async replace(agentId: string): Promise<ReplaceResult> {
    const agent = this.k.core.agentRecord(agentId);
    if (agent === undefined)
      throw new LauncherError("unknown_agent", `there is no agent ${agentId}`);
    if (agent.kind === "PM")
      throw new LauncherError(
        "kind_not_replaceable",
        "the PM is restarted with cstan pm restart, not replaced",
      );
    if (this.k.replacing.has(agentId))
      throw new LauncherError(
        "replace_running",
        `a replacement of ${agentId} is already running`,
      );
    if (this.k.core.isAgentReplaced(agentId))
      throw new LauncherError(
        "already_replaced",
        `${agentId} was already replaced`,
      );
    if (this.k.core.runningIntegrations(this.k.credential).length > 0)
      throw new LauncherError(
        "integration_running",
        "an integration is running; replace after it finished",
      );
    const role = this.k.config.roles.find((r) => r.name === agent.roleName);
    if (role === undefined)
      throw new LauncherError(
        "unknown_role",
        `the configuration has no role ${agent.roleName}`,
      );
    this.k.assertRoleSynced(role);
    const data = this.k.core.agentSeed(agentId);
    const tip = data.branch === null ? null : this.k.git.branchTip(data.branch);
    const keptRef = `refs/capstan/kept/${agentId}`;
    const reachableFrom = [
      ...(tip === null ? [] : [`refs/heads/${data.branch}`]),
      keptRef,
      "HEAD",
    ];
    const base: SeedBase =
      data.lastAcceptedCommit !== null &&
      this.k.git.reachableCommit(data.lastAcceptedCommit, reachableFrom)
        ? { sha: data.lastAcceptedCommit, source: "predecessor" }
        : { sha: this.k.git.headSha(), source: "head" };
    let seed: string;
    try {
      seed = buildSeed(
        data,
        base,
        tip,
        tip !== null && tip !== base.sha ? keptRef : null,
      );
    } catch (error) {
      if (error instanceof SeedTooLargeError)
        throw new LauncherError("seed_too_large", error.message);
      throw error;
    }
    this.k.replacing.add(agentId);
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
      let kept: { readonly ref: string; readonly tip: string } | undefined;
      try {
        // The successor continues the predecessor's branch at the base; commits past it are saved first.
        if (data.branch !== null) {
          const current = this.k.git.branchTip(data.branch);
          if (current !== null) {
            if (
              this.k.core.activeBranchHolder(data.branch, agentId) !== undefined
            )
              throw new LauncherError(
                "branch_in_use",
                `${data.branch} is held by another active agent`,
              );
            if (this.k.git.worktreeByBranch(data.branch) !== undefined)
              throw new LauncherError(
                "branch_in_use",
                `${data.branch} is still checked out in a worktree`,
              );
            if (current !== base.sha) {
              if (!this.k.git.saveRef(keptRef, current))
                throw new LauncherError(
                  "git_error",
                  `could not save ${current} at ${keptRef}`,
                );
              kept = { ref: keptRef, tip: current };
            }
            if (!this.k.git.deleteBranchIf(data.branch, current))
              throw new LauncherError(
                "git_error",
                `${data.branch} moved while it was being reset`,
              );
          }
        }
        spawned = await this.k.spawner.spawn(agent.roleName, {
          baseSha: base.sha,
          seed,
          ...(data.branch === null ? {} : { branch: data.branch }),
          ...(data.taskRef == null ? {} : { recordTaskRef: data.taskRef }),
          ...(data.taskTitle == null
            ? {}
            : { recordTaskTitle: data.taskTitle }),
        });
      } catch (error) {
        throw new LauncherError(
          "replacement_not_started",
          `${agentId} was ${released === undefined ? "already ended" : "released"} but the replacement could not start (${error instanceof Error ? error.message : String(error)}); run cstan replace ${agentId} again`,
        );
      }
      let replacementRecorded = true;
      try {
        this.k.core.recordAgentReplaced(this.k.context(), {
          predecessorId: agentId,
          successorId: spawned.agentId,
          branch: spawned.branch,
          ...(kept === undefined
            ? {}
            : { keptRef: kept.ref, keptTip: kept.tip }),
        });
      } catch (error) {
        replacementRecorded = false;
        this.k.log("replacement_not_recorded", {
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
        ...(kept === undefined ? {} : { keptRef: kept.ref }),
        replacementRecorded,
      };
    } finally {
      this.k.replacing.delete(agentId);
    }
  }

  /** Ends a worker and frees its pane, worktree and, when it holds no commits, its branch. A branch with commits is kept for the user to merge. */
  release(agentId: string): Promise<ReleaseResult> {
    return this.k.run(async () => {
      await this.k.observer.adoptNow(this.k.budget(ADOPT_BUDGET_MS));
      const agent = this.k.core.agentRecord(agentId);
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
      const row = this.k.core
        .agentPanes(this.k.credential)
        .find((candidate) => candidate.agentId === agentId);
      const outcome = await this.cleanupAgent(agentId, {
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

  /** endAgent, pane, worktree (forced only when it is a Capstan one), branch (compare-and-delete), row; in that order. */
  async cleanupAgent(
    agentId: string,
    info: {
      worktreePath?: string;
      paneId?: string;
      branch?: string;
      baseSha?: string;
      /** True only when a pane move was started and its result never reached the ledger. */
      moveMayHaveHappened?: boolean;
      /** The pane's Herdr terminal id recorded at spawn; read from the ledger row when omitted. */
      terminalId?: string | null;
      /** True only when this operation made the pane, so its id cannot have been given to another pane yet. */
      paneCreatedHere?: boolean;
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
      cancelledMessageIds = this.k.core.endAgent(
        this.k.context(),
        agentId,
        options,
      ).cancelledMessageIds;
    } catch (error) {
      const reason = error instanceof Error ? error.message : String(error);
      this.k.cleanupFailed = this.k.cleanupFailed.filter(
        (entry) => entry.agentId !== agentId,
      );
      this.k.cleanupFailed.push({ agentId, reason });
      this.k.log("cleanup_blocked", { agentId, error: String(error) });
      return { ended: false, reason };
    }
    this.k.cleanupFailed = this.k.cleanupFailed.filter(
      (entry) => entry.agentId !== agentId,
    );
    return {
      ended: true,
      cancelledMessageIds,
      ...(await this.releaseResources(agentId, info)),
    };
  }

  /**
   * Everything an ended agent still holds: its pane, its worktree (forced
   * only when it is a Capstan one) and its branch (only at the base commit). The pane row goes last
   * and only when the worktree is gone, so a refused removal stays in the
   * ledger and the next start retries it.
   */
  async releaseResources(
    agentId: string,
    info: {
      worktreePath?: string;
      paneId?: string;
      branch?: string;
      baseSha?: string;
      /** True only when a pane move was started and its result never reached the ledger. */
      moveMayHaveHappened?: boolean;
      /** The pane's Herdr terminal id recorded at spawn; read from the ledger row when omitted. */
      terminalId?: string | null;
      /** True only when this operation made the pane, so its id cannot have been given to another pane yet. */
      paneCreatedHere?: boolean;
    },
  ): Promise<ReleaseOutcome> {
    const budget = this.k.budget(CLEANUP_BUDGET_MS);
    let paneClosed: boolean | null = null;
    if (info.paneId !== undefined) {
      paneClosed = false;
      if (this.k.within(budget)) {
        try {
          const existed = info.paneCreatedHere
            ? await this.k.close(info.paneId)
            : (await this.k.closeOwned(info.paneId, {
                agentId,
                terminalId:
                  info.terminalId === undefined
                    ? this.k.core.paneTerminalId(this.k.credential, agentId)
                    : info.terminalId,
              })) === "closed";
          paneClosed = true;
          if (
            !existed &&
            info.worktreePath !== undefined &&
            info.moveMayHaveHappened === true
          )
            await this.closeMovedPane(agentId, info.worktreePath, info.paneId);
        } catch (error) {
          this.k.log("pane_not_closed", {
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
    // A branch the ledger records for another active agent (a replacement continues its predecessor's) is never deleted or force-removed here.
    const heldByOther =
      info.branch !== undefined &&
      this.k.core.activeBranchHolder(info.branch, agentId) !== undefined;
    let worktreePath = info.worktreePath;
    if (
      worktreePath === undefined &&
      info.branch !== undefined &&
      !heldByOther
    ) {
      try {
        worktreePath = this.k.git.worktreeByBranch(info.branch);
      } catch (error) {
        // Unknown is not "none": keep the branch and the row for the next start.
        this.k.log("worktree_unknown", { agentId, error: String(error) });
        return {
          paneClosed,
          worktreeRemoved: false,
          branchKept: info.branch === undefined ? null : true,
        };
      }
    }
    let worktreeRemoved: boolean | null = null;
    if (
      worktreePath !== undefined &&
      !this.k.git.worktreePresent(worktreePath)
    ) {
      // Already gone: nothing to tear down, and the cleanup is finished, so the row goes and it is never tried again.
      this.k.git.pruneWorktrees();
      this.k.log("worktree_gone", { agentId, worktreePath });
      worktreeRemoved = true;
    } else if (worktreePath !== undefined) {
      await this.k.setup.teardownWorktree(agentId, worktreePath, budget);
      this.k.log("worktree_removing", {
        agentId,
        dirty: this.k.git.worktreeDirtyCount(worktreePath),
      });
      const removal = this.k.git.worktreeRemove(
        worktreePath,
        heldByOther ? undefined : info.branch,
      );
      worktreeRemoved = removal.removed;
      // A directory git no longer knows as a worktree has nothing left for git to remove; retrying would fail the same way forever.
      if (!worktreeRemoved && NOT_A_WORKTREE.test(removal.stderr)) {
        this.k.git.pruneWorktrees();
        this.k.log("worktree_gone", {
          agentId,
          worktreePath,
          stderr: removal.stderr,
        });
        worktreeRemoved = true;
      }
      if (!worktreeRemoved) {
        this.k.log("worktree_kept", {
          agentId,
          worktreePath,
          stderr: removal.stderr,
        });
        return {
          paneClosed,
          worktreeRemoved,
          branchKept: info.branch === undefined ? null : true,
        };
      }
    }
    let branchKept: boolean | null = info.branch === undefined ? null : true;
    if (heldByOther) {
      this.k.log("branch_kept", {
        agentId,
        branch: info.branch,
        reason: "held_by_active_agent",
      });
    } else if (info.branch !== undefined && info.baseSha !== undefined) {
      branchKept = !this.k.git.deleteBranchIf(info.branch, info.baseSha);
      if (branchKept)
        this.k.log("branch_kept", { agentId, branch: info.branch });
    }
    try {
      this.k.core.clearAgentPane(this.k.context(), agentId);
    } catch (error) {
      this.k.log("pane_row_not_cleared", { agentId, error: String(error) });
    }
    return { paneClosed, worktreeRemoved, branchKept };
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
  interruptedMove(workspaceId: string | null): boolean {
    if (this.k.config.layout.spawn !== "pane" || workspaceId === null)
      return false;
    const pm = this.k.activeAgents().find((agent) => agent.kind === "PM");
    const pmWorkspace = this.k.core
      .agentPanes(this.k.credential)
      .find((row) => row.agentId === pm?.agentId)?.workspaceId;
    return pmWorkspace !== undefined && pmWorkspace !== workspaceId;
  }

  async closeMovedPane(
    agentId: string,
    worktreePath: string,
    recordedPaneId: string,
  ): Promise<void> {
    try {
      // A moved pane lands in the PM's workspace, so a pane anywhere else is the operator's.
      const pm = this.k.activeAgents().find((agent) => agent.kind === "PM");
      const pmWorkspace = this.k.core
        .agentPanes(this.k.credential)
        .find((row) => row.agentId === pm?.agentId)?.workspaceId;
      if (pmWorkspace === undefined || pmWorkspace === null) return;
      const strays = (await this.k.adapter.panesAtPath(worktreePath)).filter(
        (pane) =>
          pane.paneId !== recordedPaneId &&
          pane.workspaceId === pmWorkspace &&
          this.k.adapter.paneEntry(pane.paneId) === undefined,
      );
      if (strays.length !== 1) return;
      await this.k.close(strays[0]!.paneId);
      this.k.log("moved_pane_closed", { agentId, paneId: strays[0]!.paneId });
    } catch (error) {
      this.k.log("moved_pane_not_closed", { agentId, error: String(error) });
    }
  }
}
