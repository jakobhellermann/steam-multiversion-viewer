import ReactDOM from "react-dom/client";
import { RouterProvider, createRouter } from "@tanstack/react-router";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fetchLibrary } from "./api";
import { createQueryClient } from "./queryClient";
import { routeTree } from "./routeTree.gen";
import { parseSearch, stringifySearch } from "./searchParams";

const queryClient = createQueryClient();

// Always have the library warm so navigating back is instant.
queryClient.prefetchQuery({ queryKey: ["library"], queryFn: fetchLibrary });

// Custom (de)serialiser that keeps URL search-param values as strings
// lives in `./searchParams` — shared with the test router so route
// tests build the same URLs as the app.
const router = createRouter({
  routeTree,
  defaultPreload: "intent",
  defaultPreloadStaleTime: Infinity,
  scrollRestoration: true,
  parseSearch,
  stringifySearch,
  context: { queryClient } as { queryClient: QueryClient },
});

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}

// Entry point for the native macOS menu bar, which has no other way to
// navigate the SPA without forcing a full page reload.
declare global {
  interface Window {
    __navigate?: (to: string) => void;
  }
}

window.__navigate = (to) => {
  void router.navigate({ to: to as never });
};

const rootElement = document.getElementById("app")!;

if (!rootElement.innerHTML) {
  const root = ReactDOM.createRoot(rootElement);
  root.render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
}
