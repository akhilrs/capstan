/** Poll loop with one request in flight, hash-based change detection, pause, forced polls and back-off. */
import { createHash } from "node:crypto";

export const BACKOFF_MS: readonly number[] = [1000, 2000, 5000];

/** Delay before the next try after `failures` failures in a row; the poll interval when there are none. */
export function nextDelayMs(failures: number, intervalMs: number): number {
  if (failures <= 0) return intervalMs;
  return BACKOFF_MS[Math.min(failures, BACKOFF_MS.length) - 1]!;
}

export const MIN_INTERVAL_SECONDS = 1;
export const MAX_INTERVAL_SECONDS = 60;

/** The poll interval one `-` (`faster`, fewer seconds) or `+` (more seconds) key press away: steps of 1 s up to 10 s, then 5 s, kept within 1 to 60. */
export function stepInterval(seconds: number, faster: boolean): number {
  const step = (faster ? seconds <= 10 : seconds < 10) ? 1 : 5;
  const next = faster ? seconds - step : seconds + step;
  return Math.min(MAX_INTERVAL_SECONDS, Math.max(MIN_INTERVAL_SECONDS, next));
}

export function statusHash(status: unknown): string {
  return createHash("sha256").update(JSON.stringify(status)).digest("hex");
}

export interface PollerDeps {
  fetch(): Promise<Record<string, unknown>>;
  /** Resolves after `ms`, or early when `signal` aborts. */
  sleep(ms: number, signal: AbortSignal): Promise<void>;
  readonly intervalMs: number;
  onStatus(status: Record<string, unknown>, changed: boolean): void;
  onError(error: unknown): void;
}

export interface Poller {
  /** Runs until `stop()`. */
  run(): Promise<void>;
  stop(): void;
  /** Polls now, even when paused. */
  pollNow(): void;
  setPaused(paused: boolean): void;
  /** Changes the poll interval; the sleep in progress restarts with it. */
  setIntervalMs(intervalMs: number): void;
}

export function createPoller(deps: PollerDeps): Poller {
  let stopped = false;
  let paused = false;
  let forced = false;
  let failures = 0;
  let hash: string | undefined;
  let wake: AbortController | undefined;
  let intervalMs = deps.intervalMs;
  let restart = false;

  const pollOnce = async (): Promise<void> => {
    try {
      const status = await deps.fetch();
      failures = 0;
      const next = statusHash(status);
      deps.onStatus(status, next !== hash);
      hash = next;
    } catch (error) {
      failures += 1;
      deps.onError(error);
    }
  };

  return {
    async run() {
      while (!stopped) {
        if (!paused || forced) {
          forced = false;
          await pollOnce();
        }
        if (stopped) break;
        do {
          restart = false;
          wake = new AbortController();
          await deps.sleep(nextDelayMs(failures, intervalMs), wake.signal);
          wake = undefined;
        } while (restart && !stopped && !forced);
      }
    },
    stop() {
      stopped = true;
      wake?.abort();
    },
    pollNow() {
      forced = true;
      wake?.abort();
    },
    setPaused(value) {
      paused = value;
    },
    setIntervalMs(value) {
      intervalMs = value;
      restart = true;
      wake?.abort();
    },
  };
}

/** A sleep that a signal can cut short. */
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
