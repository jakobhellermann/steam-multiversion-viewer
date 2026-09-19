// TODO(ai-review): review for style and correctness
import { describe, expect, it, vi } from "vitest";
import { screen, within } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { server } from "../../test/msw/server";
import { renderRoute } from "../../test/renderRoute";

// Stub root-level siblings so the history route is what's being tested.
// EventSource is unavailable in jsdom and unrelated to history rendering.
vi.mock("../components/DownloadsDrawer", () => ({ DownloadsDrawer: () => null }));
vi.mock("../components/MountToggle", () => ({ MountToggle: () => null }));

const APPID = 480;
const DEPOT = 100;

/// History rows as `[changes, href]` pairs in row order (header
/// dropped). `href` comes from the row's focusable link — the
/// secondary cells are `aria-hidden`, so each row must expose exactly
/// one link.
function readRows(): Array<[string, string]> {
  return screen
    .getAllByRole("row")
    .slice(1)
    .map((row) => {
      const links = within(row).getAllByRole("link");
      expect(links).toHaveLength(1);
      const cells = row.querySelectorAll("td");
      return [cells[2].textContent ?? "", links[0].getAttribute("href") ?? ""];
    });
}

function mockDepotHistory() {
  server.use(
    http.get(`/api/apps/${APPID}`, () =>
      HttpResponse.json({
        appid: APPID,
        name: "TestApp",
        type: "game",
        developer: null,
        publisher: null,
        homepage: null,
        logo_url: null,
        icon_url: "",
        branches: [],
        private_branches: false,
        depots: [
          {
            depot_id: DEPOT,
            oslist: null,
            osarch: null,
            language: null,
            from_app_id: null,
            manifests: [
              { branch: "public", manifest_id: "200", size: 0, download_size: 0 },
              { branch: "public", manifest_id: "150", size: 0, download_size: 0 },
              { branch: "public", manifest_id: "100", size: 0, download_size: 0 },
            ],
          },
        ],
      }),
    ),
    http.get(`/api/apps/${APPID}/extra_manifests`, () => HttpResponse.json([])),
    http.get(`/api/apps/${APPID}/depots/${DEPOT}/manifests/:manifestId/game_info`, () =>
      HttpResponse.json({
        engine: { engine: "unity", data: { version: "2022.3", bundle_version: "9.9.9" } },
      }),
    ),
  );
}

describe("depot history route", () => {
  it("renders one row per version with transition counts, linking changed rows to the compare view", async () => {
    mockDepotHistory();
    let requestBody: unknown;
    server.use(
      http.post(`/api/apps/${APPID}/manifests/history`, async ({ request }) => {
        requestBody = await request.json();
        return HttpResponse.json([
          {
            depot_id: DEPOT,
            manifest_id: "200",
            branch: "public",
            creation_time: 3000,
            previous: { depot_id: DEPOT, manifest_id: "150", branch: "public" },
            added: 2,
            removed: 1,
            changed: 3,
          },
          {
            depot_id: DEPOT,
            manifest_id: "150",
            branch: "public",
            creation_time: 2000,
            previous: { depot_id: DEPOT, manifest_id: "100", branch: "public" },
            added: 0,
            removed: 0,
            changed: 0,
          },
          {
            depot_id: DEPOT,
            manifest_id: "100",
            branch: "public",
            creation_time: 1000,
            added: 0,
            removed: 0,
            changed: 0,
          },
        ]);
      }),
    );

    renderRoute({ initialEntries: [`/apps/${APPID}/depots/${DEPOT}/history`] });

    await screen.findByText("TestApp");
    await screen.findByText("+2");

    // The tracked set from app info is the walk input, newest first.
    expect(requestBody).toEqual({
      manifests: [
        { depot_id: DEPOT, manifest_id: "200", branch: "public" },
        { depot_id: DEPOT, manifest_id: "150", branch: "public" },
        { depot_id: DEPOT, manifest_id: "100", branch: "public" },
      ],
    });
    // Changed row → manifest page with compare_to set to the previous
    // version; unchanged and initial rows → the manifest page itself.
    expect(readRows()).toEqual([
      ["3 +2 −1", `/apps/${APPID}/depots/${DEPOT}/manifests/200?compare_to=150`],
      ["unchanged", `/apps/${APPID}/depots/${DEPOT}/manifests/150`],
      ["initial", `/apps/${APPID}/depots/${DEPOT}/manifests/100`],
    ]);
    // Game version column resolves per version.
    expect(screen.getAllByText("v9.9.9")).toHaveLength(3);
  });
});
