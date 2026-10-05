/** wait: blocks an agent until mail arrives. */
import { DEFAULT_WAIT_TIMEOUT_SECONDS } from "../config/capstan-config.js";
import {
  WAIT_POLL_MS,
  type CommandHandler,
  ok,
  fail,
  mapError,
  type CommandEnv,
} from "./shared.js";

export function waitHandlers(env: CommandEnv): Record<string, CommandHandler> {
  const {
    core,
    now,
    sleep,
    log,
    waits,
    context,
    delivered,
    pullFor,
    agentOf,
    abortReply,
    endWait,
  } = env;
  return {
    async wait(call) {
      const agent = agentOf(call.identity);
      if (agent === undefined) return fail("forbidden", "only an agent waits");
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
        let pulled = pullFor(agent, call.credential);
        if (!pulled && now() < deadline) {
          const { waitId } = core.beginWait(context(call.credential));
          try {
            while (!pulled && !local.signal.aborted && now() < deadline) {
              await sleep(
                Math.max(1, Math.min(WAIT_POLL_MS, deadline - now())),
                local.signal,
              );
              if (local.signal.aborted) break;
              pulled = pullFor(agent, call.credential);
            }
          } finally {
            endWait(call.credential, waitId);
          }
        }
        if (local.signal.aborted) return abortReply(local.signal);
        if (!pulled) pulled = pullFor(agent, call.credential);
        const messages = delivered(call.credential);
        return ok({
          messages,
          timedOut: !pulled,
          count: messages.length,
          actionNeededCount: messages.filter((m) => m.actionNeeded).length,
        });
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
}
