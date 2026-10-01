/**
 * The commands the daemon serves once Stage 2e is in: inbox, wait, ack, send,
 * resolve, cancel and the operator's status. Every core mutation gets its own
 * context, built at the moment of the call, so no two calls ever share an
 * idempotency key.
 */
import { AuthenticationError, AuthorizationError } from "./controller/auth.js";
import {
  MessageTransitionError,
  MutationConflictError,
  type ControllerCore,
} from "./controller/core.js";
import type {
  AgentRecord,
  Identity,
  MessageRecord,
  MutationContext,
} from "./controller/types.js";
import {
  RESOLUTION_DECISIONS,
  type ResolutionDecision,
} from "./controller/messaging.js";
import {
  DEFAULT_WAIT_TIMEOUT_SECONDS,
  NAME_PATTERN,
  type CapstanConfig,
} from "./config/capstan-config.js";
import { MAX_TEXT_BYTES, isAgentName } from "./herdr/adapter.js";
import { newContext } from "./context.js";
import {
  ControllerError,
  MAX_REPORT_SUMMARY_BYTES,
  type ReportEvidence,
  type ReportReason,
} from "./controller/core.js";
import type { CommitInspection } from "./git.js";
import { GitCheckError } from "./git.js";
import { ReportRateLimiter, oneLineSummary } from "./reports.js";
import type { IntegrationDeps } from "./integration.js";
import {
  IntegrationError,
  integrate,
  settleIntegration,
} from "./integration.js";
import {
  ReviewRequestError,
  releaseReviewerLater,
  requestReview,
  reviewText,
} from "./reviews.js";
import { LauncherError } from "./launcher.js";
import type { CommandResponse, ErrorCode } from "./daemon.js";

