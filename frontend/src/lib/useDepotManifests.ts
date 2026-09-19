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

/// All tracked manifests of one depot: the app-info entries plus the
/// user-tracked extras, deduped by manifest id — first occurrence keeps
/// its position, later ones only update the branch. `seed` adds a
/// manifest that neither list contains yet (e.g. a deep-linked one).
export function depotManifestRefs(
  appInfo: AppInfo | undefined,
  extras: ExtraManifestEntry[] | undefined,
  depotId: number,
  seed?: { manifest_id: string; branch: string },
): ManifestRef[] {
  const refs = new Map<string, ManifestRef>();
  if (seed) {
    refs.set(seed.manifest_id, { depot_id: depotId, ...seed });
  }
  const depot = appInfo?.depots.find((entry) => entry.depot_id === depotId);
  for (const manifest of depot?.manifests ?? []) {
    refs.set(manifest.manifest_id, {
      depot_id: depotId,
      manifest_id: manifest.manifest_id,
      branch: manifest.branch,
    });
  }
  for (const manifest of extras ?? []) {
    if (manifest.depot_id !== depotId) continue;
    refs.set(manifest.manifest_id, {
      depot_id: depotId,
      manifest_id: manifest.manifest_id,
      branch: manifest.branch ?? "public",
    });
  }
  return [...refs.values()];
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
