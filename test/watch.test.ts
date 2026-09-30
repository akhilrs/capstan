import assert from "node:assert/strict";
import { test } from "node:test";
import { renderWatch, signalsOf, watchStatus } from "../src/watch.js";

function status(
  overrides: Record<string, unknown> = {},
): Record<string, unknown> {
  return {
    agents: [{ agentId: "pm-agent", kind: "PM", state: "active" }],
    messages: [],
    inputClears: [],
    stuck: [],
    stalledAgentIds: [],
    ...overrides,
  };
}

async function run(
  snapshots: Array<Record<string, unknown> | "down">,
): Promise<string[]> {
  const written: string[] = [];
  let poll = 0;
  await watchStatus({
    intervalMs: 1,
    iterations: snapshots.length,
    sleep: async () => undefined,
    write: (text) => void written.push(text),
    fetch: async () => {
      const next = snapshots[poll++]!;
      if (next === "down") throw new Error("down");
      return next;
    },
  });
  return written;
}

const notified = (at: string | null) =>
  status({
    messages: [
      {
        messageId: "m-1",
        recipientAgentId: "pm-agent",
        state: "sent",
        lastNotifiedAt: at,
      },
    ],
  });

test("the first snapshot is a baseline and never rings, even when it already holds a notification", async () => {
  const out = await run([notified("2026-01-01T00:00:00Z")]);
  assert.equal(out[0]!.includes("\u0007"), false);
});

test("a new or changed lastNotifiedAt rings once; an unchanged one does not; removal never rings", async () => {
  const out = await run([
    notified(null),
    notified("2026-01-01T00:00:00Z"),
    notified("2026-01-01T00:00:00Z"),
    notified("2026-01-01T00:10:00Z"),
    status(),
  ]);
  assert.deepEqual(
    out.map((text) => text.includes("\u0007")),
    [false, true, false, true, false],
  );
});

test("a new input clear or a new stuck entry rings", async () => {
  const out = await run([
    status(),
    status({
      inputClears: [{ clearId: "c-1", messageId: "m-1", recordedAt: "t" }],
    }),
    status({
      inputClears: [{ clearId: "c-1", messageId: "m-1", recordedAt: "t" }],
    }),
    status({
      inputClears: [{ clearId: "c-1", messageId: "m-1", recordedAt: "t" }],
      stuck: [{ messageId: "m-1", reason: "pane_mismatch" }],
    }),
  ]);
  assert.deepEqual(
    out.map((text) => text.includes("\u0007")),
    [false, true, false, true],
  );
});

test("the loop ends when the controller stops answering", async () => {
  const out = await run([status(), "down"]);
  assert.match(out[1]!, /stopped answering/);
});

test("the rendered block lists agents, unresolved messages, stuck entries and stalled agents", () => {
  const text = renderWatch(
    status({
      messages: [
        {
          messageId: "m-1",
          recipientAgentId: "dev",
          state: "deferred",
          lastNotifiedAt: null,
        },
      ],
      stuck: [{ messageId: "m-1", reason: "input_unreadable" }],
      stalledAgentIds: ["dev"],
    }),
  );
  assert.match(text, /agents: pm-agent \(PM, active\)/);
  assert.match(text, /message m-1 -> dev \[deferred\] notified: no/);
  assert.match(text, /stuck m-1: input_unreadable/);
  assert.match(text, /stalled: dev/);
  assert.match(renderWatch(status()), /messages: none unresolved/);
  assert.equal(signalsOf(status()).size, 0);
});

test("everything from the daemon is shown without control or format characters", () => {
  const text = renderWatch(
    status({
      agents: [{ agentId: "a\u001b[31m", kind: "PM\u202e", state: "active" }],
      messages: [
        {
          messageId: "m\u0007",
          recipientAgentId: "r\u2028",
          state: "s\u001b",
          lastNotifiedAt: "t\u200d",
        },
      ],
      stuck: [{ messageId: "m\u001b", reason: "why\u0000" }],
      stalledAgentIds: ["x\u0007"],
    }),
  );
  assert.ok(!/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u.test(text.replace(/\n/g, " ")));
});
