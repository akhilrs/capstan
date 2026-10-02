/** Pure layout decisions: terminal mode, column split, how rows are shared between panels, and which table columns fit. */

export const MIN_COLUMNS = 60;
export const MIN_ROWS = 16;
export const WIDE_COLUMNS = 100;
/** Rows below which a wrapped header reason is cut with an ellipsis instead. */
export const WRAP_REASON_ROWS = 20;
/** Panel chrome: top and bottom border. */
export const CHROME_ROWS = 2;
/** Spare rows given to a graph never exceed this. */
export const MAX_GRAPH_ROWS = 24;

export type LayoutMode = "tiny" | "narrow" | "wide";
export type PanelId = "agents" | "pipeline" | "queue" | "findings" | "work";

export interface Layout {
  readonly mode: LayoutMode;
}

export function layoutFor(columns: number, rows: number): Layout {
  const mode: LayoutMode =
    columns < MIN_COLUMNS || rows < MIN_ROWS
      ? "tiny"
      : columns >= WIDE_COLUMNS
        ? "wide"
        : "narrow";
  return { mode };
}

/** Panels per column: stacked in one column, or agents and pipeline left, the rest right. */
export function columnsOf(
  mode: LayoutMode,
  panels: readonly PanelId[],
): readonly (readonly PanelId[])[] {
  if (mode !== "wide") return [panels];
  const left = panels.filter((p) => p === "agents" || p === "pipeline");
  const right = panels.filter((p) => p !== "agents" && p !== "pipeline");
  return [left, right];
}

/** Widths of the columns: the left takes the rounded-down half. */
export function columnWidths(
  columns: number,
  mode: LayoutMode,
): readonly number[] {
  if (mode !== "wide") return [columns];
  const left = Math.floor(columns / 2);
  return [left, columns - left];
}

export interface PanelWish {
  readonly id: PanelId;
  /** Smallest total height, borders included. */
  readonly min: number;
  /** Height that shows all of the panel's content, borders included. */
  readonly want: number;
  /** Share of spare rows when panels compete. */
  readonly weight: number;
  /** Whether the panel can use more than `want` rows (graphs, detail). */
  readonly stretch: boolean;
}

/** Panels dropped, in this order, when the column cannot pay every minimum. */
const DROP_ORDER: readonly PanelId[] = ["work", "findings", "pipeline"];
/** Who receives rows left over after every wish is met, after the focused panel. */
const STRETCH_ORDER: readonly PanelId[] = ["queue", "agents"];

/**
 * Shares `rows` between panels in one column. Every shown panel first gets its
 * minimum; then each spare row goes to the panel with the best
 * weight / (extra + 1) that still wants more (D'Hondt), so short panels reach
 * their full content before long ones take more; rows still left over go to
 * the focused panel if it can stretch, else the first stretching panel, else
 * the last panel (as blank interior rows). Returns the shown panels in order
 * with their total heights.
 */
export function fillRows(
  rows: number,
  wishes: readonly PanelWish[],
  focus: PanelId,
): ReadonlyMap<PanelId, number> {
  let shown = [...wishes];
  const minTotal = () => shown.reduce((sum, w) => sum + w.min, 0);
  for (const id of DROP_ORDER) {
    if (minTotal() <= rows) break;
    shown = shown.filter((w) => w.id !== id);
  }
  const heights = new Map<PanelId, number>(shown.map((w) => [w.id, w.min]));
  let spare = rows - minTotal();
  while (spare > 0) {
    let best: PanelWish | undefined;
    let bestScore = 0;
    for (const w of shown) {
      if (heights.get(w.id)! >= w.want) continue;
      const score = w.weight / (heights.get(w.id)! - w.min + 1);
      if (score > bestScore) {
        best = w;
        bestScore = score;
      }
    }
    if (best === undefined) break;
    heights.set(best.id, heights.get(best.id)! + 1);
    spare--;
  }
  if (spare > 0 && shown.length > 0) {
    const stretching = shown.filter((w) => w.stretch);
    const target =
      stretching.find((w) => w.id === focus) ??
      STRETCH_ORDER.map((id) => stretching.find((w) => w.id === id)).find(
        (w) => w !== undefined,
      ) ??
      shown[shown.length - 1]!;
    heights.set(target.id, heights.get(target.id)! + spare);
  }
  return heights;
}

