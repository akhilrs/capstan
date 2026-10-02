import assert from "node:assert/strict";
import { test } from "node:test";
import { bottomBorder, thumbRange, topBorder } from "../src/dash/border.js";
import { glyphsFor } from "../src/dash/glyphs.js";
import {
  areaGraph,
  graphSpans,
  meter,
  recencyBar,
  sparkline,
  stackedBar,
} from "../src/dash/graph.js";
import {
  fitLine,
  plainLine,
  plainLines,
  mergeSpans,
} from "../src/dash/lines.js";
import { gradientColor, makeTheme, PALETTE } from "../src/dash/theme.js";
import { isUtf8Locale, wantsAscii } from "../src/dash/terminal.js";
import { wrapText } from "../src/dash/overlays.js";
import { footerHints } from "../src/dash/view.js";

const g = glyphsFor(false);
const ascii = glyphsFor(true);
const plain = makeTheme({ noColor: true, reducedMotion: false });
const color = makeTheme({ noColor: false, reducedMotion: false });
const cells = (text: string) => Array.from(text).length;

test("a top border carries hotkey, title and tabs and is exactly as wide as asked", () => {
  for (const width of [30, 40, 80, 120]) {
    const line = plainLine(
      topBorder(
        width,
        {
          number: 1,
          title: "agents",
          tabs: [{ text: "5 active" }, { text: "1 ended" }],
          focused: false,
        },
        g,
        {},
      ),
    );
    assert.equal(cells(line), width, line);
    assert.ok(line.startsWith("╭─¹agents"));
    if (width >= 40) assert.ok(line.endsWith("┤ 1 ended ├─╮"));
  }
  const focused = plainLine(
    topBorder(30, { number: 3, title: "queue", focused: true }, g, {}),
  );
  assert.ok(focused.startsWith("┏━³queue") && focused.endsWith("┓"));
});

test("a narrow top border drops the last tabs first, then cuts the title", () => {
  const tabs = [{ text: "first tab" }, { text: "second tab" }];
  const line = (width: number) =>
    plainLine(
      topBorder(
        width,
        { number: 2, title: "pipeline", tabs, focused: false },
        g,
        {},
      ),
    );
  assert.ok(line(36).includes("first tab") && !line(36).includes("second tab"));
  assert.ok(line(40).includes("second tab"));
  assert.ok(!line(20).includes("tab"));
  for (const width of [12, 20, 36, 40]) assert.equal(cells(line(width)), width);
  const title = plainLine(
    topBorder(8, { title: "a very long title", focused: false }, g, {}),
  );
  assert.equal(cells(title), 8);
  assert.ok(title.includes("…"));
});

test("header tabs sit right after the title and the others stay right-aligned", () => {
  const line = plainLine(
    topBorder(
      70,
      {
        title: " cstan dash ",
        leftTabs: [{ text: "capstan" }, { text: "run active" }],
        tabs: [{ text: "15:02:23" }, { text: "- 2s +" }],
        focused: false,
      },
      g,
      {},
    ),
  );
  assert.equal(cells(line), 70);
  assert.ok(line.startsWith("╭─ cstan dash ─┤ capstan ├┤ run active ├"));
  assert.ok(line.endsWith("┤ 15:02:23 ├┤ - 2s + ├─╮"));
});

test("ASCII borders use only ASCII characters", () => {
  const top = plainLine(
    topBorder(
      40,
      { number: 1, title: "agents", tabs: [{ text: "x" }], focused: true },
      ascii,
      {},
    ),
  );
  assert.match(top, /^[\x20-\x7e]+$/);
  assert.equal(cells(top), 40);
  assert.ok(top.startsWith("+=1.agents"));
});

test("a bottom border carries tabs on both sides at the exact width", () => {
  for (const width of [20, 50, 80]) {
    const line = plainLine(
      bottomBorder(
        width,
        {
          left: [{ text: "working: inferred" }],
          right: [{ text: "1/5" }],
          focused: false,
        },
        g,
      ),
    );
    assert.equal(cells(line), width);
  }
  const wide = plainLine(
    bottomBorder(40, { right: [{ text: "1/5" }], focused: true }, g),
  );
  assert.ok(wide.startsWith("┗") && wide.endsWith("┤ 1/5 ├━┛"));
  assert.equal(
    cells(
      plainLine(
        bottomBorder(6, { left: [{ text: "too long" }], focused: false }, g),
      ),
    ),
    6,
  );
});

