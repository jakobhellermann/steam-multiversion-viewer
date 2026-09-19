// TODO(ai-review): review for style and correctness
import { describe, expect, it, vi } from "vitest";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import type { ManifestDiffEntry } from "../api";
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

/// App info with one depot whose manifests are the given
/// `[branch, manifest_id]` pairs, no extras, unity game info for every
/// manifest.
function mockApp(manifests: Array<[string, string]>) {
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
            manifests: manifests.map(([branch, manifest_id]) => ({
              branch,
              manifest_id,
              size: 0,
              download_size: 0,
            })),
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
    mockApp([
      ["public", "200"],
      ["public", "150"],
      ["public", "100"],
    ]);
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

    // The tracked set from app info is the walk input, newest first,
    // and carries only the wire fields (no client-only `branches`).
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

  it("keeps a version that both branch heads point at under the public-only default filter", async () => {
    mockApp([
      ["public", "200"],
      ["public-beta", "200"],
      ["public", "100"],
    ]);
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
            previous: { depot_id: DEPOT, manifest_id: "100", branch: "public" },
            added: 1,
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

    await screen.findByText("+1");

    // Gid 200 is the public AND the beta head; the default filter hides
    // beta, but the version still participates through its public
    // branch — and rides the wire through it.
    expect(requestBody).toEqual({
      manifests: [
        { depot_id: DEPOT, manifest_id: "200", branch: "public" },
        { depot_id: DEPOT, manifest_id: "100", branch: "public" },
      ],
    });
    expect(readRows()).toEqual([
      ["+1", `/apps/${APPID}/depots/${DEPOT}/manifests/200?compare_to=100`],
      ["initial", `/apps/${APPID}/depots/${DEPOT}/manifests/100`],
    ]);
  });

  /// History walk with two refinable transitions: 400→300 and
  /// 200→100. 300→200 has no fingerprint changes and must never
  /// produce a deep request. Each pair response waits on a gate so
  /// the test can observe the sweep's strict sequencing.
  function mockDeepHistory() {
    mockApp([
      ["public", "400"],
      ["public", "300"],
      ["public", "200"],
      ["public", "100"],
    ]);
    server.use(
      http.post(`/api/apps/${APPID}/manifests/history`, () =>
        HttpResponse.json([
          {
            depot_id: DEPOT,
            manifest_id: "400",
            branch: "public",
            creation_time: 4000,
            previous: { depot_id: DEPOT, manifest_id: "300", branch: "public" },
            added: 0,
            removed: 0,
            changed: 2,
          },
          {
            depot_id: DEPOT,
            manifest_id: "300",
            branch: "public",
            creation_time: 3000,
            previous: { depot_id: DEPOT, manifest_id: "200", branch: "public" },
            added: 0,
            removed: 0,
            changed: 0,
          },
          {
            depot_id: DEPOT,
            manifest_id: "200",
            branch: "public",
            creation_time: 2000,
            previous: { depot_id: DEPOT, manifest_id: "100", branch: "public" },
            added: 0,
            removed: 0,
            changed: 1,
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
        ]),
      ),
    );
  }

  /// Deep-sweep requests as `base→target` manifest ids, newest
  /// transition first, plus gate resolvers for each response.
  function mockDeepSweep(): {
    requested: string[];
    gates: Array<(entries: ManifestDiffEntry[]) => void>;
  } {
    const requested: string[] = [];
    const gates: Array<(entries: ManifestDiffEntry[]) => void> = [];
    const pending: Array<Promise<{ entries: ManifestDiffEntry[] }>> = [];
    const pushGate = () =>
      pending.push(new Promise((resolve) => gates.push((entries) => resolve({ entries }))));
    pushGate();
    pushGate();
    server.use(
      http.get(
        `/api/apps/${APPID}/depots/${DEPOT}/manifests/:manifestId/structured-diff-filter`,
        ({ params, request }) => {
          const target = new URL(request.url).searchParams.get("target_manifest_id");
          requested.push(`${params.manifestId}→${target}`);
          return pending[requested.length - 1].then((body) => HttpResponse.json(body));
        },
      ),
    );
    return { requested, gates };
  }

  it("deep compare refines changed counts one transition at a time, arming the links", async () => {
    mockDeepHistory();
    const { requested, gates } = mockDeepSweep();

    renderRoute({ initialEntries: [`/apps/${APPID}/depots/${DEPOT}/history`] });
    await screen.findByText("Deep compare");
    expect(readRows().map((r) => r[0])).toEqual(["2", "unchanged", "1", "initial"]);

    fireEvent.click(screen.getByRole("checkbox"));

    // Newest transition first; 300→200 (no fingerprint changes) never
    // produces a request.
    await waitFor(() => expect(requested).toEqual(["400→300"]));
    // 400→300 is in flight, 200→100 queued: both dim their fingerprint
    // counts behind a spinner.
    const spinnerCells = () =>
      screen
        .getAllByRole("row")
        .slice(1)
        .map((row) => row.querySelectorAll("td")[2].querySelector("svg") != null);
    expect(spinnerCells()).toEqual([true, false, true, false]);

    // Deep result for 400→300: one of the two changes was noise.
    gates[0]([{ path: "a.unity3d", status: "changed" }]);
    await waitFor(() => expect(requested).toEqual(["400→300", "200→100"]));
    await waitFor(() => expect(readRows()[0][0]).toBe("1"));

    // Deep result for 200→100: the single change was noise — the
    // transition promotes to `unchanged`.
    gates[1]([]);
    await waitFor(() =>
      expect(readRows().map((r) => r[0])).toEqual(["1", "unchanged", "unchanged", "initial"]),
    );
    // No spinner anywhere once the sweep is done.
    expect(spinnerCells()).toEqual([false, false, false, false]);

    // Armed rows link to the manifest page with deep compare on; the
    // changed-less middle row has no compare target to arm, but still
    // carries the toggle state along.
    const rows = readRows();
    expect(rows[0][1]).toContain("compare_to=300");
    expect(rows[0][1]).toContain("deep=true");
    expect(rows[2][1]).toContain("deep=true");
    expect(rows[1][1]).not.toContain("compare_to");
    expect(rows[1][1]).toContain("deep=true");
  });

  it("toggles deep compare off and back on instantly, without refetching warm pairs", async () => {
    mockDeepHistory();
    const { requested, gates } = mockDeepSweep();

    renderRoute({ initialEntries: [`/apps/${APPID}/depots/${DEPOT}/history`] });
    await screen.findByText("Deep compare");
    fireEvent.click(screen.getByRole("checkbox"));
    gates[0]([{ path: "a.unity3d", status: "changed" }]);
    gates[1]([]);
    await waitFor(() =>
      expect(readRows().map((r) => r[0])).toEqual(["1", "unchanged", "unchanged", "initial"]),
    );

    // Off: fingerprint counts, links without deep. The router applies
    // the search change asynchronously, so wait for the rows to settle.
    fireEvent.click(screen.getByRole("checkbox"));
    await waitFor(() =>
      expect(readRows().map((r) => r[0])).toEqual(["2", "unchanged", "1", "initial"]),
    );
    expect(readRows()[0][1]).not.toContain("deep=true");

    // On: the pair keys are still warm — deep counts come back without
    // a single pair being requested again.
    fireEvent.click(screen.getByRole("checkbox"));
    await waitFor(() =>
      expect(readRows().map((r) => r[0])).toEqual(["1", "unchanged", "unchanged", "initial"]),
    );
    expect(requested).toEqual(["400→300", "200→100"]);
  });

  it("lets the in-flight transition finish on toggle-off and resumes on toggle-on", async () => {
    mockDeepHistory();
    const { requested, gates } = mockDeepSweep();

    renderRoute({ initialEntries: [`/apps/${APPID}/depots/${DEPOT}/history`] });
    await screen.findByText("Deep compare");
    fireEvent.click(screen.getByRole("checkbox"));
    await waitFor(() => expect(requested).toEqual(["400→300"]));

    // Off while 400→300 is in flight: it completes in the background,
    // but the chain stops — 200→100 never starts. The display stays
    // on fingerprint counts while off.
    fireEvent.click(screen.getByRole("checkbox"));
    gates[0]([{ path: "a.unity3d", status: "changed" }]);
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(requested).toEqual(["400→300"]);
    expect(readRows()[0][0]).toBe("2");

    // On: the completed pair is warm (its row refines instantly) and
    // the chain resumes at the first cold pair.
    fireEvent.click(screen.getByRole("checkbox"));
    await waitFor(() => expect(readRows()[0][0]).toBe("1"));
    await waitFor(() => expect(requested).toEqual(["400→300", "200→100"]));
    gates[1]([]);
    await waitFor(() =>
      expect(readRows().map((r) => r[0])).toEqual(["1", "unchanged", "unchanged", "initial"]),
    );
  });

  it("arms deep compare from the URL", async () => {
    mockDeepHistory();
    const { requested, gates } = mockDeepSweep();

    renderRoute({
      initialEntries: [`/apps/${APPID}/depots/${DEPOT}/history?deep=true`],
    });
    gates[0]([{ path: "a.unity3d", status: "changed" }]);
    gates[1]([]);
    await waitFor(() =>
      expect(readRows().map((r) => r[0])).toEqual(["1", "unchanged", "unchanged", "initial"]),
    );
    expect(requested).toEqual(["400→300", "200→100"]);
  });
});
