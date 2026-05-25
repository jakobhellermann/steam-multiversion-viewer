// TODO(ai-review): review for style and correctness
import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useMemo } from "react";
import {
  downloadManifest,
  fetchAppInfo,
  fetchExtraManifests,
  fetchFileView,
  fetchFileViewOptional,
  fetchManifestStatuses,
  fileRawUrl,
  type ManifestRef,
  type ManifestStatusEntry,
} from "../api";
import { CompareMenu, diffTargetKey } from "../components/CompareMenu";
import { ErrorBox } from "../components/ErrorBox";
import { markShowImmediately } from "../lib/downloadsUiSignal";
import { formatDate } from "../lib/format";
import { DiffTargetBlock } from "./-file/DiffTarget";
import { FileMeta } from "./-file/FileMeta";
import { FilePreview } from "./-file/FilePreview";

type Search = {
  /// Omitted in URL when the default; readers must apply `?? "public"`.
  branch?: string;
  path: string;
  /// Diff targets, mirrored from the manifest-detail page so the
  /// "compare to" filter survives navigation. Comma-separated list of
  /// "depot-manifest" or bare "manifest" entries.
  compare_to?: string;
  /// Structured-view path search query (token-AND over node labels).
  q?: string;
  /// Structured-view facet whitelist. One entry per facet key as
  /// `f.<key>=<v1>,<v2>,…`. Keys are format-defined ("class", "kind",
  /// …); the validator preserves whatever the URL carries.
  [facet: `f.${string}`]: string | undefined;
};

export const Route = createFileRoute("/apps/$appid/depots/$depotId/manifests/$manifestId_/file")({
  validateSearch: (search: Record<string, unknown>): Search => {
    const out: Search = {
      branch:
        typeof search.branch === "string" && search.branch !== "public" ? search.branch : undefined,
      path: typeof search.path === "string" ? search.path : "",
      compare_to:
        typeof search.compare_to === "string" && search.compare_to ? search.compare_to : undefined,
      q: typeof search.q === "string" && search.q ? search.q : undefined,
    };
    // Pass `f.*` keys through verbatim — see [`StructuredView`] for the
    // shape; absent entries mean "no whitelist for this facet".
    for (const [k, v] of Object.entries(search)) {
      if (k.startsWith("f.") && typeof v === "string" && v) {
        (out as Record<string, string>)[k] = v;
      }
    }
    return out;
  },
  component: FileViewPage,
});

