import { useEffect, useState } from "react";
import { Text } from "ink";
import { sparkline } from "../format.js";
import { useTheme } from "../theme.js";

const FRAMES = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏";
const ASCII_FRAMES = "|/-\\";
export const SPINNER_MS = 120;

/** A frame counter that advances every `ms`; stays 0 and sets no timer when `enabled` is false. */
export function useTick(ms: number, enabled: boolean): number {
  const [tick, setTick] = useState(0);
  useEffect(() => {
    if (!enabled) return;
    const timer = setInterval(() => setTick((t) => t + 1), ms);
    return () => clearInterval(timer);
  }, [ms, enabled]);
  return tick;
}

export function Spinner({ tick }: { tick: number }) {
  const theme = useTheme();
  if (theme.reducedMotion) return <Text>*</Text>;
  const frames = theme.ascii ? ASCII_FRAMES : FRAMES;
  return <Text {...theme.paint("cyan")}>{frames[tick % frames.length]}</Text>;
}

export function Sparkline({
  label,
  values,
  width,
}: {
  label: string;
  values: readonly number[];
  width: number;
}) {
  const theme = useTheme();
  return (
    <Text {...theme.paint("gray")}>
      {label} {sparkline(values, width)}
    </Text>
  );
}
