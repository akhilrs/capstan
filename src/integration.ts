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
import {
  checkCommitMessage,
  COMMIT_TYPES,
  formatSubject,
  integrationBranchName,
  withSuffix,
} from "./conventions.js";
import type { MutationContext } from "./controller/types.js";
import type {
  CoveredReport,
  CoveredReportsOptions,
  IntegrationMergeInput,
  MergeResult,
} from "./git.js";

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
  /** Left out by a git that cannot judge coverage; no report is then covered. */
  coveredReports?(
    integrationHead: string,
    reports: readonly {
      readonly reportId: string;
      readonly commitSha: string;
      readonly integrationHeads?: readonly string[];
    }[],
    options?: CoveredReportsOptions,
  ): Promise<CoveredReport[]>;
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

const SUBJECT_MAX = 72;
const TYPE_RANK = ["feat", "fix"];
const LEADING_TYPE = /^[a-z]+(?:\([^)]*\))?!?:\s*/;

function firstLine(text: string): string {
  return (
    text
      .split(/\r?\n/)
      .find((line) => line.trim() !== "")
      ?.trim() ?? ""
  );
}

type CommitInfo = ReturnType<ControllerCore["integrationCommitInfo"]>;

/** The package type that names the squash: feat beats fix beats any other; with none, the leading type of the title (or of the summary when there is no plan), else feat. */
function squashType(info: CommitInfo, source: string): string {
  const types = info.packages.flatMap((p) => (p.type === null ? [] : [p.type]));
  for (const wanted of TYPE_RANK) if (types.includes(wanted)) return wanted;
  const leading = /^([a-z]+)(?:\([^)]*\))?!?:/.exec(source)?.[1];
  return (
    types[0] ??
    (leading !== undefined &&
    (COMMIT_TYPES as readonly string[]).includes(leading)
      ? leading
      : "feat")
  );
}

/**
 * The squash commit message: a Conventional Commits subject from the plan title and package types, and one
 * line per report. A worker's free text is only the subject when there is no plan.
 */
export function squashMessage(info: CommitInfo): {
  subject: string;
  body: string;
} {
  const first = firstLine(info.reports[0]?.summary ?? "");
  const source = info.planTitle !== null ? info.planTitle : first;
  const description =
    source.replace(/\s+/g, " ").trim().replace(LEADING_TYPE, "").trim() ||
    "integrate reports";
  const scopes = new Set(
    info.packages.flatMap((p) => (p.scope ? [p.scope] : [])),
  );
  const lines = info.reports.map((r) => {
    const line = `Report ${r.reportId} (${r.agentId}): ${firstLine(r.summary).replace(/\s+/g, " ")}`;
    // A worker's free text must not carry an attribution line into the commit.
    return checkCommitMessage(`chore: x\n\n${line}`, 1).length > 0
      ? `Report ${r.reportId} (${r.agentId}): summary left out`
      : line;
  });
  const subject = formatSubject(
    {
      type: squashType(info, source),
      ...(scopes.size === 1 ? { scope: [...scopes][0]! } : {}),
      breaking: info.packages.some((p) => p.breaking),
      description,
    },
    SUBJECT_MAX,
  );
  if (info.planId !== null) lines.push("", `Refs: ${info.planId}`);
  return { subject, body: lines.join("\n") };
}

/** integration/<plan-id>-<slug>, or integration/<integration-id> without a common plan; -2, -3 ... while the name is a live branch or an earlier integration's. */
async function chooseBranch(
  deps: IntegrationDeps,
  integrationId: string,
  info: CommitInfo,
): Promise<string> {
  const base =
    info.planId === null
      ? integrationBranchName({ integrationId })
      : integrationBranchName({
          integrationId: info.planId,
          planTitle: (info.planTitle ?? "").replace(LEADING_TYPE, "").trim(),
        });
  for (let n = 1; n < 1000; n += 1) {
    const name = withSuffix(base, n);
    if (
      !deps.core.integrationBranchRecorded(name) &&
      (await deps.git.branchTip(name)) === null
    )
      return name;
  }
  return integrationBranchName({ integrationId });
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
  const branch = await chooseBranch(
    deps,
    integrationId,
    deps.core.plannedCommitInfo(input.reportIds),
  );
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
  if (input.outcome === "confirmed")
    await recordCoverage(deps, input.integrationId);
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

/** Stores the reports a confirmed integration covers without having merged them. A failure is logged and retried at the next daemon start; it never blocks the confirm. */
async function recordCoverage(
  deps: IntegrationDeps,
  integrationId: string,
): Promise<void> {
  try {
    const coveredReports = deps.git.coveredReports?.bind(deps.git);
    if (coveredReports === undefined) return;
    const candidates = deps.core.coverageCandidates(
      deps.credential,
      integrationId,
    );
    if (candidates === undefined || candidates.reports.length === 0) return;
    const covered = await coveredReports(
      candidates.headSha,
      candidates.reports,
      {
        memberCommits: candidates.memberCommits,
        onSkipped: (reportId, reason) =>
          deps.log("integration_coverage_report_skipped", {
            integrationId,
            reportId,
            reason,
          }),
      },
    );
    if (covered.length === 0) return;
    deps.core.recordCoveredReports(deps.credential, integrationId, covered);
    deps.log("integration_coverage_recorded", {
      integrationId,
      covered: covered.map((c) => c.reportId),
    });
  } catch (error) {
    deps.log("integration_coverage_failed", {
      integrationId,
      error: String(error),
    });
  }
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
  if (scope === "all")
    for (const row of deps.core.settledIntegrations(deps.credential))
      if (row.state === "confirmed")
        await recordCoverage(deps, row.integrationId);
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
