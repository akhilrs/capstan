/** Spawns a worker into its own worktree and names its branch. */
import type { ControllerCore } from "../controller/core.js";
import {
  adHocBranchName,
  adHocBranchPrefix,
  reviewBranchName,
  slugify,
  withSuffix,
  workerBranchName,
} from "../conventions.js";
import {
  PaneLost,
  PhaseError,
  PromptUnrecognized,
  ShellNotReady,
} from "../herdr/adapter.js";
import { workspaceLabel } from "../herdr/naming.js";
import { HerdrError } from "../herdr/runner.js";
import { choosePlacement, type LayoutPane } from "../layout.js";
import { buildRolePrompt } from "../prompts.js";
import type { LauncherKernel } from "./kernel.js";
import {
  ADOPT_BUDGET_MS,
  DIALOG_TIMEOUT_MS,
  LauncherError,
  MAX_NOTE_LENGTH,
  START_ATTEMPTS,
  START_RETRY_DELAY_MS,
  START_TIMEOUT_MS,
  type BranchRenameResult,
  type LauncherAdapter,
  type SpawnOptions,
  type SpawnResult,
} from "./shared.js";
import { oneLine } from "./text.js";
import { normalizeTaskTitle, TASK_REF_PATTERN } from "../task-text.js";

export class SpawnOps {
  constructor(private readonly k: LauncherKernel) {}

