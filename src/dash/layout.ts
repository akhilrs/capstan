/** Pure layout decisions: which mode a terminal size gets and how many rows each panel may use. */

export const MIN_COLUMNS = 60;
export const MIN_ROWS = 16;
export const WIDE_COLUMNS = 100;
/** Rows taken by the header and by the footer. */
export const HEADER_ROWS = 2;
export const FOOTER_ROWS = 2;

export type LayoutMode = "tiny" | "narrow" | "wide";
export type PanelId = "agents" | "pipeline" | "queue" | "findings" | "work";

export interface Layout {
  readonly mode: LayoutMode;
  readonly sparklines: boolean;
}

export interface Cells {
  readonly showRole: boolean;
  readonly showAge: boolean;
  readonly showNotified: boolean;
  readonly showPane: boolean;
}

export function layoutFor(columns: number, rows: number): Layout {
  const mode: LayoutMode =
    columns < MIN_COLUMNS || rows < MIN_ROWS
      ? "tiny"
      : columns >= WIDE_COLUMNS
        ? "wide"
        : "narrow";
  return { mode, sparklines: mode === "wide" };
}

/** Optional columns of a panel row, dropped as the panel narrows: pane first, then role, notified and age. */
export function cellsFor(panelWidth: number): Cells {
  return {
    showAge: panelWidth >= 50,
    showNotified: panelWidth >= 75,
    showRole: panelWidth >= 90,
    showPane: panelWidth >= 100,
  };
}

/** Panels per column: stacked in one column, or split across two. */
export function columnsOf(
  mode: LayoutMode,
  panels: readonly PanelId[],
): readonly (readonly PanelId[])[] {
  if (mode !== "wide") return [panels];
  const left = panels.filter((p) => p === "agents" || p === "pipeline");
  const right = panels.filter((p) => p !== "agents" && p !== "pipeline");
  return [left, right];
}

/**
 * Body rows per panel inside a column of `rows` lines. Each panel costs one
 * title line; the rest is shared evenly with the leftover going to the
 * focused panel. Every panel keeps at least one body row.
 */
export function allocate(
  rows: number,
  panels: readonly PanelId[],
  focused: PanelId,
): ReadonlyMap<PanelId, number> {
  const result = new Map<PanelId, number>();
  if (panels.length === 0) return result;
  const body = Math.max(panels.length, rows - panels.length);
  const base = Math.floor(body / panels.length);
  const extra = body - base * panels.length;
  for (const panel of panels) result.set(panel, Math.max(1, base));
  const target = panels.includes(focused) ? focused : panels[0]!;
  result.set(target, result.get(target)! + extra);
  return result;
}

/** The slice of `count` rows that fits `capacity` and keeps `cursor` visible. */
export function windowOf(
  count: number,
  cursor: number,
  capacity: number,
): { readonly start: number; readonly end: number; readonly hidden: number } {
  if (capacity >= count) return { start: 0, end: count, hidden: 0 };
  const start = Math.min(Math.max(0, cursor - capacity + 1), count - capacity);
  return { start, end: start + capacity, hidden: count - capacity };
}
