// Colour must be on for the NO_COLOR comparison; this file runs in its own process.
process.env.FORCE_COLOR = "1";
delete process.env.NO_COLOR;

import assert from "node:assert/strict";
import { afterEach, test } from "node:test";
import { createElement } from "react";
import type { AppDeps, CallResult } from "../src/dash/app.js";
import {
  NOW,
  noSupervisor,
  healthy,
  message,
  crowded,
  troubled,
  type Status,
} from "./dash-fixtures.js";

// Loaded after the environment is set: Ink decides on colour when it loads.
const { render } = await import("ink-testing-library");
const { App } = await import("../src/dash/app.js");
const { makeTheme } = await import("../src/dash/theme.js");
const { resolveSelection, rowIds, formatClock } =
  await import("../src/dash/app.js");
const { buildDashModel } = await import("../src/dash/model.js");

const COLOR = /\u001b\[(3[0-7]|9[0-7]|4[0-7]|38;5;\d+|48;5;\d+)m/;
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
  // The first poll can be slow on a loaded machine; wait for the first frame instead of guessing.
  for (let waited = 0; waited < 3000; waited += 20) {
    if (!/connecting to the controller/.test(app.lastFrame() ?? "")) break;
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
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
      "workers",
      "1/3",
      "agents",
      "pipeline",
      "queue",
      "findings",
      "developer-agent",
      "STALLED",
      "! pane_mismatch",
      "ESCALATED",
    ])
      assert.ok(frame.includes(text), `${size.columns}x${size.rows}: ${text}`);
    app.unmount();
  }
});

test("a run with supervision on and no Supervisor says so beside the chip", async () => {
  const app = await open(fixture(noSupervisor()));
  const frame = app.lastFrame()!;
  assert.ok(frame.includes("NO SUPERVISOR"));
  assert.ok(frame.includes("starts with the next worker"));
  app.unmount();
});

test("the pipeline panel shows reported, review and integrated counts", async () => {
  const app = await open(fixture(troubled()));
  const frame = app.lastFrame()!;
  assert.ok(frame.includes("reported 2  ──►  review 1  ──►  integrated 1"));
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
  assert.deepEqual(f.calls, [
    { command: "peek", args: ["developer-agent", "40"] },
  ]);
  const frame = app.lastFrame()!;
  assert.ok(frame.includes("observe developer-agent"));
  assert.ok(frame.includes("line two"));
  assert.ok(frame.includes("unverified text"));
  assert.ok(frame.includes("cstan dash"), "the dashboard stays under the box");
  app.stdin.write("\u001b");
  await settle();
  assert.ok(!app.lastFrame()!.includes("observe developer-agent"));
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
  assert.ok(app.lastFrame()!.includes("received it"));
  assert.ok(app.lastFrame()!.includes("confirm"));
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
  assert.ok(frame.includes("! pane_mismatch"));
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
  assert.ok(!without.lastFrame()!.includes("work items"));
  without.unmount();
  const withWork = await open(
    fixture(
      healthy({
        work: [{ workItemId: "w1", title: "t", state: "blocked", owner: null }],
      }),
    ),
  );
  assert.ok(withWork.lastFrame()!.includes("work items"));
  withWork.unmount();
});

test("help floats over the dashboard and ? closes it", async () => {
  const app = await open(fixture(healthy()));
  app.stdin.write("?");
  await settle();
  const open1 = app.lastFrame()!;
  assert.ok(open1.includes("NAVIGATE"));
  assert.ok(open1.includes("cstan dash"), "the header stays visible");
  assert.ok(open1.includes("f   queue: show only delivery problems"));
  app.stdin.write("?");
  await settle();
  assert.ok(!app.lastFrame()!.includes("NAVIGATE"));
  app.unmount();
});

test("f toggles the problems-only queue filter and the selected message stays selected", async () => {
  const f = fixture(troubled());
  const app = await focusQueue(f);
  assert.ok(app.lastFrame()!.includes("f problems only [ ]"));
  app.stdin.write("j");
  await settle();
  app.stdin.write("f");
  await settle();
  const frame = app.lastFrame()!;
  assert.ok(frame.includes("f problems only [x]"));
  assert.ok(frame.includes("2/2"), "the failed message is the second problem");
  app.stdin.write("f");
  await settle();
  assert.ok(
    app.lastFrame()!.includes("2/3"),
    "its position among all messages",
  );
  app.stdin.write("j");
  await settle();
  app.stdin.write("f");
  await settle();
  assert.ok(
    app.lastFrame()!.includes("2/2"),
    "a row hidden by the filter falls back to the nearest position",
  );
  app.unmount();
});