function FileViewPage() {
  const { appid: appidParam, depotId: depotIdParam, manifestId } = Route.useParams();
  const search = Route.useSearch();
  const branch = search.branch ?? "public";
  const { path, compare_to } = search;
  const navigate = useNavigate({ from: Route.fullPath });
  const appid = Number(appidParam);
  const depotId = Number(depotIdParam);
  const queryClient = useQueryClient();

  const view = useQuery({
    queryKey: ["file-view", appid, depotId, manifestId, branch, path],
    queryFn: () => fetchFileView(appid, depotId, manifestId, branch, path),
    enabled: path.length > 0,
    // chunks_present + auto-fetched content depend on what's on disk, so
    // re-run when navigated back to.
    refetchOnMount: "always",
  });

  const appInfoQuery = useQuery({
    queryKey: ["app", appid],
    queryFn: () => fetchAppInfo(appid),
  });
  const extraQuery = useQuery({
    queryKey: ["extra-manifests", appid],
    queryFn: () => fetchExtraManifests(appid),
  });

  const diffTargets = useMemo(
    () => new Set(compare_to ? compare_to.split(",").filter(Boolean) : []),
    [compare_to],
  );
  const setDiffTargets = useCallback(
    (next: Set<string>) => {
      navigate({
        search: (prev) => ({
          ...prev,
          compare_to: next.size === 0 ? undefined : [...next].join(","),
        }),
        replace: true,
      });
    },
    [navigate],
  );

  // Resolve URL diff-target keys back to concrete manifest refs.
  const diffRefs = useMemo<ManifestRef[]>(() => {
    if (!appInfoQuery.data || diffTargets.size === 0) return [];
    const all: { depot_id: number; manifest_id: string; branch: string }[] = [];
    const seenAll = new Set<string>();
    for (const d of appInfoQuery.data.depots) {
      for (const m of d.manifests) {
        const k = `${d.depot_id}-${m.manifest_id}`;
        if (seenAll.has(k)) continue;
        seenAll.add(k);
        all.push({ depot_id: d.depot_id, manifest_id: m.manifest_id, branch: m.branch });
      }
    }
    for (const e of extraQuery.data ?? []) {
      const k = `${e.depot_id}-${e.manifest_id}`;
      if (seenAll.has(k)) continue;
      seenAll.add(k);
      all.push({
        depot_id: e.depot_id,
        manifest_id: e.manifest_id,
        branch: e.branch ?? "public",
      });
    }
    const out: ManifestRef[] = [];
    const seenRef = new Set<string>();
    for (const entry of all) {
      const canonical = diffTargetKey(entry.depot_id, entry.manifest_id, depotId);
      const composite = `${entry.depot_id}-${entry.manifest_id}`;
      if (
        (diffTargets.has(canonical) ||
          diffTargets.has(entry.manifest_id) ||
          diffTargets.has(composite)) &&
        !seenRef.has(composite)
      ) {
        seenRef.add(composite);
        out.push(entry);
      }
    }
    return out;
  }, [appInfoQuery.data, extraQuery.data, diffTargets, depotId]);

  // Status query gives us creation_time per manifest for the menu's
  // chronological sort. Same key as elsewhere so the cache is shared.
  const compareRefs = useMemo<ManifestRef[]>(() => {
    if (!appInfoQuery.data) return [];
    const seen = new Set<string>();
    const refs: ManifestRef[] = [];
    for (const d of appInfoQuery.data.depots) {
      for (const m of d.manifests) {
        const key = `${d.depot_id}/${m.manifest_id}`;
        if (seen.has(key)) continue;
        seen.add(key);
        refs.push({ depot_id: d.depot_id, manifest_id: m.manifest_id, branch: m.branch });
      }
    }
    for (const e of extraQuery.data ?? []) {
      const key = `${e.depot_id}/${e.manifest_id}`;
      if (seen.has(key)) continue;
      seen.add(key);
      refs.push({
        depot_id: e.depot_id,
        manifest_id: e.manifest_id,
        branch: e.branch ?? "public",
      });
    }
    return refs;
  }, [appInfoQuery.data, extraQuery.data]);
  const statusQuery = useQuery({
    queryKey: ["manifest-statuses", appid, compareRefs],
    queryFn: () => fetchManifestStatuses(appid, compareRefs),
    // Wait until both inputs to `compareRefs` have settled — firing
    // first on just `appInfo` and again once `extras` lands fragments
    // the cache into two entries (and means two slow round-trips).
    enabled: compareRefs.length > 0 && extraQuery.isSuccess,
  });

  // One file-view query per diff target. useQueries handles the dynamic
  // count safely; useQuery in a loop would break the hook rule.
  const targetViews = useQueries({
    queries: diffRefs.map((ref) => ({
      queryKey: ["file-view", appid, ref.depot_id, ref.manifest_id, ref.branch, path],
      queryFn: () => fetchFileViewOptional(appid, ref.depot_id, ref.manifest_id, ref.branch, path),
      enabled: path.length > 0,
    })),
  });

  const download = useMutation({
    mutationFn: () => downloadManifest(appid, depotId, manifestId, { branch, paths: [path] }),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["file-view", appid, depotId, manifestId, branch, path],
      });
    },
  });

  // File page is always viewport-fixed: header (measured into
  // `--app-header-h` by the root component) sits above us, we claim
  // the rest. The preview owns its own scroll-region — body never
  // scrolls on this route.
  return (
    <div className="mx-auto flex h-[calc(100dvh-var(--app-header-h,52px))] max-w-6xl flex-col p-8">
      <nav className="mb-4 flex items-center gap-2 text-sm text-slate-400">
        <Link to="/apps/$appid" params={{ appid: appidParam }} className="hover:underline">
          {appInfoQuery.data?.name ?? `App ${appid}`}
        </Link>
        <span className="text-slate-600">/</span>
        <Link
          to="/apps/$appid"
          params={{ appid: appidParam }}
          hash={`depot-${depotId}`}
          className="hover:underline"
        >
          {branch === "public" ? depotId : `${depotId} · ${branch}`}
        </Link>
        <span className="text-slate-600">/</span>
        <Link
          to="/apps/$appid/depots/$depotId/manifests/$manifestId"
          params={{ appid: appidParam, depotId: depotIdParam, manifestId }}
          search={{ branch: branch === "public" ? undefined : branch, compare_to }}
          className="hover:underline"
        >
          {manifestCrumbLabel(currentManifestCreation(statusQuery.data, depotId, manifestId))}
        </Link>
        <span className="text-slate-600">/</span>
        <span className="truncate font-medium text-slate-200">{filenameOf(path)}</span>
      </nav>

      <div className="flex items-center gap-3">
        <div className="min-w-0 flex-1">
          <h1 className="font-mono text-sm break-all">{path}</h1>
          {view.isPending && <p className="mt-1 text-sm text-slate-400">Loading…</p>}
          {view.error && (
            <div className="mt-1">
              <ErrorBox title="Failed to load file" error={view.error as Error} />
            </div>
          )}
          {view.data && (
            <FileMeta
              view={view.data}
              onDownload={() => {
                markShowImmediately();
                download.mutate();
              }}
              downloadPending={download.isPending}
            />
          )}
        </div>
        {appInfoQuery.data && extraQuery.data ? (
          <CompareMenu
            appInfo={appInfoQuery.data}
            extras={extraQuery.data}
            statuses={statusQuery.data}
            currentDepotId={depotId}
            currentManifestId={manifestId}
            selected={diffTargets}
            onChange={setDiffTargets}
            fileContext={{
              appid,
              base: { depot_id: depotId, manifest_id: manifestId, branch },
              path,
            }}
          />
        ) : (
          // Placeholder while appInfo is loading. We already know the
          // selected count from the URL `compare_to` param, so render
          // the button at its final width to avoid a layout shift when
          // the real menu mounts.
          <CompareMenuPlaceholder count={diffTargets.size} />
        )}
      </div>

      {diffRefs.length > 0 && (
        <section className="mt-8">
          <h2 className="mb-3 text-sm font-semibold text-slate-400">Compared to</h2>
          <div className="space-y-2">
            {diffRefs.map((ref, i) => {
              const status = (statusQuery.data ?? []).find(
                (s) => s.depot_id === ref.depot_id && s.manifest_id === ref.manifest_id,
              );
              return (
                <DiffTargetBlock
                  key={`${ref.depot_id}-${ref.manifest_id}`}
                  base={view.data ?? null}
                  baseLocator={{ appid, depotId, manifestId, branch, path }}
                  ref_={ref}
                  creationTime={status?.creation_time ?? 0}
                  query={targetViews[i]}
                  rawSrc={fileRawUrl(appid, ref.depot_id, ref.manifest_id, ref.branch, path)}
                  locator={{
                    appid,
                    depotId: ref.depot_id,
                    manifestId: ref.manifest_id,
                    branch: ref.branch,
                    path,
                  }}
                />
              );
            })}
          </div>
        </section>
      )}

      {view.data && (
        <FilePreview
          view={view.data}
          rawSrc={fileRawUrl(appid, depotId, manifestId, branch, path)}
          locator={{ appid, depotId, manifestId, branch, path }}
        />
      )}
    </div>
  );
}