test("the scroll thumb scales with the visible share and reaches both ends", () => {
  assert.equal(thumbRange(3, 3, 0, 3), undefined);
  assert.equal(thumbRange(5, 0, 0, 3), undefined);
  assert.deepEqual(thumbRange(10, 5, 0, 10), { start: 0, size: 5 });
  assert.deepEqual(thumbRange(10, 5, 5, 10), { start: 5, size: 5 });
  assert.deepEqual(thumbRange(100, 3, 97, 3), { start: 2, size: 1 });
});

test("an area graph is exactly cells x rows, newest sample at the right, and blank where history is missing", () => {
  const rows = areaGraph([0, 1, 2, 3, 4], 4, 2, 4, false);
  assert.equal(rows.length, 2);
  for (const row of rows) assert.equal(cells(row), 4);
  assert.equal(rows[1]![0], " ", "no history yet on the left");
  assert.match(rows[1]!, /[⠀-⣿]$/);
  assert.equal(areaGraph([], 3, 1, 5, false)[0], "   ");
  const low = areaGraph([1], 1, 1, 1000, false)[0]!;
  assert.notEqual(low, " ", "a non-zero value always lights a dot");
  const full = areaGraph([4, 4], 1, 1, 4, false)[0]!;
  assert.equal(full, "⣿");
});

