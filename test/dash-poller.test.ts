import assert from "node:assert/strict";
import { test } from "node:test";
import { createPoller, nextDelayMs, statusHash } from "../src/dash/poller.js";

test("the delay follows the interval, then backs off 1 s, 2 s, 5 s and stays there", () => {
  assert.equal(nextDelayMs(0, 2000), 2000);
  assert.deepEqual(
    [1, 2, 3, 4, 10].map((n) => nextDelayMs(n, 2000)),
    [1000, 2000, 5000, 5000, 5000],
  );
});

test("the hash is stable for equal statuses and differs when anything changes", () => {
  assert.equal(statusHash({ a: 1 }), statusHash({ a: 1 }));
  assert.notEqual(statusHash({ a: 1 }), statusHash({ a: 2 }));
});

function harness() {
  const events: string[] = [];
  const sleeps: number[] = [];
  const releases: Array<() => void> = [];
  let inFlight = 0;
  let maxInFlight = 0;
  const responses: Array<Record<string, unknown> | Error> = [];
  const poller = createPoller({
    intervalMs: 2000,
    fetch: async () => {
      inFlight += 1;
      maxInFlight = Math.max(maxInFlight, inFlight);
      await Promise.resolve();
      inFlight -= 1;
      const next = responses.shift() ?? { n: 0 };
      if (next instanceof Error) throw next;
      return next;
    },
    sleep: (ms, signal) =>
      new Promise<void>((resolve) => {
        sleeps.push(ms);
        const done = () => resolve();
        releases.push(done);
        signal.addEventListener("abort", done, { once: true });
      }),
    onStatus: (_status, changed) => events.push(changed ? "changed" : "same"),
    onError: () => events.push("error"),
  });
  return {
    poller,
    events,
    sleeps,
    responses,
    release: () => releases.shift()?.(),
    maxInFlight: () => maxInFlight,
  };
}

const tick = () => new Promise((resolve) => setImmediate(resolve));

test("an unchanged status is reported as unchanged and never overlaps requests", async () => {
  const h = harness();
  h.responses.push({ a: 1 }, { a: 1 }, { a: 2 });
  const done = h.poller.run();
  await tick();
  h.release();
  await tick();
  h.release();
  await tick();
  h.poller.stop();
  await done;
  assert.deepEqual(h.events, ["changed", "same", "changed"]);
  assert.equal(h.maxInFlight(), 1);
  assert.deepEqual(h.sleeps.slice(0, 2), [2000, 2000]);
});

test("failures back off and a later success recovers the interval", async () => {
  const h = harness();
  h.responses.push(new Error("down"), new Error("down"), { a: 1 });
  const done = h.poller.run();
  await tick();
  h.release();
  await tick();
  h.release();
  await tick();
  h.poller.stop();
  await done;
  assert.deepEqual(h.events, ["error", "error", "changed"]);
  assert.deepEqual(h.sleeps, [1000, 2000, 2000]);
});

test("pause stops polling, a forced poll still runs and stop ends the loop", async () => {
  const h = harness();
  const done = h.poller.run();
  await tick();
  assert.deepEqual(h.events, ["changed"]);
  h.poller.setPaused(true);
  h.release();
  await tick();
  assert.deepEqual(h.events, ["changed"]);
  h.responses.push({ n: 5 });
  h.poller.pollNow();
  await tick();
  assert.deepEqual(h.events, ["changed", "changed"]);
  h.poller.stop();
  await done;
});
