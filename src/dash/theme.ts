import { createContext, useContext } from "react";

export type ColorName = "red" | "yellow" | "green" | "cyan" | "gray";

export interface Theme {
  readonly noColor: boolean;
  readonly reducedMotion: boolean;
  readonly ascii: boolean;
  /** Text props for a colour; empty when colour is off. */
  paint(name: ColorName): { readonly color?: ColorName };
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
    paint: (name) => (options.noColor ? {} : { color: name }),
  };
}

export const ThemeContext = createContext<Theme>(
  makeTheme({ noColor: false, reducedMotion: false }),
);

export function useTheme(): Theme {
  return useContext(ThemeContext);
}

/** Colour is never the only carrier of meaning: every state also has a word or a symbol. */
export function colorOfState(
  state: string,
): "red" | "yellow" | "green" | "gray" {
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
    return "red";
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
    return "yellow";
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
    return "green";
  return "gray";
}
