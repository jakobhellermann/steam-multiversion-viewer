import { Link, Outlet, createRootRouteWithContext } from "@tanstack/react-router";
import { TanStackRouterDevtoolsPanel } from "@tanstack/react-router-devtools";
import { TanStackDevtools } from "@tanstack/react-devtools";
import type { QueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";

import { DownloadsDrawer } from "../components/DownloadsDrawer";
import { MountToggle } from "../components/MountToggle";
import "../styles.css";

export const Route = createRootRouteWithContext<{ queryClient: QueryClient }>()({
  component: RootComponent,
});

function RootComponent() {
  // Publish the header's measured height on `<html>` as `--app-header-h`
  // so routes can size their containers against the remaining viewport
  // without hardcoding a pixel guess. Updates whenever the header
  // itself changes size (e.g. on narrow widths).
  const headerRef = useRef<HTMLElement | null>(null);
  useEffect(() => {
    const el = headerRef.current;
    if (!el) return;
    const update = () => {
      document.documentElement.style.setProperty(
        "--app-header-h",
        `${el.getBoundingClientRect().height}px`,
      );
    };
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return (
    <>
      <header ref={headerRef} className="border-b border-slate-800">
        <div className="mx-auto flex max-w-4xl items-center px-8 py-3">
          <Link to="/" className="font-medium text-sky-400 hover:underline">
            Library
          </Link>
          <div className="ml-auto flex items-center gap-2">
            <MountToggle />
            <Link
              to="/settings"
              className="text-slate-400 hover:text-sky-400"
              aria-label="Settings"
              title="Settings"
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                width="20"
                height="20"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="2"
                strokeLinecap="round"
                strokeLinejoin="round"
              >
                <circle cx="12" cy="12" r="3" />
                <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09a1.65 1.65 0 0 0 1.51-1 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33h0a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51h0a1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82v0a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
              </svg>
            </Link>
          </div>
        </div>
      </header>
      <Outlet />
      <DownloadsDrawer />
      <TanStackDevtools
        config={{ position: "bottom-right" }}
        plugins={[{ name: "TanStack Router", render: <TanStackRouterDevtoolsPanel /> }]}
      />
    </>
  );
}
