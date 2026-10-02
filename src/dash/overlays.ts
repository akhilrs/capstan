/** Floating boxes drawn over the dashboard: help, the action confirmation and the observe screen. Pure. */
import { confirmText, type DashAction } from "./actions.js";
import { bottomBorder, topBorder, type Tab } from "./border.js";
import { cellWidth, clean, truncate } from "./format.js";
import { glyphsFor, type Glyphs } from "./glyphs.js";
import { asciiLine, fitLine, mergeSpans, span, type Line } from "./lines.js";
import type { Theme } from "./theme.js";

export interface Overlay {
  readonly lines: readonly Line[];
  readonly top: number;
  readonly left: number;
}

interface BoxSpec {
  readonly title: string;
  readonly tabs?: readonly Tab[];
  readonly bottomLeft?: readonly Tab[];
  readonly bottomRight?: readonly Tab[];
  readonly body: readonly Line[];
  readonly width: number;
  readonly size: { readonly columns: number; readonly rows: number };
}

function box(spec: BoxSpec, theme: Theme, g: Glyphs): Overlay {
  const width = Math.max(20, Math.min(spec.width, spec.size.columns - 4));
  const height = Math.min(spec.size.rows - 2, spec.body.length + 2);
  const border = theme.color("overlay");
  const edge = g.edgeFocused;
  const rows = height - 2;
  const lines: Line[] = [
    topBorder(width, { title: spec.title, tabs: spec.tabs, focused: true }, g, {
      border,
      title: theme.color("bright"),
    }),
  ];
  for (let r = 0; r < rows; r++)
    lines.push(
      mergeSpans([
        span(edge.v, { color: border }),
        span(" "),
        ...fitLine(spec.body[r] ?? [], width - 4),
        span(" "),
        span(edge.v, { color: border }),
      ]),
    );
  lines.push(
    bottomBorder(
      width,
      { left: spec.bottomLeft, right: spec.bottomRight, focused: true },
      g,
      border,
    ),
  );
  return {
    lines: theme.ascii ? lines.map(asciiLine) : lines,
    top: Math.floor((spec.size.rows - height) / 2),
    left: Math.floor((spec.size.columns - width) / 2),
  };
}

/** Words wrapped to `width` cells; a word longer than the width is cut. */
export function wrapText(text: string, width: number): string[] {
  const lines: string[] = [];
  let current = "";
  for (const word of text.split(/\s+/).filter((w) => w !== "")) {
    const next = current === "" ? word : `${current} ${word}`;
    if (cellWidth(next) <= width) current = next;
    else {
      if (current !== "") lines.push(current);
      current = truncate(word, width);
    }
  }
  if (current !== "") lines.push(current);
  return lines;
}

const text = (
  theme: Theme,
  value: string,
  role: "fg" | "dim" | "bad" | "warn" | "bright" = "fg",
): Line => [span(value, { color: theme.color(role) })];

export function helpOverlay(
  size: { columns: number; rows: number },
  theme: Theme,
): Overlay {
  const g = glyphsFor(theme.ascii);
  const head = (t: string): Line => text(theme, t, "bright");
  const body: Line[] = [
    head("NAVIGATE"),
    text(theme, "  tab / shift+tab   next / previous panel"),
    text(theme, "  1-5               jump to a panel"),
    text(theme, `  ${g.up} ${g.down}  or  j k      move the selected row`),
    text(theme, ""),
    head("ACT   (always asks first; the daemon decides)"),
    text(theme, "  o   observe the selected agent (read-only)"),
    text(theme, "  y retry   s skip   c cancel   the selected message"),
    text(theme, "      then press y again to confirm"),
    text(theme, "  f   queue: show only delivery problems"),
    text(theme, ""),
    head("VIEW"),
    text(theme, "  p   pause / resume polling      r   poll now"),
    text(theme, "  -   +   shorter / longer poll interval (1 to 60 s)"),
    text(theme, "  q   quit (ctrl+c too)       ?   close this help"),
    text(theme, ""),
    head("READING THE SCREEN"),
    text(
      theme,
      `  ${g.idle} idle   ${theme.reducedMotion ? g.working : g.spinner[0]} working (inferred)   ${g.attention} needs attention`,
    ),
    text(
      theme,
      `  ${g.ended} ended   ${g.selected} selected row   ${g.thumb} scroll position`,
    ),
    text(theme, "  working = activity within 30 s or a message in flight"),
  ];
  return box(
    {
      title: "help",
      bottomRight: [{ text: "? or esc closes", color: theme.color("dim") }],
      body,
      width: 62,
      size,
    },
    theme,
    g,
  );
}

export function confirmOverlay(
  action: DashAction,
  size: { columns: number; rows: number },
  theme: Theme,
): Overlay {
  const g = glyphsFor(theme.ascii);
  const width = Math.min(64, size.columns - 4);
  const verb = action.kind === "observe" ? "observe" : action.decision;
  const body: Line[] = wrapText(confirmText(action), width - 4).map((l) =>
    text(theme, l),
  );
  return box(
    {
      title: "confirm",
      tabs: [{ text: verb, color: theme.color("warn") }],
      body: [text(theme, ""), ...body, text(theme, "")],
      width,
      size,
    },
    theme,
    g,
  );
}

export interface PeekView {
  readonly agentId: string;
  readonly agentStatus: string;
  readonly text: string;
}

export function observeOverlay(
  peek: PeekView,
  size: { columns: number; rows: number },
  theme: Theme,
): Overlay {
  const g = glyphsFor(theme.ascii);
  const width = Math.min(104, size.columns - 4);
  const capacity = Math.max(1, Math.min(size.rows - 4, 28) - 2);
  const all = peek.text.split("\n").map((l) => clean(l));
  const tail = all.slice(-capacity);
  const body: Line[] = tail.map((l) => text(theme, truncate(l, width - 4)));
  return box(
    {
      title: `observe ${clean(peek.agentId)}`,
      tabs: [
        {
          text: `agent status: ${peek.agentStatus}`,
          color: theme.color("info"),
        },
        { text: "unverified text", color: theme.color("warn") },
      ],
      bottomLeft: [{ text: "esc or q closes", color: theme.color("dim") }],
      bottomRight: [
        { text: `tail, ${tail.length} lines`, color: theme.color("dim") },
      ],
      body: [
        ...body,
        ...Array.from({ length: Math.max(0, capacity - body.length) }, () =>
          text(theme, ""),
        ),
      ],
      width,
      size,
    },
    theme,
    g,
  );
}
