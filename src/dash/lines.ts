/** Styled text as data: a screen is a list of lines, a line is a list of spans. Pure; Ink only paints it. */
import { cellWidth, fit } from "./format.js";

export interface Span {
  readonly text: string;
  readonly color?: string | undefined;
  readonly bg?: string | undefined;
  readonly bold?: boolean | undefined;
  readonly dim?: boolean | undefined;
}

export type Line = readonly Span[];

export function span(text: string, style: Omit<Span, "text"> = {}): Span {
  return { text, ...style };
}

export function plainLine(line: Line): string {
  return line.map((s) => s.text).join("");
}

export function plainLines(lines: readonly Line[]): string[] {
  return lines.map(plainLine);
}

export function lineWidth(line: Line): number {
  return line.reduce((sum, s) => sum + cellWidth(s.text), 0);
}

/** The line cut or padded to exactly `width` cells. Cutting keeps the styles of the spans that survive. */
export function fitLine(line: Line, width: number): Line {
  const out: Span[] = [];
  let used = 0;
  for (const s of line) {
    const room = width - used;
    if (room <= 0) break;
    const w = cellWidth(s.text);
    if (w <= room) {
      out.push(s);
      used += w;
      continue;
    }
    out.push({ ...s, text: fit(s.text, room) });
    used = width;
    break;
  }
  if (used < width) out.push({ text: " ".repeat(width - used) });
  return out;
}

export function blankLine(width: number): Line {
  return [{ text: " ".repeat(width) }];
}

/** Every span of the line with the style overrides applied. */
export function styleLine(line: Line, style: Omit<Span, "text">): Line {
  return line.map((s) => ({ ...s, ...style }));
}

/** Merges neighbouring spans of equal style so the painter emits fewer elements. */
export function mergeSpans(line: Line): Line {
  const out: Span[] = [];
  for (const s of line) {
    const last = out[out.length - 1];
    if (
      last !== undefined &&
      last.color === s.color &&
      last.bg === s.bg &&
      last.bold === s.bold &&
      last.dim === s.dim
    )
      out[out.length - 1] = { ...last, text: last.text + s.text };
    else if (s.text !== "") out.push(s);
  }
  return out;
}

/** The ASCII fallback of a line: the ellipsis becomes `~`. Every other glyph already comes from the ASCII glyph set. */
export function asciiLine(line: Line): Line {
  return line.map((s) =>
    s.text.includes("…") ? { ...s, text: s.text.replaceAll("…", "~") } : s,
  );
}
