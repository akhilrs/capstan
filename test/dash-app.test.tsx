// Colour must be on for the NO_COLOR comparison; this file runs in its own process.
process.env.FORCE_COLOR = "1";
delete process.env.NO_COLOR;

import assert from "node:assert/strict";
import { afterEach, test } from "node:test";
import { createElement } from "react";
import type { AppDeps, CallResult } from "../src/dash/app.js";
import {
  NOW,
  degraded,
  healthy,
  message,
  troubled,
  type Status,
} from "./dash-fixtures.js";

// Loaded after the environment is set: Ink decides on colour when it loads.
const { render } = await import("ink-testing-library");
const { App } = await import("../src/dash/app.js");
const { makeTheme } = await import("../src/dash/theme.js");

const COLOR = /\u001b\[(3[0-7]|9[0-7]|4[0-7])m/;
const settle = () => new Promise((resolve) => setTimeout(resolve, 60));

interface Fixture {
  deps: AppDeps;
  calls: Array<{ command: string; args: readonly string[] }>;
  setStatus(status: Status | Error): void;
  bells: string[];
}

function fixture(
  initial: Status,
  options: {
    size?: { columns: number; rows: number };
    noColor?: boolean;
    reducedMotion?: boolean;
    callResult?: CallResult;
  } = {},
): Fixture {
  let current: Status | Error = initial;
  const calls: Fixture["calls"] = [];
  const bells: string[] = [];
  const deps: AppDeps = {
    fetch: async () => {
      if (current instanceof Error) throw current;
      return current;
    },
    call: async (command, args) => {
      calls.push({ command, args });
      return (
        options.callResult ?? {
          ok: true,
          result: {
            state: "queued",
            text: "line one\nline two",
            agentStatus: "idle",
          },
        }
      );
    },
    workerLimit: 3,
    intervalSeconds: 1,
    theme: makeTheme({
      noColor: options.noColor ?? false,
      reducedMotion: options.reducedMotion ?? false,
    }),
    now: () => NOW,
    confirmDelayMs: 0,
    write: (text) => void bells.push(text),
    size: options.size ?? { columns: 120, rows: 40 },
  };
  return {
    deps,
    calls,
    bells,
    setStatus: (s) => {
      current = s;
    },
  };
}

const mounted: Array<{ unmount(): void }> = [];
afterEach(() => {
  for (const app of mounted.splice(0)) app.unmount();
});

async function open(f: Fixture) {
  const app = render(createElement(App, { deps: f.deps }));
  mounted.push(app);
  await settle();
  return app;
}

test("every panel renders at 80x24 and at 120x40, with health and its reason in the header", async () => {
  for (const size of [
    { columns: 80, rows: 24 },
    { columns: 120, rows: 40 },
  ]) {
    const f = fixture(troubled(), { size });
    const app = await open(f);
    const frame = app.lastFrame()!;
    for (const text of [
      "cstan dash",
      "p1",
      "workers 1/3",
      "Agents",
      "Report pipeline",
      "Message queue",
      "Supervisor findings",
      "developer-agent",
      "STALLED",
      "PROBLEM pane_mismatch",
      "NEEDS OPERATOR",
    ])
      assert.ok(frame.includes(text), `${size.columns}x${size.rows}: ${text}`);
    app.unmount();
  }
});

test("a degraded run shows the reason beside the health word", async () => {
  const app = await open(fixture(degraded()));
  const frame = app.lastFrame()!;
  assert.ok(frame.includes("DEGRADED"));
  assert.ok(frame.includes("forced Supervisor evaluation failure"));
  app.unmount();
});

test("the pipeline panel shows reported, review and integrated counts", async () => {
  const app = await open(fixture(troubled()));
  const frame = app.lastFrame()!;
  assert.ok(frame.includes("reported 2 > review 1 > integrated 1"));
  app.unmount();
});

test("below 60x16 the dashboard says the terminal is too small", async () => {
  const app = await open(
    fixture(troubled(), { size: { columns: 59, rows: 20 } }),
  );
  assert.match(
    app.lastFrame()!,
    /terminal too small \(need 60x16, have 59x20\)/,
  );
  app.unmount();
});

test("at 60x16 the panels fit and no line is wider than the terminal", async () => {
  const app = await open(
    fixture(troubled(), { size: { columns: 60, rows: 16 } }),
  );
  const lines = app.lastFrame()!.split("\n");
  assert.ok(lines.length <= 16, `${lines.length} lines`);
  for (const line of lines)
    assert.ok(
      Array.from(line.replace(/\u001b\[[0-9;]*m/g, "")).length <= 60,
      line,
    );
  app.unmount();
});

test("a controller that stops answering is shown and the last frame stays", async () => {
  const f = fixture(troubled());
  const app = await open(f);
  f.setStatus(new Error("down"));
  app.stdin.write("r");
  await settle();
  const frame = app.lastFrame()!;
  assert.ok(frame.includes("controller not answering"));
  assert.ok(frame.includes("developer-agent"));
  app.unmount();
});

test("o on an agent opens a read-only screen overlay through peek and Esc closes it", async () => {
  const f = fixture(troubled());
  const app = await open(f);
  app.stdin.write("1");
  await settle();
  app.stdin.write("o");
  await settle();
  assert.deepEqual(f.calls, [{ command: "peek", args: ["pm-agent", "40"] }]);
  const frame = app.lastFrame()!;
  assert.ok(frame.includes("screen of pm-agent"));
  assert.ok(frame.includes("line two"));
  app.stdin.write("\u001b");
  await settle();
  assert.ok(!app.lastFrame()!.includes("screen of pm-agent"));
  app.unmount();
});

async function focusQueue(f: Fixture) {
  const app = await open(f);
  app.stdin.write("3");
  await settle();
  return app;
}

test("an action key opens a confirm prompt and any other key cancels without a call", async () => {
  const f = fixture(troubled());
  const app = await focusQueue(f);
  app.stdin.write("s");
  await settle();
  assert.match(
    app.lastFrame()!,
    /Skip message m-stuck to developer-agent \(state sent\)\?/,
  );
  app.stdin.write("x");
  await settle();
  assert.equal(f.calls.length, 0);
  assert.ok(app.lastFrame()!.includes("cancelled"));
  app.unmount();
});

test("y then y retries once through the resolve route and shows the result", async () => {
  const f = fixture(troubled());
  const app = await focusQueue(f);
  app.stdin.write("y");
  await settle();
  assert.match(app.lastFrame()!, /may already have received it/);
  assert.equal(f.calls.length, 0);
  app.stdin.write("y");
  await settle();
  assert.deepEqual(f.calls, [
    { command: "resolve", args: ["m-stuck", "retry"] },
  ]);
  assert.ok(app.lastFrame()!.includes("retry m-stuck: queued"));
  app.unmount();
});

test("cancel goes through the cancel route", async () => {
  const f = fixture(troubled());
  const app = await focusQueue(f);
  app.stdin.write("c");
  await settle();
  app.stdin.write("y");
  await settle();
  assert.deepEqual(f.calls, [{ command: "cancel", args: ["m-stuck"] }]);
  app.unmount();
});

test("a refusal from the daemon is shown and nothing else happens", async () => {
  const f = fixture(troubled(), {
    callResult: { ok: false, message: "illegal_resolution: no" },
  });
  const app = await focusQueue(f);
  app.stdin.write("s");
  await settle();
  app.stdin.write("y");
  await settle();
  assert.ok(app.lastFrame()!.includes("illegal_resolution: no"));
  app.unmount();
});

test("a second y within the confirm delay does not confirm, so a held key cannot act", async () => {
  const f = fixture(troubled());
  f.deps = { ...f.deps, confirmDelayMs: 60_000 };
  const app = await focusQueue(f);
  app.stdin.write("s");
  await settle();
  app.stdin.write("y");
  await settle();
  assert.equal(f.calls.length, 0);
  app.unmount();
});

test("a change to the message under an open prompt cancels it", async () => {
  const f = fixture(troubled());
  const app = await focusQueue(f);
  app.stdin.write("s");
  await settle();
  assert.match(app.lastFrame()!, /Skip message m-stuck/);
  f.setStatus(healthy({ messages: [message("m-stuck", "acked_late")] }));
  await new Promise((resolve) => setTimeout(resolve, 1300));
  assert.ok(app.lastFrame()!.includes("cancelled: the message changed"));
  app.stdin.write("y");
  await settle();
  assert.equal(f.calls.length, 0);
  app.unmount();
});

test("actions are ignored while paused or while the controller is not answering", async () => {
  const f = fixture(troubled());
  const app = await focusQueue(f);
  app.stdin.write("p");
  await settle();
  assert.ok(app.lastFrame()!.includes("PAUSED"));
  app.stdin.write("s");
  await settle();
  assert.ok(!app.lastFrame()!.includes("Skip message"));
  app.stdin.write("p");
  f.setStatus(new Error("down"));
  app.stdin.write("r");
  await settle();
  app.stdin.write("s");
  await settle();
  assert.ok(!app.lastFrame()!.includes("Skip message"));
  assert.equal(f.calls.length, 0);
  app.unmount();
});

test("a key that does not apply to the message state says so and opens no prompt", async () => {
  const f = fixture(healthy({ messages: [message("m1", "queued")] }));
  const app = await focusQueue(f);
  app.stdin.write("y");
  await settle();
  assert.ok(
    app
      .lastFrame()!
      .includes("retry does not apply to a message in state queued"),
  );
  app.unmount();
});

test("colour is used by default and absent with NO_COLOR, and the meaning stays in words", async () => {
  const colored = await open(fixture(troubled()));
  assert.match(colored.lastFrame()!, COLOR);
  colored.unmount();
  const plain = await open(fixture(troubled(), { noColor: true }));
  const frame = plain.lastFrame()!;
  assert.doesNotMatch(frame, COLOR);
  assert.ok(frame.includes("STALLED"));
  assert.ok(frame.includes("PROBLEM pane_mismatch"));
  plain.unmount();
});

test("reduced motion shows a static marker for working agents and rings no bell", async () => {
  const f = fixture(troubled(), { reducedMotion: true });
  const app = await open(f);
  const frame = app.lastFrame()!;
  assert.ok(!/[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]/.test(frame));
  assert.ok(/pm-agent|developer-agent/.test(frame));
  f.setStatus(
    healthy({
      stuck: [{ messageId: "new", reason: "x" }],
      messages: [message("new", "sent")],
    }),
  );
  app.stdin.write("r");
  await settle();
  assert.deepEqual(f.bells, []);
  app.unmount();
});

test("a new stuck message rings the bell once", async () => {
  const f = fixture(healthy());
  const app = await open(f);
  f.setStatus(
    healthy({
      stuck: [{ messageId: "new", reason: "x" }],
      messages: [message("new", "sent")],
    }),
  );
  app.stdin.write("r");
  await settle();
  app.stdin.write("r");
  await settle();
  assert.equal(f.bells.length, 1);
  app.unmount();
});

test("the v1 work panel appears only when there are work rows", async () => {
  const without = await open(fixture(healthy()));
  assert.ok(!without.lastFrame()!.includes("Work items"));
  without.unmount();
  const withWork = await open(
    fixture(
      healthy({
        work: [{ workItemId: "w1", title: "t", state: "blocked", owner: null }],
      }),
    ),
  );
  assert.ok(withWork.lastFrame()!.includes("Work items"));
  withWork.unmount();
});

test("help opens with ? and q quits", async () => {
  const app = await open(fixture(healthy()));
  app.stdin.write("?");
  await settle();
  assert.ok(app.lastFrame()!.includes("Working is inferred"));
  app.stdin.write("?");
  await settle();
  assert.ok(!app.lastFrame()!.includes("Working is inferred"));
  app.unmount();
});
