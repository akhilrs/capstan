/** Meters, stacked stage bars and history graphs as pure functions over numbers. */
import type { Glyphs } from "./glyphs.js";
import type { Span } from "./lines.js";
import type { StateColor, Theme } from "./theme.js";

const BRAILLE_LEFT = [0x01, 0x02, 0x04, 0x40] as const;
const BRAILLE_RIGHT = [0x08, 0x10, 0x20, 0x80] as const;

/**
 * An area graph `rows` lines high and `cells` wide. Each braille cell holds
 * two samples side by side and four dot levels; ASCII mode draws `#` columns
 * one level per cell. The newest sample is at the right; missing history is
 * blank on the left. A non-zero value always lights at least its lowest dot.
 */
export function areaGraph(
  series: readonly number[],
  cells: number,
  rows: number,
  max: number,
  ascii: boolean,
): string[] {
  const perCell = ascii ? 1 : 2;
  const need = cells * perCell;
  const samples = series.slice(-need);
  const pad = need - samples.length;
  const scale = Math.max(1, max);
  const levels = ascii ? rows : rows * 4;
  const heights = [
    ...Array<number>(pad).fill(0),
    ...samples.map((v) =>
      v <= 0
        ? 0
        : Math.min(levels, Math.max(1, Math.round((v / scale) * levels))),
    ),
  ];
  const lines: string[] = [];
  for (let row = 0; row < rows; row++) {
    let text = "";
    for (let c = 0; c < cells; c++) {
      if (ascii) {
        text += (heights[c] ?? 0) > rows - 1 - row ? "#" : " ";
        continue;
      }
      let bits = 0;
      for (let dot = 0; dot < 4; dot++) {
        const level = (rows - 1 - row) * 4 + (3 - dot);
        if (level < (heights[2 * c] ?? 0)) bits |= BRAILLE_LEFT[dot]!;
        if (level < (heights[2 * c + 1] ?? 0)) bits |= BRAILLE_RIGHT[dot]!;
      }
      text += bits === 0 ? " " : String.fromCodePoint(0x2800 + bits);
    }
    lines.push(text);
  }
  return lines;
}

/** Graph rows as spans, each row coloured by the height it represents. */
export function graphSpans(lines: readonly string[], theme: Theme): Span[][] {
  return lines.map((text, row) => [
    {
      text,
      color: theme.gradient((lines.length - row - 0.5) / lines.length),
    },
  ]);
}

/** `used` of `limit` as a meter of `cells` cells, coloured along the gradient; empty cells are a different glyph so colour is not needed. */
export function meter(
  used: number,
  limit: number,
  cells: number,
  g: Glyphs,
  theme: Theme,
): Span[] {
  const fraction = limit <= 0 ? (used > 0 ? 1 : 0) : Math.min(1, used / limit);
  const filled = used > 0 ? Math.max(1, Math.round(fraction * cells)) : 0;
  const spans: Span[] = [];
  for (let i = 0; i < filled; i++)
    spans.push({
      text: g.full,
      color: theme.gradient(cells <= 1 ? fraction : i / (cells - 1)),
    });
  if (filled < cells)
    spans.push({
      text: g.empty.repeat(cells - filled),
      color: theme.color("dim"),
    });
  return spans;
}

/** A bar whose fill shows recency: `fraction` 1 is full. One colour, not a load gradient. */
export function recencyBar(
  fraction: number,
  cells: number,
  g: Glyphs,
  theme: Theme,
): Span[] {
  const filled = Math.min(
    cells,
    Math.max(0, Math.ceil(fraction * cells - 1e-9)),
  );
  return [
    { text: g.full.repeat(filled), color: theme.color("ok") },
    { text: g.empty.repeat(cells - filled), color: theme.color("dim") },
  ];
}

export interface StagePart {
  readonly count: number;
  readonly kind: "good" | "progress" | "bad";
}

/** Segments sized by count (at least one cell for a non-zero part), always exactly `cells` wide when there is any count. */
export function stackedBar(
  parts: readonly StagePart[],
  cells: number,
  g: Glyphs,
  theme: Theme,
): Span[] {
  const total = parts.reduce((sum, p) => sum + p.count, 0);
  if (total === 0)
    return [{ text: g.empty.repeat(cells), color: theme.color("dim") }];
  const present = parts.filter((p) => p.count > 0);
  const widths = present.map((p) =>
    Math.max(1, Math.round((p.count / total) * cells)),
  );
  let sum = widths.reduce((a, b) => a + b, 0);
  for (let i = widths.length - 1; sum !== cells && i >= 0;) {
    const step = sum > cells ? -1 : 1;
    if (step === -1 && widths[i]! <= 1) {
      i--;
      continue;
    }
    widths[i] = widths[i]! + step;
    sum += step;
  }
  const color: Record<StagePart["kind"], StateColor> = {
    good: "ok",
    progress: "warn",
    bad: "bad",
  };
  const glyph = {
    good: g.stageGood,
    progress: g.stageProgress,
    bad: g.stageBad,
  };
  return present.map((p, i) => ({
    text: glyph[p.kind].repeat(widths[i]!),
    color: theme.color(color[p.kind]),
  }));
}

const BLOCKS = "▁▂▃▄▅▆▇█";
const ASCII_BLOCKS = "_.-:=+*#";

/** One bar per sample scaled to `max`, `width` cells wide with the newest at the right; missing history is blank. */
export function sparkline(
  series: readonly number[],
  width: number,
  max: number,
  ascii: boolean,
): string {
  const levels = ascii ? ASCII_BLOCKS : BLOCKS;
  const shown = series.slice(-width);
  const scale = Math.max(1, max);
  return (
    " ".repeat(width - shown.length) +
    shown
      .map((v) =>
        v <= 0
          ? levels[0]!
          : levels[Math.min(7, Math.floor((v / scale) * 7.999))]!,
      )
      .join("")
  );
}
