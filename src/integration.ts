/**
 * Integration: the controller merges reviewed reports, in the order given, onto
 * the project's HEAD without checking anything out, and keeps the result on a
 * branch until the PM or the operator settles it. A conflict is reported and
 * never resolved here.
 */
import { randomUUID } from "node:crypto";
import type {
  ControllerCore,
  IntegrationOutcome,
  IntegrationRecord,
} from "./controller/core.js";
import type { MutationContext } from "./controller/types.js";
import type { IntegrationMergeInput, MergeResult } from "./git.js";

export class IntegrationError extends Error {
  override readonly name = "IntegrationError";
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

export interface IntegrationGit {
  headCommit(): Promise<string>;
  commitExists(sha: string): Promise<boolean>;
  merge(input: IntegrationMergeInput): Promise<MergeResult>;
  branchTip(branch: string): Promise<string | null>;
  isInHead(sha: string): Promise<boolean>;
  deleteBranch(branch: string, sha: string): Promise<boolean>;
}

export interface IntegrationDeps {
  readonly core: ControllerCore;
  readonly git: IntegrationGit;
  readonly context: (credential: string) => MutationContext;
  readonly credential: string;
  readonly log: (event: string, details: Record<string, unknown>) => void;
}

/** Integrations this process is merging right now; a `running` row outside this set belongs to a cut-off run. */
const inFlight = new Set<string>();

/** The whole id keeps the name unique, so a branch with this name is never another integration's. */
function branchFor(integrationId: string): string {
  return `capstan/integration/${integrationId}`;
}

const SUBJECT_MAX = 72;
const COMMIT_TYPES = "feat|fix|refactor|docs|test|chore|style|perf|ci";
const LEADING_TYPE = new RegExp(`^(${COMMIT_TYPES})(?:\\([^)]+\\))?!?:`);

function firstLine(text: string): string {
  return (
    text
      .split(/\r?\n/)
      .find((line) => line.trim() !== "")
      ?.trim() ?? ""
  );
}

/** The squash commit message: `<type>: <title>` and one line per report. */
export function squashMessage(info: {
  readonly planTitle: string | null;
  readonly reports: readonly {
    readonly reportId: string;
    readonly agentId: string;
    readonly summary: string;
  }[];
}): { subject: string; body: string } {
  const first = firstLine(info.reports[0]?.summary ?? "");
  const type = LEADING_TYPE.exec(first)?.[1] ?? "chore";
  const title =
    (info.planTitle ?? first).replace(/\s+/g, " ").trim() ||
    "integrate reports";
  const prefix = `${type}: `;
  const room = SUBJECT_MAX - prefix.length;
  const subject =
    prefix + (title.length > room ? `${title.slice(0, room - 3)}...` : title);
  const body = info.reports
    .map(
      (r) =>
        `Report ${r.reportId} (${r.agentId}): ${firstLine(r.summary).replace(/\s+/g, " ")}`,
    )
    .join("\n");
  return { subject, body };
}

/** Merges the reports in order and returns the recorded outcome. A refusal throws before anything is created. */
export async function integrate(
  deps: IntegrationDeps,
  input: {
    readonly reportIds: readonly string[];
    readonly requestedBy: string;
  },
): Promise<IntegrationRecord> {
  await recoverIntegrations(deps, "pending");
  const baseSha = await deps.git.headCommit();
  const integrationId = randomUUID();
  const branch = branchFor(integrationId);
  const begun = deps.core.beginIntegration(deps.context(deps.credential), {
    integrationId,
    reportIds: input.reportIds,
    baseSha,
    branch,
    requestedBy: input.requestedBy,
  });
  inFlight.add(integrationId);
  try {
    const finish = (outcome: IntegrationOutcome): IntegrationRecord =>
      deps.core.finishIntegration(deps.context(deps.credential), {
        integrationId,
        outcome,
      });
    let result: MergeResult;
    try {
      const missing = await firstMissingCommit(deps, begun);
      result =
        missing === undefined
          ? await deps.git.merge({
              baseSha,
              branch,
              ...squashMessage(deps.core.integrationCommitInfo(integrationId)),
              merges: begun.reports.map((report) => ({
                reportId: report.reportId,
                sha: report.commitSha,
              })),
            })
          : {
              kind: "failed",
              reason: `the commit of report ${missing} no longer exists`,
            };
    } catch (error) {
      deps.log("integration_merge_error", {
        integrationId,
        error: String(error),
      });
      result = { kind: "failed", reason: "git could not run the merge" };
    }
    try {
      return finish(result);
    } catch (error) {
      // The branch must not outlive a record that does not say it exists, and the row must not stay running.
      if (result.kind === "merged")
        await deps.git
          .deleteBranch(branch, result.headSha)
          .catch(() => undefined);
      try {
        finish({ kind: "failed", reason: "the outcome could not be recorded" });
      } catch {
        // The next integration or the next daemon start clears the row.
      }
      throw error;
    }
  } finally {
    inFlight.delete(integrationId);
  }
}

async function firstMissingCommit(
  deps: IntegrationDeps,
  record: IntegrationRecord,
): Promise<string | undefined> {
  for (const report of record.reports)
    if (!(await deps.git.commitExists(report.commitSha)))
      return report.reportId;
  return undefined;
}

/**
 * Settles a merged integration. Confirming needs the integrated commit to be
 * part of the project's HEAD already (the PM or operator merged the branch);
 * the branch is then removed. Discarding removes the branch at once.
 */
export async function settleIntegration(
  deps: IntegrationDeps,
  input: {
    readonly integrationId: string;
    readonly outcome: "confirmed" | "discarded";
  },
): Promise<{
  readonly record: IntegrationRecord;
  readonly branchRemoved: boolean;
}> {
  const before = deps.core.integration(input.integrationId);
  if (before.state !== "merged" || before.headSha === null)
    throw new IntegrationError(
      "not_merged_state",
      `the integration is ${before.state}; only a merged one can be settled`,
    );
  if (
    input.outcome === "confirmed" &&
    !(await deps.git.isInHead(before.headSha))
  )
    throw new IntegrationError(
      "not_in_head",
      `merge branch ${before.branch} into the project's HEAD first, then confirm`,
    );
  const record = deps.core.settleIntegration(
    deps.context(deps.credential),
    input,
  );
  const branchRemoved = await deps.git
    .deleteBranch(before.branch, before.headSha)
    .catch(() => false);
  if (!branchRemoved) {
    pendingSweep.add(input.integrationId);
    deps.log("integration_branch_not_removed", {
      integrationId: input.integrationId,
      branch: before.branch,
    });
  }
  return { record, branchRemoved };
}

/** Settled integrations whose branch could not be deleted in this process; the next integration retries them, and the daemon start looks at all settled rows. */
const pendingSweep = new Set<string>();

/** Removes the branch of a settled integration whose deletion failed earlier, while it still points at the recorded commit. */
async function sweepSettledBranches(
  deps: IntegrationDeps,
  scope: "pending" | "all",
): Promise<void> {
  const rows =
    scope === "all"
      ? deps.core.settledIntegrations(deps.credential)
      : [...pendingSweep].map((id) => deps.core.integration(id));
  for (const row of rows) {
    if (row.headSha === null) continue;
    try {
      const tip = await deps.git.branchTip(row.branch);
      if (tip !== row.headSha) {
        pendingSweep.delete(row.integrationId);
        continue;
      }
      if (await deps.git.deleteBranch(row.branch, row.headSha)) {
        pendingSweep.delete(row.integrationId);
        deps.log("integration_branch_swept", {
          integrationId: row.integrationId,
        });
      }
    } catch (error) {
      deps.log("integration_branch_sweep_failed", {
        integrationId: row.integrationId,
        error: String(error),
      });
    }
  }
}

/** A `running` integration that this process is not merging was cut off: its branch is removed and it is marked failed. Branches of settled integrations that could not be deleted earlier are swept first. */
export async function recoverIntegrations(
  deps: IntegrationDeps,
  scope: "pending" | "all" = "all",
): Promise<void> {
  await sweepSettledBranches(deps, scope);
  for (const row of deps.core.runningIntegrations(deps.credential)) {
    if (inFlight.has(row.integrationId)) continue;
    try {
      const tip = await deps.git.branchTip(row.branch);
      if (tip !== null && !(await deps.git.deleteBranch(row.branch, tip)))
        deps.log("integration_branch_not_removed", {
          integrationId: row.integrationId,
          branch: row.branch,
        });
      deps.core.finishIntegration(deps.context(deps.credential), {
        integrationId: row.integrationId,
        outcome: {
          kind: "failed",
          reason: "the daemon stopped while the integration was merging",
        },
      });
      deps.log("integration_recovered", { integrationId: row.integrationId });
    } catch (error) {
      deps.log("integration_not_recovered", {
        integrationId: row.integrationId,
        error: String(error),
      });
    }
  }
}
