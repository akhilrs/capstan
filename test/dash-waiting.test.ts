import assert from "node:assert/strict";
import { test } from "node:test";
import { plainLines } from "../src/dash/lines.js";
import { makeTheme } from "../src/dash/theme.js";
import { cellWidth } from "../src/dash/format.js";
import { stripRows } from "../src/dash/waiting.js";
import { buildFrame } from "../src/dash/view.js";
import { iso, showcase } from "./dash-fixtures.js";
import { modelOf, viewOf } from "./dash-view-helpers.js";

const theme = makeTheme({ noColor: true, reducedMotion: true });

const confirm = (id: string, ago: number) => ({
  integrationId: id,
  branch: "integration/x",
  createdAt: iso(ago),
  reviewState: "passed",
});
const proposal = (id: string) => ({
  proposalId: id,
  kind: "add-role",
  proposer: "pm-1",
  reason: "need a tester for the dashboard work".repeat(4),
  createdAt: iso(40),
});

export const waitingStatus = (extra = {}) =>
  showcase({
    awaitingConfirm: [confirm("0123456789abcdef", 200)],
    pendingProposals: [proposal("p-7")],
    pause: {
      run: null,
      agents: [
        { agentId: "developer-2", pausedAt: iso(90), reason: "stalled" },
      ],
    },
    ...extra,
  });

test("nothing waiting takes no rows and leaves the frame as it was", () => {
  const plain = plainLines(buildFrame(modelOf(), viewOf(120, 36), theme).lines);
  assert.equal(stripRows(modelOf(), 36), 0);
  assert.ok(!plain.join("\n").includes("waiting on you"));
});

test("items get one line each up to four, then +N more, between the header and the panels", () => {
  const status = waitingStatus({
    awaitingConfirm: ["a", "b", "c"].map((id, i) =>
      confirm(`${id}0123456789`, 100 + i),
    ),
  });
  const model = modelOf(status);
  assert.equal(model.waiting.length, 5);
  assert.equal(stripRows(model, 36), 5);
  const lines = plainLines(buildFrame(model, viewOf(120, 36), theme).lines);
  assert.equal(lines.length, 36);
  assert.match(
    lines[3]!,
    /integration a0123456 merged, review passed: cstan integrate confirm pending\s+1m40s/,
  );
  assert.match(lines[6]!, /proposal p-7 add-role by pm-1: need a tester/);
  assert.match(lines[7]!, /\+1 more/);
  assert.match(lines[8]!, /agents/, "the panels follow the strip");
});

test("below 30 rows the strip is one summary line", () => {
  const model = modelOf(waitingStatus());
  assert.equal(stripRows(model, 29), 1);
  const lines = plainLines(buildFrame(model, viewOf(100, 29), theme).lines);
  assert.equal(lines.length, 29);
  assert.match(
    lines.find((l) => l.includes("waiting on you")) ?? "",
    /waiting on you: 1 integration confirm, 1 proposal, 1 agent paused\s+oldest 3m20s/,
  );
});

test("items that do not fit the budget collapse to the summary", () => {
  const model = modelOf(waitingStatus());
  assert.equal(stripRows(model, 30), 3);
  const many = modelOf(
    waitingStatus({
      awaitingConfirm: Array.from({ length: 9 }, (_, i) =>
        confirm(`i${i}`, 10),
      ),
    }),
  );
  assert.equal(stripRows(many, 30), 5);
  assert.equal(stripRows(many, 29), 1);
});

test("a paused run is listed with its reason and age", () => {
  const model = modelOf(
    showcase({
      pause: {
        run: { pausedAt: iso(300), reason: "operator break", actorId: "op" },
        agents: [],
      },
    }),
  );
  const lines = plainLines(buildFrame(model, viewOf(120, 36), theme).lines);
  assert.match(lines[3]!, /run paused: operator break\s+5m00s/);
});

test("strip text truncates with an ellipsis and never overflows from 60 to 200 columns", () => {
  const model = modelOf(waitingStatus());
  for (const t of [
    theme,
    makeTheme({ noColor: true, reducedMotion: true, ascii: true }),
  ])
    for (const columns of [60, 61, 79, 80, 99, 100, 119, 120, 160, 199, 200])
      for (const rows of [24, 36]) {
        const lines = plainLines(
          buildFrame(model, viewOf(columns, rows), t).lines,
        );
        assert.equal(lines.length, rows);
        for (const line of lines)
          assert.equal(cellWidth(line), columns, `${columns}x${rows}: ${line}`);
      }
  const narrow = plainLines(buildFrame(model, viewOf(60, 36), theme).lines);
  assert.match(narrow.find((l) => l.includes("proposal p-7")) ?? "", /…/);
});
