/** The dashboard as a pure function: model and view state in, a fixed-size grid of styled lines out. Ink only paints it. */
import { availableDecisions, type DashAction } from "./actions.js";
import { bottomBorder, thumbRange, topBorder, type Tab } from "./border.js";
import { age, ageDetail, cellWidth, durationText, truncate } from "./format.js";
import { glyphsFor, type Glyphs } from "./glyphs.js";
import {
  areaGraph,
  graphSpans,
  meter,
  recencyBar,
  sparkline,
  stackedBar,
  type StagePart,
} from "./graph.js";
import {
  agentSections,
  CHROME_ROWS,
  columnsOf,
  columnWidths,
  fillRows,
  fits,
  layoutFor,
  MAX_GRAPH_ROWS,
  pipelineSections,
  queueSections,
  TWO_GRAPHS_ROWS,
  windowOf,
  WRAP_REASON_ROWS,
  type PanelId,
  type PanelWish,
} from "./layout.js";
import {
  asciiLine,
  blankLine,
  fitLine,
  lineWidth,
  mergeSpans,
  span,
  styleLine,
  type Line,
  type Span,
} from "./lines.js";
import {
  PIPELINE_ITEMS,
  pipelineItems,
  queueRows,
  type AgentRow,
  type DashModel,
  type FindingRow,
  type MessageRow,
  type PipelineItem,
  type PipelineStage,
  type WorkRow,
} from "./model.js";
import { colorOfState, type ColorRole, type Theme } from "./theme.js";

export type Link = "starting" | "ok" | "down" | "toolarge";

export interface Rings {
  readonly unresolved: readonly number[];
  readonly working: readonly number[];
  readonly oldest: readonly number[];
}

export const EMPTY_RINGS: Rings = { unresolved: [], working: [], oldest: [] };

export interface ViewState {
  readonly size: { readonly columns: number; readonly rows: number };
  readonly focus: PanelId;
  /** Selected row index per panel, already resolved against the visible rows. */
  readonly selected: Readonly<Record<PanelId, number>>;
  readonly problemsOnly: boolean;
  readonly paused: boolean;
  readonly link: Link;
  /** Time since the last good poll, for example `1s`. */
  readonly linkAge: string;
  readonly clock: string;
  readonly intervalSeconds: number;
  readonly nowMs: number;
  readonly tick: number;
  readonly rings: Rings;
  readonly highlight: ReadonlyMap<string, number>;
  readonly notice: string | null;
}

export interface Frame {
  readonly lines: readonly Line[];
  /** Panels that fit on screen, in focus order. */
  readonly shown: readonly PanelId[];
}

const NUMBERS: Record<PanelId, number> = {
  agents: 1,
  pipeline: 2,
  queue: 3,
  findings: 4,
  work: 5,
};
const TITLES: Record<PanelId, string> = {
  agents: "agents",
  pipeline: "pipeline",
  queue: "queue",
  findings: "findings",
  work: "work items",
};
export const PANEL_ORDER: readonly PanelId[] = [
  "agents",
  "pipeline",
  "queue",
  "findings",
  "work",
];

export function visiblePanels(model: DashModel): PanelId[] {
  return model.work.length > 0
    ? [...PANEL_ORDER]
    : PANEL_ORDER.filter((p) => p !== "work");
}

interface Ctx {
  readonly model: DashModel;
  readonly view: ViewState;
  readonly theme: Theme;
  readonly g: Glyphs;
  readonly w: number;
  readonly h: number;
  readonly focused: boolean;
}

const bodyOf = (ctx: Ctx) => ctx.h - CHROME_ROWS;
const contentOf = (ctx: Ctx) => ctx.w - 4;

// ---------------------------------------------------------------- tables

interface Col {
  readonly key: string;
  readonly title: string;
  readonly width: number;
  readonly right?: boolean;
  readonly flex?: boolean;
}

function resolveCols(contentWidth: number, cols: readonly Col[]): Col[] {
  const fixed =
    cols.filter((c) => c.flex !== true).reduce((a, c) => a + c.width, 0) +
    (cols.length - 1);
  return cols.map((c) =>
    c.flex === true ? { ...c, width: Math.max(6, contentWidth - fixed) } : c,
  );
}

function cellLine(value: Span | readonly Span[], col: Col): Line {
  const spans = Array.isArray(value) ? (value as Span[]) : [value as Span];
  if (col.right !== true || lineWidth(spans) >= col.width)
    return fitLine(spans, col.width);
  return [{ text: " ".repeat(col.width - lineWidth(spans)) }, ...spans];
}

function tableHeader(cols: readonly Col[], theme: Theme): Line {
  return mergeSpans(
    cols.flatMap((c, i): Span[] => [
      ...(i > 0 ? [span(" ")] : []),
      ...cellLine(span(c.title, { color: theme.color("dim"), bold: true }), c),
    ]),
  );
}

interface RowSpec {
  readonly cells: Readonly<Record<string, Span | readonly Span[]>>;
  readonly selected: boolean;
  readonly changed: boolean;
  readonly dim?: boolean;
}

