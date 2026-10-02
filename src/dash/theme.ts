import { createContext, useContext } from "react";

export type StateColor = "ok" | "warn" | "bad" | "dim";

export type ColorRole =
  | "fg"
  | "bright"
  | "dim"
  | "ok"
  | "warn"
  | "bad"
  | "info"
  | "selectBg"
  | "border.header"
  | "border.agents"
  | "border.agents.focus"
  | "border.pipeline"
  | "border.pipeline.focus"
  | "border.queue"
  | "border.queue.focus"
  | "border.findings"
  | "border.findings.focus"
  | "border.work"
  | "border.work.focus"
  | "overlay";

/** Truecolor values; chalk downsamples them to 256 or 16 colours by the terminal's capability. */
export const PALETTE: Readonly<Record<ColorRole, string>> = {
  fg: "#cccccc",
  bright: "#eeeeee",
  dim: "#6c6c6c",
  ok: "#77ca9b",
  warn: "#cbc06c",
  bad: "#dc4c4c",
  info: "#6cb4d8",
  selectBg: "#3b4252",
  "border.header": "#5a5a5a",
  "border.agents": "#556d59",
  "border.agents.focus": "#77ca9b",
  "border.pipeline": "#5c588d",
  "border.pipeline.focus": "#8a85c9",
  "border.queue": "#805252",
  "border.queue.focus": "#d17f7f",
  "border.findings": "#6c6c4b",
  "border.findings.focus": "#cbc06c",
  "border.work": "#4b6a7a",
  "border.work.focus": "#7fb3cf",
  overlay: "#eeeeee",
};

const GRADIENT_STOPS = [
  { at: 0, hex: PALETTE.ok },
  { at: 0.6, hex: PALETTE.warn },
  { at: 1, hex: PALETTE.bad },
] as const;

function channels(hex: string): [number, number, number] {
  const n = Number.parseInt(hex.slice(1), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

/** The meter and graph colour at `fraction` (0 to 1): ok, then warn at 60 %, then bad. */
export function gradientColor(fraction: number): string {
  const t = Math.min(1, Math.max(0, fraction));
  const upper = GRADIENT_STOPS.findIndex((stop) => t <= stop.at);
  if (upper <= 0) return GRADIENT_STOPS[0].hex;
  const from = GRADIENT_STOPS[upper - 1]!;
  const to = GRADIENT_STOPS[upper]!;
  const k = (t - from.at) / (to.at - from.at);
  const a = channels(from.hex);
  const b = channels(to.hex);
  return `#${[0, 1, 2]
    .map((i) =>
      Math.round(a[i]! + (b[i]! - a[i]!) * k)
        .toString(16)
        .padStart(2, "0"),
    )
    .join("")}`;
}

export interface Theme {
  readonly noColor: boolean;
  readonly reducedMotion: boolean;
  readonly ascii: boolean;
  /** The colour of a role, or undefined when colour is off. */
  color(role: ColorRole): string | undefined;
  /** The gradient colour at `fraction`, or undefined when colour is off. */
  gradient(fraction: number): string | undefined;
}

export function makeTheme(options: {
  noColor: boolean;
  reducedMotion: boolean;
  ascii?: boolean;
}): Theme {
  return {
    noColor: options.noColor,
    reducedMotion: options.reducedMotion,
    ascii: options.ascii ?? false,
    color: (role) => (options.noColor ? undefined : PALETTE[role]),
    gradient: (fraction) =>
      options.noColor ? undefined : gradientColor(fraction),
  };
}

export const ThemeContext = createContext<Theme>(
  makeTheme({ noColor: false, reducedMotion: false }),
);

export function useTheme(): Theme {
  return useContext(ThemeContext);
}

/** Colour is never the only carrier of meaning: every state also has a word or a symbol. */
export function colorOfState(state: string): StateColor {
  if (
    [
      "failed",
      "conflicted",
      "degraded",
      "escalated",
      "expired",
      "rejected",
      "unacked",
      "findings",
    ].includes(state)
  )
    return "bad";
  if (
    [
      "deferred",
      "queued",
      "sent",
      "started",
      "running",
      "evaluating",
      "open",
    ].includes(state)
  )
    return "warn";
  if (
    [
      "passed",
      "merged",
      "confirmed",
      "accepted",
      "healthy",
      "active",
      "acked",
    ].includes(state)
  )
    return "ok";
  return "dim";
}
