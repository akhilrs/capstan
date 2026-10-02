/** Titled borders: panel title, hotkey number and tabs embedded in the top edge, tabs and a counter in the bottom edge. Pure. */
import { cellWidth, truncate } from "./format.js";
import type { Glyphs } from "./glyphs.js";
import type { Line, Span } from "./lines.js";

export interface Tab {
  readonly text: string;
  readonly color?: string | undefined;
}

export interface BorderColors {
  readonly border?: string | undefined;
  readonly title?: string | undefined;
}

function tabSpans(tabs: readonly Tab[], g: Glyphs, border?: string): Span[] {
  return tabs.flatMap((tab): Span[] => [
    { text: g.tabOpen, color: border },
    { text: ` ${tab.text} `, color: tab.color },
    { text: g.tabClose, color: border },
  ]);
}

function tabsWidth(tabs: readonly Tab[]): number {
  return tabs.reduce((sum, t) => sum + cellWidth(t.text) + 4, 0);
}

export interface TopBorder {
  readonly number?: number | undefined;
  readonly title: string;
  /** Tabs right after the title, for the header. */
  readonly leftTabs?: readonly Tab[] | undefined;
  /** Right-aligned tabs; the last ones are dropped first when the line is too narrow. */
  readonly tabs?: readonly Tab[] | undefined;
  readonly focused: boolean;
}

/** The top edge as one line of exactly `width` cells. */
export function topBorder(
  width: number,
  o: TopBorder,
  g: Glyphs,
  colors: BorderColors,
): Line {
  const e = o.focused ? g.edgeFocused : g.edge;
  const prefix = o.number === undefined ? "" : (g.hotkey[o.number] ?? "");
  let title = `${prefix}${o.title}`;
  let left = [...(o.leftTabs ?? [])];
  let right = [...(o.tabs ?? [])];
  const fill = () =>
    width -
    3 -
    cellWidth(title) -
    (left.length > 0 ? 1 + tabsWidth(left) : 0) -
    (right.length > 0 ? tabsWidth(right) + 1 : 0);
  while (fill() < 0 && right.length > 0) right = right.slice(0, -1);
  while (fill() < 0 && left.length > 0) left = left.slice(0, -1);
  if (fill() < 0)
    title = truncate(title, Math.max(1, cellWidth(title) + fill()));
  const b = colors.border;
  return [
    { text: e.tl + e.h, color: b },
    { text: title, color: colors.title, bold: true },
    ...(left.length > 0
      ? [{ text: e.h, color: b }, ...tabSpans(left, g, b)]
      : []),
    { text: e.h.repeat(Math.max(0, fill())), color: b },
    ...tabSpans(right, g, b),
    { text: (right.length > 0 ? e.h : "") + e.tr, color: b },
  ];
}

export interface BottomBorder {
  readonly left?: readonly Tab[] | undefined;
  readonly right?: readonly Tab[] | undefined;
  readonly focused: boolean;
}

/** The bottom edge as one line of exactly `width` cells. */
export function bottomBorder(
  width: number,
  o: BottomBorder,
  g: Glyphs,
  border?: string,
): Line {
  const e = o.focused ? g.edgeFocused : g.edge;
  let left = [...(o.left ?? [])];
  let right = [...(o.right ?? [])];
  const fill = () =>
    width -
    2 -
    (left.length > 0 ? 1 + tabsWidth(left) : 0) -
    (right.length > 0 ? tabsWidth(right) + 1 : 0);
  while (fill() < 0 && left.length > 0) left = left.slice(0, -1);
  while (fill() < 0 && right.length > 0) right = right.slice(0, -1);
  return [
    { text: e.bl + (left.length > 0 ? e.h : ""), color: border },
    ...tabSpans(left, g, border),
    { text: e.h.repeat(Math.max(0, fill())), color: border },
    ...tabSpans(right, g, border),
    { text: (right.length > 0 ? e.h : "") + e.br, color: border },
  ];
}

/** The scroll thumb range inside `rows` body rows: where the visible window of `view` out of `total` sits. */
export function thumbRange(
  total: number,
  view: number,
  top: number,
  rows: number,
): { readonly start: number; readonly size: number } | undefined {
  if (total <= view || view <= 0 || rows <= 0) return undefined;
  const size = Math.max(1, Math.round((view / total) * rows));
  const travel = rows - size;
  const start = Math.round((top / (total - view)) * travel);
  return { start, size };
}
