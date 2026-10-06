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
import {
  NOW,
  crowded,
  iso,
  showcase,
  supervisionState,
} from "./dash-fixtures.js";

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
  assert.doesNotMatch(lines.join("\n"), /supervision off/);
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
  assert.doesNotMatch(lines.join("\n"), /supervision on/);
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
  assert.ok(text.includes("1 stale (target ended)"));
  assert.ok(!text.includes("needs operator"));
  assert.ok(text.includes("escalated 2/2 (target ended)"));
  assert.ok(!text.includes("ESCALATED"));
  const row = frame.lines.find((l) =>
    l.some((s) => s.text.includes("escalated 2/2")),
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
    lines[reports + 1]!.slice(80).replaceAll(/[│┃]/g, "").trim() === "",
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
  assert.ok(text.includes("3 stale (target ended)"));
});

const staleMail = {
  ...showcase(),
  pmMail: { pending: 3, oldestAgeSeconds: 780, stale: true },
};

for (const [columns, rows] of [
  [80, 24],
  [120, 36],
] as const) {
  test(`stale PM mail adds a banner at ${columns}x${rows}, readable without colour`, () => {
    const frame = buildFrame(modelOf(staleMail), viewOf(columns, rows), theme);
    const lines = plainLines(frame.lines);
    assert.equal(lines.length, rows);
    for (const line of lines)
      assert.equal(Array.from(line).length, columns, line);
    assert.match(lines[0]!, /PM MAIL STALE: 3 messages pending, oldest 13 min/);
    matchesGolden(`dash-stale-mail-${columns}x${rows}`, lines);
  });

  test(`stale PM mail banner at ${columns}x${rows} is coloured and bold with colour on, and absent when not stale`, () => {
    const coloured = makeTheme({ noColor: false, reducedMotion: true });
    const frame = buildFrame(
      modelOf(staleMail),
      viewOf(columns, rows),
      coloured,
    );
    assert.ok(
      frame.lines[0]!.some((s) => s.bold === true && s.color !== undefined),
    );
    const calm = buildFrame(
      modelOf({
        ...staleMail,
        pmMail: { pending: 1, oldestAgeSeconds: 30, stale: false },
      }),
      viewOf(columns, rows),
      theme,
    );
    assert.deepEqual(
      plainLines(calm.lines),
      plainLines(buildFrame(modelOf(), viewOf(columns, rows), theme).lines),
    );
  });
}

// ------------------------------------------------- queue collapse, totals, header, stale, strip

const emptyQueue = () => showcase({ messages: [], stuck: [] });
const colour = makeTheme({ noColor: false, reducedMotion: true });
const queueTop = (lines: readonly string[]) =>
  lines.findIndex((l) => l.includes("queue"));

for (const [columns, rows] of [
  [80, 24],
  [100, 30],
  [120, 36],
  [160, 45],
] as const) {
  test(`an empty queue is three rows at ${columns}x${rows}, also when focused`, () => {
    for (const focus of ["queue", "agents"] as const) {
      const frame = buildFrame(
        modelOf(emptyQueue()),
        viewOf(columns, rows, { focus }),
        theme,
      );
      const lines = plainLines(frame.lines);
      assert.equal(lines.length, rows);
      const top = queueTop(lines);
      assert.ok(top > 0);
      const span =
        columns >= 100 ? lines.slice(top, top + 3) : lines.slice(top);
      assert.match(span[1]!, /no unresolved messages/);
      assert.match(span[1]!, /max 6 since/);
      assert.match(span[2]!, /[╰┗]/);
      assert.ok(frame.shown.includes("queue"));
      assert.deepEqual(frame.shown, [
        "agents",
        "pipeline",
        "queue",
        "findings",
      ]);
    }
  });
}

test("a collapsed queue shows no max when the queue was never used, and the f filter with messages does not collapse it", () => {
  const quiet = plainLines(
    buildFrame(
      modelOf(emptyQueue()),
      viewOf(120, 36, { rings: EMPTY_RINGS }),
      theme,
    ).lines,
  ).join("\n");
  assert.match(
    quiet,
    /no unresolved messages \s*│|no unresolved messages\s+[│┃]/,
  );
  assert.doesNotMatch(quiet, /no unresolved messages.*max/);
  const filtered = plainLines(
    buildFrame(
      modelOf(
        showcase({
          messages: [
            {
              messageId: "m1",
              recipientAgentId: "developer-1",
              state: "sent",
              sequence: 1,
              queuedAt: iso(5),
              deferredReason: null,
              stateReason: null,
              lastNotifiedAt: iso(5),
            },
          ],
          stuck: [],
        }),
      ),
      viewOf(120, 36, { problemsOnly: true }),
      theme,
    ).lines,
  ).join("\n");
  assert.ok(filtered.includes("no delivery problems"));
  assert.ok(filtered.includes("── selected") === false);
});

