// TODO(ai-review): review for style and correctness
import { useQuery, type UseQueryResult } from "@tanstack/react-query";
import { useMemo } from "react";

import {
  fetchAppInfo,
  fetchExtraManifests,
  fetchManifestStatuses,
  type AppId,
  type AppInfo,
  type ExtraManifestEntry,
  type ManifestRef,
  type ManifestStatusEntry,
} from "../api";
import { queryKeys } from "./queryKeys";

export function useAppInfo(appid: AppId): UseQueryResult<AppInfo> {
  return useQuery({ queryKey: queryKeys.app(appid), queryFn: () => fetchAppInfo(appid) });
}

export function useExtraManifests(appid: AppId): UseQueryResult<ExtraManifestEntry[]> {
  return useQuery({
    queryKey: queryKeys.extraManifests(appid),
    queryFn: () => fetchExtraManifests(appid),
  });
}

/// Every tracked manifest of the app (app-info heads, then user-tracked extras), once per
/// `(depot, gid)` with the first branch it was seen on.
export function appManifestRefs(
  appInfo: AppInfo | undefined,
  extras: ExtraManifestEntry[] | undefined,
): ManifestRef[] {
  if (!appInfo) return [];
  const byKey = new Map<string, ManifestRef>();
  const add = (ref: ManifestRef) => {
    const key = `${ref.depot_id}/${ref.manifest_id}`;
    if (!byKey.has(key)) byKey.set(key, ref);
  };
  for (const depot of appInfo.depots) {
    for (const m of depot.manifests) {
      add({ depot_id: depot.depot_id, manifest_id: m.manifest_id, branch: m.branch });
    }
  }
  for (const e of extras ?? []) {
    add({ depot_id: e.depot_id, manifest_id: e.manifest_id, branch: e.branch ?? "public" });
  }
  return [...byKey.values()];
}

/// App info, tracked extras, and the cache status of every manifest in [`appManifestRefs`].
export function useAppManifestStatuses(appid: AppId): {
  appInfo: UseQueryResult<AppInfo>;
  extras: UseQueryResult<ExtraManifestEntry[]>;
  statuses: UseQueryResult<ManifestStatusEntry[]>;
} {
  const appInfo = useAppInfo(appid);
  const extras = useExtraManifests(appid);
  const refs = useMemo(
    () => appManifestRefs(appInfo.data, extras.data),
    [appInfo.data, extras.data],
  );
  const statuses = useQuery({
    queryKey: queryKeys.manifestStatuses(appid, refs),
    queryFn: () => fetchManifestStatuses(appid, refs),
    // Refs built before extras land would be a different, short-lived key.
    enabled: refs.length > 0 && extras.isSuccess,
  });
  return { appInfo, extras, statuses };
}

export function findStatus(
  statuses: ManifestStatusEntry[] | undefined,
  depotId: number,
  manifestId: string,
): ManifestStatusEntry | undefined {
  return statuses?.find((s) => s.depot_id === depotId && s.manifest_id === manifestId);
}
