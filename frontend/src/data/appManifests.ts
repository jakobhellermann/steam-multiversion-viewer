// TODO(ai-review): review for style and correctness
import {
  keepPreviousData,
  queryOptions,
  useQuery,
  useQueryClient,
  type UseQueryResult,
} from "@tanstack/react-query";
import { useEffect, useRef } from "react";

import {
  fetchAppInfo,
  fetchAppManifestStatuses,
  fetchExtraManifests,
  fetchManifestStatuses,
  type AppId,
  type AppInfo,
  type ExtraManifestEntry,
  type ManifestRef,
  type ManifestStatusEntry,
} from "./api";
import { queryKeys } from "./queryKeys";

/// Preloaded app info can be a last-known copy; once older than the backend's HTTP max-age it
/// is refetched.
export function appInfoQuery(appid: AppId) {
  return queryOptions({
    queryKey: queryKeys.app(appid),
    queryFn: () => fetchAppInfo(appid),
    staleTime: 5 * 60 * 1000,
  });
}

export function extraManifestsQuery(appid: AppId) {
  return queryOptions({
    queryKey: queryKeys.extraManifests(appid),
    queryFn: () => fetchExtraManifests(appid),
  });
}

export function useAppInfo(appid: AppId): UseQueryResult<AppInfo> {
  return useQuery({ ...appInfoQuery(appid), refetchOnMount: true });
}

export function useExtraManifests(appid: AppId): UseQueryResult<ExtraManifestEntry[]> {
  return useQuery(extraManifestsQuery(appid));
}

/// App info, tracked extras, and the cache status of every manifest the backend derives from
/// them.
export function useAppManifestStatuses(appid: AppId): {
  appInfo: UseQueryResult<AppInfo>;
  extras: UseQueryResult<ExtraManifestEntry[]>;
  statuses: UseQueryResult<ManifestStatusEntry[]>;
} {
  const queryClient = useQueryClient();
  const appInfo = useAppInfo(appid);
  const extras = useExtraManifests(appid);
  const statuses = useQuery({
    queryKey: queryKeys.appManifestStatuses(appid),
    queryFn: () => fetchAppManifestStatuses(appid),
    // A preload may cover only the manifests the backend has on disk.
    refetchOnMount: true,
  });
  // The backend derives the batch from its last fetched app info, so a refetched app info with
  // new content (a new build) outdates it. Structural sharing keeps an unchanged refetch's
  // reference.
  const seenAppInfo = useRef(appInfo.data);
  useEffect(() => {
    if (seenAppInfo.current === appInfo.data) return;
    const outdated = seenAppInfo.current !== undefined;
    seenAppInfo.current = appInfo.data;
    if (outdated) {
      void queryClient.invalidateQueries({
        queryKey: queryKeys.appManifestStatuses(appid),
        exact: true,
      });
    }
  }, [appInfo.data, appid, queryClient]);
  return { appInfo, extras, statuses };
}

export function findStatus(
  statuses: ManifestStatusEntry[] | undefined,
  depotId: number,
  manifestId: string,
): ManifestStatusEntry | undefined {
  return statuses?.find((s) => s.depot_id === depotId && s.manifest_id === manifestId);
}

/// One manifest's cache status: from the app-wide `batch` when that is cached, otherwise its own
/// (preloadable) request, which needn't wait for app info, extras and every other manifest.
export function useManifestStatus(
  appid: AppId,
  ref: ManifestRef,
  batch: ManifestStatusEntry[] | undefined,
): { data: ManifestStatusEntry | undefined; isPending: boolean; error: Error | null } {
  const fromBatch = findStatus(batch, ref.depot_id, ref.manifest_id);
  const refs = [ref];
  const own = useQuery({
    queryKey: queryKeys.manifestStatuses(appid, refs),
    queryFn: () => fetchManifestStatuses(appid, refs),
    enabled: fromBatch === undefined,
    placeholderData: keepPreviousData,
  });
  return {
    data: fromBatch ?? own.data?.[0],
    isPending: fromBatch === undefined && own.isPending,
    error: own.error,
  };
}
