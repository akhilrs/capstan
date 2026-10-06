/**
 * Writes the fixtures the Rust dashboard is tested against: what this Node implementation computes for the cases in
 * dash-golden-cases.ts. Run `npm run build && node dist/test/dash-parity-export.js` after an intended change and commit
 * the result; dash-parity.test.ts fails while the committed files differ from a fresh export.
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import {
  MESSAGE_STATES,
  type ResolutionDecision,
} from "../src/controller/messaging.js";
import {
  ACTION_KEYS,
  availableDecisions,
  confirmText,
  observeAction,
  toWireCall,
  type DashAction,
} from "../src/dash/actions.js";
import {
  age,
  ageDetail,
  cellWidth,
  clean,
  commitShort,
  durationText,
  fit,
  sparkline,
  taskSummary,
  truncate,
} from "../src/dash/format.js";
import {
  EMPTY_CHANGES,
  PIPELINE_ITEMS,
  HIGHLIGHT_POLLS,
  RECENT_ENDED_SHOWN,
  TASK_NONE,
  WORKING_WINDOW_MS,
  buildDashModel,
  historySample,
  isWorkerKind,
  pipelineItems,
  queueCollapsed,
  queueRows,
  trackChanges,
  visibleAgents,
  type ChangeState,
  type DashModel,
} from "../src/dash/model.js";
import {
  confirmOverlay,
  helpOverlay,
  observeOverlay,
} from "../src/dash/overlays.js";
import { BACKOFF_MS, nextDelayMs, stepInterval } from "../src/dash/poller.js";
import { buildFrame, footerHints, visiblePanels } from "../src/dash/view.js";
import { glyphsFor } from "../src/dash/glyphs.js";
import { PALETTE, gradientColor } from "../src/dash/theme.js";
import { NOW, healthy, message } from "./dash-fixtures.js";
import {
  CASES,
  OBSERVE_PEEK,
  THEMES,
  viewStateOf,
  type ParityCase,
} from "./dash-golden-cases.js";

const root = path.resolve(import.meta.dirname, "..", "..");
export const PARITY_DIRECTORY = path.join(root, "dash", "tests", "parity");

function changesJson(state: ChangeState) {
  return {
    fingerprints: [...state.fingerprints],
    highlight: [...state.highlight],
  };
}

/** The model after a poll in which the first agent changed and the first message resolved. */
export function nextPoll(model: DashModel): DashModel {
  return {
    ...model,
    agents: model.agents.map((a, i) =>
      i === 0 ? { ...a, fingerprint: `${a.fingerprint}x` } : a,
    ),
    queue: { ...model.queue, messages: model.queue.messages.slice(1) },
  };
}

function helpersOf(model: DashModel) {
  const first = trackChanges(EMPTY_CHANGES, model);
  const changed = nextPoll(model);
  const second = trackChanges(first, changed);
  const third = trackChanges(second, changed);
  const fourth = trackChanges(third, changed);
  const ids = (rows: readonly { readonly id: string }[]) =>
    rows.map((r) => r.id);
  return {
    visibleAgents: {
      recent: ids(visibleAgents(model.agents, false)),
      all: ids(visibleAgents(model.agents, true)),
    },
    queueRows: {
      all: ids(queueRows(model, false)),
      problems: ids(queueRows(model, true)),
    },
    queueCollapsed: queueCollapsed(model),
    pipelineItems: ids(pipelineItems(model)),
    historySample: historySample(model, NOW),
    historySampleLater: historySample(model, NOW + 90_500),
    trackChanges: [first, second, third, fourth].map(changesJson),
    visiblePanels: visiblePanels(model),
  };
}

function actionsOf(model: DashModel): DashAction[] {
  const actions: DashAction[] = [];
  for (const message of model.queue.messages.slice(0, 2))
    for (const decision of availableDecisions(message))
      actions.push({ kind: "resolve", decision, message });
  const agent = model.agents.find((a) => a.state === "active");
  const observe = agent === undefined ? undefined : observeAction(agent);
  if (observe !== undefined) actions.push(observe);
  return actions;
}

