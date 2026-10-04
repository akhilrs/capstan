/** link: external references on a task. */
import { ControllerError } from "../controller/core.js";
import {
  EXTERNAL_REF_KINDS,
  NEXORA_STATES,
  isNexoraState,
  type ExternalRefKind,
} from "../nexora.js";
import {
  SAFE_AGENT_ID,
  type CommandHandler,
  ok,
  fail,
  mapError,
  type CommandEnv,
} from "./shared.js";

export function linkHandlers(env: CommandEnv): Record<string, CommandHandler> {
  const { core, log, renameBranch, context, workerManager } = env;
  return {
    async link(call) {
      const requestedBy = workerManager(call.identity);
      if (requestedBy === undefined)
        return fail("forbidden", "only the PM or the operator may link");
      try {
        const [first, ...rest] = call.args;
        if (first === "bind") {
          const [refId, agentId, ...extra] = rest;
          if (
            refId === undefined ||
            agentId === undefined ||
            extra.length > 0 ||
            !SAFE_AGENT_ID.test(refId) ||
            !SAFE_AGENT_ID.test(agentId)
          )
            return fail(
              "invalid_request",
              "link bind needs a requirement ref id and an agent id",
            );
          log("link_bind_requested", { requestedBy, refId, agentId });
          const link = core.bindRequirement(context(call.credential), {
            refId,
            agentId,
          });
          return ok({
            refKind: link.refKind,
            refId: link.refId,
            boundAgentId: link.boundAgentId,
            boundAt: link.boundAt,
            ...(await renameBranch(agentId, refId)),
          });
        }
        const [refId, externalId, syncedState, ...extra] = rest;
        if (
          first === undefined ||
          !(EXTERNAL_REF_KINDS as readonly string[]).includes(first) ||
          refId === undefined ||
          externalId === undefined ||
          extra.length > 0
        )
          return fail(
            "invalid_request",
            "link needs requirement|plan|package, a ref id, a Nexora id and optionally a state, or link bind <requirement-ref-id> <agent-id>",
          );
        if (syncedState !== undefined && !isNexoraState(syncedState))
          return fail(
            "invalid_request",
            `the state must be one of ${NEXORA_STATES.join(", ")}`,
          );
        log("link_requested", { requestedBy, kind: first, refId });
        const link = core.linkExternal(context(call.credential), {
          refKind: first as ExternalRefKind,
          refId,
          externalId,
          ...(syncedState === undefined ? {} : { syncedState }),
        });
        return ok({
          refKind: link.refKind,
          refId: link.refId,
          externalId: link.externalId,
          syncedState: link.syncedState,
          wanted: link.wanted,
          drift: link.drift,
        });
      } catch (error) {
        if (error instanceof ControllerError)
          return fail("rejected", `link_refused: ${error.message}`);
        if (error instanceof TypeError)
          return fail("invalid_request", error.message);
        return mapError(error);
      }
    },
  };
}
