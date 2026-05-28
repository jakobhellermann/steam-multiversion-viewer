// TODO(ai-review): review for style and correctness
import { describe, expect, it, vi } from "vitest";
import { screen } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { server } from "../../test/msw/server";
import { libraryFixture } from "../../test/msw/handlers";
import { createQueryClient, renderRoute } from "../../test/renderRoute";

// Stub root-level siblings so the library route is what we're testing.
// EventSource is unavailable in jsdom and unrelated to library rendering.
vi.mock("../components/DownloadsDrawer", () => ({ DownloadsDrawer: () => null }));
vi.mock("../components/MountToggle", () => ({ MountToggle: () => null }));

/** Read the library table as `[name, playtime]` pairs in row order. */
function readTable(): Array<[string, string]> {
  const bodyRows = screen.getAllByRole("row").slice(1); // drop header
  return bodyRows.map((row) => {
    const cells = row.querySelectorAll("td");
    return [cells[0].textContent ?? "", cells[1].textContent ?? ""];
  });
}

describe("library route", () => {
  it("renders rows from the library response, sorted by playtime desc", async () => {
    renderRoute({ initialEntries: ["/"] });

    await screen.findByText("Half-Life 2");

    expect(readTable()).toEqual([
      ["Half-Life 2", "12.0 h"],
      ["Team Fortress 2", "1.0 h"],
      ["Dota 2", "12 min"],
    ]);
  });

  it("shows an error box when the library endpoint fails", async () => {
    server.use(
      http.get("/api/library", () => HttpResponse.json({ error: "boom" }, { status: 500 })),
    );

    renderRoute({ initialEntries: ["/"] });

    const heading = await screen.findByText("Failed to load library");
    // ErrorBox should render the message from the API, not a generic fallback.
    expect(heading.parentElement?.textContent).toContain("boom");
  });

  it("serves library data from cache on remount — only one network call", async () => {
    let calls = 0;
    server.use(
      http.get("/api/library", () => {
        calls += 1;
        return HttpResponse.json(libraryFixture);
      }),
    );

    const queryClient = createQueryClient();

    const first = renderRoute({ queryClient });
    await screen.findByText("Half-Life 2");
    first.unmount();

    renderRoute({ queryClient });
    await screen.findByText("Half-Life 2");
    // The router transition is async even with a warm cache, but the data
    // itself must come from the cache — the network handler must not run.
    expect(calls).toBe(1);
  });

  it("inverse: with no shared cache, each mount hits the network", async () => {
    let calls = 0;
    server.use(
      http.get("/api/library", () => {
        calls += 1;
        return HttpResponse.json(libraryFixture);
      }),
    );

    const a = renderRoute();
    await screen.findByText("Half-Life 2");
    a.unmount();

    renderRoute();
    await screen.findByText("Half-Life 2");

    // Proves the previous test isn't trivially passing — a fresh QueryClient
    // really does trigger a fetch.
    expect(calls).toBe(2);
  });
});