function exportCase(c: ParityCase) {
  const model = buildDashModel(c.status, NOW, c.workerLimit);
  return {
    name: c.name,
    status: c.status,
    nowMs: NOW,
    workerLimit: c.workerLimit,
    model,
    helpers: helpersOf(model),
    views: c.views.map((spec) => {
      const theme = THEMES[spec.theme];
      const view = viewStateOf(spec);
      const size = view.size;
      return {
        view: { ...view, highlight: [...view.highlight] },
        theme,
        frame: buildFrame(model, view, theme),
        overlays: {
          help: helpOverlay(size, theme),
          confirm: actionsOf(model).map((action) => ({
            action,
            overlay: confirmOverlay(action, size, theme),
          })),
          observe: observeOverlay(OBSERVE_PEEK, size, theme),
        },
      };
    }),
  };
}

function formatFixture() {
  const texts = [
    "",
    "plain",
    "a b c d e f g h i j",
    "wide 日本語テキスト",
    "emoji 😀 ok",
    "tab\there\u0007bell",
    "zero​width line",
    "combining é mark",
    "a-very-long-agent-identifier-with-many-parts-and-more",
  ];
  const nowMs = NOW;
  const times = [
    null,
    "",
    "not a date",
    new Date(NOW).toISOString(),
    new Date(NOW - 59_000).toISOString(),
    new Date(NOW - 61_000).toISOString(),
    new Date(NOW - 3_599_000).toISOString(),
    new Date(NOW - 3_700_000).toISOString(),
    new Date(NOW - 86_399_000).toISOString(),
    new Date(NOW - 200_000_000).toISOString(),
    new Date(NOW + 5_000).toISOString(),
  ];
  const tasks = [
    null,
    [],
    [{ id: "plan-1", label: "plan-1 One (1/2)" }],
    [
      { id: "plan-1", label: "plan-1 One (1/2)" },
      { id: "PM-9", label: "PM-9 Nine" },
      { id: "plan-3", label: "plan-3 Three (0/4)" },
    ],
  ];
  return {
    cellWidth: texts.map((text) => ({ text, out: cellWidth(text) })),
    truncate: texts.flatMap((text) =>
      [0, 1, 2, 5, 12, 30].map((width) => ({
        text,
        width,
        out: truncate(text, width),
      })),
    ),
    fit: texts.flatMap((text) =>
      [0, 1, 5, 12, 30].map((width) => ({
        text,
        width,
        out: fit(text, width),
      })),
    ),
    clean: [...texts, "­‎﻿x y"].map((text) => ({
      text,
      out: clean(text),
    })),
    age: times.map((iso) => ({ iso, nowMs, out: age(iso, nowMs) })),
    ageDetail: times.map((iso) => ({ iso, nowMs, out: ageDetail(iso, nowMs) })),
    durationText: [
      0, 1, 59, 60, 61, 599, 600, 3599, 3600, 3661, 86_399, 86_400, 200_000,
    ].map((seconds) => ({ seconds, out: durationText(seconds) })),
    commitShort: [null, "", "abc", "0123456789abcdef"].map((sha) => ({
      sha,
      out: commitShort(sha),
    })),
    sparkline: [
      [[], 5],
      [[0, 0, 0], 5],
      [[1, 2, 3, 4, 5, 6, 7, 8], 8],
      [[1, 2, 3, 4, 5, 6, 7, 8], 4],
      [[0, 5, 10, 2.5], 10],
    ].map(([values, width]) => ({
      values,
      width,
      out: sparkline(values as number[], width as number),
    })),
    taskSummary: tasks.flatMap((list) =>
      [0, 8, 14, 30, 60, 120].map((width) => ({
        tasks: list,
        width,
        out: taskSummary(list, width),
      })),
    ),
    isWorkerKind: ["PM", "Supervisor", "Developer", "Architect", ""].map(
      (kind) => ({ kind, out: isWorkerKind(kind) }),
    ),
    constants: {
      workingWindowMs: WORKING_WINDOW_MS,
      pipelineItems: PIPELINE_ITEMS,
      highlightPolls: HIGHLIGHT_POLLS,
      recentEndedShown: RECENT_ENDED_SHOWN,
      taskNone: TASK_NONE,
    },
    theme: {
      palette: PALETTE,
      gradient: [0, 0.1, 0.3, 0.6, 0.8, 1, -1, 2].map((fraction) => ({
        fraction,
        out: gradientColor(fraction),
      })),
    },
    hints: (
      ["agents", "pipeline", "queue", "findings", "work"] as const
    ).flatMap((focus) =>
      [false, true].map((ascii) => ({
        focus,
        ascii,
        out: footerHints(focus, glyphsFor(ascii)),
      })),
    ),
  };
}

