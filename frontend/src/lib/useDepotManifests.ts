// TODO(ai-review): review for style and correctness
import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";

import {
  fetchAppInfo,
  fetchExtraManifests,
  type AppInfo,
  type ExtraManifestEntry,
  type ManifestRef,
} from "../api";

/// One tracked manifest of a depot. A gid can sit on several branches
/// at once (public and beta heads pointing at the same build):
/// `branches` lists them all; `branch` is the single one used on the
/// wire — `public` when reachable through it, else the first seen.
export type DepotManifest = ManifestRef & { branches: string[] };

/// The wire form of a [`DepotManifest`]: the manifest ref without the
/// client-only `branches`.
export function manifestRefOf(manifest: DepotManifest): ManifestRef {
  return {
    depot_id: manifest.depot_id,
    manifest_id: manifest.manifest_id,
    branch: manifest.branch,
  };
}

/// All tracked manifests of one depot — the app-info heads plus the
/// user-tracked extras, one entry per gid, every branch the gid is
/// reachable through attached. `seed` adds a gid neither list
/// contains (e.g. a deep-linked one).
export function depotManifestRefs(
  appInfo: AppInfo | undefined,
  extras: ExtraManifestEntry[] | undefined,
  depotId: number,
  seed?: { manifest_id: string; branch: string },
): DepotManifest[] {
  const byId = new Map<string, DepotManifest>();
  const add = (manifestId: string, branch: string) => {
    let manifest = byId.get(manifestId);
    if (!manifest) {
      manifest = { depot_id: depotId, manifest_id: manifestId, branch, branches: [] };
      byId.set(manifestId, manifest);
    }
    if (!manifest.branches.includes(branch)) {
      manifest.branches.push(branch);
    }
  };
  if (seed) {
    add(seed.manifest_id, seed.branch);
  }
  const depot = appInfo?.depots.find((entry) => entry.depot_id === depotId);
  for (const manifest of depot?.manifests ?? []) {
    add(manifest.manifest_id, manifest.branch);
  }
  for (const extra of extras ?? []) {
    if (extra.depot_id !== depotId) continue;
    add(extra.manifest_id, extra.branch ?? "public");
  }
  return [...byId.values()].map((manifest) => ({
    ...manifest,
    branch: manifest.branches.includes("public") ? "public" : manifest.branch,
  }));
}

/// Queries behind [`depotManifestRefs`]: the app's info and tracked
/// extras, plus the derived depot manifest list. `ready` gates the
/// consumers that must not fire with an incomplete manifest set.
export function useDepotManifests(
  appid: number,
  depotId: number,
  seed?: { manifest_id: string; branch: string },
) {
  const appInfo = useQuery({ queryKey: ["app", appid], queryFn: () => fetchAppInfo(appid) });
  const extras = useQuery({
    queryKey: ["extra-manifests", appid],
    queryFn: () => fetchExtraManifests(appid),
  });
  const seedId = seed?.manifest_id;
  const seedBranch = seed?.branch;
  const manifests = useMemo(
    () =>
      depotManifestRefs(
        appInfo.data,
        extras.data,
        depotId,
        seedId != null && seedBranch != null
          ? { manifest_id: seedId, branch: seedBranch }
          : undefined,
      ),
    [appInfo.data, extras.data, depotId, seedId, seedBranch],
  );
  return { appInfo, extras, manifests, ready: appInfo.isSuccess && extras.isSuccess };
}
