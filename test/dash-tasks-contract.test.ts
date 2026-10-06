import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { plainLines } from "../src/dash/lines.js";
import { makeTheme } from "../src/dash/theme.js";
import { buildFrame } from "../src/dash/view.js";
import { modelOf, viewOf } from "./dash-view-helpers.js";

const fixture = path.resolve(
  import.meta.dirname,
  "..",
  "..",
  "test",
  "fixtures",
  "status-active-tasks.json",
);

test("the dashboard renders the status-tasks contract fixture", (t) => {
  if (!existsSync(fixture)) {
    t.skip("status-tasks contract fixture not present");
    return;
  }
  const status = JSON.parse(readFileSync(fixture, "utf8")) as Record<
    string,
    unknown
  >;
  const model = modelOf(status);
  assert.deepEqual(
    model.header.tasks?.map((task) => task.label),
    ["plan-1 the plan (1/2)", "NX-7 Fix the thing"],
  );
  const task = (id: string) => model.agents.find((a) => a.agentId === id)?.task;
  assert.equal(task("developer-agent"), "plan-1/wp1 NX-1 First");
  assert.equal(task("dev-two"), "plan-1/wp2 Second");
  assert.equal(task("architect"), "plan-1 · plan");
  assert.equal(task("dev-three"), "NX-7 Fix the thing");
  assert.equal(task("reviewer-live"), "review dev-three r1");
  assert.equal(task("plain"), "-");
  assert.equal(task("pm-agent"), "-");
  const lines = plainLines(
    buildFrame(
      model,
      viewOf(160, 45),
      makeTheme({ noColor: true, reducedMotion: true }),
    ).lines,
  );
  assert.ok(
    lines.some((l) =>
      l.includes("working on: plan-1 the plan (1/2) · NX-7 Fix the thing"),
    ),
  );
});
