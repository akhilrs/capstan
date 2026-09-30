import { randomUUID } from "node:crypto";
import type { ControllerCore } from "./controller/core.js";
import type { MutationContext } from "./controller/types.js";

/**
 * A context for exactly one core mutation. It is built at the moment of the
 * call and never reused: a reused idempotency key would replay the stored
 * result of the first call instead of running the second.
 */
export function newContext(
  core: ControllerCore,
  credential: string,
): MutationContext {
  const id = randomUUID();
  return {
    credential,
    requestId: `req-${id}`,
    idempotencyKey: `idem-${id}`,
    expectedVersion: core.stateVersion,
    inputRevision: core.inputRevision,
  };
}