/** One body line of width `w - 2`: marker column, table cells, one pad cell. */
function tableRow(ctx: Ctx, cols: readonly Col[], row: RowSpec): Line {
  const { theme, g } = ctx;
  const marker = row.selected
    ? span(g.selected, { color: theme.color("bright"), bold: true })
    : row.changed && theme.reducedMotion
      ? span(g.changed)
      : span(" ");
  const cells = cols.flatMap((c, i): Span[] => [
    ...(i > 0 ? [span(" ")] : []),
    ...cellLine(row.cells[c.key] ?? span(""), c),
  ]);
  let line: Line = [marker, ...cells, span(" ")];
  if (row.selected)
    line = styleLine(line, { bg: theme.color("selectBg"), bold: true });
  else if (row.changed && !theme.reducedMotion)
    line = line.map((s) => ({ ...s, bold: true }));
  if (row.dim === true) line = styleLine(line, { dim: true });
  return mergeSpans(line);
}

function textLine(ctx: Ctx, text: string, role: ColorRole = "dim"): Line {
  return fitLine(
    [
      span(" "),
      span(truncate(text, contentOf(ctx)), { color: ctx.theme.color(role) }),
    ],
    ctx.w - 2,
  );
}

function indent(line: Line, w: number): Line {
  return fitLine([span(" "), ...line], w);
}

// ------------------------------------------------------------- assembly

function borderColor(ctx: Ctx, id: PanelId): string | undefined {
  return ctx.theme.color(
    `border.${id}${ctx.focused ? ".focus" : ""}` as ColorRole,
  );
}

interface PanelSpec {
  readonly id: PanelId;
  readonly tabs?: readonly Tab[];
  readonly bottomLeft?: readonly Tab[];
  readonly bottomRight?: readonly Tab[];
  readonly body: readonly Line[];
  readonly thumb?:
    { readonly start: number; readonly size: number } | undefined;
  /** Body row where the thumb range starts. */
  readonly thumbTop?: number;
}

function assemble(ctx: Ctx, spec: PanelSpec): Line[] {
  const { g, theme, w, h, focused } = ctx;
  const border = borderColor(ctx, spec.id);
  const edge = focused ? g.edgeFocused : g.edge;
  const rows = h - CHROME_ROWS;
  const out: Line[] = [
    topBorder(
      w,
      {
        number: NUMBERS[spec.id],
        title: TITLES[spec.id],
        tabs: spec.tabs,
        focused,
      },
      g,
      { border, title: theme.color(focused ? "bright" : "fg") },
    ),
  ];
  for (let r = 0; r < rows; r++) {
    const inner = fitLine(spec.body[r] ?? [], w - 2);
    const t = spec.thumb;
    const top = spec.thumbTop ?? 0;
    const inThumb =
      t !== undefined && r >= top + t.start && r < top + t.start + t.size;
    out.push(
      mergeSpans([
        span(edge.v, { color: border }),
        ...inner,
        inThumb
          ? span(g.thumb, { color: theme.color("fg") })
          : span(edge.v, { color: border }),
      ]),
    );
  }
  out.push(
    bottomBorder(
      w,
      { left: spec.bottomLeft, right: spec.bottomRight, focused },
      g,
      border,
    ),
  );
  return out.slice(0, h);
}

/**
 * The position tab of a panel's bottom border. A focused panel has a cursor, so
 * it shows `cursor/total`. An unfocused panel has none; it shows the visible
 * range `first-last/total`, and only when rows are hidden.
 */
const counter = (
  ctx: Ctx,
  selected: number,
  total: number,
  win: { readonly start: number; readonly end: number },
): Tab[] => {
  const info = ctx.theme.color("info");
  if (total <= 0) return [];
  if (ctx.focused) return [{ text: `${selected + 1}/${total}`, color: info }];
  if (win.end - win.start >= total) return [];
  return [{ text: `${win.start + 1}-${win.end}/${total}`, color: info }];
};

const widest = (words: readonly string[], floor: number): number =>
  words.reduce((n, w) => Math.max(n, cellWidth(w)), floor);

function stateSpan(ctx: Ctx, text: string, state = text): Span {
  return span(text, { color: ctx.theme.color(colorOfState(state)) });
}

// ---------------------------------------------------------------- agents

function agentState(
  a: AgentRow,
  nowMs: number,
): { word: string; role: ColorRole } {
  if (a.state !== "active") return { word: a.state, role: "dim" };
  if (a.lost)
    return { word: a.pausedAt === null ? "LOST" : "LOST+paused", role: "bad" };
  if (a.pausedAt !== null)
    return { word: `paused ${age(a.pausedAt, nowMs)}`, role: "warn" };
  if (a.stalled) return { word: "STALLED", role: "bad" };
  if (a.blocked) return { word: "BLOCKED", role: "bad" };
  if (a.working) return { word: "working", role: "ok" };
  return { word: "idle", role: "fg" };
}

const ACTIVITY_WINDOW_SECONDS = 30;

const AGENT_NAME_MAX = 20;