test("the problems filter shows an empty state when nothing is stuck", async () => {
  const app = await focusQueue(
    fixture(healthy({ messages: [message("m1", "queued")] })),
  );
  app.stdin.write("f");
  await settle();
  assert.ok(app.lastFrame()!.includes("no delivery problems"));
  app.unmount();
});

test("- and + change the poll interval within 1 to 60 seconds and show it in the top border", async () => {
  const app = await open(fixture(healthy()));
  assert.ok(app.lastFrame()!.includes("- 1s +"));
  app.stdin.write("-");
  await settle();
  assert.ok(app.lastFrame()!.includes("- 1s +"), "1 s is the floor");
  app.stdin.write("+");
  await settle();
  assert.ok(app.lastFrame()!.includes("- 2s +"));
  app.stdin.write("+");
  await settle();
  assert.ok(app.lastFrame()!.includes("- 3s +"));
  app.stdin.write("-");
  await settle();
  assert.ok(app.lastFrame()!.includes("- 2s +"));
  app.unmount();
});

test("an action key in another panel or on an agent does not open a prompt", async () => {
  const f = fixture(troubled());
  const app = await open(f);
  app.stdin.write("1");
  await settle();
  app.stdin.write("s");
  await settle();
  assert.ok(!app.lastFrame()!.includes("confirm"));
  app.unmount();
});

test("q quits", async () => {
  const app = await open(fixture(healthy()));
  app.stdin.write("q");
  await settle();
  app.unmount();
});

test("a selection follows its row id, and falls back to the old position when the row is gone", () => {
  const ids = ["a", "b", "c"];
  assert.equal(resolveSelection(ids, { id: "c", index: 0 }), 2);
  assert.equal(resolveSelection(ids, { id: "gone", index: 1 }), 1);
  assert.equal(resolveSelection(ids, { id: "gone", index: 9 }), 2);
  assert.equal(resolveSelection(ids, { id: null, index: 1 }), 1);
  assert.equal(resolveSelection([], { id: "a", index: 3 }), 0);
});

test("the ids of the rows shown follow the problems filter", () => {
  const model = buildDashModel(troubled(), NOW, 3);
  assert.equal(rowIds(model, false).queue.length, 3);
  assert.deepEqual(rowIds(model, true).queue, ["m-stuck", "m-failed"]);
  assert.deepEqual(rowIds(model, false).agents, [
    "developer-agent",
    "pm-agent",
  ]);
  const crowdedModel = buildDashModel(crowded(), NOW, 3);
  assert.equal(rowIds(crowdedModel, false).agents.length, 7);
  assert.equal(rowIds(crowdedModel, false, true).agents.length, 10);
  assert.equal(rowIds(crowdedModel, false).agents[0], "pm-1");
});

test("the clock is zero-padded hours, minutes and seconds", () => {
  assert.match(formatClock(NOW), /^\d\d:\d\d:\d\d$/);
  assert.equal(
    formatClock(new Date(2026, 9, 2, 4, 5, 6).getTime()),
    "04:05:06",
  );
});

test("j, k and the arrows move through every row of the agents panel, ended agents included", async () => {
  const f = fixture(crowded(), {
    size: { columns: 118, rows: 34 },
    noColor: true,
  });
  const app = await open(f);
  app.stdin.write("1");
  await settle();
  app.stdin.write("e");
  await settle();
  assert.match(app.lastFrame()!, /┤ 1\/10 ├/);
  for (const key of ["j", "j", "j"]) {
    app.stdin.write(key);
    await settle();
  }
  assert.match(app.lastFrame()!, /┤ 4\/10 ├/);
  const selected = app
    .lastFrame()!
    .split("\n")
    .filter((l) => l.includes("▌"));
  assert.equal(selected.length, 1);
  assert.ok(selected[0]!.includes("reviewer-4"), selected[0]);
  app.stdin.write("\u001b[B");
  await settle();
  assert.match(app.lastFrame()!, /┤ 5\/10 ├/);
  app.stdin.write("k");
  app.stdin.write("\u001b[A");
  await settle();
  assert.match(app.lastFrame()!, /┤ 3\/10 ├/);
  for (let i = 0; i < 12; i++) app.stdin.write("j");
  await settle();
  assert.match(app.lastFrame()!, /┤ 10\/10 ├/);
  assert.ok(
    app.lastFrame()!.includes("supervisor-1"),
    "the oldest is reachable",
  );
  app.unmount();
});

