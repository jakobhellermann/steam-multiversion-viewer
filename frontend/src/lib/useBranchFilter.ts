// TODO(ai-review): review for style and correctness
import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

export type BranchFilter = {
  /// Branch names currently hidden. Empty = all shown.
  hidden: Set<string>;
  toggle: (name: string) => void;
  showAll: () => void;
  hideAll: () => void;
  only: (name: string) => void;
};

function defaultHidden(branches: string[]): Set<string> {
  // Default to showing only "public" when it exists; otherwise show all
  // so we never start with an empty list.
  return branches.includes("public") ? new Set(branches.filter((b) => b !== "public")) : new Set();
}

/// Branch visibility filter shared between the app-overview page and the
/// compare menu. Persisted (per appid) in the query cache so the choice
/// survives navigation between the two — the pages are never mounted at
/// the same time, so reading the cache on mount + writing through on
/// change is enough; no live subscription needed.
///
/// `branches` is the selectable set known to the caller (the overview
/// knows every branch; the compare menu only its candidates). `pin` is an
/// optional pre-change hook that captures scroll position and returns a
/// restore fn, so a list reflow doesn't yank the viewport.
export function useBranchFilter(
  appid: number,
  branches: string[],
  pin?: () => () => void,
): BranchFilter {
  const queryClient = useQueryClient();
  const key = ["branch-filter", appid] as const;
  const [hidden, setLocal] = useState<Set<string>>(
    () => queryClient.getQueryData<Set<string>>(key) ?? defaultHidden(branches),
  );
  const set = (next: Set<string> | ((prev: Set<string>) => Set<string>)) => {
    const restore = pin?.();
    setLocal((prev) => {
      const value = typeof next === "function" ? next(prev) : next;
      queryClient.setQueryData(key, value);
      return value;
    });
    if (restore) requestAnimationFrame(restore);
  };
  return {
    hidden,
    toggle: (name) =>
      set((prev) => {
        const n = new Set(prev);
        if (n.has(name)) n.delete(name);
        else n.add(name);
        return n;
      }),
    showAll: () => set(new Set()),
    hideAll: () => set(new Set(branches)),
    only: (name) => set(new Set(branches.filter((b) => b !== name))),
  };
}