function agentsPanel(ctx: Ctx): Line[] {
  const { model, view, theme, g } = ctx;
  const cw = contentOf(ctx);
  const agents = model.agents;
  const active = agents.filter((a) => a.state === "active").length;
  const sec = agentSections(bodyOf(ctx), agents.length);
  const nameWidth = Math.min(
    AGENT_NAME_MAX,
    widest(
      agents.map((a) => a.agentId),
      12,
    ),
  );
  const paneWidth = widest(
    agents.map((a) => a.paneId ?? "-"),
    4,
  );
  // Optional columns go, rightmost first, before an agent name is cut.
  const optional = [
    { key: "pane", ok: fits(cw, "agentsPane") },
    { key: "activity", ok: fits(cw, "agentsActivity") },
    { key: "role", ok: fits(cw, "agentsRole") },
  ];
  const build = (drop: number): Col[] => {
    const kept = new Set(
      optional.filter((o, i) => o.ok && i >= drop).map((o) => o.key),
    );
    return resolveCols(cw, [
      { key: "glyph", title: " ", width: 1 },
      { key: "agent", title: "AGENT", width: nameWidth, flex: true },
      ...(kept.has("role") ? [{ key: "role", title: "ROLE", width: 10 }] : []),
      { key: "gen", title: "GEN", width: 3 },
      {
        key: "state",
        title: "STATE",
        width: agents.some((a) => a.pausedAt !== null) ? 11 : 7,
      },
      ...(kept.has("activity")
        ? [{ key: "activity", title: "ACTIVITY", width: 8 }]
        : []),
      { key: "age", title: "AGE", width: 6, right: true },
      { key: "q", title: "Q", width: 2, right: true },
      ...(kept.has("pane")
        ? [{ key: "pane", title: "PANE", width: paneWidth }]
        : []),
    ]);
  };
  let cols = build(0);
  for (
    let drop = 1;
    drop <= optional.length &&
    cols.find((c) => c.key === "agent")!.width < nameWidth;
    drop++
  )
    cols = build(drop);
  const win = windowOf(agents.length, view.selected.agents, sec.rows);
  const rowOf = (a: AgentRow, isSelected: boolean): Line => {
    const st = agentState(a, view.nowMs);
    const spinner = g.spinner[view.tick % g.spinner.length] ?? g.working;
    const glyph =
      a.state !== "active"
        ? span(g.ended, { color: theme.color("dim") })
        : st.role === "bad"
          ? span(g.attention, { color: theme.color("bad") })
          : a.working
            ? span(theme.reducedMotion ? g.working : spinner, {
                color: theme.color("ok"),
              })
            : span(g.idle, { color: theme.color("ok") });
    const last = Date.parse(a.lastActivityAt);
    const idleSeconds = Number.isNaN(last)
      ? Infinity
      : Math.max(0, (view.nowMs - last) / 1000);
    return tableRow(ctx, cols, {
      selected: isSelected,
      changed: view.highlight.has(`a:${a.id}`),
      dim: a.state !== "active",
      cells: {
        glyph,
        agent: span(a.agentId, { color: theme.color("fg") }),
        role: span(a.kind, { color: theme.color("fg") }),
        gen: span(`g${a.generation}`, { color: theme.color("dim") }),
        state: span(st.word, { color: theme.color(st.role) }),
        activity:
          a.state === "active"
            ? recencyBar(
                1 - Math.min(1, idleSeconds / ACTIVITY_WINDOW_SECONDS),
                8,
                g,
                theme,
              )
            : span(""),
        age: span(ageDetail(a.lastActivityAt, view.nowMs), {
          color: theme.color("fg"),
        }),
        q: span(String(a.queueDepth), {
          color: theme.color(a.queueDepth > 0 ? "warn" : "dim"),
        }),
        pane: span(a.paneId ?? "-", { color: theme.color("dim") }),
      },
    });
  };
  const body: Line[] = [];
  if (sec.header > 0) body.push(indent(tableHeader(cols, theme), ctx.w - 2));
  if (agents.length === 0) body.push(textLine(ctx, "no agents"));
  agents
    .slice(win.start, win.end)
    .forEach((a, i) =>
      body.push(
        rowOf(a, ctx.focused && win.start + i === view.selected.agents),
      ),
    );
  if (sec.graph > 0) {
    const limit = model.header.workerLimit;
    body.push(
      textLine(
        ctx,
        `working agents (inferred) ${g.rule} now ${model.counts.working} ${g.rule} since dash start`,
      ),
    );
    graphSpans(
      areaGraph(
        view.rings.working,
        cw,
        Math.min(MAX_GRAPH_ROWS, sec.graph - 1),
        Math.max(1, limit ?? 0, ...view.rings.working),
        g.ascii,
      ),
      theme,
    ).forEach((spans) => body.push(indent(spans, ctx.w - 2)));
  }
  const ended = agents.length - active;
  const tabs: Tab[] = [
    { text: `${active} active`, color: theme.color("fg") },
    ...(ended > 0
      ? [{ text: `${ended} ended`, color: theme.color("dim") }]
      : []),
  ];
  return assemble(ctx, {
    id: "agents",
    tabs,
    bottomLeft: [{ text: "working: inferred", color: theme.color("dim") }],
    bottomRight: counter(ctx, view.selected.agents, agents.length, win),
    body,
    thumb: thumbRange(agents.length, sec.rows, win.start, sec.rows),
    thumbTop: sec.header,
  });
}

// -------------------------------------------------------------- pipeline

function stageKind(state: string): StagePart["kind"] {
  const role = colorOfState(state);
  return role === "ok" ? "good" : role === "bad" ? "bad" : "progress";
}

function stageTotal(stage: PipelineStage): string {
  return stage.capped ? `${PIPELINE_ITEMS}+` : String(stage.total);
}

