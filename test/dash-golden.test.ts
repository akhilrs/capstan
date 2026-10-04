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
import { cellWidth } from "../src/dash/format.js";
import { NOW, crowded, showcase, supervisionState } from "./dash-fixtures.js";

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

test("supervision off in config reads as off, not as a fault", () => {
  const status = showcase({
    supervisionState: supervisionState({
      enabled: false,
      supervisor: null,
      lastCheck: null,
    }),
  });
  const lines = plainLines(
    buildFrame(modelOf(status), viewOf(100, 30), theme).lines,
  );
  assert.match(lines[1]!, /SUPERVISION OFF/);
  assert.match(lines[2]!, /enabled = false/);
  assert.match(lines[3]!, /supervision off/);
});

test("supervision on with a Supervisor running shows its state, the last check and open findings", () => {
  const status = showcase({
    supervisionState: supervisionState({ openFindings: 2 }),
  });
  const lines = plainLines(
    buildFrame(modelOf(status), viewOf(100, 30), theme).lines,
  );
  assert.match(lines[1]!, /SUPERVISED/);
  assert.doesNotMatch(lines[1]!, /OFF|DEGRADED/);
  const text = `${lines[1]!} ${lines[2]!}`;
  assert.match(text, /supervisor-1 active/);
  assert.match(text, /check acked 3m ago/);
  assert.match(text, /2 open findings/);
  assert.match(lines[3]!, /supervision on/);
});

