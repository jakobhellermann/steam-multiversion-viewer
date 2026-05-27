import ReactDOM from "react-dom/client";
import {
  RouterProvider,
  createRouter,
  parseSearchWith,
  stringifySearchWith,
} from "@tanstack/react-router";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fetchLibrary } from "./api";
import { routeTree } from "./routeTree.gen";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: false,
      staleTime: Infinity,
      refetchOnMount: false,
      refetchOnWindowFocus: false,
    },
  },
});

// Always have the library warm so navigating back is instant.
queryClient.prefetchQuery({ queryKey: ["library"], queryFn: fetchLibrary });

// Custom (de)serialiser that keeps URL search-param values as strings,
// disabling the default JSON-ish coercion that turns `?id=12345…` into
// a `number`. Steam manifest IDs are i64s that exceed
// `Number.MAX_SAFE_INTEGER`, so the default round-trips them with
// precision loss — we keep them verbatim.
const router = createRouter({
  routeTree,
  defaultPreload: "intent",
  defaultPreloadStaleTime: Infinity,
  scrollRestoration: true,
  parseSearch: parseSearchWith((s) => s),
  stringifySearch: stringifySearchWith((v) => (typeof v === "string" ? v : String(v))),
  context: { queryClient } as { queryClient: QueryClient },
});

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}

const rootElement = document.getElementById("app")!;

if (!rootElement.innerHTML) {
  const root = ReactDOM.createRoot(rootElement);
  root.render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
}