/// Visually identical to the active state of the real `CompareMenu`
/// trigger so the layout doesn't shift when `appInfo` finishes loading.
/// We can render this immediately because the selected count comes from
/// the URL — no backend roundtrip needed.
function CompareMenuPlaceholder({ count }: { count: number }) {
  return (
    <button
      type="button"
      disabled
      className={`rounded border px-3 py-1.5 text-sm whitespace-nowrap opacity-60 ${
        count > 0
          ? "border-sky-700 bg-sky-950/40 text-sky-200"
          : "border-slate-700 bg-slate-900 text-slate-300"
      }`}
      title="Loading…"
    >
      Compare to{count > 0 && <span className="ml-1.5 tabular-nums">({count})</span>}
    </button>
  );
}

/// Pick the `creation_time` for the currently-viewed manifest out of
/// the shared status-query result. Returns 0 (treated as "unknown") if
/// the status query hasn't loaded yet or this manifest isn't in it.
function currentManifestCreation(
  statuses: ManifestStatusEntry[] | undefined,
  depotId: number,
  manifestId: string,
): number {
  if (!statuses) return 0;
  const me = statuses.find((s) => s.depot_id === depotId && s.manifest_id === manifestId);
  return me?.creation_time ?? 0;
}

/// Label for the manifest segment of the breadcrumb. Falls back to
/// "manifest" while the status query is still loading the creation
/// time.
function manifestCrumbLabel(creationTime: number): string {
  if (creationTime > 0) return formatDate(creationTime);
  return "manifest";
}

function filenameOf(path: string): string {
  const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return slash >= 0 ? path.slice(slash + 1) : path;
}