test("a collapsed queue hands its rows to agents and the right column, wide and narrow", () => {
  const wide = plainLines(
    buildFrame(modelOf(emptyQueue()), viewOf(160, 45), theme).lines,
  );
  const rowOf = (lines: readonly string[], word: string, from: number) =>
    lines.findIndex((l) => l.slice(from).includes(word));
  assert.ok(rowOf(wide, "queue", 80) < rowOf(wide, "pipeline", 80));
  assert.ok(rowOf(wide, "pipeline", 80) < rowOf(wide, "findings", 80));
  assert.equal(rowOf(wide, "pipeline", 0), rowOf(wide, "pipeline", 80));
  const narrow = plainLines(
    buildFrame(modelOf(emptyQueue()), viewOf(80, 36), theme).lines,
  );
  const open = plainLines(buildFrame(modelOf(), viewOf(80, 36), theme).lines);
  const height = (lines: readonly string[], word: string) =>
    lines.findIndex((l) => l.includes("pipeline")) >= 0 ? word : "";
  assert.ok(height(narrow, "x") === "x" && height(open, "x") === "x");
  const agents = (lines: readonly string[]) =>
    lines.findIndex((l) => l.includes("pipeline")) -
    lines.findIndex((l) => l.includes("agents"));
  assert.ok(agents(narrow) >= agents(open));
});

test("pipeline totals read the true counts with no 20+, and fall back to the capped text without them", () => {
  const counted = showcase({
    pipelineCounts: {
      reports: { accepted: 50, rejected: 10 },
      reviews: { passed: 40, findings: 5 },
      integrations: { confirmed: 20, merged: 1 },
    },
  });
  for (const [columns, rows] of [
    [120, 36],
    [160, 45],
  ] as const) {
    const text = plainLines(
      buildFrame(modelOf(counted), viewOf(columns, rows), theme).lines,
    ).join("\n");
    assert.match(
      text,
      /reported 60 +[─-]+[►>] +review 45 +[─-]+[►>] +integrated 21/,
    );
    assert.ok(!text.includes("20+"));
    assert.match(text, /accepted 50/);
  }
  const twenty = Array.from({ length: 20 }, (_, i) => ({
    reportId: `r${i}`,
    agentId: "developer-1",
    commitSha: "a".repeat(40),
    state: "accepted",
    createdAt: iso(i),
  }));
  const old = plainLines(
    buildFrame(modelOf(showcase({ reports: twenty })), viewOf(120, 36), theme)
      .lines,
  ).join("\n");
  assert.match(old, /reported 20\+/);
});

test("supervision on with no Supervisor: idle (dim) with no worker, NO SUPERVISOR (warn) with one", () => {
  const idle = showcase({
    agents: [],
    panes: [],
    supervisionState: supervisionState({
      supervisor: null,
      lastCheck: null,
      openFindings: 2,
    }),
  });
  const chipOf = (status: Record<string, unknown>, text: string) => {
    const frame = buildFrame(modelOf(status), viewOf(120, 36), colour);
    const found = frame.lines[1]!.find((s) => s.text.includes(text));
    assert.ok(found, text);
    return found;
  };
  const dim = chipOf(idle, "SUPERVISION IDLE");
  assert.equal(dim.color, colour.color("dim"));
  assert.notEqual(dim.color, colour.color("warn"));
  const lines = plainLines(
    buildFrame(modelOf(idle), viewOf(120, 36), theme).lines,
  );
  assert.match(lines[1]!, /starts with the next worker; 2 open findings/);
  assert.ok(!lines.join("\n").includes("NO SUPERVISOR"));
  const warn = chipOf(
    showcase({
      supervisionState: supervisionState({ supervisor: null, lastCheck: null }),
    }),
    "NO SUPERVISOR",
  );
  assert.equal(warn.color, colour.color("warn"));
  const border = lines[3]!;
  assert.ok(!/supervision (on|off)/.test(border), border);
});