function stageLine(
  ctx: Ctx,
  name: string,
  stage: PipelineStage,
  barCells: number,
): Line {
  const { theme, g } = ctx;
  const order = { good: 0, progress: 1, bad: 2 } as const;
  const parts = Object.entries(stage.counts)
    .map(([state, count]): StagePart & { state: string } => ({
      state,
      count,
      kind: stageKind(state),
    }))
    .sort((a, b) => order[a.kind] - order[b.kind]);
  const words = parts.map((p) => `${p.state} ${p.count}`).join("  ");
  return indent(
    mergeSpans([
      span(name.padEnd(12), { color: theme.color("fg") }),
      span(" "),
      ...stackedBar(parts, barCells, g, theme),
      span(` ${stageTotal(stage).padStart(3)}  `, {
        color: theme.color("bright"),
      }),
      span(words, { color: theme.color("dim") }),
    ]),
    ctx.w - 2,
  );
}

/** The longest pipeline state word, `conflicted`. */
const PIPELINE_STATE_WIDTH = 10;

function pipelinePanel(ctx: Ctx): Line[] {
  const { model, view, theme, g } = ctx;
  const cw = contentOf(ctx);
  const items = pipelineItems(model);
  const sec = pipelineSections(bodyOf(ctx), items.length);
  const p = model.pipeline;
  const body: Line[] = [];
  const flow = `reported ${stageTotal(p.reports)}  ${g.arrow}  review ${stageTotal(p.reviews)}  ${g.arrow}  integrated ${stageTotal(p.integrations)}`;
  if (sec.flow > 0) body.push(textLine(ctx, flow, "fg"));
  if (sec.compactSummary) {
    const last = items[0];
    const sparkWidth = Math.max(4, Math.min(14, Math.floor((cw - 40) / 2)));
    const spark = (series: readonly number[], max: number) =>
      sparkline(series, sparkWidth, max, g.ascii);
    body.push(
      textLine(
        ctx,
        `msgs ${spark(view.rings.unresolved, Math.max(1, ...view.rings.unresolved))} ${model.counts.unresolved}   working ${spark(view.rings.working, Math.max(1, model.header.workerLimit ?? 0, ...view.rings.working))} ${model.counts.working}${last ? `   last: ${last.state} ${last.label}` : ""}`,
      ),
    );
  }
  if (sec.stages > 0) {
    const barCells = Math.min(16, Math.max(6, Math.floor(cw / 4)));
    const lines = [
      stageLine(ctx, "reports", p.reports, barCells),
      stageLine(ctx, "reviews", p.reviews, barCells),
      stageLine(ctx, "integrations", p.integrations, barCells),
    ];
    if (sec.gapped)
      lines.forEach((line, i) => {
        if (i > 0) body.push(blankLine(ctx.w - 2));
        body.push(line);
      });
    else body.push(...lines.slice(0, sec.stages));
  }
  let win = { start: 0, end: 0, hidden: 0 };
  if (sec.header > 0 && items.length > 0) {
    const cols = resolveCols(cw, [
      { key: "stage", title: "STAGE", width: 11 },
      { key: "state", title: "STATE", width: PIPELINE_STATE_WIDTH },
      { key: "who", title: "WHO / COMMIT", width: 12, flex: true },
      { key: "age", title: "AGE", width: 6, right: true },
    ]);
    body.push(indent(tableHeader(cols, theme), ctx.w - 2));
    win = windowOf(items.length, view.selected.pipeline, sec.items);
    items.slice(win.start, win.end).forEach((item: PipelineItem, i) =>
      body.push(
        tableRow(ctx, cols, {
          selected: ctx.focused && win.start + i === view.selected.pipeline,
          changed: view.highlight.has(`p:${item.id}`),
          cells: {
            stage: span(item.stage, { color: theme.color("fg") }),
            state: stateSpan(ctx, item.state),
            who: span(item.label, { color: theme.color("fg") }),
            age: span(ageDetail(item.createdAt, view.nowMs), {
              color: theme.color("dim"),
            }),
          },
        }),
      ),
    );
  }
  const headerLines = body.length - (win.end - win.start);
  return assemble(ctx, {
    id: "pipeline",
    tabs: fits(cw, "pipelineTab")
      ? [{ text: "reported > review > integrated", color: theme.color("dim") }]
      : [],
    bottomRight:
      sec.header > 0
        ? counter(ctx, view.selected.pipeline, items.length, win)
        : [],
    body,
    thumb: thumbRange(items.length, sec.items, win.start, sec.items),
    thumbTop: headerLines,
  });
}

// ----------------------------------------------------------------- queue

function messageDetail(m: MessageRow): string {
  if (m.problem !== null) return `! ${m.problem}`;
  if (m.deferredReason !== null) return `deferred: ${m.deferredReason}`;
  return m.stateReason ?? "";
}

