/**
 * The commands the daemon serves once Stage 2e is in: inbox, wait, ack, send,
 * resolve, cancel and the operator's status. Every core mutation gets its own
 * context, built at the moment of the call, so no two calls ever share an
 * idempotency key.
 */
import { messageHandlers } from "./commands/messages.js";
import { agentHandlers } from "./commands/agents.js";
import { reportHandlers } from "./commands/reports.js";
import { planHandlers } from "./commands/plans.js";
import { linkHandlers } from "./commands/links.js";
import { relayHandlers } from "./commands/relay.js";
import { findingHandlers } from "./commands/findings.js";
import { statusHandlers } from "./commands/status.js";
import { operatorHandlers } from "./commands/operator.js";
import { waitHandlers } from "./commands/wait.js";
import {
  LAUNCHER_LIMIT_MS,
  type CommandDependencies,
  type CommandHandler,
  type CommandSet,
} from "./commands/shared.js";
import { createCommandEnv } from "./commands/env.js";

export {
  FRAME_RESERVE,
  LAUNCHER_LIMIT_MS,
  MAX_SEND_BODY_BYTES,
  MAX_STATUS_CLEARS,
  MAX_STATUS_MESSAGES,
  WAIT_POLL_MS,
  abortableSleep,
  mapError,
} from "./commands/shared.js";
export type {
  CommandCall,
  CommandDependencies,
  CommandHandler,
  CommandSet,
  DriverSnapshot,
  LauncherApi,
} from "./commands/shared.js";

export function createCommandHandlers(deps: CommandDependencies): CommandSet {
  const env = createCommandEnv(deps);
  const { core, log, hostWaitSeconds, agentOf } = env;

  const handlers: Record<string, CommandHandler> = {
    ...messageHandlers(env),
    ...agentHandlers(env),
    ...reportHandlers(env),
    ...planHandlers(env),
    ...linkHandlers(env),
    ...relayHandlers(env),
    ...findingHandlers(env),
    ...statusHandlers(env),
    ...operatorHandlers(env),
    ...waitHandlers(env),
  };

  /** A worker's command other than inbox and wait (and the operator agent's op, whose answers are exact) also tells the caller when mail waits for it; the PM reads its mail by inbox and wait. */
  const withNotice =
    (handler: CommandHandler): CommandHandler =>
    async (call) => {
      const response = await handler(call);
      if (
        response === null ||
        !response.ok ||
        call.identity.agent == null ||
        call.identity.agent.kind === "PM" ||
        typeof response.result !== "object" ||
        response.result === null ||
        Array.isArray(response.result)
      )
        return response;
      try {
        const { count, oldestQueuedAt } = core.unreadSummary(call.credential);
        if (count === 0) return response;
        return {
          ok: true,
          result: { ...response.result, unread: { count, oldestQueuedAt } },
        };
      } catch (error) {
        log("unread_notice_failed", { error: String(error) });
        return response;
      }
    };
  const served: Record<string, CommandHandler> = {};
  for (const [name, handler] of Object.entries(handlers))
    served[name] =
      name === "inbox" || name === "wait" || name === "op"
        ? handler
        : withNotice(handler);

  return {
    handlers: served,
    limitMs(command, identity) {
      if (command === "wait") return hostWaitSeconds(agentOf(identity)) * 1000;
      if (
        command === "launch" ||
        command === "spawn" ||
        command === "request-review" ||
        command === "plan" ||
        command === "integrate" ||
        command === "prompt" ||
        command === "release" ||
        command === "pm-restart"
      )
        return LAUNCHER_LIMIT_MS;
      // A replacement is a release and a spawn, each with its own budget, behind whatever else the launcher runs.
      if (command === "replace") return 3 * LAUNCHER_LIMIT_MS;
      return undefined;
    },
  };
}
