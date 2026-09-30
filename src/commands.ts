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
import type { ResolutionDecision } from "./controller/messaging.js";
import {
  DEFAULT_WAIT_TIMEOUT_SECONDS,
  type CapstanConfig,
} from "./config/capstan-config.js";
import { MAX_TEXT_BYTES, isAgentName } from "./herdr/adapter.js";
import { newContext } from "./context.js";
import type { CommandResponse, ErrorCode } from "./daemon.js";

export const WAIT_POLL_MS = 250;
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
  readonly log?: (event: string, details: Record<string, unknown>) => void;
}

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
  if (error instanceof AuthenticationError)
    return fail("unauthorized", "credential not accepted");
  if (error instanceof AuthorizationError)
    return fail("forbidden", error.message);
  if (error instanceof MessageTransitionError)
    return fail("rejected", `${error.code}: ${error.message}`);
  if (error instanceof MutationConflictError)
    return fail("conflict", error.message);
  if (error instanceof TypeError)
    return fail(
      "invalid_request",
      error.message.replace(/[\p{Cc}\p{Cf}]/gu, " ").slice(0, 200),
    );
  return fail("error", "the command failed");
}

export function createCommandHandlers(deps: CommandDependencies): CommandSet {
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
        if (target !== "@pm" && !SAFE_AGENT_ID.test(target))
          return fail("invalid_request", "the recipient id is not valid");
        if (Buffer.byteLength(body, "utf8") > MAX_SEND_BODY_BYTES)
          return fail(
            "body_too_large",
            `a message body may be at most ${MAX_SEND_BODY_BYTES} bytes`,
          );
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
      return command === "wait"
        ? hostWaitSeconds(agentOf(identity)) * 1000
        : undefined;
    },
  };
}
