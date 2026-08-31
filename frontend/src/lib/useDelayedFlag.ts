import { useEffect, useState } from "react";

/// True once `active` has stayed true continuously for `delayMs`.
/// Resets immediately when `active` goes false. Use to defer a
/// loading spinner past a short fetch so a fast response never flashes.
export function useDelayedFlag(active: boolean, delayMs: number): boolean {
  const [elapsed, setElapsed] = useState(false);
  useEffect(() => {
    if (!active) {
      setElapsed(false);
      return;
    }
    const t = setTimeout(() => setElapsed(true), delayMs);
    return () => clearTimeout(t);
  }, [active, delayMs]);
  return active && elapsed;
}
