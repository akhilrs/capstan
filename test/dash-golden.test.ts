import assert from "node:assert/strict";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { plainLines } from "../src/dash/lines.js";
import {
  confirmOverlay,
  helpOverlay,
  observeOverlay,
  type Overlay,
} from "../src/dash/overlays.js";
import { makeTheme } from "../src/dash/theme.js";
import { buildFrame, EMPTY_RINGS } from "../src/dash/view.js";
import { modelOf, viewOf } from "./dash-view-helpers.js";
import { NOW, showcase } from "./dash-fixtures.js";

const root = path.resolve(import.meta.dirname, "..", "..");
const goldenDirectory = path.join(root, "test", "golden");

/** Set UPDATE_GOLDEN=1 to rewrite the files after an intended visual change, then read the diff. */
function matchesGolden(name: string, lines: readonly string[]): void {
  const file = path.join(goldenDirectory, `${name}.txt`);
  const text = `${lines.join("\n")}\n`;
  if (process.env.UPDATE_GOLDEN === "1") {
    mkdirSync(goldenDirectory, { recursive: true });
    writeFileSync(file, text);
    return;
  }
  assert.ok(existsSync(file), `${file} is missing; run with UPDATE_GOLDEN=1`);
  assert.equal(text, readFileSync(file, "utf8"), name);
}

const theme = makeTheme({ noColor: true, reducedMotion: true });

function overlaid(base: readonly string[], overlay: Overlay): string[] {
  const out = base.map((l) => Array.from(l));
  overlay.lines.forEach((line, r) => {
    const row = out[overlay.top + r];
    if (row === undefined) return;
    const cells = Array.from(line.map((s) => s.text).join(""));
    cells.forEach((ch, c) => {
      row[overlay.left + c] = ch;
    });
  });
  return out.map((r) => r.join(""));
}

for (const [columns, rows] of [
  [80, 24],
  [120, 36],
  [160, 45],
] as const) {
  test(`the ${columns}x${rows} screen matches its golden file and fills the terminal exactly`, () => {
    const frame = buildFrame(modelOf(), viewOf(columns, rows), theme);
    const lines = plainLines(frame.lines);
    assert.equal(lines.length, rows);
    for (const line of lines)
      assert.equal(Array.from(line).length, columns, line);
    matchesGolden(`dash-${columns}x${rows}`, lines);
  });
}

test("the same screens in ASCII mode use only ASCII characters", () => {
  const ascii = makeTheme({ noColor: true, reducedMotion: true, ascii: true });
  for (const [columns, rows] of [
    [80, 24],
    [120, 36],
    [160, 45],
  ] as const) {
    const lines = plainLines(
      buildFrame(modelOf(), viewOf(columns, rows), ascii).lines,
    );
    for (const line of lines) {
      assert.equal(Array.from(line).length, columns);
      assert.match(line, /^[\x20-\x7e]*$/, line);
    }
  }
  matchesGolden(
    "dash-120x36-ascii",
    plainLines(buildFrame(modelOf(), viewOf(120, 36), ascii).lines),
  );
});

test("the overlays match their golden files at 120x36", () => {
  const size = { columns: 120, rows: 36 };
  const base = plainLines(buildFrame(modelOf(), viewOf(120, 36), theme).lines);
  const model = modelOf();
  const message = model.queue.messages[0]!;
  matchesGolden("dash-help", overlaid(base, helpOverlay(size, theme)));
  matchesGolden(
    "dash-confirm-retry",
    overlaid(
      base,
      confirmOverlay(
        { kind: "resolve", decision: "retry", message },
        size,
        theme,
      ),
    ),
  );
  matchesGolden(
    "dash-observe",
    overlaid(
      base,
      observeOverlay(
        {
          agentId: "developer-2",
          agentStatus: "blocked",
          text: "developer-2 pane p4\n\nReading docs/spike-herdr-agents.md\nEdit(src/dash/theme.ts)\n  Updated src/dash/theme.ts with 12 additions\n\nThinking... (3m 41s, esc to interrupt)\n\n  waiting for tool permission:\n  Bash(npm run check)\n  1. Yes   2. No\n",
        },
        size,
        theme,
      ),
    ),
  );
});

test("a degraded run without a link wraps its reason, dims the stale frame and keeps the last good data", () => {
  const frame = buildFrame(
    modelOf(),
    viewOf(100, 30, { link: "down", linkAge: "12s", rings: EMPTY_RINGS }),
    theme,
  );
  const lines = plainLines(frame.lines);
  assert.match(lines[1]!, /NO LINK/);
  assert.match(lines[2]!, /controller not answering/);
  matchesGolden("dash-100x30-no-link", lines);
});