const staleFinding = (id: string, target: string, state = "escalated") => ({
  findingId: id,
  targetAgentId: target,
  severity: "high",
  state,
  interventions: 2,
  stateReason: "no progress",
});

const staleStatus = () =>
  showcase({
    messages: [],
    stuck: [],
    agentFindings: [
      staleFinding("0192a1-aaaa", "developer-0"),
      staleFinding("0192a2-bbbb", "ghost-9", "open"),
      staleFinding("0192a3-cccc", "developer-2"),
    ],
  });

test("stale findings are dim, plain, sorted last under a caption, and do not count as needing the operator", () => {
  const frame = buildFrame(modelOf(staleStatus()), viewOf(160, 45), colour);
  const lines = plainLines(frame.lines);
  const text = lines.join("\n");
  const live = lines.findIndex((l) => l.includes("ESCALATED"));
  const caption = lines.findIndex((l) => l.includes("2 stale (target ended)"));
  const first = lines.findIndex((l) => l.includes("0192a1"));
  assert.ok(live > 0 && live < caption && caption < first);
  assert.ok(text.includes("1 needs operator"));
  const dimRows = frame.lines.filter((l) =>
    l.some((s) => s.text.includes("0192a1") || s.text.includes("0192a2")),
  );
  assert.equal(dimRows.length, 2);
  for (const row of dimRows)
    for (const s of row.filter((x) => x.text.trim() !== ""))
      if (s.color !== undefined)
        assert.ok(
          ![colour.color("bad"), colour.color("warn")].includes(s.color),
          s.text,
        );
  for (const row of dimRows.concat(
    frame.lines.filter((l) => l.some((s) => s.text.includes("2 stale"))),
  ))
    assert.ok(
      row.every(
        (s) => s.dim === true || s.text.trim() === "" || s.text.includes("│"),
      ) || row.some((s) => s.dim === true),
    );
  assert.ok(text.includes("escalated 2/2 (target ended)"));
});

test("with no room for stale rows the caption alone summarises them, and live findings keep their space", () => {
  const status = staleStatus();
  const tight = plainLines(
    buildFrame(modelOf(status), viewOf(100, 24, { focus: "agents" }), theme)
      .lines,
  ).join("\n");
  assert.ok(tight.includes("2 stale (target ended)"));
  assert.ok(tight.includes("developer-2"));
  const model = modelOf(status);
  assert.equal(model.findings.filter((f) => !f.stale).length, 1);
});

test("the golden screens for an empty queue, a waiting strip, stale findings and idle supervision", () => {
  const waiting = showcase({
    awaitingConfirm: [
      {
        integrationId: "0123456789abcdef",
        branch: "integration/x",
        createdAt: iso(200),
        reviewState: "passed",
      },
    ],
    pendingProposals: [
      {
        proposalId: "p-7",
        kind: "add-role",
        proposer: "pm-1",
        reason: "need a tester",
        createdAt: iso(40),
      },
    ],
    pause: {
      run: { pausedAt: iso(300), reason: "operator break", actorId: "op" },
      agents: [
        { agentId: "developer-2", pausedAt: iso(90), reason: "stalled" },
      ],
    },
  });
  const idle = emptyQueue();
  idle.agents = [];
  idle.panes = [];
  idle.supervisionState = supervisionState({
    supervisor: null,
    lastCheck: null,
    openFindings: 2,
  });
  for (const [columns, rows] of [
    [80, 24],
    [120, 36],
  ] as const) {
    matchesGolden(
      `dash-empty-queue-${columns}x${rows}`,
      plainLines(
        buildFrame(modelOf(emptyQueue()), viewOf(columns, rows), theme).lines,
      ),
    );
  }
  matchesGolden(
    "dash-waiting-120x36",
    plainLines(buildFrame(modelOf(waiting), viewOf(120, 36), theme).lines),
  );
  matchesGolden(
    "dash-waiting-80x24",
    plainLines(buildFrame(modelOf(waiting), viewOf(80, 24), theme).lines),
  );
  matchesGolden(
    "dash-stale-findings-120x36",
    plainLines(
      buildFrame(modelOf(staleStatus()), viewOf(120, 36), theme).lines,
    ),
  );
  matchesGolden(
    "dash-supervision-idle-120x36",
    plainLines(buildFrame(modelOf(idle), viewOf(120, 36), theme).lines),
  );
});
