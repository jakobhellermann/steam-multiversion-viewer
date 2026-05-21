import { Link, Outlet, createRootRouteWithContext } from "@tanstack/react-router";
import { TanStackRouterDevtoolsPanel } from "@tanstack/react-router-devtools";
import { TanStackDevtools } from "@tanstack/react-devtools";
import type { QueryClient } from "@tanstack/react-query";

import "../styles.css";

export const Route = createRootRouteWithContext<{ queryClient: QueryClient }>()({
  component: RootComponent,
});

function RootComponent() {
  return (
    <>
      <header className="border-b border-slate-800">
        <div className="max-w-4xl mx-auto px-8 py-3">
          <Link to="/" className="text-sky-400 hover:underline font-medium">
            Library
          </Link>
        </div>
      </header>
      <Outlet />
      <TanStackDevtools
        config={{ position: "bottom-right" }}
        plugins={[{ name: "TanStack Router", render: <TanStackRouterDevtoolsPanel /> }]}
      />
    </>
  );
}
