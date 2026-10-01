import assert from "node:assert/strict";
import { test } from "node:test";
import { choosePlacement, comparePaneIds } from "../src/layout.js";

const AUTO = { split: "auto" as const, minColumns: 60, minRows: 12 };
const pane = (paneId: string, width: number, height: number) => ({
  paneId,
  width,
  height,
});

test("a wide pane is split to the right and a tall pane downwards", () => {
  assert.deepEqual(choosePlacement([pane("w1:p1", 200, 50)], AUTO), {
    targetPaneId: "w1:p1",
    direction: "right",
  });
  assert.deepEqual(choosePlacement([pane("w1:p1", 80, 60)], AUTO), {
    targetPaneId: "w1:p1",
    direction: "down",
  });
});

test("the other direction is tried when the preferred one would leave a pane too small", () => {
  assert.equal(
    choosePlacement([pane("w1:p1", 100, 50)], AUTO)!.direction,
    "down",
  );
  assert.equal(
    choosePlacement([pane("w1:p1", 130, 20)], { ...AUTO, minRows: 30 }),
    undefined,
    "right leaves 65 columns but 20 rows, down leaves 10 rows",
  );
});

test("a fixed direction is the only one tried, and nothing is placed when it does not fit", () => {
  const wide = [pane("w1:p1", 200, 50)];
  assert.equal(
    choosePlacement(wide, { ...AUTO, split: "down" })!.direction,
    "down",
  );
  assert.equal(
    choosePlacement(wide, { ...AUTO, split: "right" })!.direction,
    "right",
  );
  assert.equal(
    choosePlacement([pane("w1:p1", 100, 50)], { ...AUTO, split: "right" }),
    undefined,
  );
  assert.equal(choosePlacement([pane("w1:p1", 50, 10)], AUTO), undefined);
  assert.equal(choosePlacement([], AUTO), undefined);
});

test("the largest pane is split first, falling back to the next one, and ties go by the numeric pane id", () => {
  const panes = [
    pane("w1:p1", 100, 25),
    pane("w1:p2", 200, 50),
    pane("w1:p10", 200, 50),
  ];
  assert.equal(choosePlacement(panes, AUTO)!.targetPaneId, "w1:p2");
  assert.equal(
    choosePlacement([pane("w1:p1", 40, 10), pane("w1:p2", 200, 50)], AUTO)!
      .targetPaneId,
    "w1:p2",
  );
  assert.ok(comparePaneIds("w1:p2", "w1:p10") < 0);
  assert.ok(comparePaneIds("w2:p1", "w10:p1") < 0);
  assert.equal(comparePaneIds("w1:p1", "w1:p1"), 0);
});

test("a size that is not a positive safe integer counts as no fit", () => {
  for (const bad of [0, -5, 1.5, Number.NaN, Number.POSITIVE_INFINITY, 2 ** 60])
    assert.equal(
      choosePlacement([pane("w1:p1", bad, 50), pane("w1:p2", 200, bad)], AUTO),
      undefined,
      String(bad),
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
