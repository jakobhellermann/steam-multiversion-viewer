// TODO(ai-review): review for style and correctness
import { hashKey, type Query, type QueryClient, type QueryKey } from "@tanstack/react-query";

const preloaded = new Set<string>();
let onInitialPage = true;

/// Seeds `queryClient` with the entries the backend embedded into `index.html`.
export function applyPreload(queryClient: QueryClient) {
  const script = document.getElementById("preload");
  if (!script) return;
  const entries = JSON.parse(script.textContent!) as {
    key: QueryKey;
    data: unknown;
    updated_at?: number;
    incomplete?: boolean;
  }[];
  for (const { key, data, updated_at, incomplete } of entries) {
    queryClient.setQueryData(key, data, { updatedAt: updated_at });
    if (incomplete) {
      // Shown until the refetch that queries with `refetchOnMount` start for stale data.
      void queryClient.invalidateQueries({ queryKey: key, exact: true, refetchType: "none" });
    } else {
      preloaded.add(hashKey(key));
    }
  }
}

/// Marks the end of the page the HTML was served for; later mounts no longer see its preloads
/// as just delivered.
export function leaveInitialPage() {
  onInitialPage = false;
}

/// Whether the query holds data the HTML delivered for the page still shown, for
/// `refetchOnMount` callbacks that would otherwise refetch it right away.
export function isFreshPreload(query: Pick<Query, "queryHash">): boolean {
  return onInitialPage && preloaded.has(query.queryHash);
}