function queuePanel(ctx: Ctx): Line[] {
  const { model, view, theme, g } = ctx;
  const cw = contentOf(ctx);
  const rows = queueRows(model, view.problemsOnly);
  const sec = queueSections(
    bodyOf(ctx),
    Math.max(1, rows.length),
    rows.length > 0,
  );
  const cols = resolveCols(cw, [
    { key: "glyph", title: " ", width: 1 },
    { key: "seq", title: "SEQ", width: 5, right: true },
    { key: "to", title: "TO", width: 12 },
    { key: "state", title: "STATE", width: 8 },
    { key: "age", title: "AGE", width: 6, right: true },
    ...(fits(cw, "queueNotified") ? [{ key: "n", title: "N", width: 1 }] : []),
    { key: "detail", title: "DETAIL", width: 10, flex: true },
  ]);
  const body: Line[] = [];
  if (sec.header > 0) body.push(indent(tableHeader(cols, theme), ctx.w - 2));
  const selectedIndex = view.selected.queue;
  const win = windowOf(rows.length, selectedIndex, sec.list);
  if (rows.length === 0)
    body.push(
      textLine(
        ctx,
        view.problemsOnly ? "no delivery problems" : "no unresolved messages",
      ),
    );
  rows.slice(win.start, win.end).forEach((m, i) => {
    const problem = m.problem !== null;
    body.push(
      tableRow(ctx, cols, {
        selected: ctx.focused && win.start + i === selectedIndex,
        changed: view.highlight.has(`m:${m.id}`),
        cells: {
          glyph: problem
            ? span(g.attention, { color: theme.color("bad") })
            : span(g.idle, { color: theme.color(colorOfState(m.state)) }),
          seq: span(`#${m.sequence}`, { color: theme.color("fg") }),
          to: span(m.recipientAgentId, { color: theme.color("fg") }),
          state: stateSpan(ctx, m.state),
          age: span(ageDetail(m.queuedAt, view.nowMs), {
            color: theme.color("fg"),
          }),
          n: span(m.notified ? g.notified : g.notNotified, {
            color: theme.color("dim"),
          }),
          detail: span(messageDetail(m), {
            color: theme.color(problem ? "bad" : "dim"),
          }),
        },
      }),
    );
  });
  const used = body.length;
  const selected = rows[Math.min(selectedIndex, rows.length - 1)];
  if (sec.detail > 0) {
    while (
      body.length <
      used + (sec.list - Math.min(sec.list, win.end - win.start))
    )
      body.push(blankLine(ctx.w - 2));
    const caption = `${g.rule.repeat(2)} selected `;
    body.push(
      textLine(ctx, caption + g.rule.repeat(Math.max(0, cw - caption.length))),
    );
    if (selected !== undefined) {
      const decisions = availableDecisions(selected);
      body.push(
        textLine(
          ctx,
          `#${selected.sequence} to ${selected.recipientAgentId}  ${selected.state}  queued ${ageDetail(selected.queuedAt, view.nowMs)} ago`,
          "fg",
        ),
        textLine(
          ctx,
          `message ${selected.messageId}  ${selected.notified ? "notified" : "not notified"}`,
        ),
        textLine(
          ctx,
          `${messageDetail(selected) || `state ${selected.state}`}${
            decisions.length > 0
              ? `   can: ${decisions.join(", ")}`
              : "   no action in this state"
          }`,
          selected.problem !== null ? "bad" : "dim",
        ),
      );
    }
  }
  if (sec.graphs > 0) {
    const two = sec.graphs >= TWO_GRAPHS_ROWS;
    const first = two ? Math.floor(sec.graphs / 2) : sec.graphs;
    const draw = (
      caption: string,
      rowsCount: number,
      series: readonly number[],
      max: number,
    ) => {
      body.push(textLine(ctx, caption));
      graphSpans(
        areaGraph(
          series,
          cw,
          Math.min(MAX_GRAPH_ROWS, rowsCount - 1),
          max,
          g.ascii,
        ),
        theme,
      ).forEach((spans) => body.push(indent(spans, ctx.w - 2)));
    };
    draw(
      `unresolved messages ${g.rule} now ${model.counts.unresolved} ${g.rule} max ${Math.max(0, ...view.rings.unresolved)} ${g.rule} since dash start`,
      first,
      view.rings.unresolved,
      Math.max(1, ...view.rings.unresolved),
    );
    if (two)
      draw(
        `oldest unresolved message (age) ${g.rule} now ${durationText(view.rings.oldest.at(-1) ?? 0)} ${g.rule} since dash start`,
        sec.graphs - first,
        view.rings.oldest,
        Math.max(60, ...view.rings.oldest),
      );
  }
  const problems = model.queue.messages.filter(
    (m) => m.problem !== null,
  ).length;
  const tabs: Tab[] = [
    {
      text: problems > 0 ? `${problems} stuck or failed` : "no problems",
      color: theme.color(problems > 0 ? "bad" : "dim"),
    },
    ...(fits(cw, "queueClearsTab") && model.queue.inputClearCount > 0
      ? [
          {
            text: `input clears ${model.queue.inputClearCount}`,
            color: theme.color("dim"),
          },
        ]
      : []),
    {
      text: `f problems only [${view.problemsOnly ? "x" : " "}]`,
      color: theme.color(view.problemsOnly ? "warn" : "dim"),
    },
  ];
  return assemble(ctx, {
    id: "queue",
    tabs,
    bottomRight: counter(ctx, selectedIndex, rows.length, win),
    body,
    thumb: thumbRange(rows.length, sec.list, win.start, sec.list),
    thumbTop: sec.header,
  });
}

// -------------------------------------------------------------- findings

/** The longest severity word, `critical`. */
const FINDING_SEVERITY_WIDTH = 8;

/** The recorded reason, marked when the target is ended or not in the agent list. */
function findingReason(f: FindingRow): string {
  const marker = f.targetState === "active" ? "" : `(target ${f.targetState}) `;
  return `${marker}${f.stateReason ?? ""}`.trim();
}

