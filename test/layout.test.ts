import assert from "node:assert/strict";
import { test } from "node:test";
import { choosePlacement, comparePaneIds } from "../src/layout.js";

const LIMITS = { pmWidthPercent: 60, minColumns: 60, minRows: 12 };
const pane = (paneId: string, width: number, height: number) => ({
  paneId,
  width,
  height,
});
const PM = "w1:p1";

test("the first worker is split off to the right of the PM, which keeps its share of the width", () => {
  assert.deepEqual(choosePlacement([pane(PM, 200, 50)], PM, LIMITS), {
    targetPaneId: PM,
    direction: "right",
    keep: 0.6,
  });
  assert.equal(
    choosePlacement([pane(PM, 200, 50)], PM, {
      ...LIMITS,
      pmWidthPercent: 70,
    })!.keep,
    0.7,
  );
});

test("the PM keeps the left and is never split down: later workers stack in the column on its right", () => {
  const panes = [pane(PM, 120, 50), pane("w1:p2", 80, 50)];
  assert.deepEqual(choosePlacement(panes, PM, LIMITS), {
    targetPaneId: "w1:p2",
    direction: "down",
    keep: 0.5,
  });
  const stacked = [
    pane(PM, 120, 50),
    pane("w1:p2", 80, 25),
    pane("w1:p3", 80, 25),
  ];
  assert.equal(choosePlacement(stacked, PM, LIMITS)!.targetPaneId, "w1:p2");
  const tall = [
    pane(PM, 120, 50),
    pane("w1:p2", 80, 20),
    pane("w1:p3", 80, 30),
  ];
  assert.equal(choosePlacement(tall, PM, LIMITS)!.targetPaneId, "w1:p3");
  const placement = choosePlacement(tall, PM, LIMITS)!;
  assert.notEqual(placement.targetPaneId, PM);
});

test("a tie in height goes to the lowest pane number, not the lexical order", () => {
  const panes = [
    pane(PM, 100, 50),
    pane("w1:p10", 80, 30),
    pane("w1:p2", 80, 30),
  ];
  assert.equal(choosePlacement(panes, PM, LIMITS)!.targetPaneId, "w1:p2");
});

test("nothing is placed when the PM's width or the worker column cannot hold the minimum", () => {
  assert.equal(
    choosePlacement([pane(PM, 120, 50)], PM, LIMITS),
    undefined,
    "120 columns leave the PM 72 and the worker 48, below 60",
  );
  assert.equal(
    choosePlacement([pane(PM, 200, 50), pane("w1:p2", 80, 20)], PM, LIMITS),
    undefined,
    "a 20-row worker pane would leave 10 rows",
  );
  assert.equal(
    choosePlacement([pane(PM, 200, 50), pane("w1:p2", 50, 40)], PM, LIMITS),
    undefined,
    "the worker column is narrower than the minimum",
  );
});

test("a layout without the PM pane, or with unusable sizes, places nothing", () => {
  assert.equal(
    choosePlacement([pane("w1:p2", 200, 50)], PM, LIMITS),
    undefined,
  );
  assert.equal(choosePlacement([], PM, LIMITS), undefined);
  assert.equal(choosePlacement([pane(PM, 0, 50)], PM, LIMITS), undefined);
  assert.equal(choosePlacement([pane(PM, 200.5, 50)], PM, LIMITS), undefined);
  assert.equal(
    choosePlacement(
      [pane(PM, 200, 50), pane("w1:p2", Number.NaN, 40)],
      PM,
      LIMITS,
    )!.targetPaneId,
    PM,
    "an unusable worker pane is ignored",
  );
});

test("pane ids with leading zeros or very long numbers still sort in a fixed order", () => {
  assert.notEqual(comparePaneIds("w1:p01", "w1:p1"), 0);
  assert.equal(
    comparePaneIds("w1:p01", "w1:p1"),
    -comparePaneIds("w1:p1", "w1:p01"),
  );
  assert.ok(
    comparePaneIds("w1:p" + "9".repeat(30), "w1:p1" + "0".repeat(30)) < 0,
  );
  assert.ok(comparePaneIds("w1:p2", "w1:p10") < 0);
});
