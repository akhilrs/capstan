import assert from "node:assert/strict";
import { test } from "node:test";
import {
  allocate,
  cellsFor,
  columnsOf,
  layoutFor,
  windowOf,
} from "../src/dash/layout.js";

test("the layout mode follows the 60x16 minimum and the 100 column breakpoint", () => {
  assert.equal(layoutFor(59, 40).mode, "tiny");
  assert.equal(layoutFor(80, 15).mode, "tiny");
  assert.equal(layoutFor(60, 16).mode, "narrow");
  assert.equal(layoutFor(99, 40).mode, "narrow");
  assert.equal(layoutFor(100, 40).mode, "wide");
});

test("sparklines show only in wide mode", () => {
  assert.equal(layoutFor(99, 40).sparklines, false);
  assert.equal(layoutFor(100, 40).sparklines, true);
});

test("optional columns drop as a panel narrows: pane, then role, notified and age", () => {
  assert.deepEqual(cellsFor(110), {
    showAge: true,
    showNotified: true,
    showRole: true,
    showPane: true,
  });
  assert.deepEqual(cellsFor(80), {
    showAge: true,
    showNotified: true,
    showRole: false,
    showPane: false,
  });
  assert.deepEqual(cellsFor(60), {
    showAge: true,
    showNotified: false,
    showRole: false,
    showPane: false,
  });
  assert.equal(cellsFor(49).showAge, false);
});

test("panels stack in one column below 100 columns and split in two from there", () => {
  const panels = ["agents", "pipeline", "queue", "findings", "work"] as const;
  assert.deepEqual(columnsOf("narrow", panels), [panels]);
  assert.deepEqual(columnsOf("wide", panels), [
    ["agents", "pipeline"],
    ["queue", "findings", "work"],
  ]);
});

test("every panel keeps a body row and the focused one gets the leftover", () => {
  const all = ["agents", "pipeline", "queue", "findings"] as const;
  const a = allocate(14, all, "queue");
  assert.deepEqual([...a.values()], [2, 2, 4, 2]);
  const sum = [...a.values()].reduce((x, y) => x + y, 0);
  assert.equal(sum + all.length, 14);
  const tight = allocate(3, all, "agents");
  for (const rows of tight.values()) assert.ok(rows >= 1);
  assert.equal(allocate(10, [], "agents").size, 0);
});

test("the window keeps the cursor visible and reports hidden rows", () => {
  assert.deepEqual(windowOf(3, 0, 5), { start: 0, end: 3, hidden: 0 });
  assert.deepEqual(windowOf(10, 0, 4), { start: 0, end: 4, hidden: 6 });
  assert.deepEqual(windowOf(10, 9, 4), { start: 6, end: 10, hidden: 6 });
  assert.deepEqual(windowOf(10, 5, 4), { start: 2, end: 6, hidden: 6 });
});