function findingsPanel(ctx: Ctx): Line[] {
  const { model, view, theme, g } = ctx;
  const cw = contentOf(ctx);
  const rows = model.findings;
  const body0 = bodyOf(ctx);
  const header = body0 >= 3 ? 1 : 0;
  const cols = resolveCols(cw, [
    { key: "glyph", title: " ", width: 1 },
    { key: "id", title: "ID", width: 6 },
    {
      key: "target",
      title: "TARGET",
      width: widest(
        rows.map((f) => f.targetAgentId),
        6,
      ),
    },
    { key: "sev", title: "SEV", width: FINDING_SEVERITY_WIDTH },
    { key: "state", title: "STATE", width: 9 },
    { key: "int", title: "INT", width: 3 },
    { key: "reason", title: "REASON", width: 10, flex: true },
  ]);
  const win = windowOf(rows.length, view.selected.findings, body0 - header);
  const body: Line[] = [];
  if (header > 0) body.push(indent(tableHeader(cols, theme), ctx.w - 2));
  if (rows.length === 0) body.push(textLine(ctx, "no open findings"));
  rows.slice(win.start, win.end).forEach((f: FindingRow, i) =>
    body.push(
      tableRow(ctx, cols, {
        selected: ctx.focused && win.start + i === view.selected.findings,
        changed: view.highlight.has(`f:${f.id}`),
        dim: f.targetState === "ended",
        cells: {
          glyph: f.needsOperator
            ? span(g.attention, { color: theme.color("bad") })
            : span(g.idle, { color: theme.color("warn") }),
          id: span(f.findingId.slice(0, 6), { color: theme.color("fg") }),
          target: span(f.targetAgentId, { color: theme.color("fg") }),
          sev: span(f.severity, { color: theme.color("fg") }),
          state: span(f.needsOperator ? "ESCALATED" : f.state, {
            color: theme.color(f.needsOperator ? "bad" : "warn"),
          }),
          int: span(`${f.interventions}/2`, { color: theme.color("dim") }),
          reason: span(findingReason(f), {
            color: theme.color("fg"),
          }),
        },
      }),
    ),
  );
  const needs = rows.filter(
    (f) => f.needsOperator && f.targetState !== "ended",
  ).length;
  const orphaned = rows.filter((f) => f.targetState === "ended").length;
  const unknown = rows.filter((f) => f.targetState === "unknown").length;
  return assemble(ctx, {
    id: "findings",
    tabs: [
      ...(needs > 0
        ? [{ text: `${needs} needs operator`, color: theme.color("bad") }]
        : []),
      ...(orphaned > 0
        ? [{ text: `${orphaned} target ended`, color: theme.color("dim") }]
        : []),
      ...(unknown > 0
        ? [{ text: `${unknown} target unknown`, color: theme.color("dim") }]
        : []),
    ],
    bottomRight: counter(ctx, view.selected.findings, rows.length, win),
    body,
    thumb: thumbRange(rows.length, body0 - header, win.start, body0 - header),
    thumbTop: header,
  });
}

function workPanel(ctx: Ctx): Line[] {
  const { model, view, theme } = ctx;
  const cw = contentOf(ctx);
  const rows = model.work;
  const body0 = bodyOf(ctx);
  const header = body0 >= 3 ? 1 : 0;
  const cols = resolveCols(cw, [
    { key: "id", title: "WORK ITEM", width: 14 },
    { key: "state", title: "STATE", width: 20 },
    { key: "title", title: "TITLE", width: 10, flex: true },
  ]);
  const win = windowOf(rows.length, view.selected.work, body0 - header);
  const body: Line[] = [];
  if (header > 0) body.push(indent(tableHeader(cols, theme), ctx.w - 2));
  rows.slice(win.start, win.end).forEach((r: WorkRow, i) =>
    body.push(
      tableRow(ctx, cols, {
        selected: ctx.focused && win.start + i === view.selected.work,
        changed: view.highlight.has(`w:${r.id}`),
        cells: {
          id: span(r.workItemId, { color: theme.color("fg") }),
          state: stateSpan(ctx, r.state),
          title: span(r.title, { color: theme.color("fg") }),
        },
      }),
    ),
  );
  return assemble(ctx, {
    id: "work",
    tabs: [{ text: "v1", color: theme.color("dim") }],
    bottomRight: counter(ctx, view.selected.work, rows.length, win),
    body,
    thumb: thumbRange(rows.length, body0 - header, win.start, body0 - header),
    thumbTop: header,
  });
}

const RENDER: Record<PanelId, (ctx: Ctx) => Line[]> = {
  agents: agentsPanel,
  pipeline: pipelinePanel,
  queue: queuePanel,
  findings: findingsPanel,
  work: workPanel,
};

function wishFor(id: PanelId, model: DashModel, view: ViewState): PanelWish {
  const count = (n: number) => Math.max(1, n);
  switch (id) {
    case "agents": {
      return {
        id,
        min: 3,
        want: CHROME_ROWS + 1 + count(model.agents.length),
        weight: 4,
        stretch: true,
      };
    }
    case "pipeline":
      return {
        id,
        min: 3,
        want: CHROME_ROWS + 1 + 3 + 2 + 1 + pipelineItems(model).length,
        weight: 2,
        stretch: false,
      };
    case "queue":
      return {
        id,
        min: 3,
        want:
          CHROME_ROWS + 1 + count(queueRows(model, view.problemsOnly).length),
        weight: 4,
        stretch: true,
      };
    case "findings":
      return {
        id,
        min: 3,
        want: CHROME_ROWS + 1 + count(model.findings.length),
        weight: 1,
        stretch: false,
      };
    case "work":
      return {
        id,
        min: 3,
        want: CHROME_ROWS + 1 + count(model.work.length),
        weight: 1,
        stretch: false,
      };
  }
}