test("an ASCII graph draws # columns one level per cell", () => {
  const rows = areaGraph([0, 1, 2], 3, 2, 2, true);
  assert.deepEqual(rows, ["  #", " ##"]);
  for (const row of areaGraph([5, 9, 1], 3, 4, 9, true))
    assert.match(row, /^[ #]+$/);
});

test("graph rows are coloured by height, and not at all without colour", () => {
  const rows = graphSpans(["ab", "cd"], color);
  assert.equal(rows[0]![0]!.color, gradientColor(0.75));
  assert.equal(rows[1]![0]!.color, gradientColor(0.25));
  assert.equal(graphSpans(["ab"], plain)[0]![0]!.color, undefined);
});

test("the gradient runs from ok through warn to bad", () => {
  assert.equal(gradientColor(0), PALETTE.ok);
  assert.equal(gradientColor(0.6), PALETTE.warn);
  assert.equal(gradientColor(1), PALETTE.bad);
  assert.equal(gradientColor(-1), PALETTE.ok);
  assert.equal(gradientColor(2), PALETTE.bad);
  assert.match(gradientColor(0.3), /^#[0-9a-f]{6}$/);
});

test("a meter is filled with a glyph that differs from empty so colour is optional", () => {
  assert.equal(plainLine(meter(3, 4, 8, g, plain)), "██████░░");
  assert.equal(plainLine(meter(0, 4, 8, g, plain)), "░░░░░░░░");
  assert.equal(plainLine(meter(9, 4, 8, g, plain)), "████████");
  assert.equal(
    plainLine(meter(1, 0, 4, g, plain)),
    "████",
    "over a limit of zero is full",
  );
  assert.equal(plainLine(meter(0, 0, 4, g, plain)), "░░░░");
  assert.equal(
    plainLine(meter(1, 100, 4, g, plain)),
    "█░░░",
    "any use shows one cell",
  );
  assert.equal(plainLine(meter(3, 4, 8, ascii, plain)), "######..");
  const spans = meter(4, 4, 4, g, color);
  assert.equal(spans[0]!.color, PALETTE.ok);
  assert.equal(spans[3]!.color, PALETTE.bad);
});

test("a recency bar is full for fresh activity and empty for old", () => {
  assert.equal(plainLine(recencyBar(1, 8, g, plain)), "████████");
  assert.equal(plainLine(recencyBar(0, 8, g, plain)), "░░░░░░░░");
  assert.equal(plainLine(recencyBar(0.5, 8, g, plain)), "████░░░░");
});

test("a stacked bar is exactly as wide as asked with one cell at least per present part", () => {
  const bar = (
    parts: Array<[number, "good" | "progress" | "bad"]>,
    width: number,
  ) =>
    plainLine(
      stackedBar(
        parts.map(([count, kind]) => ({ count, kind })),
        width,
        g,
        plain,
      ),
    );
  assert.equal(
    bar(
      [
        [5, "good"],
        [1, "bad"],
      ],
      12,
    ),
    "██████████▒▒",
  );
  assert.equal(
    bar(
      [
        [3, "good"],
        [1, "progress"],
        [1, "bad"],
      ],
      14,
    ).length,
    14,
  );
  assert.equal(
    bar(
      [
        [100, "good"],
        [1, "bad"],
      ],
      6,
    ),
    "█████▒",
  );
  assert.equal(bar([], 6), "░░░░░░");
  assert.equal(bar([[0, "good"]], 4), "░░░░");
  assert.equal(
    bar(
      [
        [1, "good"],
        [1, "progress"],
        [1, "bad"],
      ],
      3,
    ),
    "█▓▒",
  );
});

test("a sparkline scales to its maximum, pads missing history and has an ASCII form", () => {
  assert.equal(sparkline([0, 4, 8], 5, 8, false), "  ▁▄█");
  assert.equal(sparkline([0, 8], 2, 8, true), "_#");
  assert.equal(sparkline([], 3, 0, false), "   ");
});

test("fitting a line cuts and pads to the width and keeps the styles of the surviving spans", () => {
  const line = fitLine(
    [
      { text: "abc", bold: true },
      { text: "defgh", color: "#fff" },
    ],
    6,
  );
  assert.equal(plainLine(line), "abcde…");
  assert.equal(line[0]!.bold, true);
  assert.equal(plainLine(fitLine([{ text: "ab" }], 5)), "ab   ");
  assert.equal(
    mergeSpans([{ text: "a" }, { text: "b" }, { text: "c", dim: true }]).length,
    2,
  );
  assert.deepEqual(plainLines([[{ text: "x" }]]), ["x"]);
});

test("locale detection: the first of LC_ALL, LC_CTYPE and LANG decides, and unset means UTF-8", () => {
  assert.equal(isUtf8Locale({}), true);
  assert.equal(isUtf8Locale({ LANG: "en_US.UTF-8" }), true);
  assert.equal(isUtf8Locale({ LANG: "en_US.utf8" }), true);
  assert.equal(isUtf8Locale({ LANG: "C" }), false);
  assert.equal(isUtf8Locale({ LANG: "POSIX" }), false);
  assert.equal(isUtf8Locale({ LC_ALL: "C", LANG: "en_US.UTF-8" }), false);
  assert.equal(
    isUtf8Locale({ LC_ALL: "", LC_CTYPE: "de_DE.UTF-8", LANG: "C" }),
    true,
  );
  assert.equal(wantsAscii({ TERM: "dumb" }), true);
  assert.equal(wantsAscii({ LANG: "C" }), true);
  assert.equal(
    wantsAscii({ TERM: "xterm-256color", LANG: "en_US.UTF-8" }),
    false,
  );
});

test("text wraps on word boundaries within the width and never overflows it", () => {
  const lines = wrapText(
    "Retry message 0192f4c1-3a7e-7b2d to developer-2 (state unacked)? Press y again to confirm.",
    30,
  );
  for (const line of lines) assert.ok(cells(line) <= 30, line);
  assert.equal(
    lines.join(" "),
    "Retry message 0192f4c1-3a7e-7b2d to developer-2 (state unacked)? Press y again to confirm.",
  );
  assert.equal(wrapText("a".repeat(50), 10)[0]!.length, 10);
  assert.deepEqual(wrapText("", 10), []);
});

test("footer hints depend on the focused panel and the arrow keys have an ASCII name", () => {
  const keys = (focus: "agents" | "queue" | "findings", glyphs = g) =>
    footerHints(focus, glyphs).map((h) => h.key);
  assert.ok(keys("queue").includes("y") && keys("queue").includes("f"));
  assert.ok(!keys("agents").includes("y") && keys("agents").includes("o"));
  assert.ok(!keys("findings").includes("o"));
  assert.ok(keys("queue").includes("-/+"));
  assert.ok(keys("queue", ascii).includes("updown"));
});
