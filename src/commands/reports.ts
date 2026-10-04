/** report, request-review and integrate: a worker's report and the review and integration it leads to. */
import { NAME_PATTERN } from "../config/capstan-config.js";
import {
  ControllerError,
  MAX_REPORT_SUMMARY_BYTES,
  type ReportEvidence,
} from "../controller/core.js";
import { checkCommitMessage } from "../conventions.js";
import { type CommitInspection } from "../git.js";
import { GitCheckError } from "../git.js";
import { oneLineSummary } from "../reports.js";
import { type IntegrationDeps } from "../integration.js";
import {
  IntegrationError,
  integrate,
  settleIntegration,
} from "../integration.js";
import { ReviewRequestError, requestReview } from "../reviews.js";
import {
  MAX_CHECKED_COMMITS,
  SAFE_AGENT_ID,
  type CommandHandler,
  ok,
  fail,
  mapError,
  REPORT_REASON_TEXT,
  type CommandEnv,
} from "./shared.js";

export function reportHandlers(
  env: CommandEnv,
): Record<string, CommandHandler> {
  const {
    deps,
    core,
    now,
    log,
    reportLimiter,
    context,
    agentOf,
    workerManager,
    isArchitect,
    reviewIntegrator,
  } = env;
  return {
    async report(call) {
      const caller = agentOf(call.identity);
      if (
        caller === undefined ||
        caller.state !== "active" ||
        (caller.kind !== "Developer" && caller.kind !== "Verifier")
      )
        return fail("forbidden", "only a worker agent can report");
      if (call.args.length !== 2)
        return fail(
          "invalid_request",
          "report needs a commit id and a summary",
        );
      const sha = call.args[0]!.toLowerCase();
      if (!/^[0-9a-f]{40}$/.test(sha))
        return fail(
          "invalid_request",
          "the commit must be a full 40-character id (git rev-parse HEAD)",
        );
      const summary = oneLineSummary(call.args[1]!, MAX_REPORT_SUMMARY_BYTES);
      if (summary === "")
        return fail("invalid_request", "the summary must not be empty");
      if (deps.inspectCommit === undefined)
        return fail("not_configured", "reports need a git repository");
      try {
        // Every report command counts, a repeat included: a repeat still costs a read.
        if (
          !reportLimiter.allow(`${caller.agentId}:${caller.generation}`, now())
        )
          return fail(
            "rejected",
            "rate_limited: at most 10 reports a minute; wait and report once",
          );
        const known = core.acceptedReportFor(
          caller.agentId,
          caller.generation,
          sha,
        );
        if (known !== undefined)
          // A repeat writes nothing: no row, no mutation record, no notice.
          return ok({
            reportId: known.reportId,
            state: "accepted",
            duplicate: true,
            announced: known.notifiedMessageId !== null,
          });
        const row = core
          .agentPanes(deps.controllerCredential)
          .find((candidate) => candidate.agentId === caller.agentId);
        const branch = row?.branch ?? null;
        const baseSha = row?.baseSha?.toLowerCase() ?? null;
        const inspection: CommitInspection =
          branch === null || baseSha === null
            ? {
                commitExists: false,
                branchTip: null,
                isAncestorOfTip: false,
                isAncestorOfBase: false,
              }
            : await deps.inspectCommit({ branch, baseSha, sha });
        if (inspection.committedAt != null) {
          const committed = Date.parse(inspection.committedAt);
          const unread = core
            .messagesFor(caller.agentId)
            .filter(
              (message) =>
                (message.state === "queued" ||
                  message.state === "deferred" ||
                  message.state === "sent" ||
                  message.state === "unacked") &&
                Date.parse(message.queuedAt) <= committed,
            );
          if (unread.length > 0)
            return fail(
              "rejected",
              `unread_messages: ${unread.length} message(s) to you are not acknowledged (${unread.map((message) => message.messageId).join(", ")}); run cstan inbox, act on and ack them, then report again`,
            );
        }
        if (
          deps.newCommitMessages !== undefined &&
          branch !== null &&
          baseSha !== null &&
          inspection.commitExists &&
          inspection.isAncestorOfTip &&
          !inspection.isAncestorOfBase &&
          sha !== baseSha
        ) {
          const found = await deps.newCommitMessages({
            sha,
            baseSha,
            ownBranch: branch,
            limit: MAX_CHECKED_COMMITS,
          });
          if ("tooMany" in found) {
            log("report_commit_refused", {
              agentId: caller.agentId,
              rule: "too_many_commits",
            });
            return fail(
              "rejected",
              `commit_message: more than ${MAX_CHECKED_COMMITS} new commits since the base; squash or rebase your branch down and report the new tip`,
            );
          }
          const violations = found.flatMap((commit) =>
            checkCommitMessage(commit.message, commit.parents).map((v) => ({
              commit,
              ...v,
            })),
          );
          if (violations.length > 0) {
            const shown = violations.slice(0, 5).map((v) => {
              const subject = (v.commit.message.split("\n")[0] ?? "").slice(
                0,
                80,
              );
              return `commit_message: ${v.commit.sha.slice(0, 8)} "${subject}" breaks ${v.rule}: ${v.reason}`;
            });
            const more =
              violations.length > 5
                ? ` (and ${violations.length - 5} more)`
                : "";
            for (const rule of new Set(violations.map((v) => v.rule)))
              log("report_commit_refused", { agentId: caller.agentId, rule });
            return fail(
              "rejected",
              `${shown.join("; ")}${more}. No report was recorded. Fix: for a commit you have not reported yet, reword it (git commit --amend for the tip, or git rebase -i then reword for an earlier one) and report the new full id; or put the fixed commits on a new branch (git switch -c <new-branch> <base>, git cherry-pick the work, reword) and report from there. Use Conventional Commits subjects (type(scope): description) and no Claude Co-Authored-By, Claude-Session or 'Generated with Claude Code' lines; check git log of the range before reporting.`,
            );
          }
        }
        const evidence: ReportEvidence = {
          generation: caller.generation,
          branch: branch ?? "",
          baseSha,
          ...inspection,
          checkedAt: new Date(now()).toISOString(),
        };
        const result = core.recordAgentReport(context(call.credential), {
          commitSha: sha,
          summary,
          evidence,
        });
        const record = result.record;
        if (record.state === "rejected")
          return fail(
            "rejected",
            `report_rejected: ${REPORT_REASON_TEXT[record.reason!]} (report ${record.reportId} is recorded)`,
          );
        log("report_accepted", {
          reportId: record.reportId,
          agentId: record.agentId,
          duplicate: result.duplicate,
        });
        return ok({
          reportId: record.reportId,
          state: "accepted",
          duplicate: result.duplicate,
          announced: record.notifiedMessageId !== null,
        });
      } catch (error) {
        if (error instanceof GitCheckError) {
          log("report_check_failed", { error: error.message });
          return fail(
            "error",
            "the controller could not check the commit just now; report again",
          );
        }
        if (
          error instanceof ControllerError &&
          error.message.startsWith("report limit")
        )
          return fail("rejected", `report_limit: ${error.message}`);
        return mapError(error);
      }
    },

    async "request-review"(call) {
      const caller = agentOf(call.identity);
      if (
        caller === undefined ||
        caller.state !== "active" ||
        (caller.kind !== "PM" && !isArchitect(caller))
      )
        return fail(
          "forbidden",
          "only the PM or the architect can request a review",
        );
      if (call.args.length < 1 || call.args.length > 2)
        return fail(
          "invalid_request",
          "request-review needs a report or integration id and optionally a reviewer role",
        );
      const reportId = call.args[0]!;
      if (!SAFE_AGENT_ID.test(reportId))
        return fail(
          "invalid_request",
          "the report or integration id is not valid",
        );
      const role = call.args[1];
      if (role !== undefined && !NAME_PATTERN.test(role))
        return fail("invalid_request", "the reviewer role name is not valid");
      try {
        core.assertRunNotPaused("request-review");
      } catch (error) {
        return mapError(error);
      }
      if (
        deps.launcher === undefined ||
        deps.config === undefined ||
        deps.commitExists === undefined
      )
        return fail(
          "not_configured",
          "reviews need capstan.toml, Herdr and a git repository",
        );
      log("review_requested", { requestedBy: caller.agentId, reportId });
      try {
        const { review, spawnState } = await requestReview(
          {
            core,
            launcher: deps.launcher,
            config: deps.config,
            commitExists: deps.commitExists,
            context,
            log,
          },
          {
            subjectId: reportId,
            requestedRole: role,
            pmCredential: call.credential,
          },
        );
        return ok({
          reviewId: review.reviewId,
          round: review.round,
          reviewerAgentId: review.reviewerAgentId,
          reviewerRole: review.reviewerRole,
          commit: review.commitSha,
          state: review.state,
          reviewerState: spawnState,
        });
      } catch (error) {
        if (error instanceof ReviewRequestError)
          return fail("rejected", `${error.code}: ${error.message}`);
        if (error instanceof ControllerError)
          return fail("rejected", `review_refused: ${error.message}`);
        if (error instanceof GitCheckError)
          return fail(
            "error",
            "the controller could not check the commit just now",
          );
        return mapError(error);
      }
    },

    async integrate(call) {
      const requestedBy = reviewIntegrator(call.identity);
      if (requestedBy === undefined)
        return fail(
          "forbidden",
          "only the PM, the operator or the architect may integrate reports",
        );
      if (
        call.args[0] === "confirm" &&
        workerManager(call.identity) === undefined
      )
        return fail(
          "forbidden",
          "only the PM or the operator may confirm an integration",
        );
      if (call.args.length < 1)
        return fail(
          "invalid_request",
          "integrate needs report ids, or confirm|discard and an integration id",
        );
      try {
        core.assertRunNotPaused("integrate");
      } catch (error) {
        return mapError(error);
      }
      if (deps.integrationGit === undefined)
        return fail("not_configured", "integration needs a git repository");
      const integrationDeps: IntegrationDeps = {
        core,
        git: deps.integrationGit,
        context,
        credential: deps.controllerCredential,
        log,
      };
      const [first, second] = call.args;
      try {
        if (first === "confirm" || first === "discard") {
          if (call.args.length !== 2 || !SAFE_AGENT_ID.test(second!))
            return fail(
              "invalid_request",
              `integrate ${first} needs one integration id`,
            );
          log("integration_settle_requested", {
            requestedBy,
            integrationId: second,
            outcome: first,
          });
          const settled = await settleIntegration(integrationDeps, {
            integrationId: second!,
            outcome: first === "confirm" ? "confirmed" : "discarded",
          });
          return ok({
            integrationId: settled.record.integrationId,
            state: settled.record.state,
            branch: settled.record.branch,
            branchRemoved: settled.branchRemoved,
          });
        }
        if (call.args.some((id) => !SAFE_AGENT_ID.test(id)))
          return fail("invalid_request", "a report id is not valid");
        log("integration_requested", { requestedBy, reportIds: call.args });
        const record = await integrate(integrationDeps, {
          reportIds: call.args,
          requestedBy,
        });
        return ok({
          integrationId: record.integrationId,
          state: record.state,
          base: record.baseSha,
          branch: record.state === "merged" ? record.branch : null,
          head: record.headSha,
          reports: record.reports.map((r) => r.reportId),
          conflict:
            record.state === "conflicted"
              ? {
                  reportId: record.conflictReportId,
                  files: record.conflictFiles,
                }
              : null,
          failure: record.failureReason,
        });
      } catch (error) {
        if (error instanceof IntegrationError)
          return fail("rejected", `${error.code}: ${error.message}`);
        if (error instanceof ControllerError)
          return fail("rejected", `integration_refused: ${error.message}`);
        if (error instanceof GitCheckError)
          return fail("error", "the controller could not run git just now");
        return mapError(error);
      }
    },
  };
}
