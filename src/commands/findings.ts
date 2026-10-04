/** finding and review: a supervisor's findings and a reviewer's verdicts. */
import { ControllerError } from "../controller/core.js";
import { releaseReviewerLater, reviewText } from "../reviews.js";
import { type CommandResponse } from "../daemon.js";
import {
  type CommandHandler,
  findingAnswer,
  ok,
  fail,
  mapError,
  type CommandEnv,
} from "./shared.js";

export function findingHandlers(
  env: CommandEnv,
): Record<string, CommandHandler> {
  const { deps, core, now, log, findingLimiter, context, agentOf } = env;
  return {
    finding(call) {
      const caller = agentOf(call.identity);
      if (caller?.kind !== "Supervisor" || caller.state !== "active")
        return fail(
          "forbidden",
          "only a Supervisor can raise or check a finding",
        );
      const refuse = (error: unknown): CommandResponse =>
        error instanceof ControllerError
          ? fail("rejected", `finding_refused: ${error.message}`)
          : mapError(error);
      try {
        if (call.args[0] === "check" && call.args.length !== 5) {
          if (call.args.length !== 4)
            return fail(
              "invalid_request",
              "finding check needs a finding id, resolved or unresolved, and the evidence",
            );
          const finding = core.checkFinding(context(call.credential), {
            findingId: call.args[1]!,
            result: call.args[2]!,
            evidence: call.args[3]!,
          });
          log("finding_checked", {
            findingId: finding.findingId,
            state: finding.state,
            interventions: finding.interventions,
          });
          return ok(findingAnswer(finding));
        }
        if (call.args.length !== 5)
          return fail(
            "invalid_request",
            "finding needs an agent id, a severity, the evidence, the requested correction and the done-when condition",
          );
        if (!findingLimiter.allow(caller.agentId, now()))
          return fail(
            "rejected",
            "finding_rate_limit: too many findings; wait a minute",
          );
        const finding = core.raiseFinding(context(call.credential), {
          targetAgentId: call.args[0]!,
          severity: call.args[1]!,
          evidence: call.args[2]!,
          correction: call.args[3]!,
          doneWhen: call.args[4]!,
        });
        log("finding_raised", {
          findingId: finding.findingId,
          targetAgentId: finding.targetAgentId,
          severity: finding.severity,
        });
        return ok(findingAnswer(finding));
      } catch (error) {
        return refuse(error);
      }
    },

    review(call) {
      const caller = agentOf(call.identity);
      if (caller?.kind !== "Verifier" || caller.state !== "active")
        return fail("forbidden", "only a reviewer can answer a review");
      if (call.args.length !== 2)
        return fail(
          "invalid_request",
          "review needs a verdict (pass or findings) and a text",
        );
      const verdict = call.args[0]!;
      if (verdict !== "pass" && verdict !== "findings")
        return fail("invalid_request", "the verdict must be pass or findings");
      const text = reviewText(call.args[1]!);
      if (text === "")
        return fail("invalid_request", "the review text must not be empty");
      try {
        const review = core.completeReview(context(call.credential), {
          verdict,
          text,
        });
        log("review_completed", {
          reviewId: review.reviewId,
          state: review.state,
        });
        if (deps.launcher !== undefined)
          releaseReviewerLater({ launcher: deps.launcher, log }, review);
        return ok({
          reviewId: review.reviewId,
          round: review.round,
          state: review.state,
          announced: review.notifiedMessageId !== null,
        });
      } catch (error) {
        if (error instanceof ControllerError)
          return fail("rejected", `review_refused: ${error.message}`);
        return mapError(error);
      }
    },
  };
}