test("a fresh project with supervision off does not look like a fault", () => {
  const status = showcase({
    supervision: {
      enabled: false,
      health: "degraded",
      targetEpoch: 0,
      checkpointEpoch: null,
      checkpointAssignmentId: null,
      replacementAttempts: 0,
    },
    supervisionReason: null,
  });
  const lines = plainLines(
    buildFrame(modelOf(status), viewOf(100, 30), theme).lines,
  );
  assert.match(lines[1]!, /SUPERVISION OFF/);
  assert.doesNotMatch(lines[1]!, /DEGRADED/);
  assert.match(lines[2]!, /not a fault/);
  assert.match(lines[3]!, /supervision off/);
  const stale = showcase({
    supervision: {
      enabled: false,
      health: "degraded",
      targetEpoch: 3,
      checkpointEpoch: null,
      checkpointAssignmentId: null,
      replacementAttempts: 1,
    },
    supervisionReason: "lost contact with developer-2",
  });
  const withReason = plainLines(
    buildFrame(modelOf(stale), viewOf(100, 30), theme).lines,
  );
  assert.match(withReason[1]!, /DEGRADED/);
  assert.match(withReason[1]!, /lost contact with developer-2/);
  assert.doesNotMatch(
    withReason[2]!,
    /epoch/,
    "epochs mean nothing while supervision is off",
  );
  const catchingUp = showcase({
    supervision: {
      enabled: true,
      health: "evaluating",
      targetEpoch: 5,
      checkpointEpoch: 4,
      checkpointAssignmentId: null,
      replacementAttempts: 0,
    },
    supervisionReason: null,
  });
  const live = plainLines(
    buildFrame(modelOf(catchingUp), viewOf(100, 30), theme).lines,
  );
  assert.match(live[2]!, /epoch 4\/5/);
  assert.match(withReason[2]!, /replacements 1/);
});

test("the colour build paints the frame with state colours and the NO_COLOR build with none", () => {
  const colored = buildFrame(
    modelOf(),
    viewOf(120, 36),
    makeTheme({ noColor: false, reducedMotion: false }),
  );
  const spans = colored.lines.flat();
  assert.ok(spans.some((s) => s.color === "#dc4c4c"));
  assert.ok(spans.some((s) => s.bg === "#3b4252"));
  assert.ok(spans.some((s) => s.color === "#77ca9b"));
  const plain = buildFrame(modelOf(), viewOf(120, 36), theme).lines.flat();
  assert.ok(plain.every((s) => s.color === undefined && s.bg === undefined));
  assert.ok(
    plain.some((s) => s.bold === true),
    "bold still marks the selected row",
  );
});

test("the stale frame is dimmed when the link is down and the focused queue box is heavy-lined", () => {
  const down = buildFrame(
    modelOf(),
    viewOf(120, 36, { link: "down" }),
    theme,
  ).lines;
  assert.ok(down[10]!.every((s) => s.dim === true || s.text === ""));
  const up = buildFrame(modelOf(), viewOf(120, 36), theme);
  assert.ok(
    up.lines[3]!.map((s) => s.text)
      .join("")
      .includes("┏━³queue"),
  );
  assert.deepEqual(up.shown, ["agents", "pipeline", "queue", "findings"]);
});

test("reduced motion shows a static marker for working agents and the spinner animates otherwise", () => {
  const animated = makeTheme({ noColor: true, reducedMotion: false });
  const frame = (tick: number) =>
    plainLines(
      buildFrame(modelOf(), viewOf(120, 36, { tick }), animated).lines,
    ).join("\n");
  assert.ok(frame(0).includes("⠋ developer-1"));
  assert.ok(frame(1).includes("⠙ developer-1"));
  assert.ok(frame(0).includes("⠋ designer-1"));
  const still = plainLines(
    buildFrame(modelOf(), viewOf(120, 36), theme).lines,
  ).join("\n");
  assert.ok(still.includes("* developer-1"));
  assert.ok(!/[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]/.test(still.split("\n").slice(0, 12).join("\n")));
});

test("a changed row is marked with + under reduced motion and bold otherwise", () => {
  const highlight = new Map([["m:0192f510-0b4e-79aa", 2]]);
  const still = plainLines(
    buildFrame(modelOf(), viewOf(120, 36, { highlight }), theme).lines,
  );
  assert.ok(still.some((l) => l.includes("+●   #44")));
});

test("long text cut by the width never changes the frame size, even at the smallest size", () => {
  const long = showcase({
    supervisionReason: "x".repeat(300),
    agents: Array.from({ length: 30 }, (_, i) => ({
      agentId: `a-very-long-agent-identifier-${i}`,
      roleName: "dev",
      kind: "Developer",
      generation: 1,
      state: "active",
      lastActivityAt: new Date(NOW - i * 1000).toISOString(),
    })),
  });
  for (const [columns, rows] of [
    [60, 16],
    [61, 17],
    [99, 20],
    [100, 20],
    [200, 60],
  ] as const) {
    const lines = plainLines(
      buildFrame(modelOf(long), viewOf(columns, rows), theme).lines,
    );
    assert.equal(lines.length, rows, `${columns}x${rows}`);
    for (const line of lines)
      assert.equal(Array.from(line).length, columns, `${columns}x${rows}`);
  }
});
