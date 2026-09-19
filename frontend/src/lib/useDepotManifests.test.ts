// TODO(ai-review): review for style and correctness
import { describe, expect, test } from "vitest";
import type { AppInfo, DepotEntry, ExtraManifestEntry } from "../api";
import { depotManifestRefs } from "./useDepotManifests";

function depot(depotId: number, manifests: Array<[string, string]>): DepotEntry {
  return {
    depot_id: depotId,
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
  };
}

function appInfo(depots: DepotEntry[]): AppInfo {
  return {
    appid: 1,
    name: "",
    type: "",
    developer: null,
    publisher: null,
    homepage: null,
    logo_url: null,
    icon_url: "",
    branches: [],
    depots,
    private_branches: false,
  };
}

function extra(depotId: number, manifestId: string, branch: string | null): ExtraManifestEntry {
  return { depot_id: depotId, manifest_id: manifestId, branch };
}

describe("depotManifestRefs", () => {
  test("merges app-info and extras of the depot, extras winning the branch", () => {
    const refs = depotManifestRefs(
      appInfo([
        depot(7, [
          ["public", "A"],
          ["public", "B"],
        ]),
      ]),
      [extra(7, "A", "beta"), extra(8, "C", null), extra(7, "C2", null)],
      7,
    );
    expect(refs).toEqual([
      { depot_id: 7, manifest_id: "A", branch: "beta" },
      { depot_id: 7, manifest_id: "B", branch: "public" },
      { depot_id: 7, manifest_id: "C2", branch: "public" },
    ]);
  });

  test("seed survives when no list contains it", () => {
    const refs = depotManifestRefs(appInfo([depot(7, [["public", "A"]])]), [], 7, {
      manifest_id: "Z",
      branch: "beta",
    });
    expect(refs).toEqual([
      { depot_id: 7, manifest_id: "Z", branch: "beta" },
      { depot_id: 7, manifest_id: "A", branch: "public" },
    ]);
  });

  test("seed known to app-info keeps the list entry's branch", () => {
    const refs = depotManifestRefs(appInfo([depot(7, [["beta", "A"]])]), [], 7, {
      manifest_id: "A",
      branch: "public",
    });
    expect(refs).toEqual([{ depot_id: 7, manifest_id: "A", branch: "beta" }]);
  });

  test("no app-info and no extras still yields the seed", () => {
    expect(
      depotManifestRefs(undefined, undefined, 7, { manifest_id: "Z", branch: "public" }),
    ).toEqual([{ depot_id: 7, manifest_id: "Z", branch: "public" }]);
  });
});
