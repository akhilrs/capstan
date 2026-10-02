import { useEffect, useState } from "react";

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
