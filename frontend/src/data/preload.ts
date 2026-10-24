// TODO(ai-review): review for style and correctness
import type { QueryClient, QueryKey } from "@tanstack/react-query";

/// Seeds `queryClient` with the entries the backend embedded into `index.html`.
export function applyPreload(queryClient: QueryClient) {
  const script = document.getElementById("preload");
  if (!script) return;
  const entries = JSON.parse(script.textContent!) as { key: QueryKey; data: unknown }[];
  for (const { key, data } of entries) queryClient.setQueryData(key, data);
}
