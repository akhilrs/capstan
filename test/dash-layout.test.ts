import assert from "node:assert/strict";
import { test } from "node:test";
import {
  agentSections,
  columnsOf,
  columnWidths,
  fillRows,
  fits,
  layoutFor,
  pipelineSections,
  queueSections,
  windowOf,
  type PanelWish,
} from "../src/dash/layout.js";

const wish = (
  id: PanelWish["id"],
  min: number,
  want: number,
  weight: number,
  stretch = false,
): PanelWish => ({ id, min, want, weight, stretch });

test("the layout mode follows the 60x16 minimum and the 100 column breakpoint", () => {
  assert.equal(layoutFor(59, 40).mode, "tiny");
  assert.equal(layoutFor(80, 15).mode, "tiny");
  assert.equal(layoutFor(60, 16).mode, "narrow");
  assert.equal(layoutFor(99, 40).mode, "narrow");
  assert.equal(layoutFor(100, 40).mode, "wide");
});

test("panels stack in one column below 100 columns and split in two from there", () => {
  const panels = ["agents", "pipeline", "queue", "findings", "work"] as const;
  assert.deepEqual(columnsOf("narrow", panels), [panels]);
  assert.deepEqual(columnsOf("wide", panels), [
    ["agents", "pipeline"],
    ["queue", "findings", "work"],
  ]);
  assert.deepEqual(columnWidths(80, "narrow"), [80]);
  assert.deepEqual(columnWidths(121, "wide"), [60, 61]);
});

test("every panel gets its minimum and short panels reach their content before long ones take more", () => {
  const heights = fillRows(
    40,
    [
      wish("agents", 3, 9, 4, true),
      wish("pipeline", 3, 23, 2),
      wish("queue", 3, 8, 4, true),
      wish("findings", 3, 5, 1),
    ],
    "queue",
  );
  const total = [...heights.values()].reduce((a, b) => a + b, 0);
  assert.equal(total, 40);
  for (const rows of heights.values()) assert.ok(rows >= 3);
  assert.equal(heights.get("queue"), 8);
  assert.equal(heights.get("agents"), 9);
  assert.equal(heights.get("findings"), 5);
});

test("rows stay equal to the budget whatever the wishes are", () => {
  for (const rows of [12, 13, 19, 32, 41, 80]) {
    const heights = fillRows(
      rows,
      [
        wish("agents", 3, 9, 4, true),
        wish("pipeline", 3, 23, 2),
        wish("queue", 3, 8, 4, true),
        wish("findings", 3, 5, 1),
      ],
      "agents",
    );
    assert.equal(
      [...heights.values()].reduce((a, b) => a + b, 0),
      rows,
      `${rows} rows`,
    );
  }
});

test("spare rows after every wish go to the focused panel if it stretches, else the queue, then agents", () => {
  const wishes = [
    wish("agents", 3, 5, 4, true),
    wish("pipeline", 3, 5, 1),
    wish("queue", 3, 5, 4, true),
    wish("findings", 3, 5, 1),
  ];
  assert.deepEqual([...fillRows(30, wishes, "agents").values()], [15, 5, 5, 5]);
  assert.deepEqual([...fillRows(30, wishes, "queue").values()], [5, 5, 15, 5]);
  assert.deepEqual(
    [...fillRows(30, wishes, "findings").values()],
    [5, 5, 15, 5],
  );
  const noStretch = wishes.map((w) => ({ ...w, stretch: false }));
  assert.deepEqual(
    [...fillRows(30, noStretch, "queue").values()],
    [5, 5, 5, 15],
  );
});

test("a column that cannot pay every minimum drops work, then findings, then pipeline", () => {
  const all = [
    wish("agents", 3, 9, 4, true),
    wish("pipeline", 3, 9, 1),
    wish("queue", 3, 9, 4, true),
    wish("findings", 3, 9, 1),
    wish("work", 3, 9, 1),
  ];
  assert.deepEqual(
    [...fillRows(12, all, "queue").keys()],
    ["agents", "pipeline", "queue", "findings"],
  );
  assert.deepEqual(
    [...fillRows(9, all, "queue").keys()],
    ["agents", "pipeline", "queue"],
  );
  assert.deepEqual([...fillRows(6, all, "queue").keys()], ["agents", "queue"]);
  assert.equal(fillRows(10, [], "queue").size, 0);
});

test("the window keeps the cursor visible and reports hidden rows", () => {
  assert.deepEqual(windowOf(3, 0, 5), { start: 0, end: 3, hidden: 0 });
  assert.deepEqual(windowOf(10, 0, 4), { start: 0, end: 4, hidden: 6 });
  assert.deepEqual(windowOf(10, 9, 4), { start: 6, end: 10, hidden: 6 });
  assert.deepEqual(windowOf(10, 5, 4), { start: 2, end: 6, hidden: 6 });
  assert.deepEqual(windowOf(4, 0, 0), { start: 0, end: 0, hidden: 4 });
});

test("optional table columns drop below their content-width thresholds", () => {
  assert.equal(fits(60, "agentsPane"), true);
  assert.equal(fits(59, "agentsPane"), false);
  assert.equal(fits(46, "agentsRole"), true);
  assert.equal(fits(45, "agentsRole"), false);
  assert.equal(fits(40, "agentsActivity"), true);
  assert.equal(fits(55, "queueNotified"), false);
});

test("the queue shares body rows between header, list, selected detail and graphs", () => {
  assert.deepEqual(queueSections(1, 5, true), {
    header: 0,
    list: 1,
    detail: 0,
    graphs: 0,
  });
  assert.deepEqual(queueSections(4, 5, true), {
    header: 1,
    list: 3,
    detail: 0,
    graphs: 0,
  });
  const roomy = queueSections(25, 5, true);
  assert.equal(roomy.header, 1);
  assert.equal(roomy.list, 5);
  assert.equal(roomy.detail, 4);
  assert.equal(roomy.graphs, 15);
  assert.equal(
    roomy.header + roomy.list + roomy.detail + roomy.graphs,
    25,
    "no row is left unused",
  );
});

test("the queue shows no selected-detail area when there is nothing to select", () => {
  const empty = queueSections(25, 1, false);
  assert.equal(empty.detail, 0);
  assert.equal(empty.graphs, 23);
});

test("agents are one scrolling list, then a graph with what is left", () => {
  assert.deepEqual(agentSections(4, 10), { header: 1, rows: 3, graph: 0 });
  assert.deepEqual(agentSections(13, 10), {
    header: 1,
    rows: 10,
    graph: 2 > 3 ? 2 : 0,
  });
  assert.deepEqual(agentSections(18, 6), { header: 1, rows: 6, graph: 11 });
  assert.equal(agentSections(8, 0).rows, 1, "the empty row is kept");
});

test("the pipeline collapses from the table to the stage bars to a flow line", () => {
  assert.deepEqual(pipelineSections(1, 14), {
    flow: 1,
    stages: 0,
    compactSummary: false,
    gapped: false,
    header: 0,
    items: 0,
  });
  assert.equal(pipelineSections(2, 14).compactSummary, true);
  const bars = pipelineSections(5, 14);
  assert.equal(bars.stages, 3);
  assert.equal(bars.header, 0);
  const full = pipelineSections(21, 14);
  assert.equal(full.gapped, true);
  assert.equal(full.stages, 5);
  assert.equal(full.items, 14);
  const tight = pipelineSections(8, 14);
  assert.equal(tight.gapped, false);
  assert.equal(tight.stages, 3);
  assert.equal(tight.items, 3);
});