test("supervision on with no Supervisor says it starts with the next worker", () => {
  const status = showcase({
    supervisionState: supervisionState({ supervisor: null, lastCheck: null }),
  });
  const lines = plainLines(
    buildFrame(modelOf(status), viewOf(100, 30), theme).lines,
  );
  assert.match(lines[1]!, /NO SUPERVISOR/);
  assert.doesNotMatch(lines[1]!, /OFF|DEGRADED/);
  assert.match(`${lines[1]!} ${lines[2]!}`, /starts with the next worker/);
  assert.match(lines[3]!, /supervision on/);
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
    supervisionState: supervisionState({
      supervisor: { agentId: "x".repeat(300), state: "active" },
    }),
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

const EDGES = new Set([..."│┃╮╯┓┛+|█#"]);

test("every line of every screen is exactly the terminal width and every panel line ends in a border", () => {
  const ascii = makeTheme({ noColor: true, reducedMotion: true, ascii: true });
  for (const status of [showcase(), crowded()])
    for (let columns = 60; columns <= 200; columns++)
      for (const rows of [16, 24, 36, 45]) {
        for (const t of [theme, ascii]) {
          const frame = buildFrame(
            modelOf(status),
            viewOf(columns, rows, { focus: "agents" }),
            t,
          );
          const lines = plainLines(frame.lines);
          assert.equal(lines.length, rows);
          lines.forEach((line, row) => {
            const where = `${columns}x${rows} row ${row}: ${line}`;
            assert.equal(cellWidth(line), columns, where);
            assert.equal(Array.from(line).length, columns, where);
            if (row < rows - 1)
              assert.ok(EDGES.has(line.at(-1)!), `no right edge, ${where}`);
          });
        }
      }
});

for (const [columns, rows] of [
  [80, 24],
  [118, 34],
  [160, 45],
] as const) {
  test(`the crowded run (1 active, 9 ended) at ${columns}x${rows} matches its golden file`, () => {
    const frame = buildFrame(
      modelOf(crowded(), 3),
      viewOf(columns, rows, { focus: "agents" }),
      theme,
    );
    matchesGolden(`dash-crowded-${columns}x${rows}`, plainLines(frame.lines));
  });
}

test("ended agents are listed after the active ones, newest first, with a count and a scroll position", () => {
  const text = plainLines(
    buildFrame(modelOf(crowded(), 3), viewOf(118, 34), theme).lines,
  ).join("\n");
  assert.ok(text.includes("1 active") && text.includes("9 ended"));
  const at = (name: string) => text.indexOf(`${name} `);
  assert.ok(
    at("pm-1") < at("reviewer-6") && at("reviewer-6") < at("reviewer-5"),
  );
  const small = plainLines(
    buildFrame(modelOf(crowded(), 3), viewOf(80, 24), theme).lines,
  ).join("\n");
  assert.match(small, /1-\d+\/7/, "hidden rows are shown as a range");
});

test("state, severity and reviewer words are never cut", () => {
  const text = plainLines(
    buildFrame(modelOf(crowded(), 3), viewOf(160, 45), theme).lines,
  ).join("\n");
  for (const word of ["confirmed", "medium", "reviewer-3 -> developer-1"])
    assert.ok(text.includes(word), word);
  assert.ok(!/confirm…|med…/.test(text));
  const sized = plainLines(
    buildFrame(
      modelOf(
        crowded({
          agentFindings: [
            {
              findingId: "f-critical",
              targetAgentId: "pm-1",
              severity: "critical",
              state: "open",
              interventions: 0,
              stateReason: "x",
            },
          ],
          integrations: [
            {
              integrationId: "i9",
              state: "conflicted",
              createdAt: "2026-10-02T11:59:00.000Z",
            },
          ],
        }),
        3,
      ),
      viewOf(118, 34),
      theme,
    ).lines,
  ).join("\n");
  assert.ok(sized.includes("critical") && sized.includes("conflicted"));
});

test("the selected detail area is hidden when the queue has nothing to select", () => {
  const empty = plainLines(
    buildFrame(modelOf(crowded(), 3), viewOf(118, 34), theme).lines,
  ).join("\n");
  assert.ok(!empty.includes("selected"));
  assert.ok(empty.includes("f problems only [ ]"));
  const full = plainLines(buildFrame(modelOf(), viewOf(118, 34), theme).lines);
  assert.ok(full.join("\n").includes("── selected"));
});

test("an escalated finding on an ended agent is dimmed, says so and is not counted as needing the operator", () => {
  const frame = buildFrame(modelOf(crowded(), 3), viewOf(160, 45), theme);
  const text = plainLines(frame.lines).join("\n");
  assert.ok(text.includes("1 target ended"));
  assert.ok(!text.includes("needs operator"));
  assert.ok(text.includes("ESCALATED 2/2 (target ended)"));
  const row = frame.lines.find((l) =>
    l.some((s) => s.text.includes("ESCALATED")),
  );
  assert.ok(row?.some((s) => s.dim === true));
});

test("stage bars are separated by blank rows when there is room, and use glyphs, not backgrounds", () => {
  const color = makeTheme({ noColor: false, reducedMotion: true });
  const frame = buildFrame(modelOf(crowded(), 3), viewOf(160, 45), color);
  const lines = plainLines(frame.lines);
  const at = (word: string) => lines.findIndex((l) => l.includes(word));
  const reports = at("reports  ");
  assert.ok(reports > 0);
  assert.ok(
    lines[reports + 1]!.slice(0, 80).replaceAll(/[│┃]/g, "").trim() === "",
  );
  assert.ok(lines[reports + 2]!.includes("reviews"));
  for (const line of frame.lines)
    for (const s of line)
      assert.equal(
        s.bg === undefined || s.bg === color.color("selectBg"),
        true,
      );
});

test("an empty worker meter is drawn with the empty glyph and no colour", () => {
  const plain = plainLines(
    buildFrame(modelOf(crowded(), 3), viewOf(118, 34), theme).lines,
  ).join("\n");
  assert.ok(
    plain.includes("workers ░░░░░░░░ 0/3") ||
      plain.includes("workers ░░░░░░░░░░░░ 0/3"),
  );
});

test("the findings panel keeps the reason and marks ended and unknown targets", () => {
  const finding = (id: string, target: string, state: string) => ({
    findingId: id,
    targetAgentId: target,
    severity: "low",
    state,
    interventions: 1,
    stateReason: "why-kept",
  });
  const text = plainLines(
    buildFrame(
      modelOf(
        crowded({
          agentFindings: [
            finding("f-esc", "designer-1", "escalated"),
            finding("f-open", "designer-1", "open"),
            finding("f-unk", "ghost-9", "open"),
          ],
        }),
        3,
      ),
      viewOf(160, 45),
      theme,
    ).lines,
  ).join("\n");
  assert.equal(text.match(/\(target ended\) why-kept/g)?.length, 2);
  assert.ok(text.includes("(target unknown) why-kept"));
  assert.ok(
    text.includes("2 target ended") && text.includes("1 target unknown"),
  );
});