test("e toggles between the recent ended agents and all of them, and selection stays valid", async () => {
  const f = fixture(crowded(), {
    size: { columns: 118, rows: 34 },
    noColor: true,
  });
  const app = await open(f);
  app.stdin.write("1");
  await settle();
  assert.match(app.lastFrame()!, /┤ 1\/7 ├/);
  for (let i = 0; i < 12; i++) app.stdin.write("j");
  await settle();
  assert.match(app.lastFrame()!, /┤ 7\/7 ├/);
  assert.ok(!/supervisor-1 +Supervisor/.test(app.lastFrame()!));
  app.stdin.write("e");
  await settle();
  assert.match(app.lastFrame()!, /\/10 ├/);
  for (let i = 0; i < 12; i++) app.stdin.write("j");
  await settle();
  assert.match(app.lastFrame()!, /┤ 10\/10 ├/);
  app.stdin.write("e");
  await settle();
  assert.match(app.lastFrame()!, /┤ 7\/7 ├/);
  app.unmount();
});

test("j and k move the cursor in the pipeline, queue and findings panels too", async () => {
  const f = fixture(troubled(), {
    size: { columns: 118, rows: 40 },
    noColor: true,
  });
  const app = await open(f);
  for (const [panel, total] of [
    ["2", 4],
    ["3", 3],
  ] as const) {
    app.stdin.write(panel);
    await settle();
    assert.match(app.lastFrame()!, new RegExp(`┤ 1/${total} ├`), panel);
    app.stdin.write("j");
    await settle();
    assert.match(app.lastFrame()!, new RegExp(`┤ 2/${total} ├`), panel);
    app.stdin.write("k");
    await settle();
    assert.match(app.lastFrame()!, new RegExp(`┤ 1/${total} ├`), panel);
  }
  const two = fixture(
    {
      ...troubled(),
      agentFindings: [
        ...(troubled().agentFindings as unknown[]),
        {
          findingId: "f-second",
          targetAgentId: "pm-agent",
          severity: "low",
          state: "open",
          interventions: 0,
          stateReason: "x",
        },
      ],
    },
    { size: { columns: 118, rows: 40 }, noColor: true },
  );
  const second = await open(two);
  second.stdin.write("4");
  await settle();
  assert.match(second.lastFrame()!, /┤ 1\/2 ├/);
  second.stdin.write("j");
  await settle();
  assert.match(second.lastFrame()!, /┤ 2\/2 ├/);
  second.unmount();
  app.unmount();
});

test("an unfocused panel shows no cursor counter unless rows are hidden", async () => {
  const f = fixture(crowded(), {
    size: { columns: 118, rows: 34 },
    noColor: true,
  });
  const app = await open(f);
  const frame = app.lastFrame()!;
  assert.ok(!/┤ \d+\/10 ├/.test(frame), "agents has no cursor while unfocused");
  assert.match(frame, /┤ 1-\d+\/12 ├/, "the pipeline hides rows");
  app.unmount();
});

test("o on an ended agent says so and makes no call", async () => {
  const f = fixture(crowded(), {
    size: { columns: 118, rows: 34 },
    noColor: true,
  });
  const app = await open(f);
  app.stdin.write("1");
  await settle();
  app.stdin.write("j");
  await settle();
  app.stdin.write("o");
  await settle();
  assert.deepEqual(f.calls, []);
  assert.ok(app.lastFrame()!.includes("reviewer-6 has ended"));
  app.unmount();
});

test("the problems-only filter starts off", async () => {
  const app = await open(fixture(troubled()));
  assert.ok(app.lastFrame()!.includes("f problems only [ ]"));
  app.unmount();
});