/** The slice of `count` rows that fits `capacity` and keeps `cursor` visible. */
export function windowOf(
  count: number,
  cursor: number,
  capacity: number,
): { readonly start: number; readonly end: number; readonly hidden: number } {
  if (capacity <= 0) return { start: 0, end: 0, hidden: count };
  if (capacity >= count) return { start: 0, end: count, hidden: 0 };
  const start = Math.min(Math.max(0, cursor - capacity + 1), count - capacity);
  return { start, end: start + capacity, hidden: count - capacity };
}

/** Optional table columns by the inner content width of a panel (`width - 4`). */
export const COLUMN_MIN_WIDTH = {
  agentsPane: 60,
  queueNotified: 56,
  agentsRole: 46,
  agentsActivity: 40,
  queueClearsTab: 51,
  pipelineTab: 45,
} as const;

export function fits(
  contentWidth: number,
  column: keyof typeof COLUMN_MIN_WIDTH,
): boolean {
  return contentWidth >= COLUMN_MIN_WIDTH[column];
}

/** How rows inside a panel are shared. Inputs are the body rows left after the borders. */
export interface QueueSections {
  readonly header: number;
  readonly list: number;
  readonly detail: number;
  /** Rows for graphs including their caption lines. */
  readonly graphs: number;
}

export const DETAIL_ROWS = 4;
export const GRAPH_MIN_ROWS = 4;
export const TWO_GRAPHS_ROWS = 14;

export function queueSections(
  body: number,
  messages: number,
  hasSelection: boolean,
): QueueSections {
  const header = body >= 3 ? 1 : 0;
  const avail = body - header;
  const spare = avail - messages;
  const detail = hasSelection && spare >= DETAIL_ROWS + 1 ? DETAIL_ROWS : 0;
  const graphSpare = spare - detail;
  const graphs = graphSpare >= GRAPH_MIN_ROWS ? graphSpare : 0;
  return {
    header,
    list: Math.min(messages, avail - detail - graphs),
    detail,
    graphs,
  };
}

export interface AgentSections {
  readonly header: number;
  /** Rows for agents, active first and then ended, scrolled as one list. */
  readonly rows: number;
  readonly graph: number;
}

export function agentSections(body: number, agents: number): AgentSections {
  const header = body >= 3 ? 1 : 0;
  const avail = body - header;
  const rows = Math.min(Math.max(1, agents), avail);
  const left = avail - rows;
  return { header, rows, graph: left >= 3 ? left : 0 };
}

export interface PipelineSections {
  readonly flow: number;
  /** Stage bars, or the one-line history summary in the compact form. */
  readonly stages: number;
  readonly compactSummary: boolean;
  /** A blank row between the stage bars, so full-block bars do not merge into one slab. */
  readonly gapped: boolean;
  readonly header: number;
  readonly items: number;
}

/** Rows a gapped stage block takes: three bars and the two blank rows between them. */
export const GAPPED_STAGE_ROWS = 5;

export function pipelineSections(
  body: number,
  items: number,
): PipelineSections {
  if (body <= 2)
    return {
      flow: Math.min(1, body),
      stages: 0,
      compactSummary: body === 2,
      gapped: false,
      header: 0,
      items: 0,
    };
  const gapped = body - 1 - GAPPED_STAGE_ROWS >= 3;
  const stages = gapped ? GAPPED_STAGE_ROWS : Math.min(3, body - 1);
  const rest = body - 1 - stages;
  if (rest < 3)
    return {
      flow: 1,
      stages,
      compactSummary: false,
      gapped: false,
      header: 0,
      items: 0,
    };
  return {
    flow: 1,
    stages,
    compactSummary: false,
    gapped,
    header: 1,
    items: Math.min(items, rest - 1),
  };
}
