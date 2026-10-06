/** The waiting-on-you strip: confirms, proposals and pauses that need the operator, one full-width line each. Pure. */
import { ageDetail, cellWidth, truncate } from "./format.js";
import type { Glyphs } from "./glyphs.js";
import { fitLine, span, type Line, type Span } from "./lines.js";
import type { DashModel, WaitingItem } from "./model.js";
import type { Theme } from "./theme.js";

/** Terminals with fewer rows than this get the one-line summary. */
export const STRIP_FULL_ROWS = 30;
/** Item lines shown before `+N more`. */
export const STRIP_MAX_ITEMS = 4;
/** Rows the strip may take at most, whatever the terminal height. */
const STRIP_MAX_ROWS = 5;
/** Rows kept for the header, footer and panels at minimum. */
const KEPT_ROWS = 25;

/** How many rows the strip takes for this model and terminal; 0 when nothing waits. */
export function stripRows(model: DashModel, rows: number): number {
  const n = model.waiting.length;
  if (n === 0) return 0;
  if (rows < STRIP_FULL_ROWS) return 1;
  const lines = Math.min(n, STRIP_MAX_ITEMS) + (n > STRIP_MAX_ITEMS ? 1 : 0);
  const budget = Math.min(STRIP_MAX_ROWS, rows - KEPT_ROWS);
  return lines <= budget ? lines : 1;
}

function plural(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}

function summaryText(items: readonly WaitingItem[]): string {
  const count = (kind: WaitingItem["kind"]) =>
    items.filter((i) => i.kind === kind).length;
  const parts = [
    ...(count("integration") > 0
      ? [plural(count("integration"), "integration confirm")]
      : []),
    ...(count("proposal") > 0 ? [plural(count("proposal"), "proposal")] : []),
    ...(count("run-paused") > 0 ? ["run paused"] : []),
    ...(count("agent-paused") > 0
      ? [plural(count("agent-paused"), "agent") + " paused"]
      : []),
  ];
  return `waiting on you: ${parts.join(", ")}`;
}

function itemLine(
  item: WaitingItem,
  width: number,
  nowMs: number,
  theme: Theme,
  g: Glyphs,
): Line {
  const when = item.since === null ? "" : ageDetail(item.since, nowMs);
  const tail = when === "" ? "" : `  ${when}`;
  const room = Math.max(0, width - 3 - cellWidth(tail) - 1);
  const label = truncate(item.label.replace(/:\s*$/, ""), room);
  const gap = Math.max(0, width - 3 - cellWidth(label) - cellWidth(tail) - 1);
  const paused = item.kind === "run-paused" || item.kind === "agent-paused";
  const spans: Span[] = [
    span(` ${g.attention} `, { color: theme.color("warn"), bold: true }),
    span(label, { color: theme.color(paused ? "warn" : "fg") }),
    span(" ".repeat(gap)),
    span(tail, { color: theme.color("dim") }),
  ];
  return fitLine(spans, width);
}

/** The strip's lines, exactly `stripRows(model, rows)` of them, each `width` cells wide. */
export function waitingStrip(
  model: DashModel,
  size: { readonly columns: number; readonly rows: number },
  nowMs: number,
  theme: Theme,
  g: Glyphs,
): Line[] {
  const rows = stripRows(model, size.rows);
  const items = model.waiting;
  const width = size.columns;
  if (rows === 0) return [];
  if (rows === 1 && (size.rows < STRIP_FULL_ROWS || items.length > 1))
    return [summaryLine(items, width, nowMs, theme, g)];
  const lines = items
    .slice(0, STRIP_MAX_ITEMS)
    .map((item) => itemLine(item, width, nowMs, theme, g));
  if (items.length > STRIP_MAX_ITEMS)
    lines.push(
      fitLine(
        [
          span(`   +${items.length - STRIP_MAX_ITEMS} more`, {
            color: theme.color("dim"),
          }),
        ],
        width,
      ),
    );
  return lines;
}

function summaryLine(
  items: readonly WaitingItem[],
  width: number,
  nowMs: number,
  theme: Theme,
  g: Glyphs,
): Line {
  const times = items
    .map((i) => Date.parse(i.since ?? ""))
    .filter((t) => !Number.isNaN(t));
  const oldest =
    times.length === 0
      ? ""
      : `  oldest ${ageDetail(new Date(Math.min(...times)).toISOString(), nowMs)}`;
  const text = truncate(summaryText(items) + oldest, Math.max(0, width - 4));
  return fitLine(
    [
      span(` ${g.attention} `, { color: theme.color("warn"), bold: true }),
      span(text, { color: theme.color("warn") }),
    ],
    width,
  );
}