// ---------------------------------------------------------------- header

function healthChip(
  model: DashModel,
  view: ViewState,
  g: Glyphs,
): { text: string; role: ColorRole; reason: string; reasonRole: ColorRole } {
  if (view.link === "down" || view.link === "toolarge")
    return {
      text: `${g.linkDown} NO LINK`,
      role: "bad",
      reason:
        view.link === "down"
          ? "controller not answering, retrying; showing the last good frame"
          : "status too large for the control socket; showing the last good frame",
      reasonRole: "bad",
    };
  const h = model.header;
  switch (h.health) {
    case "healthy":
      return {
        text: `${g.idle} HEALTHY`,
        role: "ok",
        reason: "",
        reasonRole: "dim",
      };
    case "evaluating":
      return {
        text: `${g.idle} EVALUATING`,
        role: "warn",
        reason: "",
        reasonRole: "dim",
      };
    case "degraded":
      if (!h.supervisionEnabled && h.healthReason === null)
        return {
          text: `${g.ended} SUPERVISION OFF`,
          role: "dim",
          reason:
            "health reads degraded until supervision is enabled; this is the starting value, not a fault",
          reasonRole: "dim",
        };
      return {
        text: `${g.attention} DEGRADED`,
        role: "bad",
        reason: h.healthReason ?? "reason not recorded",
        reasonRole: "bad",
      };
    default:
      return { text: "? UNKNOWN", role: "dim", reason: "", reasonRole: "dim" };
  }
}

function headerLines(
  model: DashModel,
  view: ViewState,
  theme: Theme,
  g: Glyphs,
): Line[] {
  const W = view.size.columns;
  const H = view.size.rows;
  const h = model.header;
  const color = (role: ColorRole) => theme.color(role);
  const border = color("border.header");
  const chip = healthChip(model, view, g);
  const rightTabs: Tab[] = [
    ...(view.paused ? [{ text: "PAUSED", color: color("warn") }] : []),
    { text: view.clock, color: color("fg") },
    {
      text: `- ${view.intervalSeconds}s +`,
      color: color("info"),
    },
  ];
  const top = topBorder(
    W,
    {
      title: " cstan dash ",
      leftTabs: [
        { text: h.projectId, color: color("fg") },
        { text: `run ${h.runState}`, color: color("fg") },
        ...(h.runPause === null
          ? []
          : [
              {
                text: `PAUSED ${age(h.runPause.pausedAt, view.nowMs)}: ${h.runPause.reason}`,
                color: color("warn"),
              },
            ]),
        ...(h.fullAutoMinutes === undefined
          ? []
          : [{ text: `FULL AUTO ${h.fullAutoMinutes}m`, color: color("bad") }]),
        ...(h.grants === undefined
          ? []
          : [{ text: `grants ${h.grants}`, color: color("warn") }]),
      ],
      tabs: rightTabs,
      focused: false,
    },
    g,
    { border, title: color("bright") },
  );
  const cw = W - 4;
  const meterCells = W >= 120 ? 12 : 8;
  const linkText =
    view.link === "ok"
      ? `${g.linkUp} ${W >= 100 ? "answering " : ""}${view.linkAge}`
      : `${g.linkDown} ${view.linkAge} ago`;
  const workers: Span[] =
    h.workerLimit === null
      ? [span(`workers ${h.workers}`, { color: color("fg") })]
      : [
          span("workers ", { color: color("fg") }),
          ...meter(h.workers, h.workerLimit, meterCells, g, theme),
          span(` ${h.workers}/${h.workerLimit}`, { color: color("fg") }),
        ];
  const right: Span[] = [
    ...workers,
    span("   "),
    span(linkText, { color: color(view.link === "ok" ? "info" : "bad") }),
  ];
  const rightWidth = lineWidth(right);
  const chipSpan = span(chip.text, { color: color(chip.role), bold: true });
  const room = cw - cellWidth(chip.text) - 2 - rightWidth - 1;
  const wrap = cellWidth(chip.reason) > room && H >= WRAP_REASON_ROWS;
  const edgeSpan = span(g.edge.v, { color: border });
  const content = (line: Line): Line =>
    mergeSpans([
      edgeSpan,
      span(" "),
      ...fitLine(line, cw),
      span(" "),
      edgeSpan,
    ]);
  const strip: Line[] = wrap
    ? [
        content([
          chipSpan,
          span(" ".repeat(Math.max(1, cw - cellWidth(chip.text) - rightWidth))),
          ...right,
        ]),
        content([
          span("  "),
          span(truncate(chip.reason, cw - 2), {
            color: color(chip.reasonRole),
          }),
        ]),
      ]
    : [
        content([
          chipSpan,
          span(chip.reason === "" ? "" : "  "),
          span(truncate(chip.reason, Math.max(0, room)), {
            color: color(chip.reasonRole),
          }),
          span(
            " ".repeat(
              Math.max(
                1,
                cw -
                  cellWidth(chip.text) -
                  (chip.reason === "" ? 0 : 2) -
                  Math.min(cellWidth(chip.reason), Math.max(0, room)) -
                  rightWidth,
              ),
            ),
          ),
          ...right,
        ]),
      ];
  const bottomTabs: Tab[] = [
    {
      text: `supervision ${h.supervisionEnabled ? "on" : "off"}`,
      color: color("fg"),
    },
    ...(h.supervisionEnabled &&
    h.targetEpoch !== null &&
    h.targetEpoch > 0 &&
    h.checkpointEpoch !== h.targetEpoch
      ? [
          {
            text: `epoch ${h.checkpointEpoch ?? "-"}/${h.targetEpoch}`,
            color: color("warn"),
          },
        ]
      : []),
    ...(h.replacementAttempts > 0
      ? [
          {
            text: `replacements ${h.replacementAttempts}`,
            color: color("warn"),
          },
        ]
      : []),
  ];
  return [
    top,
    ...strip,
    bottomBorder(W, { left: bottomTabs, focused: false }, g, border),
  ];
}

