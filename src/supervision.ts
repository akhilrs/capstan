import { newContext } from "./context.js";
import type { ControllerCore } from "./controller/core.js";
import type { ResolvedSupervision } from "./config/capstan-config.js";

/** How long no worker may have been active before the controller releases its Supervisor. */
export const SUPERVISOR_IDLE_MS = 10 * 60 * 1000;
const SPAWN_RETRY_MS = 60 * 1000;

export interface SupervisionLauncher {
  spawn(roleName: string): Promise<{ readonly state: string }>;
  release(agentId: string): Promise<unknown>;
}

export interface SupervisionHandle {
  stop(): Promise<void>;
}

/**
 * Keeps one Supervisor running while workers are active and queues it a
 * routine check. It does nothing without an active PM, a configured
 * Supervisor role and `supervision.enabled`. A failed spawn is logged and
 * tried again after a minute; a Supervisor is released after ten minutes
 * without any active worker.
 */
export function startSupervision(options: {
  readonly core: ControllerCore;
  readonly launcher: SupervisionLauncher;
  readonly credential: string;
  readonly supervision: ResolvedSupervision;
  readonly supervisorRole: string | undefined;
  readonly intervalMs: number;
  readonly now?: () => number;
  readonly log: (event: string, details: Record<string, unknown>) => void;
}): SupervisionHandle {
  const now = options.now ?? Date.now;
  let running: Promise<void> | undefined;
  let lastWorkerSeen = now();
  let retryAt = 0;
  let releaseRetryAt = 0;
  let stopped = false;
  const tickOnce = async (): Promise<void> => {
    if (
      !options.supervision.enabled ||
      options.supervisorRole === undefined ||
      stopped
    )
      return;
    const agents = options.core
      .listAgents()
      .filter((agent) => agent.state === "active");
    if (agents.filter((agent) => agent.kind === "PM").length !== 1) return;
    const workers = agents.filter(
      (agent) => agent.kind === "Developer" || agent.kind === "Verifier",
    );
    const supervisors = agents.filter((agent) => agent.kind === "Supervisor");
    if (workers.length > 0) lastWorkerSeen = now();
    if (workers.length > 0 && supervisors.length === 0 && now() >= retryAt) {
      try {
        const result = await options.launcher.spawn(options.supervisorRole);
        options.log("supervisor_started", { state: result.state });
      } catch (error) {
        retryAt = now() + SPAWN_RETRY_MS;
        options.log("supervisor_spawn_failed", { error: String(error) });
      }
      return;
    }
    if (supervisors.length === 1) {
      if (
        workers.length === 0 &&
        now() - lastWorkerSeen >= SUPERVISOR_IDLE_MS &&
        now() >= releaseRetryAt
      ) {
        try {
          await options.launcher.release(supervisors[0]!.agentId);
          options.log("supervisor_released", {
            agentId: supervisors[0]!.agentId,
          });
        } catch (error) {
          releaseRetryAt = now() + SPAWN_RETRY_MS;
          options.log("supervisor_release_failed", { error: String(error) });
        }
        return;
      }
      const queued = options.core.queueSupervisionCheck(
        newContext(options.core, options.credential),
        options.supervision.checkSeconds,
      );
      if (queued.queued)
        options.log("supervision_check_queued", {
          cancelled: queued.cancelled,
        });
    }
  };
  const timer = setInterval(() => {
    if (running !== undefined || stopped) return;
    running = tickOnce()
      .catch((error: unknown) =>
        options.log("supervision_failed", { error: String(error) }),
      )
      .finally(() => {
        running = undefined;
      });
  }, options.intervalMs);
  timer.unref();
  return {
    async stop() {
      stopped = true;
      clearInterval(timer);
      await running;
    },
  };
}