export const WAIT_POLL_MS = 250;
const VALIDATION_MESSAGE = /^(?:[a-z][^\n]*\b(?:must|needs)\b|unknown\b)/;
// A line that reads like the driver's frame or like a message header in
// `cstan inbox` output. It is tested on a normalized copy (NFKC, lower case,
// no combining marks, joiners or blank fillers) and only horizontal space may
// precede the text, so the scan is linear in the body length.
const IGNORABLE = /[\p{M}\u200c\u200d\u2800\u115f\u1160\u3164\uffa0]/gu;
// Cyrillic and Greek letters that look like the Latin letters of the frame text.
const CONFUSABLES: Readonly<Record<string, string>> = {
  а: "a",
  с: "c",
  е: "e",
  і: "i",
  о: "o",
  р: "p",
  ѕ: "s",
  т: "t",
  ԁ: "d",
  ɡ: "g",
  һ: "h",
  к: "k",
  м: "m",
  ո: "n",
  ɑ: "a",
  α: "a",
  ε: "e",
  ι: "i",
  ο: "o",
  ρ: "p",
  τ: "t",
  κ: "k",
  μ: "m",
  ν: "v",
  ѡ: "w",
  у: "y",
  ɴ: "n",
  г: "r",
};
const CONFUSABLE = new RegExp(`[${Object.keys(CONFUSABLES).join("")}]`, "gu");
const FRAME_LOOKALIKE =
  /^[ \t]*(?:\[capstan message |acknowledge with: cstan ack |message \S+ \[[a-z_]+\] from )/m;

function imitatesFrame(body: string): boolean {
  return FRAME_LOOKALIKE.test(
    body
      .normalize("NFKC")
      .toLowerCase()
      .replace(IGNORABLE, "")
      .replace(CONFUSABLE, (letter) => CONFUSABLES[letter] ?? letter),
  );
}
const SAFE_AGENT_ID = /^[A-Za-z0-9._:-]{1,128}$/;
export const MAX_STATUS_MESSAGES = 200;
export const MAX_STATUS_CLEARS = 50;
/** Room the driver's frame header and footer need inside the adapter's text limit. */
export const FRAME_RESERVE = 1024;
export const MAX_SEND_BODY_BYTES = MAX_TEXT_BYTES - FRAME_RESERVE;

export interface CommandCall {
  readonly credential: string;
  readonly identity: Identity;
  readonly args: readonly string[];
  /** Aborts with reason `closed` (the client went away) or `shutdown`. */
  readonly signal: AbortSignal;
  /** How long this command may run; set for `wait` only. */
  readonly limitMs?: number;
}

/** A reply, or null when nothing may be written because the client is gone. */
export type CommandHandler = (
  call: CommandCall,
) => Promise<CommandResponse | null> | CommandResponse | null;

export interface DriverSnapshot {
  readonly stalledAgentIds: readonly string[];
  readonly stuck: readonly {
    readonly messageId: string;
    readonly reason: string;
  }[];
}

export interface CommandDependencies {
  readonly core: ControllerCore;
  readonly config?: CapstanConfig;
  /** The project credential; used only for cleanup that the caller's own credential cannot do. */
  readonly controllerCredential: string;
  readonly now?: () => number;
  readonly sleep?: (ms: number, signal: AbortSignal) => Promise<void>;
  readonly driverSnapshot?: () => DriverSnapshot;
  readonly launcher?: LauncherApi;
  /** Whether a commit exists in the project repository; the daemon passes the real check. */
  readonly commitExists?: (sha: string) => Promise<boolean>;
  /** Looks a reported commit up in git; the daemon passes the real one. */
  readonly inspectCommit?: (input: {
    readonly branch: string;
    readonly baseSha: string | null;
    readonly sha: string;
  }) => Promise<CommitInspection>;
  /** The git operations integration needs; the daemon passes the real ones. */
  readonly integrationGit?: IntegrationDeps["git"];
  readonly log?: (event: string, details: Record<string, unknown>) => void;
}

/** What the commands need from the launcher. */
export interface LauncherApi {
  launchPm(): Promise<unknown>;
  restartPm(): Promise<unknown>;
  spawn(
    roleName: string,
    options?: { baseSha?: string },
  ): Promise<{ readonly state: string; readonly agentId: string }>;
  release(agentId: string): Promise<unknown>;
  status(): unknown;
}

/** The time a launcher operation may run; it covers the operation budgets and one queued operation. */
export const LAUNCHER_LIMIT_MS = 600_000;

export interface CommandSet {
  readonly handlers: Readonly<Record<string, CommandHandler>>;
  /** The time a command may run, or undefined for the daemon's default limit. */
  limitMs(command: string, identity: Identity): number | undefined;
}

export function abortableSleep(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    if (signal.aborted) return resolve();
    const timer = setTimeout(done, ms);
    function done(): void {
      clearTimeout(timer);
      signal.removeEventListener("abort", done);
      resolve();
    }
    signal.addEventListener("abort", done, { once: true });
  });
}

function ok(result: unknown): CommandResponse {
  return { ok: true, result };
}

function fail(code: ErrorCode, message: string): CommandResponse {
  return { ok: false, code, message };
}

export function mapError(error: unknown): CommandResponse {
  if (error instanceof LauncherError)
    return fail(
      error.code === "not_configured" ? "not_configured" : "rejected",
      `${error.code}: ${error.message}`,
    );
  if (error instanceof AuthenticationError)
    return fail("unauthorized", "credential not accepted");
  if (error instanceof AuthorizationError)
    return fail("forbidden", error.message);
  if (error instanceof MessageTransitionError)
    return fail("rejected", `${error.code}: ${error.message}`);
  if (error instanceof MutationConflictError)
    return fail("conflict", error.message);
  // The core's validators throw TypeErrors whose messages say what the input
  // must be; any other TypeError is a bug and is not shown to the client.
  if (
    error instanceof TypeError &&
    VALIDATION_MESSAGE.test(error.message) &&
    !/Received /.test(error.message)
  )
    return fail(
      "invalid_request",
      Array.from(error.message.replace(/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/gu, " "))
        .slice(0, 200)
        .join(""),
    );
  return fail("error", "the command failed");
}

