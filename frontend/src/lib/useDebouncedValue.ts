// TODO(ai-review): review for style and correctness
import { useEffect, useState } from "react";

/// `value` once it has stopped changing for `delayMs`.
export function useDebouncedValue<T>(value: T, delayMs: number): T {
  const [settled, setSettled] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setSettled(value), delayMs);
    return () => clearTimeout(t);
  }, [value, delayMs]);
  return settled;
}
