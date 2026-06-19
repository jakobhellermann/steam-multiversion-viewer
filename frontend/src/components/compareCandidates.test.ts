// TODO(ai-review): review for style and correctness
import { describe, expect, test } from "vitest";
import type { AppInfo, DepotEntry, ExtraManifestEntry } from "../api";
import { buildDepotGroups, filterGroupsByBranch } from "./compareCandidates";

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

describe("buildDepotGroups", () => {
  test("collapses a gid reachable via multiple branches into one entry with all branches", () => {
    const info = appInfo([
      depot(10, [
        ["public-beta", "A"],
        ["public", "A"],
        ["public", "B"],
      ]),
    ]);
    const groups = buildDepotGroups(info, [], undefined, 10, "X");
    const cands = groups[0].candidates;
    expect(cands.map((c) => c.manifestId)).toEqual(["A", "B"]);
    expect(cands.find((c) => c.manifestId === "A")?.branches).toEqual(["public-beta", "public"]);
  });

  test("merges an extra's branch into an existing official gid", () => {
    const info = appInfo([depot(10, [["public", "A"]])]);
    const extras: ExtraManifestEntry[] = [
      { depot_id: 10, manifest_id: "A", branch: "experimental" },
    ];
    const groups = buildDepotGroups(info, extras, undefined, 10, "X");
    expect(groups[0].candidates[0].branches).toEqual(["public", "experimental"]);
  });
});

describe("filterGroupsByBranch", () => {
  const info = appInfo([
    depot(10, [
      ["public-beta", "A"],
      ["public", "A"],
      ["public", "B"],
    ]),
  ]);

  test("keeps a gid whose first-listed branch is hidden but which has a visible branch", () => {
    const groups = buildDepotGroups(info, [], undefined, 10, "X");
    const filtered = filterGroupsByBranch(groups, new Set(["public-beta"]), 10);
    expect(filtered[0].candidates.map((c) => c.manifestId)).toEqual(["A", "B"]);
  });

  test("labels a kept gid with a visible branch", () => {
    const groups = buildDepotGroups(info, [], undefined, 10, "X");
    const filtered = filterGroupsByBranch(groups, new Set(["public-beta"]), 10);
    expect(filtered[0].candidates.find((c) => c.manifestId === "A")?.branch).toBe("public");
  });

  test("drops a gid only when all its branches are hidden", () => {
    const groups = buildDepotGroups(info, [], undefined, 10, "X");
    const filtered = filterGroupsByBranch(groups, new Set(["public", "public-beta"]), 10);
    // Depot 10 is the current depot, so it stays as an (empty) group.
    expect(filtered[0].candidates).toEqual([]);
  });

  test("always keeps the current manifest even if its branch is hidden", () => {
    const groups = buildDepotGroups(info, [], undefined, 10, "A");
    const filtered = filterGroupsByBranch(groups, new Set(["public", "public-beta"]), 10);
    expect(filtered[0].candidates.map((c) => c.manifestId)).toEqual(["A"]);
  });
});