// ---------------------------------------------------------------- footer

interface Hint {
  readonly key: string;
  readonly label: string;
  readonly priority: number;
}

export function footerHints(focus: PanelId, g: Glyphs): readonly Hint[] {
  const hints: Hint[] = [];
  hints.push({ key: `${g.up}${g.down}`, label: "select", priority: 5 });
  if (focus === "queue")
    hints.push(
      { key: "y", label: "retry", priority: 8 },
      { key: "s", label: "skip", priority: 7 },
      { key: "c", label: "cancel", priority: 7 },
      { key: "f", label: "problems", priority: 6 },
    );
  if (focus === "agents")
    hints.push({ key: "o", label: "observe", priority: 8 });
  hints.push(
    { key: "tab", label: "focus", priority: 4 },
    { key: "-/+", label: "interval", priority: 2 },
    { key: "p", label: "pause", priority: 3 },
    { key: "r", label: "refresh", priority: 3 },
    { key: "?", label: "help", priority: 10 },
    { key: "q", label: "quit", priority: 10 },
  );
  return hints;
}

function footerLine(view: ViewState, theme: Theme, g: Glyphs): Line {
  const W = view.size.columns;
  const noticeText =
    view.notice === null ? "" : truncate(view.notice, Math.floor(W / 2));
  const noticeWidth = noticeText === "" ? 0 : cellWidth(noticeText) + 2;
  let hints = [...footerHints(view.focus, g)];
  const widthOf = (list: readonly Hint[]) =>
    list.reduce(
      (sum, hint) => sum + cellWidth(hint.key) + 1 + cellWidth(hint.label) + 2,
      1,
    );
  while (widthOf(hints) + noticeWidth > W && hints.length > 2) {
    let lowest = 0;
    hints.forEach((hint, i) => {
      if (hint.priority < hints[lowest]!.priority) lowest = i;
    });
    hints = hints.filter((_, i) => i !== lowest);
  }
  const spans: Span[] = [span(" ")];
  for (const hint of hints) {
    spans.push(
      span(hint.key, { color: theme.color("info"), bold: true }),
      span(` ${hint.label}  `, { color: theme.color("dim") }),
    );
  }
  const used = lineWidth(spans);
  const gap = Math.max(1, W - used - noticeWidth);
  return fitLine(
    [
      ...spans,
      span(" ".repeat(gap)),
      ...(noticeText === ""
        ? []
        : [
            span(noticeText, { color: theme.color("bright"), bold: true }),
            span("  "),
          ]),
    ],
    W,
  );
}

// ------------------------------------------------------------------ frame

export function buildFrame(
  model: DashModel,
  view: ViewState,
  theme: Theme,
): Frame {
  const g = glyphsFor(theme.ascii);
  const { columns, rows } = view.size;
  const header = headerLines(model, view, theme, g);
  const bodyRows = rows - header.length - 1;
  const layout = layoutFor(columns, rows);
  const widths = columnWidths(columns, layout.mode);
  const plan = columnsOf(layout.mode, visiblePanels(model));
  const shown: PanelId[] = [];
  const columnLines = plan.map((panels, index) => {
    const heights = fillRows(
      bodyRows,
      panels.map((id) => wishFor(id, model, view)),
      view.focus,
    );
    const lines: Line[] = [];
    for (const id of panels) {
      const h = heights.get(id);
      if (h === undefined) continue;
      shown.push(id);
      lines.push(
        ...RENDER[id]({
          model,
          view,
          theme,
          g,
          w: widths[index]!,
          h,
          focused: view.focus === id,
        }),
      );
    }
    while (lines.length < bodyRows) lines.push(blankLine(widths[index]!));
    return lines;
  });
  const body: Line[] = [];
  for (let r = 0; r < bodyRows; r++)
    body.push(mergeSpans(columnLines.flatMap((column) => column[r] ?? [])));
  const all = [...header, ...body, footerLine(view, theme, g)];
  const stale = view.link === "down" || view.link === "toolarge";
  const painted = stale
    ? all.map((l, i) =>
        i < header.length || i === all.length - 1
          ? l
          : styleLine(l, { dim: true }),
      )
    : all;
  const lines = theme.ascii ? painted.map(asciiLine) : painted;
  return { lines, shown };
}

export type { DashAction };
