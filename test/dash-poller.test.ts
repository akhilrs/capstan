import assert from "node:assert/strict";
import { test } from "node:test";
import {
  createPoller,
  nextDelayMs,
  statusHash,
  stepInterval,
} from "../src/dash/poller.js";

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

test("changing the interval restarts the sleep with the new value and does not poll early", async () => {
  const h = harness();
  const done = h.poller.run();
  await tick();
  assert.deepEqual(h.sleeps, [2000]);
  h.poller.setIntervalMs(5000);
  await tick();
  assert.deepEqual(h.sleeps, [2000, 5000]);
  assert.deepEqual(h.events, ["changed"], "no extra poll");
  h.release();
  h.release();
  await tick();
  assert.deepEqual(h.events, ["changed", "same"]);
  assert.equal(h.sleeps.at(-1), 5000);
  h.poller.stop();
  await done;
});

test("a forced poll during an interval change still polls once", async () => {
  const h = harness();
  const done = h.poller.run();
  await tick();
  h.poller.setIntervalMs(3000);
  h.poller.pollNow();
  await tick();
  assert.deepEqual(h.events, ["changed", "same"]);
  h.poller.stop();
  await done;
});

test("the interval steps by 1 s up to 10 s and by 5 s above, inside 1 to 60", () => {
  assert.equal(stepInterval(2, false), 3, "+ lengthens");
  assert.equal(stepInterval(2, true), 1, "- shortens");
  assert.equal(stepInterval(1, true), 1);
  assert.equal(stepInterval(9, false), 10);
  assert.equal(stepInterval(10, false), 15);
  assert.equal(stepInterval(15, true), 10);
  assert.equal(stepInterval(10, true), 9);
  assert.equal(stepInterval(60, false), 60);
  assert.equal(stepInterval(58, false), 60);
  for (let n = 1; n <= 60; n++)
    for (const faster of [true, false]) {
      const next = stepInterval(n, faster);
      assert.ok(Number.isInteger(next) && next >= 1 && next <= 60);
    }
});
