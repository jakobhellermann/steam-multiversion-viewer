// TODO(ai-review): review for style and correctness
import { render } from "@testing-library/react";
import { RouterProvider, createMemoryHistory, createRouter } from "@tanstack/react-router";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createQueryClient } from "#/queryClient";
import { routeTree } from "#/routeTree.gen";
import { parseSearch, stringifySearch } from "#/searchParams";

export type RenderRouteOptions = {
  /** Initial URL stack. First entry wins; later entries simulate prior navigations. */
  initialEntries?: string[];
  /** Reuse a QueryClient across mounts to assert cache hits. */
  queryClient?: QueryClient;
};

export { createQueryClient };

export function renderRoute(opts: RenderRouteOptions = {}) {
  const queryClient = opts.queryClient ?? createQueryClient();
  const history = createMemoryHistory({ initialEntries: opts.initialEntries ?? ["/"] });
  const router = createRouter({
    routeTree,
    history,
    context: { queryClient },
    // Same search (de)serialisation as the app router, so tests build
    // the URLs the app builds.
    parseSearch,
    stringifySearch,
    // Disable intent-preload so tests don't fire hover-driven prefetches.
    defaultPreload: false,
  });

  const utils = render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );

  return { ...utils, router, queryClient };
}