const MAX_STATUS_REPORTS = 20;

const REPORT_REASON_TEXT: Readonly<Record<ReportReason, string>> = {
  agent_changed:
    "your agent or its branch changed while the controller checked; report again",
  no_branch: "no branch is recorded for you",
  no_base: "no base commit is recorded for you",
  commit_missing: "that commit does not exist in the repository",
  not_new_on_branch:
    "that commit is your base commit or older; report a commit you made on your branch",
  not_on_branch: "that commit is not on your branch",
};

export function createCommandHandlers(deps: CommandDependencies): CommandSet {
  const reportLimiter = new ReportRateLimiter();
  const { core } = deps;
  const now = deps.now ?? Date.now;
  const sleep = deps.sleep ?? abortableSleep;
  const log = deps.log ?? (() => undefined);

  const context = (credential: string): MutationContext =>
    newContext(core, credential);

  const describe = (message: MessageRecord) => {
    const sender = core.senderOf(message.senderActorId);
    return {
      messageId: message.messageId,
      sequence: message.sequence,
      from: sender.role,
      fromAgentId: sender.agentId ?? sender.role,
      body: message.body,
      state: message.state,
      sentAt: message.sentAt,
    };
  };

  /** What an agent sees: only messages that were delivered to it and still need an ack. */
  const delivered = (credential: string) =>
    core
      .agentInbox(credential)
      .filter((row) => row.state === "sent" || row.state === "unacked")
      .map(describe);

  const pull = (credential: string): boolean =>
    core.pullMessage(context(credential)).message !== null;

  const agentOf = (identity: Identity): AgentRecord | undefined =>
    identity.agent ?? undefined;

  /** Who is asking to start or end workers: the operator, or a PM agent that is still active. */
  const workerManager = (identity: Identity): string | undefined => {
    if (identity.role === "operator") return "operator";
    const agent = agentOf(identity);
    return agent?.kind === "PM" && agent.state === "active"
      ? agent.agentId
      : undefined;
  };

  const waits = new Map<
    string,
    { readonly controller: AbortController; readonly done: Promise<void> }
  >();

  const hostWaitSeconds = (agent: AgentRecord | undefined): number => {
    if (agent === undefined || deps.config === undefined)
      return DEFAULT_WAIT_TIMEOUT_SECONDS;
    const role = deps.config.roles.find((r) => r.name === agent.roleName);
    const host = deps.config.hosts.find((h) => h.name === role?.host);
    return host?.waitTimeoutSeconds ?? DEFAULT_WAIT_TIMEOUT_SECONDS;
  };

  const abortReply = (signal: AbortSignal): CommandResponse | null => {
    if (signal.reason === "superseded")
      return fail("superseded", "a newer wait replaced this one");
    if (signal.reason === "shutdown")
      return fail("shutting_down", "the daemon is shutting down");
    return null;
  };

  const endWait = (credential: string, waitId: string): void => {
    try {
      core.endWait(context(credential), waitId);
      return;
    } catch (error) {
      log("wait_end_failed", { waitId, error: String(error) });
    }
    try {
      core.endWaitAsController(context(deps.controllerCredential), waitId);
    } catch (error) {
      log("wait_end_controller_failed", { waitId, error: String(error) });
    }
  };

  const handlers: Record<string, CommandHandler> = {
    inbox(call) {
      try {
        const { identity } = call;
        if (identity.role === "operator") {
          if (call.args.length !== 1)
            return fail("invalid_request", "inbox needs exactly one agent id");
          const agentId = call.args[0]!;
          if (!SAFE_AGENT_ID.test(agentId))
            return fail("invalid_request", "the agent id is not valid");
          if (core.agentRecord(agentId) === undefined)
            return fail("unknown_agent", "no agent has this id");
          return ok({
            messages: core.agentInbox(call.credential, agentId).map(describe),
          });
        }
        if (call.args.length !== 0)
          return fail("invalid_request", "inbox takes no arguments");
        const agent = agentOf(identity);
        if (agent === undefined)
          return fail("forbidden", "the caller is not an agent");
        if (agent.kind === "PM") pull(call.credential);
        return ok({ messages: delivered(call.credential) });
      } catch (error) {
        return mapError(error);
      }
    },

    ack(call) {
      try {
        if (call.args.length !== 1)
          return fail("invalid_request", "ack needs one message id");
        const record = core.ackMessage(context(call.credential), call.args[0]!);
        return ok({ messageId: record.messageId, state: record.state });
      } catch (error) {
        return mapError(error);
      }
    },

    send(call) {
      try {
        if (call.args.length !== 2)
          return fail("invalid_request", "send needs a recipient and a text");
        const [target, body] = call.args as [string, string];
        if (Buffer.byteLength(body, "utf8") > MAX_SEND_BODY_BYTES)
          return fail(
            "body_too_large",
            `a message body may be at most ${MAX_SEND_BODY_BYTES} bytes`,
          );
        if (imitatesFrame(body))
          return fail(
            "invalid_request",
            "a message body must not contain a line that looks like a Capstan message frame",
          );
        if (target !== "@pm" && !SAFE_AGENT_ID.test(target))
          return fail("invalid_request", "the recipient id is not valid");
        const caller = agentOf(call.identity);
        let recipient: AgentRecord | undefined;
        if (target === "@pm") {
          const pms = core
            .listAgents()
            .filter((a) => a.kind === "PM" && a.state === "active");
          if (pms.length > 1)
            return fail(
              "ambiguous_recipient",
              "more than one PM is active; name the agent id",
            );
          recipient = pms[0];
        } else recipient = core.agentRecord(target);
        if (recipient === undefined || recipient.state !== "active")
          return fail(
            "unknown_recipient",
            "the recipient is not an active agent",
          );
        if (caller !== undefined) {
          if (caller.agentId === recipient.agentId)
            return fail("self_send", "an agent cannot send to itself");
          if (caller.kind !== "PM" && recipient.kind !== "PM")
            return fail(
              "recipient_not_allowed",
              "an agent other than the PM may send only to the PM",
            );
        }
        if (recipient.kind !== "PM" && !isAgentName(recipient.agentId))
          return fail(
            "recipient_not_deliverable",
            "this agent id cannot be used as a Herdr agent name",
          );
        const { messageId } = core.enqueueMessage(context(call.credential), {
          recipientAgentId: recipient.agentId,
          body,
        });
        return ok({ messageId });
      } catch (error) {
        return mapError(error);
      }
    },

    resolve(call) {
      try {
        if (call.args.length < 2 || call.args.length > 3)
          return fail(
            "invalid_request",
            "resolve needs a message id, a decision and an optional note",
          );
        const [messageId, decision, note] = call.args as [
          string,
          string,
          string | undefined,
        ];
        if (!(RESOLUTION_DECISIONS as readonly string[]).includes(decision))
          return fail(
            "invalid_request",
            `the decision must be one of ${RESOLUTION_DECISIONS.join(", ")}`,
          );
        const before = core.message(messageId)?.state;
        const record = core.resolveMessage(
          context(call.credential),
          messageId,
          decision as ResolutionDecision,
          note,
        );
        const warn =
          decision === "retry" && (before === "sent" || before === "unacked");
        return ok({
          messageId: record.messageId,
          state: record.state,
          ...(warn
            ? {
                warning: "the recipient may already have received this message",
              }
            : {}),
        });
      } catch (error) {
        return mapError(error);
      }
    },

    cancel(call) {
      try {
        if (call.args.length !== 1)
          return fail("invalid_request", "cancel needs one message id");
        const record = core.resolveMessage(
          context(call.credential),
          call.args[0]!,
          "cancel",
        );
        return ok({ messageId: record.messageId, state: record.state });
      } catch (error) {
        return mapError(error);
      }
    },

    async launch(call) {
      if (deps.launcher === undefined)
        return fail(
          "not_configured",
          "launching agents needs capstan.toml and Herdr",
        );
      if (call.args.length !== 0)
        return fail("invalid_request", "launch takes no arguments");
      try {
        return ok(await deps.launcher.launchPm());
      } catch (error) {
        return mapError(error);
      }
    },

    async spawn(call) {
      const requestedBy = workerManager(call.identity);
      if (requestedBy === undefined)
        return fail(
          "forbidden",
          "only the PM or the operator may spawn workers",
        );
      if (call.args.length !== 1)
        return fail("invalid_request", "spawn needs one role name");
      if (!NAME_PATTERN.test(call.args[0]!))
        return fail("invalid_request", "the role name is not valid");
      if (deps.launcher === undefined)
        return fail(
          "not_configured",
          "spawning agents needs capstan.toml and Herdr",
        );
      log("spawn_requested", { requestedBy, role: call.args[0] });
      try {
        return ok(await deps.launcher.spawn(call.args[0]!));
      } catch (error) {
        return mapError(error);
      }
    },

    async release(call) {
      const requestedBy = workerManager(call.identity);
      if (requestedBy === undefined)
        return fail(
          "forbidden",
          "only the PM or the operator may release workers",
        );
      if (call.args.length !== 1)
        return fail("invalid_request", "release needs one agent id");
      const agentId = call.args[0]!;
      if (!SAFE_AGENT_ID.test(agentId))
        return fail("invalid_request", "the agent id is not valid");
      if (deps.launcher === undefined)
        return fail(
          "not_configured",
          "releasing agents needs capstan.toml and Herdr",
        );
      log("release_requested", { requestedBy, agentId });
      try {
        return ok(await deps.launcher.release(agentId));
      } catch (error) {
        return mapError(error);
      }
    },

    async "pm-restart"(call) {
      if (deps.launcher === undefined)
        return fail(
          "not_configured",
          "restarting the PM needs capstan.toml and Herdr",
        );
      if (call.args.length !== 0)
        return fail("invalid_request", "pm restart takes no arguments");
      try {
        return ok(await deps.launcher.restartPm());
      } catch (error) {
        return mapError(error);
      }
    },

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
      if (caller?.kind !== "PM" || caller.state !== "active")
        return fail("forbidden", "only the PM can request a review");
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
      const requestedBy = workerManager(call.identity);
      if (requestedBy === undefined)
        return fail(
          "forbidden",
          "only the PM or the operator may integrate reports",
        );
      if (call.args.length < 1)
        return fail(
          "invalid_request",
          "integrate needs report ids, or confirm|discard and an integration id",
        );
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

    status(call) {
      try {
        const result: Record<string, unknown> = {
          ...(core.statusSnapshot() as unknown as Record<string, unknown>),
          agents: core.listAgents(),
        };
        if (call.identity.role === "operator") {
          const unresolved = core.unresolvedMessages(
            call.credential,
            MAX_STATUS_MESSAGES,
          );
          const snapshot = deps.driverSnapshot?.() ?? {
            stalledAgentIds: [],
            stuck: [],
          };
          result.messages = unresolved.messages.map((m) => ({
            messageId: m.messageId,
            recipientAgentId: m.recipientAgentId,
            state: m.state,
            sequence: m.sequence,
            queuedAt: m.queuedAt,
            deferredReason: m.deferredReason,
            stateReason: m.stateReason,
            lastNotifiedAt: m.lastNotifiedAt,
          }));
          result.messagesTruncated = unresolved.truncated;
          result.stalledAgentIds = snapshot.stalledAgentIds;
          result.stuck = snapshot.stuck;
          result.inputClears = core.inputClears(
            call.credential,
            MAX_STATUS_CLEARS,
          );
          result.panes = core.agentPanes(call.credential);
          result.reviews = core
            .reviews(call.credential, MAX_STATUS_REPORTS)
            .map((r) => ({
              reviewId: r.reviewId,
              reportId: r.reportId,
              integrationId: r.integrationId,
              round: r.round,
              state: r.state,
              authorAgentId: r.authorAgentId,
              reviewerAgentId: r.reviewerAgentId,
              announced: r.notifiedMessageId !== null,
              createdAt: r.createdAt,
            }));
          result.integrations = core
            .integrations(call.credential, MAX_STATUS_REPORTS)
            .map((i) => ({
              integrationId: i.integrationId,
              state: i.state,
              branch: i.state === "merged" ? i.branch : null,
              reports: i.reports.map((r) => r.reportId),
              conflictReportId: i.conflictReportId,
              createdAt: i.createdAt,
            }));
          result.reports = core
            .agentReports(call.credential, MAX_STATUS_REPORTS)
            .map((r) => ({
              reportId: r.reportId,
              agentId: r.agentId,
              generation: r.generation,
              commitSha: r.commitSha,
              state: r.state,
              reason: r.reason,
              announced: r.notifiedMessageId !== null,
              createdAt: r.createdAt,
            }));
          if (deps.launcher !== undefined) {
            const launcherStatus = deps.launcher.status() as Record<
              string,
              unknown
            >;
            result.cleanupFailed = launcherStatus.cleanupFailed;
            result.orphanPanes = launcherStatus.orphanPanes;
          }
        }
        return ok(result);
      } catch (error) {
        return mapError(error);
      }
    },

    async wait(call) {
      const agent = agentOf(call.identity);
      if (agent === undefined || agent.kind !== "PM")
        return fail("forbidden", "only the PM waits");
      if (call.args.length !== 0)
        return fail("invalid_request", "wait takes no arguments");
      const deadline =
        now() + (call.limitMs ?? DEFAULT_WAIT_TIMEOUT_SECONDS * 1000);
      const local = new AbortController();
      const link = (): void => local.abort(call.signal.reason);
      if (call.signal.aborted) link();
      else call.signal.addEventListener("abort", link, { once: true });
      let finished!: () => void;
      const done = new Promise<void>((resolve) => {
        finished = resolve;
      });
      const entry = { controller: local, done };
      const previous = waits.get(agent.agentId);
      waits.set(agent.agentId, entry);
      previous?.controller.abort("superseded");
      try {
        if (previous !== undefined) {
          await Promise.race([
            previous.done,
            new Promise<void>((resolve) => {
              if (local.signal.aborted) resolve();
              else
                local.signal.addEventListener("abort", () => resolve(), {
                  once: true,
                });
            }),
          ]);
        }
        if (local.signal.aborted) return abortReply(local.signal);
        let pulled = pull(call.credential);
        if (!pulled && now() < deadline) {
          const { waitId } = core.beginWait(context(call.credential));
          try {
            while (!pulled && !local.signal.aborted && now() < deadline) {
              await sleep(
                Math.max(1, Math.min(WAIT_POLL_MS, deadline - now())),
                local.signal,
              );
              if (local.signal.aborted) break;
              pulled = pull(call.credential);
            }
          } finally {
            endWait(call.credential, waitId);
          }
        }
        if (local.signal.aborted) return abortReply(local.signal);
        if (!pulled) pulled = pull(call.credential);
        return ok({ messages: delivered(call.credential), timedOut: !pulled });
      } catch (error) {
        if (local.signal.aborted) return abortReply(local.signal);
        log("wait_failed", { agentId: agent.agentId, error: String(error) });
        return mapError(error);
      } finally {
        if (waits.get(agent.agentId) === entry) waits.delete(agent.agentId);
        call.signal.removeEventListener("abort", link);
        finished();
      }
    },
  };

  return {
    handlers,
    limitMs(command, identity) {
      if (command === "wait") return hostWaitSeconds(agentOf(identity)) * 1000;
      if (
        command === "launch" ||
        command === "spawn" ||
        command === "request-review" ||
        command === "integrate" ||
        command === "release" ||
        command === "pm-restart"
      )
        return LAUNCHER_LIMIT_MS;
      return undefined;
    },
  };
}
