export { clean } from "../watch.js";

const SPARK = "▁▂▃▄▅▆▇█";
const WIDE = /[ᄀ-ᅟ⺀-꓏가-힣豈-﫿︰-﹯＀-｠￠-￦]|\p{Extended_Pictographic}/u;
const segmenter = new Intl.Segmenter(undefined, { granularity: "grapheme" });

function graphemeWidth(grapheme: string): number {
  return WIDE.test(grapheme) ? 2 : 1;
}

/** Terminal cells the text takes: wide characters count two. */
export function cellWidth(text: string): number {
  let width = 0;
  for (const { segment } of segmenter.segment(text))
    width += graphemeWidth(segment);
  return width;
}

/** The text cut to `width` cells with an ellipsis, never splitting a character. */
export function truncate(text: string, width: number): string {
  if (width <= 0) return "";
  if (cellWidth(text) <= width) return text;
  let used = 0;
  let out = "";
  for (const { segment } of segmenter.segment(text)) {
    const w = graphemeWidth(segment);
    if (used + w > width - 1) break;
    out += segment;
    used += w;
  }
  return `${out}…`;
}

/** The text cut or padded on the right to exactly `width` cells. */
export function fit(text: string, width: number): string {
  const cut = truncate(text, width);
  return cut + " ".repeat(Math.max(0, width - cellWidth(cut)));
}

/** Seconds, minutes, hours or days since `iso`; `-` when the time is missing or unreadable. */
export function age(iso: string | null | undefined, nowMs: number): string {
  if (!iso) return "-";
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return "-";
  const seconds = Math.max(0, Math.floor((nowMs - then) / 1000));
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  if (seconds < 86_400) return `${Math.floor(seconds / 3600)}h`;
  return `${Math.floor(seconds / 86_400)}d`;
}

export function commitShort(sha: string | null | undefined): string {
  return sha ? sha.slice(0, 7) : "-";
}

/** One bar per sample, scaled to the largest value in the series (a flat zero series is the lowest bar). */
export function sparkline(values: readonly number[], width: number): string {
  const shown = values.slice(-width);
  const max = Math.max(...shown, 0);
  return shown
    .map((v) =>
      max === 0 ? SPARK[0]! : SPARK[Math.min(7, Math.floor((v / max) * 7))]!,
    )
    .join("");
}

/** Appends a sample and keeps the newest `limit`. */
export function pushSample(
  ring: readonly number[],
  value: number,
  limit: number,
): number[] {
  return [...ring, value].slice(-limit);
}
