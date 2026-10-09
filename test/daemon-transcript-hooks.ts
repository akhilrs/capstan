/**
 * The determinism hooks of the Node daemon the transcript exporter runs (test/daemon-transcript-export.ts). The exporter
 * starts `node --import dist/test/daemon-transcript-hooks.js dist/src/cli.js daemon` with `CAPSTAN_TRANSCRIPT_SEED` set;
 * this module then, before the daemon's own code runs:
 *
 *  - installs the seeded clock and randomness of test/kernel-parity-hooks.ts for the life of the process (the same SHA-256
 *    counter stream and the same 1 ms clock rust/crates/kernel/src/env.rs `SeededEnv` has), so every `new Date()`,
 *    `Date.now()`, `randomUUID()` and `randomBytes()` of the daemon is one reading of a deterministic stream;
 *  - keeps the daemon's periodic timers (the report relay ticks every two seconds and reads the clock) from ever firing,
 *    so the number of clock readings depends on the requests alone;
 *  - makes a timer of under a second that fires move the seeded clock by its delay (the poll of a wait that times out);
 *  - writes one `{"event":"request_start"}` line to standard output whenever the daemon starts to identify a request's
 *    credential. The exporter uses these lines as a barrier (a concurrent request is sent after the previous one started);
 *    they are not log entries and never reach a transcript.
 *
 * Nothing in src/ is edited: the controller reads time and randomness from node:crypto and `Date` directly, and the hooks
 * replace those.
 */
import { ControllerCore } from "../src/controller/core.js";
import { installHooks } from "./kernel-parity-hooks.js";

const seed = process.env.CAPSTAN_TRANSCRIPT_SEED;
if (seed === undefined || seed === "")
  throw new Error("CAPSTAN_TRANSCRIPT_SEED is not set");

const hooks = installHooks(seed);
// `run` keeps the hooks active for as long as the promise it is given is pending: forever.
void hooks.run(() => new Promise<never>(() => undefined));

const realSetInterval = globalThis.setInterval;
globalThis.setInterval = ((
  handler: (...args: unknown[]) => void,
  ms?: number,
  ...args: unknown[]
) =>
  (ms ?? 0) >= 1_000
    ? // A timer that never fires but is still a real Timeout, so `unref` and `clearInterval` work on it.
      realSetInterval(() => undefined, 2_147_483_647)
    : realSetInterval(handler, ms, ...args)) as typeof setInterval;

// A timer that fires moves the seeded clock by its delay, so a wait with a limit ends after the timers it slept (about
// the limit in real time) and not after as many readings of a clock that only counts them.
const realSetTimeout = globalThis.setTimeout;
globalThis.setTimeout = ((
  handler: (...args: unknown[]) => void,
  ms?: number,
  ...args: unknown[]
) =>
  typeof handler === "function" && (ms ?? 0) > 0 && (ms ?? 0) < 1_000
    ? realSetTimeout(
        (...inner: unknown[]) => {
          hooks.advance(ms!);
          handler(...inner);
        },
        ms,
        ...args,
      )
    : realSetTimeout(handler, ms, ...args)) as unknown as typeof setTimeout;

const identify = ControllerCore.prototype.identify;
ControllerCore.prototype.identify = function (
  this: ControllerCore,
  ...args: Parameters<ControllerCore["identify"]>
) {
  process.stdout.write('{"event":"request_start"}\n');
  return identify.apply(this, args);
};
