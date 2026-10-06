/** The dashboard's golden and parity cases as data: one list drives the golden test and the Rust parity export. */
import { readFileSync } from "node:fs";
import path from "node:path";
import { plainLines } from "../src/dash/lines.js";
import {
  confirmOverlay,
  helpOverlay,
  observeOverlay,
  type Overlay,
  type PeekView,
} from "../src/dash/overlays.js";
import { makeTheme, type Theme } from "../src/dash/theme.js";
import { buildFrame, EMPTY_RINGS, type ViewState } from "../src/dash/view.js";
import {
  NOW,
  crowded,
  iso,
  showcase,
  showcaseRings,
  supervisionState,
  tasksShowcase,
  type Status,
} from "./dash-fixtures.js";
import { modelOf, viewOf } from "./dash-view-helpers.js";

const root = path.resolve(import.meta.dirname, "..", "..");

export type ThemeKind = "plain" | "ascii" | "color" | "motion";

export const THEMES: Readonly<Record<ThemeKind, Theme>> = {
  plain: makeTheme({ noColor: true, reducedMotion: true }),
  ascii: makeTheme({ noColor: true, reducedMotion: true, ascii: true }),
  color: makeTheme({ noColor: false, reducedMotion: false }),
  motion: makeTheme({ noColor: true, reducedMotion: false }),
};

/** The view state fields a case changes; `highlight` is a list of pairs because it must survive JSON. */
export interface ViewOverrides {
  readonly focus?: ViewState["focus"];
  readonly selected?: ViewState["selected"];
  readonly problemsOnly?: boolean;
  readonly showAllEnded?: boolean;
  readonly paused?: boolean;
  readonly link?: ViewState["link"];
  readonly linkAge?: string;
  readonly tick?: number;
  readonly notice?: string | null;
  readonly rings?: "showcase" | "empty";
  readonly highlight?: readonly (readonly [string, number])[];
}

export interface ViewSpec {
  readonly columns: number;
  readonly rows: number;
  readonly theme: ThemeKind;
  readonly overrides?: ViewOverrides;
}

export interface ParityCase {
  readonly name: string;
  readonly status: Status;
  readonly workerLimit: number | null;
  readonly views: readonly ViewSpec[];
}

export function viewStateOf(spec: ViewSpec): ViewState {
  const { rings, highlight, ...rest } = spec.overrides ?? {};
  return viewOf(spec.columns, spec.rows, {
    ...rest,
    ...(rings === "empty" ? { rings: EMPTY_RINGS } : {}),
    ...(highlight === undefined ? {} : { highlight: new Map(highlight) }),
  });
}

export const staleMail = (): Status => ({
  ...showcase(),
  pmMail: { pending: 3, oldestAgeSeconds: 780, stale: true },
});

export const emptyQueue = (): Status => showcase({ messages: [], stuck: [] });

