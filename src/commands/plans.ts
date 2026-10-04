/** plan: packages, tasks and their sync with Nexora. */
import { isAgentName } from "../herdr/adapter.js";
import { ControllerError, MAX_REVIEW_ROUNDS } from "../controller/core.js";
import { GitCheckError } from "../git.js";
import { parsePlanBody } from "../plans.js";
import {
  ReviewRequestError,
  releaseReviewerLater,
  requestReview,
} from "../reviews.js";
import {
  SAFE_AGENT_ID,
  type CommandHandler,
  ok,
  fail,
  mapError,
  type CommandEnv,
} from "./shared.js";

export function planHandlers(env: CommandEnv): Record<string, CommandHandler> {
  const { deps, core, log, renameBranch, context, agentOf, workerManager } =
    env;
  return {
    async plan(call) {
      const [sub, ...rest] = call.args;
      if (
        sub !== "open" &&
        sub !== "submit" &&
        sub !== "show" &&
        sub !== "assign" &&
        sub !== "signoff" &&
        sub !== "cancel"
      )
        return fail(
          "invalid_request",
          "plan needs open, submit, show, assign, signoff or cancel",
        );
      const config = deps.config;
      if (config === undefined || !config.architect.enabled)
        return fail(
          "not_configured",
          "plans need [architect] enabled = true in capstan.toml",
        );
      try {
        if (sub === "open") {
          const requestedBy = workerManager(call.identity);
          if (requestedBy === undefined)
            return fail(
              "forbidden",
              "only the PM or the operator may open a plan",
            );
          const [tierText, title, supersedes, ...extra] = rest;
          if (
            (tierText !== "normal" && tierText !== "high-risk") ||
            title === undefined ||
            extra.length > 0
          )
            return fail(
              "invalid_request",
              "plan open needs normal|high-risk, a title and optionally a superseded plan id",
            );
          if (supersedes !== undefined && !SAFE_AGENT_ID.test(supersedes))
            return fail(
              "invalid_request",
              "the superseded plan id is not valid",
            );
          log("plan_open_requested", { requestedBy, tier: tierText });
          const plan = core.openPlan(context(call.credential), {
            tier: tierText === "normal" ? "normal" : "high_risk",
            title,
            ...(supersedes === undefined
              ? {}
              : { supersedesPlanId: supersedes }),
          });
          return ok({
            planId: plan.planId,
            tier: plan.tier,
            state: plan.state,
          });
        }
        if (sub === "cancel") {
          if (call.identity.role !== "operator")
            return fail("forbidden", "only the operator may cancel a plan");
          const [planId, packageId, ...extra] = rest;
          if (
            planId === undefined ||
            extra.length > 0 ||
            ![planId, packageId ?? planId].every((id) => SAFE_AGENT_ID.test(id))
          )
            return fail(
              "invalid_request",
              "plan cancel needs a plan id and optionally a package id",
            );
          log("plan_cancel_requested", { planId, packageId });
          const cancelled = core.cancelPlan(context(call.credential), {
            planId,
            ...(packageId === undefined ? {} : { packageId }),
          });
          if (cancelled.reviewerAgentId !== null && deps.launcher !== undefined)
            releaseReviewerLater(
              { launcher: deps.launcher, log },
              {
                reviewId: cancelled.reviewId!,
                reviewerAgentId: cancelled.reviewerAgentId,
              },
            );
          return ok({
            planId: cancelled.planId,
            packageId: cancelled.packageId,
            cancelledPackages: cancelled.cancelledPackages,
            notified: cancelled.notified,
          });
        }
        if (sub === "assign") {
          const requestedBy = workerManager(call.identity);
          if (requestedBy === undefined)
            return fail(
              "forbidden",
              "only the PM or the operator may assign a package",
            );
          const [planId, packageId, agentId, ...tail] = rest;
          const withEarly = tail[0] === "--early";
          const early = withEarly ? tail[1] : undefined;
          const extra = withEarly ? tail.slice(2) : tail;
          if (
            planId === undefined ||
            packageId === undefined ||
            agentId === undefined ||
            extra.length > 0 ||
            (withEarly && early === undefined) ||
            ![planId, packageId, agentId].every((id) => SAFE_AGENT_ID.test(id))
          )
            return fail(
              "invalid_request",
              'plan assign needs a plan id, a package id and an agent id, and takes --early only with a reason: --early "<reason>"',
            );
          if (!isAgentName(agentId))
            return fail(
              "recipient_not_deliverable",
              "this agent id cannot be used as a Herdr agent name",
            );
          log("plan_assign_requested", {
            requestedBy,
            planId,
            packageId,
            agentId,
          });
          const assigned = core.assignPackage(context(call.credential), {
            planId,
            packageId,
            agentId,
            ...(early === undefined ? {} : { early }),
          });
          const named = await renameBranch(agentId, `${planId}/${packageId}`);
          return ok({
            planId,
            packageId: assigned.packageId,
            agentId,
            messageId: assigned.assignmentMessageId,
            early: assigned.unmet.length > 0,
            unmet: assigned.unmet,
            ...named,
          });
        }
        if (sub === "signoff") {
          const caller = agentOf(call.identity);
          if (
            caller?.kind !== "Developer" ||
            caller.state !== "active" ||
            caller.roleName !== config.architect.role
          )
            return fail(
              "forbidden",
              "not_architect: only the designated architect may sign off a plan",
            );
          const [planId, integrationId, summary, ...extra] = rest;
          if (
            planId === undefined ||
            integrationId === undefined ||
            summary === undefined ||
            extra.length > 0 ||
            ![planId, integrationId].every((id) => SAFE_AGENT_ID.test(id))
          )
            return fail(
              "invalid_request",
              "plan signoff needs a plan id, an integration id and a summary",
            );
          log("plan_signoff_requested", {
            requestedBy: caller.agentId,
            planId,
            integrationId,
          });
          const signed = core.recordSignoff(context(call.credential), {
            planId,
            integrationId,
            summary,
          });
          return ok({
            planId,
            integrationId: signed.integrationId,
            signedAt: signed.createdAt,
          });
        }
        if (sub === "submit") {
          const caller = agentOf(call.identity);
          if (
            caller?.kind !== "Developer" ||
            caller.state !== "active" ||
            caller.roleName !== config.architect.role
          )
            return fail(
              "forbidden",
              "not_architect: only the designated architect may submit a plan",
            );
          if (rest.length !== 2 || !SAFE_AGENT_ID.test(rest[0]!))
            return fail(
              "invalid_request",
              "plan submit needs a plan id and the plan JSON",
            );
          const [planId, bodyText] = rest as [string, string];
          const existing = core.planRecord(call.credential, planId);
          if (existing === undefined)
            return fail("rejected", `unknown_plan: no plan ${planId}`);
          const { plan } = existing;
          if (plan.state !== "draft")
            return fail(
              "rejected",
              `plan_not_open: plan ${planId} is ${plan.state}, not a draft`,
            );
          const needsReview =
            (plan.tier === "high_risk" &&
              config.architect.planReview !== "never") ||
            config.architect.planReview === "always";
          if (
            needsReview &&
            core.planReviewRounds(call.credential, planId) >= MAX_REVIEW_ROUNDS
          )
            return fail(
              "rejected",
              `review_limit: plan ${planId} used ${MAX_REVIEW_ROUNDS} review rounds; the PM has been told`,
            );
          if (
            needsReview &&
            (deps.launcher === undefined || deps.commitExists === undefined)
          )
            return fail(
              "not_configured",
              "a plan review needs Herdr and a git repository",
            );
          const parsed = parsePlanBody(bodyText, {
            maxPackages: config.architect.maxPackages,
          });
          if (!parsed.ok)
            return fail("rejected", `invalid_plan: ${parsed.reason}`);
          if (deps.integrationGit === undefined)
            return fail("not_configured", "plans need a git repository");
          const baseSha = await deps.integrationGit.headCommit();
          log("plan_submit_requested", {
            requestedBy: caller.agentId,
            planId,
          });
          const stored = core.submitPlan(context(call.credential), {
            planId,
            bodyJson: JSON.stringify(parsed.plan),
            baseSha,
            review: needsReview,
          });
          if (!needsReview)
            return ok({
              planId: stored.planId,
              revision: stored.currentRevision,
              state: stored.state,
            });
          try {
            const { review, spawnState } = await requestReview(
              {
                core,
                launcher: deps.launcher!,
                config,
                commitExists: deps.commitExists!,
                context,
                log,
              },
              {
                subjectId: planId,
                requestedRole: config.architect.reviewerRole ?? undefined,
                pmCredential: call.credential,
              },
            );
            return ok({
              planId: stored.planId,
              revision: stored.currentRevision,
              state: "in_review",
              reviewId: review.reviewId,
              reviewerAgentId: review.reviewerAgentId,
              reviewerState: spawnState,
            });
          } catch (error) {
            const reason =
              error instanceof ReviewRequestError ||
              error instanceof ControllerError
                ? error.message
                : "the reviewer could not be started";
            try {
              core.abandonPlanReview(context(call.credential), {
                planId,
                reason,
              });
            } catch (cleanupError) {
              log("plan_review_abandon_failed", {
                planId,
                error: String(cleanupError),
              });
            }
            if (error instanceof ReviewRequestError)
              return fail("rejected", `${error.code}: ${error.message}`);
            if (error instanceof ControllerError)
              return fail("rejected", `review_refused: ${error.message}`);
            throw error;
          }
        }
        if (
          rest.length > 1 ||
          (rest[0] !== undefined && !SAFE_AGENT_ID.test(rest[0]))
        )
          return fail("invalid_request", "plan show takes an optional plan id");
        const links = core.externalLinks(call.credential);
        const linkFields = (
          refKind: "plan" | "package",
          refId: string,
        ): Record<string, unknown> => {
          const link = links.find(
            (l) => l.refKind === refKind && l.refId === refId,
          );
          return link === undefined
            ? {}
            : {
                externalId: link.externalId,
                syncedState: link.syncedState,
                wanted: link.wanted,
                drift: link.drift,
              };
        };
        if (rest[0] === undefined)
          return ok({
            plans: core.listPlans(call.credential).map((p) => ({
              planId: p.planId,
              tier: p.tier,
              state: p.state,
              title: p.title,
              ...linkFields("plan", p.planId),
            })),
          });
        const detail = core.planRecord(call.credential, rest[0]);
        if (detail === undefined)
          return fail("rejected", `unknown_plan: no plan ${rest[0]}`);
        return ok({
          plan: detail.plan,
          revision:
            detail.revision === null
              ? null
              : {
                  revision: detail.revision.revision,
                  baseSha: detail.revision.baseSha,
                  authorAgentId: detail.revision.authorAgentId,
                  createdAt: detail.revision.createdAt,
                  body: JSON.parse(detail.revision.bodyJson) as unknown,
                },
          ...(links.some((l) => l.refKind === "plan" && l.refId === rest[0])
            ? { nexora: linkFields("plan", rest[0]) }
            : {}),
          packages: detail.packages.map((pkg) => ({
            ...pkg,
            ...linkFields("package", `${rest[0]}/${pkg.packageId}`),
          })),
          signoffs: detail.signoffs,
        });
      } catch (error) {
        if (error instanceof ControllerError)
          return fail("rejected", `plan_refused: ${error.message}`);
        if (error instanceof GitCheckError)
          return fail("error", "the controller could not run git just now");
        return mapError(error);
      }
    },
  };
}
