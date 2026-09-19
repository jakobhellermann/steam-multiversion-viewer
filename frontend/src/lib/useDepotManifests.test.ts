// TODO(ai-review): review for style and correctness
import { describe, expect, test } from "vitest";
import type { AppInfo, DepotEntry, ExtraManifestEntry } from "../api";
import { depotManifestRefs, manifestRefOf } from "./useDepotManifests";

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
  test("collects every branch a gid is reachable through; public wins the wire branch", () => {
    const refs = depotManifestRefs(
      appInfo([
        depot(7, [
          ["public", "A"],
          ["public-beta", "A"],
          ["public", "B"],
        ]),
      ]),
      [extra(7, "A", "beta"), extra(8, "D", null)],
      7,
    );
    expect(refs).toEqual([
      {
        depot_id: 7,
        manifest_id: "A",
        branch: "public",
        branches: ["public", "public-beta", "beta"],
      },
      { depot_id: 7, manifest_id: "B", branch: "public", branches: ["public"] },
    ]);
  });

  test("a beta-only gid keeps its branch", () => {
    const refs = depotManifestRefs(
      appInfo([depot(7, [["public", "A"]])]),
      [extra(7, "C", "beta")],
      7,
    );
    expect(refs).toEqual([
      { depot_id: 7, manifest_id: "A", branch: "public", branches: ["public"] },
      { depot_id: 7, manifest_id: "C", branch: "beta", branches: ["beta"] },
    ]);
  });

  test("a seed gid unseen by both lists joins with its branch", () => {
    const refs = depotManifestRefs(appInfo([depot(7, [["public", "A"]])]), [], 7, {
      manifest_id: "Z",
      branch: "beta",
    });
    expect(refs).toEqual([
      { depot_id: 7, manifest_id: "Z", branch: "beta", branches: ["beta"] },
      { depot_id: 7, manifest_id: "A", branch: "public", branches: ["public"] },
    ]);
  });

  test("a seed branch merges into a gid the lists also know", () => {
    const refs = depotManifestRefs(appInfo([depot(7, [["public", "A"]])]), [], 7, {
      manifest_id: "A",
      branch: "beta",
    });
    expect(refs).toEqual([
      { depot_id: 7, manifest_id: "A", branch: "public", branches: ["beta", "public"] },
    ]);
  });
});

describe("manifestRefOf", () => {
  test("drops the client-only branches", () => {
    expect(
      manifestRefOf({
        depot_id: 7,
        manifest_id: "A",
        branch: "public",
        branches: ["public", "beta"],
      }),
    ).toEqual({ depot_id: 7, manifest_id: "A", branch: "public" });
  });
});