export const waitingStatus = (): Status =>
  showcase({
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

export const staleFinding = (
  id: string,
  target: string,
  state = "escalated",
) => ({
  findingId: id,
  targetAgentId: target,
  severity: "high",
  state,
  interventions: 2,
  stateReason: "no progress",
});

export const staleStatus = (): Status =>
  showcase({
    messages: [],
    stuck: [],
    agentFindings: [
      staleFinding("0192a1-aaaa", "developer-0"),
      staleFinding("0192a2-bbbb", "ghost-9", "open"),
      staleFinding("0192a3-cccc", "developer-2"),
    ],
  });

export const idleStatus = (): Status => {
  const idle = emptyQueue();
  idle.agents = [];
  idle.panes = [];
  idle.supervisionState = supervisionState({
    supervisor: null,
    lastCheck: null,
    openFindings: 2,
  });
  return idle;
};

export const pausedStatus = (): Status =>
  showcase({
    run: { state: "paused", stateVersion: 4 },
    pause: {
      run: {
        scope: "run",
        agentId: null,
        reason: "waiting for the user",
        actorId: "owner",
        pausedAt: iso(300),
      },
      agents: [
        {
          scope: "agent",
          agentId: "developer-2",
          reason: "inspect",
          actorId: "owner",
          pausedAt: iso(120),
        },
      ],
    },
  });

/** The statuses of a daemon older than the dashboard: each drops what a newer daemon added. */
export function withoutKeys(status: Status, keys: readonly string[]): Status {
  const copy: Status = { ...status };
  for (const key of keys) delete copy[key];
  return copy;
}

export function legacyInspectStatus(): Status {
  const file = path.join(root, "test", "golden", "legacy-status-inspect.json");
  return (JSON.parse(readFileSync(file, "utf8")) as { status: Status }).status;
}

export const OBSERVE_PEEK: PeekView = {
  agentId: "developer-2",
  agentStatus: "blocked",
  text: "developer-2 pane p4\n\nReading docs/spike-herdr-agents.md\nEdit(src/dash/theme.ts)\n  Updated src/dash/theme.ts with 12 additions\n\nThinking... (3m 41s, esc to interrupt)\n\n  waiting for tool permission:\n  Bash(npm run check)\n  1. Yes   2. No\n",
};

const plain = (columns: number, rows: number, overrides?: ViewOverrides) =>
  ({
    columns,
    rows,
    theme: "plain",
    ...(overrides === undefined ? {} : { overrides }),
  }) satisfies ViewSpec;

/** Every case exported to the Rust parity fixtures; the golden screens are the first ones. */
export const CASES: readonly ParityCase[] = [
  {
    name: "showcase",
    status: showcase(),
    workerLimit: 4,
    views: [
      plain(80, 24),
      plain(120, 36),
      plain(160, 45),
      { ...plain(120, 36), theme: "ascii" },
      { ...plain(120, 36), theme: "color" },
      { ...plain(120, 36, { tick: 1 }), theme: "motion" },
      plain(120, 36, { highlight: [["m:0192f510-0b4e-79aa", 2]] }),
      plain(100, 30, { link: "down", linkAge: "12s", rings: "empty" }),
      plain(120, 36, { link: "starting" }),
      plain(120, 36, { link: "toolarge" }),
      plain(120, 36, { paused: true, notice: "retry sent" }),
      plain(120, 36, { focus: "agents", showAllEnded: true }),
      plain(120, 36, { focus: "findings" }),
      plain(120, 36, { focus: "pipeline" }),
      plain(120, 36, { focus: "work" }),
      plain(120, 36, { problemsOnly: true }),
      plain(60, 16),
      plain(99, 20),
      plain(200, 60),
    ],
  },
  {
    name: "tasks",
    status: tasksShowcase(),
    workerLimit: 4,
    views: [plain(80, 24), plain(160, 45)],
  },
  {
    name: "paused",
    status: pausedStatus(),
    workerLimit: 4,
    views: [plain(120, 36)],
  },
  {
    name: "crowded",
    status: crowded(),
    workerLimit: 3,
    views: [
      plain(80, 24, { focus: "agents" }),
      plain(118, 34, { focus: "agents" }),
      plain(160, 45, { focus: "agents" }),
      plain(160, 45),
    ],
  },
  {
    name: "empty-queue",
    status: emptyQueue(),
    workerLimit: 4,
    views: [plain(80, 24), plain(120, 36), plain(100, 30, { focus: "agents" })],
  },
  {
    name: "waiting",
    status: waitingStatus(),
    workerLimit: 4,
    views: [plain(80, 24), plain(120, 36)],
  },
  {
    name: "stale-mail",
    status: staleMail(),
    workerLimit: 4,
    views: [plain(80, 24), plain(120, 36)],
  },
  {
    name: "stale-findings",
    status: staleStatus(),
    workerLimit: 4,
    views: [plain(120, 36), { ...plain(160, 45), theme: "color" }],
  },
  {
    name: "supervision-idle",
    status: idleStatus(),
    workerLimit: 4,
    views: [plain(120, 36)],
  },
  {
    name: "older-no-active-tasks",
    status: withoutKeys(tasksShowcase(), ["activeTasks"]),
    workerLimit: null,
    views: [plain(120, 36)],
  },
  {
    name: "older-no-supervision-state",
    status: withoutKeys(showcase(), ["supervisionState"]),
    workerLimit: 4,
    views: [plain(120, 36)],
  },
  {
    name: "older-no-pm-mail",
    status: withoutKeys(staleMail(), ["pmMail"]),
    workerLimit: 4,
    views: [plain(120, 36)],
  },
  {
    name: "legacy-status-inspect",
    status: legacyInspectStatus(),
    workerLimit: null,
    views: [plain(120, 36), plain(80, 24)],
  },
];

/** A golden text file and how to produce its lines from a case's view. */
export interface Golden {
  readonly file: string;
  readonly case: string;
  /** Index into the case's `views`. */
  readonly view: number;
  readonly overlay?: "help" | "confirm" | "observe";
}

export const GOLDENS: readonly Golden[] = [
  { file: "dash-80x24", case: "showcase", view: 0 },
  { file: "dash-120x36", case: "showcase", view: 1 },
  { file: "dash-160x45", case: "showcase", view: 2 },
  { file: "dash-120x36-ascii", case: "showcase", view: 3 },
  { file: "dash-help", case: "showcase", view: 1, overlay: "help" },
  { file: "dash-confirm-retry", case: "showcase", view: 1, overlay: "confirm" },
  { file: "dash-observe", case: "showcase", view: 1, overlay: "observe" },
  { file: "dash-100x30-no-link", case: "showcase", view: 7 },
  { file: "dash-tasks-80x24", case: "tasks", view: 0 },
  { file: "dash-tasks-160x45", case: "tasks", view: 1 },
  { file: "dash-120x36-paused", case: "paused", view: 0 },
  { file: "dash-crowded-80x24", case: "crowded", view: 0 },
  { file: "dash-crowded-118x34", case: "crowded", view: 1 },
  { file: "dash-crowded-160x45", case: "crowded", view: 2 },
  { file: "dash-empty-queue-80x24", case: "empty-queue", view: 0 },
  { file: "dash-empty-queue-120x36", case: "empty-queue", view: 1 },
  { file: "dash-waiting-80x24", case: "waiting", view: 0 },
  { file: "dash-waiting-120x36", case: "waiting", view: 1 },
  { file: "dash-stale-mail-80x24", case: "stale-mail", view: 0 },
  { file: "dash-stale-mail-120x36", case: "stale-mail", view: 1 },
  { file: "dash-stale-findings-120x36", case: "stale-findings", view: 0 },
  { file: "dash-supervision-idle-120x36", case: "supervision-idle", view: 0 },
];

export function caseNamed(name: string): ParityCase {
  const found = CASES.find((c) => c.name === name);
  if (found === undefined) throw new Error(`no case ${name}`);
  return found;
}

export function overlaid(base: readonly string[], overlay: Overlay): string[] {
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

/** The text lines a golden file holds. */
export function goldenLines(golden: Golden): string[] {
  const c = caseNamed(golden.case);
  const spec = c.views[golden.view]!;
  const theme = THEMES[spec.theme];
  const model = modelOf(c.status, c.workerLimit);
  const view = viewStateOf(spec);
  const base = plainLines(buildFrame(model, view, theme).lines);
  if (golden.overlay === undefined) return base;
  const size = view.size;
  if (golden.overlay === "help")
    return overlaid(base, helpOverlay(size, theme));
  if (golden.overlay === "observe")
    return overlaid(base, observeOverlay(OBSERVE_PEEK, size, theme));
  return overlaid(
    base,
    confirmOverlay(
      { kind: "resolve", decision: "retry", message: model.queue.messages[0]! },
      size,
      theme,
    ),
  );
}

export { NOW, showcaseRings };
