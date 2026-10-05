/** inbox, ack, send, resolve and cancel: the message commands. */
import { type AgentRecord } from "../controller/types.js";
import {
  RESOLUTION_DECISIONS,
  type ResolutionDecision,
} from "../controller/messaging.js";
import { type ResolvedOperator } from "../config/capstan-config.js";
import { isAgentName } from "../herdr/adapter.js";
import {
  imitatesFrame,
  SAFE_AGENT_ID,
  MAX_SEND_BODY_BYTES,
  type CommandHandler,
  ok,
  fail,
  mapError,
  type CommandEnv,
} from "./shared.js";

export function messageHandlers(
  env: CommandEnv,
): Record<string, CommandHandler> {
  const {
    deps,
    core,
    context,
    describe,
    delivered,
    pullFor,
    agentOf,
    isArchitect,
  } = env;
  return {
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
        const agent = agentOf(identity);
        if (call.args.length === 1 && call.args[0] === "--hook") {
          // Read-only: the hook runs after every tool call and never pulls or prints a body.
          if (agent === undefined)
            return fail("forbidden", "the caller is not an agent");
          const { count, oldestQueuedAt, messageIds, actionNeeded } =
            core.unreadSummary(call.credential);
          return ok({ count, oldestQueuedAt, messageIds, actionNeeded });
        }
        if (call.args.length !== 0)
          return fail("invalid_request", "inbox takes no arguments");
        if (agent === undefined)
          return fail("forbidden", "the caller is not an agent");
        pullFor(agent, call.credential);
        const messages = delivered(call.credential);
        return ok({
          messages,
          count: messages.length,
          actionNeededCount: messages.filter((m) => m.actionNeeded).length,
        });
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
        const actionNeeded = call.args[0] === "--action";
        const args = actionNeeded ? call.args.slice(1) : call.args;
        if (args.length !== 2)
          return fail(
            "invalid_request",
            "send needs a recipient and a text, after an optional --action",
          );
        const [target, body] = args as [string, string];
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
        const operatorConfig: ResolvedOperator | undefined =
          deps.config?.operator;
        if (
          caller !== undefined &&
          caller.kind !== "PM" &&
          operatorConfig?.enabled === true &&
          recipient.kind === "Developer" &&
          recipient.roleName === operatorConfig.role
        )
          return fail(
            "recipient_not_allowed",
            "only the PM sends to the operator agent; the controller tells it what it needs",
          );
        if (caller !== undefined) {
          if (caller.agentId === recipient.agentId)
            return fail("self_send", "an agent cannot send to itself");
          if (
            caller.kind !== "PM" &&
            recipient.kind !== "PM" &&
            !(caller.kind === "Developer" && isArchitect(recipient)) &&
            !(isArchitect(caller) && recipient.kind === "Developer")
          )
            return fail(
              "recipient_not_allowed",
              "an agent other than the PM may send only to the PM, a developer to the architect, or the architect to a developer",
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
          actionNeeded,
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
  };
}