function actionsFixture() {
  const rows = MESSAGE_STATES.flatMap((state) =>
    [true, false].map((active) => ({
      ...buildDashModel(
        healthy({ messages: [message("0192abcd-0000", state)] }),
        NOW,
        3,
      ).queue.messages[0]!,
      recipientActive: active,
    })),
  );
  const decisions: ResolutionDecision[] = ["retry", "skip", "cancel"];
  const model = buildDashModel(
    healthy({ messages: [message("0192abcd-0000", "sent")] }),
    NOW,
    3,
  );
  const agents = [
    ...model.agents,
    { ...model.agents[0]!, state: "ended" },
    { ...model.agents[0]!, agentId: "odd\u0007agent​id" },
  ];
  const resolves: DashAction[] = rows.flatMap((message) =>
    decisions.map((decision) => ({ kind: "resolve", decision, message })),
  );
  const observes: DashAction[] = agents.flatMap((agent) => {
    const action = observeAction(agent);
    return action === undefined ? [] : [action];
  });
  return {
    keys: ACTION_KEYS,
    available: rows.map((message) => ({
      message,
      out: availableDecisions(message),
    })),
    observe: agents.map((agent) => ({
      agent,
      out: observeAction(agent) ?? null,
    })),
    confirm: [...resolves, ...observes].map((action) => ({
      action,
      out: confirmText(action),
    })),
    wire: [...resolves, ...observes].map((action) => ({
      action,
      out: toWireCall(action),
    })),
  };
}

function pollerFixture() {
  return {
    backoffMs: BACKOFF_MS,
    nextDelayMs: [0, 1, 2, 3, 4, 9].flatMap((failures) =>
      [1000, 2000, 10_000].map((intervalMs) => ({
        failures,
        intervalMs,
        out: nextDelayMs(failures, intervalMs),
      })),
    ),
    stepInterval: [1, 2, 5, 9, 10, 11, 15, 20, 55, 60].flatMap((seconds) =>
      [true, false].map((faster) => ({
        seconds,
        faster,
        out: stepInterval(seconds, faster),
      })),
    ),
  };
}

/** Every fixture file by name, as the text the exporter writes. */
export function exportFixtures(): Map<string, string> {
  const files = new Map<string, string>();
  for (const c of CASES)
    files.set(`${c.name}.json`, JSON.stringify(exportCase(c)) + "\n");
  files.set("format.json", JSON.stringify(formatFixture()) + "\n");
  files.set("actions.json", JSON.stringify(actionsFixture()) + "\n");
  files.set("poller.json", JSON.stringify(pollerFixture()) + "\n");
  return files;
}

if (import.meta.filename === process.argv[1]) {
  mkdirSync(PARITY_DIRECTORY, { recursive: true });
  const files = exportFixtures();
  for (const [name, text] of files)
    writeFileSync(path.join(PARITY_DIRECTORY, name), text);
  process.stdout.write(`wrote ${files.size} fixtures to ${PARITY_DIRECTORY}\n`);
}