  /**
   * `baseSha` makes the worker's worktree and branch start at that commit (a review) instead of the project's HEAD.
   * The branch is named after `task` (a `<plan-id>/<package-id>` or a requirement ref id), after `reviewTarget` (a
   * reviewer), or is `branch` itself (a replacement continues its predecessor's); otherwise it is an ad-hoc `chore/` name.
   */
  spawn(roleName: string, options: SpawnOptions = {}): Promise<SpawnResult> {
    try {
      this.k.core.assertRunNotPaused("spawn");
    } catch (error) {
      return Promise.reject(error);
    }
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
    let taskFields: { taskRef?: string; taskTitle?: string };
    try {
      taskFields = this.taskFields(options);
    } catch (error) {
      return Promise.reject(error);
    }
    return this.k.runStarting(async (budget) => {
      await this.k.observer.adoptNow(this.k.budget(ADOPT_BUDGET_MS));
      const role = this.k.config.roles.find((r) => r.name === roleName);
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
      if (
        this.k.config.operator?.enabled !== true &&
        this.k.isOperatorRole(role.name, role.kind)
      )
        throw new LauncherError(
          "operator_disabled",
          `the role ${role.name} is the Operator and [operator] is not enabled`,
        );
      this.k.assertRoleSynced(role);
      // The Supervisor watches the workers and does not take one of their places; the Architect and the Operator do not either unless the configuration says they count.
      const architectCounts =
        this.k.config.architect?.countTowardWorkerLimit === true;
      const operatorCounts =
        this.k.config.operator?.countTowardWorkerLimit === true;
      const exempt = (name: string, kind: string): boolean =>
        kind === "Supervisor" ||
        (!architectCounts && this.k.isArchitectRole(name, kind)) ||
        (!operatorCounts &&
          this.k.config.operator?.enabled === true &&
          this.k.isOperatorRole(name, kind));
      const workers = this.k
        .activeAgents()
        .filter((a) => a.kind !== "PM" && !exempt(a.roleName, a.kind));
      const limit = this.k.config.limits.maxWorkers;
      if (!exempt(role.name, role.kind) && workers.length >= limit) {
        const stuck = workers
          .map((a) => ({
            id: a.agentId,
            failure: this.k.cleanupFailed.find((c) => c.agentId === a.agentId),
          }))
          .filter((entry) => entry.failure !== undefined)
          .map((entry) => `${entry.id}: ${entry.failure!.reason}`);
        throw new LauncherError(
          "worker_limit",
          `${workers.length} of ${limit} workers are active (${workers.map((a) => a.agentId).join(", ")}); release one with cstan release <agent-id>${stuck.length === 0 ? "" : `; a cleanup failed for ${stuck.join("; ")}`}`,
        );
      }
      // Fails with the git requirement's message before an agent record or a hub exists.
      this.k.git.headSha();
      const pm = this.k.activeAgents().find((a) => a.kind === "PM");
      const pmPane =
        pm === undefined
          ? undefined
          : this.k.core
              .agentPanes(this.k.credential)
              .find((r) => r.agentId === pm.agentId);
      if (pmPane === undefined || pmPane.workspaceId === null)
        throw new LauncherError(
          "pm_not_launched",
          "launch the PM first with cstan start",
        );
      // Herdr refuses to close a pane whose workspace has worktree children, so
      // worktrees hang under the long-lived hub workspace, never under the PM's.
      if ((await this.k.hub.ensureHubWithoutPm(budget)) === "failed")
        throw new LauncherError(
          "hub_unavailable",
          "the hub workspace could not be opened",
        );
      const hub = this.k.core.fallbackPane(this.k.credential);
      if (hub === undefined)
        throw new LauncherError(
          "hub_unavailable",
          "the hub workspace could not be opened",
        );
      budget.check("creating the agent");
      const agent = this.k.createAgent(role);
      const info: {
        worktreePath?: string;
        paneId?: string;
        branch?: string;
        baseSha?: string;
        moveMayHaveHappened?: boolean;
        paneCreatedHere?: boolean;
      } = {};
      let step = "worktree";
      try {
        const branch = this.spawnBranch(agent.agentId, role, options);
        info.branch = branch;
        if (!this.k.git.branchNameValid(branch))
          throw new LauncherError(
            "invalid_branch",
            `git does not accept the branch name ${branch}`,
          );
        const baseSha = options.baseSha ?? this.k.git.headSha();
        info.baseSha = baseSha;
        this.k.core.recordAgentPane(this.k.context(), {
          agentId: agent.agentId,
          workspaceId: null,
          paneId: null,
          worktreePath: null,
          branch,
          baseSha,
          ...taskFields,
        });
        budget.check("creating the worktree");
        const tree = await this.k.adapter.createWorktree({
          workspaceId: hub.workspaceId,
          branch,
          label: workspaceLabel(this.k.project, agent.agentId),
          base: baseSha,
        });
        info.worktreePath = tree.path;
        info.paneId = tree.paneId;
        info.paneCreatedHere = true;
        const terminalId = await this.k.terminalOf(tree.paneId, agent.agentId);
        this.k.core.recordAgentPane(this.k.context(), {
          agentId: agent.agentId,
          workspaceId: tree.workspaceId,
          paneId: tree.paneId,
          worktreePath: tree.path,
          branch,
          baseSha,
          ...taskFields,
          ...(terminalId === null ? {} : { terminalId }),
        });
        if (this.k.config.worktree !== undefined) {
          step = "setup";
          await this.k.setup.setupWorktree(
            this.k.config.worktree,
            agent.agentId,
            tree.path,
            budget,
          );
        }
        let paneId = tree.paneId;
        let placement: SpawnResult["placement"] = "tab";
        let placementNote: string | undefined;
        if (this.k.config.layout.spawn === "pane") {
          step = "place";
          budget.check("placing the worker pane");
          info.moveMayHaveHappened = true;
          const outcome =
            pmPane.paneId === null
              ? { note: "the PM has no recorded pane" }
              : await this.placeWorkerPane(
                  tree.paneId,
                  tree.path,
                  pmPane.paneId,
                );
          if ("placed" in outcome) {
            paneId = outcome.placed.paneId;
            placement = "pane";
            info.paneId = paneId;
            const placedTerminal = await this.k.terminalOf(
              paneId,
              agent.agentId,
            );
            this.k.core.recordAgentPane(this.k.context(), {
              agentId: agent.agentId,
              workspaceId: outcome.placed.workspaceId,
              paneId,
              worktreePath: tree.path,
              branch,
              baseSha,
              ...taskFields,
              ...(placedTerminal === null
                ? {}
                : { terminalId: placedTerminal }),
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
        step = "start";
        budget.check("starting the worker");
        const promptText = buildRolePrompt({
          roleName: role.name,
          kind: role.kind,
          agentId: agent.agentId,
          waitTimeoutSeconds: this.k.waitSeconds(role),
          rolePrompt: role.promptText,
          ...(this.k.architectPrompt() === undefined
            ? {}
            : {
                architect: this.k.architectPrompt()!,
                isArchitect: this.k.isArchitectRole(role.name, role.kind),
              }),
          ...(this.k.operatorPrompt() === undefined
            ? {}
            : {
                operator: this.k.operatorPrompt()!,
                isOperator: this.k.isOperatorRole(role.name, role.kind),
              }),
          ...(this.k.researcherPrompt() === undefined
            ? {}
            : {
                researcher: this.k.researcherPrompt()!,
                isResearcher: this.k.isResearcherRole(role.name, role.kind),
              }),
          ...this.k.nexoraPrompt(),
          ...(options.seed === undefined
            ? {}
            : { replacementSeed: options.seed }),
        });
        const promptFile = this.k.adapter.writePromptFile(promptText);
        const started = await this.startWithRetry(agent.agentId, {
          name: agent.agentId,
          kind: this.k.hostKind(role),
          paneId,
          args: this.k.arguments(
            role,
            { text: promptText, file: promptFile },
            tree.path,
          ),
          environment: this.k.environment(agent.credential),
          timeoutMs: START_TIMEOUT_MS,
        });
        this.k.agentsStarted += 1;
        // The worker's workspace is gone once its pane is placed, and the PM's and the watch workspace already carry the project token.
        await this.k.describe({
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
        step = "trust";
        const answered = await this.k.adapter.answerTrustDialog({
          paneId,
          timeoutMs: DIALOG_TIMEOUT_MS,
          log: (entry) => this.k.log("trust_dialog_key", { ...entry }),
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
        const failure = this.spawnFailure(error, step);
        this.k.log("spawn_failed", {
          agentId: agent.agentId,
          step,
          error: oneLine(failure.message, MAX_NOTE_LENGTH),
        });
        await this.k.releaser.cleanupAgent(agent.agentId, info);
        throw failure;
      }
    });
  }

  /** The task ref and title a spawn stores on every pane row it writes, normalized; keys are omitted when absent. */
  taskFields(options: SpawnOptions): { taskRef?: string; taskTitle?: string } {
    const ref = options.task ?? options.recordTaskRef;
    const title = options.title ?? options.recordTaskTitle;
    const fields: { taskRef?: string; taskTitle?: string } = {};
    try {
      if (ref !== undefined) {
        if (!TASK_REF_PATTERN.test(ref))
          throw new TypeError(
            "the task must be <plan-id>/<package-id> or a requirement ref id",
          );
        fields.taskRef = ref;
      }
      if (title !== undefined) fields.taskTitle = normalizeTaskTitle(title);
    } catch (error) {
      throw new LauncherError(
        "invalid_task",
        error instanceof Error ? error.message : String(error),
      );
    }
    return fields;
  }

  /** The branch a new worker gets; a name that is taken gets `-2`, `-3`, ... */
  spawnBranch(
    agentId: string,
    role: { readonly name: string; readonly kind: string },
    options: SpawnOptions,
  ): string {
    if (options.branch !== undefined) {
      if (
        this.k.git.branchTip(options.branch) !== null ||
        this.k.git.worktreeByBranch(options.branch) !== undefined ||
        this.k.core.activeBranchHolder(options.branch, agentId) !== undefined
      )
        throw new LauncherError(
          "branch_in_use",
          `${options.branch} still exists; it cannot be continued`,
        );
      return options.branch;
    }
    let wanted: string;
    if (options.reviewTarget !== undefined)
      wanted = reviewBranchName(agentId, options.reviewTarget);
    else if (options.task !== undefined)
      wanted = this.taskBranch(role, options.task, options);
    else wanted = adHocBranchName(agentId, slugify(options.title ?? role.name));
    return this.freeBranch(wanted, agentId);
  }

  taskBranch(
    role: { readonly name: string; readonly kind: string },
    task: string,
    options: { readonly type?: string; readonly title?: string },
  ): string {
    let naming: ReturnType<ControllerCore["taskNaming"]>;
    try {
      naming = this.k.core.taskNaming(this.k.credential, task);
    } catch (error) {
      throw new LauncherError(
        "unknown_task",
        error instanceof Error ? error.message : String(error),
      );
    }
    const type =
      options.type ??
      naming.type ??
      (this.k.isResearcherRole(role.name, role.kind) ? "docs" : "feat");
    return workerBranchName({
      type,
      taskId: naming.taskId,
      slug: options.title ?? naming.title,
    });
  }

  /** `wanted`, or the first of `wanted-2`, `wanted-3`, ... that no branch, worktree or active agent's record holds. */
  freeBranch(wanted: string, agentId: string): string {
    for (let n = 1; n < 1000; n += 1) {
      const name = withSuffix(wanted, n);
      if (
        this.k.git.branchTip(name) === null &&
        this.k.git.worktreeByBranch(name) === undefined &&
        this.k.core.activeBranchHolder(name, agentId) === undefined
      )
        return name;
    }
    throw new LauncherError(
      "invalid_branch",
      `no free branch name starting with ${wanted}`,
    );
  }

  /**
   * Gives an assignee's branch the name of its task when it still has no commit after its base and no report;
   * otherwise the branch is kept. Never fails the assignment: the answer says what happened.
   */
  renameBranchForTask(
    agentId: string,
    task: string,
  ): Promise<BranchRenameResult> {
    return this.k.run(async () => {
      const row = this.k.core
        .agentPanes(this.k.credential)
        .find((candidate) => candidate.agentId === agentId);
      const agent = this.k.core.agentRecord(agentId);
      if (row?.branch == null || agent === undefined)
        return { branch: null, renamed: false, note: "no branch recorded" };
      const current = row.branch;
      const kept = (why: string): BranchRenameResult => ({
        branch: current,
        renamed: false,
        note: `branch kept: ${current} (${why})`,
      });
      // Only an ad-hoc chore/<agent-id>-<slug> name is replaced; a task-based name from spawn (or an older one) stays.
      const adHocPrefix = adHocBranchPrefix(agentId);
      if (!current.startsWith(adHocPrefix))
        return kept("it already has a task-based name");
      // The spawn's title lives on in the ad-hoc slug; a slug that is just the role name carries no title.
      const spawnSlug = current.slice(adHocPrefix.length);
      const title =
        spawnSlug === "" || spawnSlug === slugify(agent.roleName)
          ? undefined
          : spawnSlug;
      let wanted: string;
      try {
        wanted = this.taskBranch(
          { name: agent.roleName, kind: agent.kind },
          task,
          title === undefined ? {} : { title },
        );
      } catch (error) {
        return kept(error instanceof Error ? error.message : String(error));
      }
      if (current === wanted) return { branch: current, renamed: false };
      if (this.k.core.agentHasReports(agentId))
        return kept("the agent has reported");
      const tip = this.k.git.branchTip(current);
      if (tip === null || row.baseSha === null || tip !== row.baseSha)
        return kept("it already has commits");
      const name = this.freeBranch(wanted, agentId);
      if (!this.k.git.branchNameValid(name))
        return kept("git does not accept the new name");
      const moved = this.k.git.renameBranch(current, name);
      if (!moved.renamed) return kept(moved.stderr || "git refused the rename");
      try {
        this.k.core.renameAgentBranch(this.k.context(), {
          agentId,
          from: current,
          to: name,
        });
      } catch (error) {
        this.k.git.renameBranch(name, current);
        return kept(error instanceof Error ? error.message : String(error));
      }
      this.k.log("branch_renamed", { agentId, from: current, to: name });
      return { branch: name, renamed: true };
    });
  }

  /** A LauncherError passes unchanged; anything else names the failing step, with the adapter's own code when it has one. */
  spawnFailure(error: unknown, step: string): Error {
    if (error instanceof LauncherError) return error;
    const message = oneLine(
      error instanceof Error ? error.message : String(error),
      MAX_NOTE_LENGTH,
    );
    const code =
      error instanceof HerdrError && /^[a-z0-9_]+$/.test(error.code)
        ? error.code
        : "spawn_failed";
    return new LauncherError(code, `step ${step}: ${message}`);
  }

  /**
   * Starts a worker's agent. A pane whose shell was not ready yet may be ready a
   * moment later, so those two failures are tried again; the agent is not
   * running when they are thrown. Every other error is final.
   */
  async startWithRetry(
    agentId: string,
    input: Parameters<LauncherAdapter["startAgent"]>[0],
  ): ReturnType<LauncherAdapter["startAgent"]> {
    let earlier: unknown;
    for (let attempt = 1; ; attempt += 1) {
      try {
        return await this.k.adapter.startAgent(input);
      } catch (error) {
        // A pane the failed attempt tainted refuses the next one; the first failure is the real one.
        if (error instanceof PhaseError && earlier !== undefined) throw earlier;
        const retryable =
          (error instanceof PromptUnrecognized ||
            error instanceof ShellNotReady) &&
          this.k.adapter.paneEntry(input.paneId)?.agent === undefined;
        if (!retryable || attempt >= START_ATTEMPTS) throw error;
        earlier = error;
        this.k.log("start_retry", {
          agentId,
          attempt,
          error: oneLine(String(error), MAX_NOTE_LENGTH),
        });
        await this.k.sleep(START_RETRY_DELAY_MS);
      }
    }
  }

  /** Splits a new worker pane into the PM's tab, or says why it stays a tab. A pane that was lost in the move is an error, not a fallback. */
  async placeWorkerPane(
    paneId: string,
    worktreePath: string,
    pmPaneId: string,
  ): Promise<
    | { readonly placed: { paneId: string; workspaceId: string } }
    | { readonly note: string }
  > {
    const layoutConfig = this.k.config.layout;
    try {
      const layout = await this.k.adapter.paneLayout(pmPaneId);
      if (layout.zoomed) return { note: "the PM's tab has a zoomed pane" };
      const live = new Set(
        this.k.core
          .agentPanes(this.k.credential)
          .filter((row) => {
            const agent = this.k.core.agentRecord(row.agentId);
            return (
              row.paneId !== null &&
              agent?.state === "active" &&
              (agent.kind === "PM" ||
                !this.k.cleanupFailed.some((c) => c.agentId === row.agentId)) &&
              this.k.adapter.paneEntry(row.paneId) !== undefined
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
      const placed = await this.k.adapter.placePane({
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
      this.k.log("placement_failed", { paneId, error: String(error) });
      const reason = oneLine(
        error instanceof Error ? error.message : String(error),
        MAX_NOTE_LENGTH,
      );
      return {
        note: `the pane could not be placed${reason === "" ? "" : ` (${reason})`}`,
      };
    }
  }
}
