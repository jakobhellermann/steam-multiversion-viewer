// TODO(ai-review): review for style and correctness
import { afterAll, afterEach, beforeAll, vi } from "vitest";
import { cleanup } from "@testing-library/react";
import { server } from "./msw/server";

// Devtools panels mount their own runtime that breaks under jsdom on unmount.
vi.mock("@tanstack/react-devtools", () => ({ TanStackDevtools: () => null }));
vi.mock("@tanstack/react-router-devtools", () => ({
  TanStackRouterDevtoolsPanel: () => null,
}));

// The test DOM has no EventSource; routes get no server events.
class SilentEventSource extends EventTarget {
  close() {}
}
globalThis.EventSource ??= SilentEventSource as unknown as typeof EventSource;

beforeAll(() => server.listen({ onUnhandledRequest: "error" }));
afterEach(() => {
  cleanup();
  server.resetHandlers();
});
afterAll(() => server.close());
