import assert from "node:assert/strict";
import { test } from "node:test";
import { rowIds } from "../src/dash/app.js";
import { plainLines } from "../src/dash/lines.js";
import { RECENT_ENDED_SHOWN, visibleAgents } from "../src/dash/model.js";
import { makeTheme } from "../src/dash/theme.js";
import { buildFrame } from "../src/dash/view.js";
import { crowded } from "./dash-fixtures.js";
import { modelOf, viewOf } from "./dash-view-helpers.js";

const theme = makeTheme({ noColor: true, reducedMotion: true });
const calm = () => crowded({ agentFindings: [] });
const ids = (status = calm(), all = false) =>
  visibleAgents(modelOf(status).agents, all).map((a) => a.agentId);

test("by default every active agent and only the latest ended agents are listed", () => {
  assert.equal(RECENT_ENDED_SHOWN, 5);
  assert.deepEqual(ids(), [
    "pm-1",
    "reviewer-6",
    "reviewer-5",
    "reviewer-4",
    "reviewer-3",
    "reviewer-2",
  ]);
});

test("show-all lists every agent", () => {
  assert.equal(ids(calm(), true).length, 10);
});

test("a lost ended agent always shows, and does not use up a recent slot", () => {
  const shown = ids(
    crowded({ agentFindings: [], lostAgentIds: ["supervisor-1"] }),
  );
  assert.ok(shown.includes("supervisor-1"));
  assert.ok(shown.includes("reviewer-2"));
  assert.equal(shown.length, 7);
});

test("a blocked ended agent always shows", () => {
  const shown = ids(
    crowded({
      agentFindings: [
        {
          findingId: "f1",
          targetAgentId: "designer-1",
          severity: "high",
          state: "escalated",
          interventions: 2,
        },
      ],
    }),
  );
  assert.ok(shown.includes("designer-1"));
});

test("the panel keeps the full counts and renders only the visible rows", () => {
  const text = (all: boolean) =>
    plainLines(
      buildFrame(
        modelOf(calm(), 3),
        viewOf(118, 34, { focus: "agents", showAllEnded: all }),
        theme,
      ).lines,
    ).join("\n");
  const some = text(false);
  assert.ok(some.includes("1 active") && some.includes("9 ended"));
  assert.ok(
    /reviewer-2 +Reviewer/.test(some) && !/reviewer-1 +Reviewer/.test(some),
  );
  const every = text(true);
  assert.ok(
    /reviewer-1 +Reviewer/.test(every) &&
      /supervisor-1 +Supervisor/.test(every),
  );
});

test("selection ids follow the visible rows in both modes", () => {
  const model = modelOf(calm());
  assert.equal(rowIds(model, false, false).agents.length, 6);
  assert.equal(rowIds(model, false, true).agents.length, 10);
});
