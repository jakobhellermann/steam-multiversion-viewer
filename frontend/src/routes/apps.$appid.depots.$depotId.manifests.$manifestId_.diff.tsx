// TODO(ai-review): review for style and correctness
import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";

import {
  fetchAppInfo,
  fetchExtraManifests,
  fetchFileViewOptional,
  fetchManifestStatuses,
  fileRawUrl,
  type FileView,
  type ManifestRef,
} from "../api";
import { ManifestSwitcher } from "../components/ManifestSwitcher";
import { ErrorBox } from "../components/ErrorBox";
import { formatDate } from "../lib/format";
import { DiffView } from "./-file/DiffView";
import { FilePreview } from "./-file/FilePreview";
import { StructuredDiffView } from "./-file/StructuredDiffView";
import type { FileLocator } from "./-file/types";

type Search = {
  /// Branch of the base manifest. Omitted in URL when `"public"`.
  branch?: string;
  path: string;
  /// Target side identity — required to know what to diff against.
  target_depot_id: number;
  target_manifest_id: string;
  /// Target's branch. Omitted in URL when `"public"`.
  target_branch?: string;
};

export const Route = createFileRoute("/apps/$appid/depots/$depotId/manifests/$manifestId_/diff")({
  validateSearch: (search: Record<string, unknown>): Search => ({
    branch:
      typeof search.branch === "string" && search.branch !== "public" ? search.branch : undefined,
    path: typeof search.path === "string" ? search.path : "",
    target_depot_id:
      typeof search.target_depot_id === "string"
        ? Number(search.target_depot_id)
        : typeof search.target_depot_id === "number"
          ? search.target_depot_id
          : 0,
    target_manifest_id:
      typeof search.target_manifest_id === "string" ? search.target_manifest_id : "",
    target_branch:
      typeof search.target_branch === "string" && search.target_branch !== "public"
        ? search.target_branch
        : undefined,
  }),
  component: DiffPage,
});

function DiffPage() {
  const { appid: appidParam, depotId: depotIdParam, manifestId } = Route.useParams();
  const search = Route.useSearch();
  const navigate = useNavigate({ from: Route.fullPath });
  const branch = search.branch ?? "public";
  const targetBranch = search.target_branch ?? "public";
  const { path, target_depot_id: targetDepotId, target_manifest_id: targetManifestId } = search;
  const appid = Number(appidParam);
  const depotId = Number(depotIdParam);

  const appInfoQuery = useQuery({
    queryKey: ["app", appid],
    queryFn: () => fetchAppInfo(appid),
  });
  const extraQuery = useQuery({
    queryKey: ["extra-manifests", appid],
    queryFn: () => fetchExtraManifests(appid),
  });
  const baseRef: ManifestRef = { depot_id: depotId, manifest_id: manifestId, branch };
  const targetRef: ManifestRef = {
    depot_id: targetDepotId,
    manifest_id: targetManifestId,
    branch: targetBranch,
  };
  const statusQuery = useQuery({
    queryKey: ["manifest-statuses", appid, [baseRef, targetRef]],
    queryFn: () => fetchManifestStatuses(appid, [baseRef, targetRef]),
  });
  // Full status query for the manifest switcher — shared cache key with
  // the manifest detail + file pages so it's usually a cache hit.
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
  const switcherStatusQuery = useQuery({
    queryKey: ["manifest-statuses", appid, compareRefs],
    queryFn: () => fetchManifestStatuses(appid, compareRefs),
    enabled: compareRefs.length > 0 && extraQuery.isSuccess,
  });
  // Confirm the file exists on both sides up front — saves the diff
  // endpoint a roundtrip for a missing path and lets us surface a
  // user-friendly error instead of a backend 404.
  const baseView = useQuery({
    queryKey: ["file-view", appid, depotId, manifestId, branch, path],
    queryFn: () => fetchFileViewOptional(appid, depotId, manifestId, branch, path),
    enabled: path.length > 0,
  });
  const targetView = useQuery({
    queryKey: ["file-view", appid, targetDepotId, targetManifestId, targetBranch, path],
    queryFn: () =>
      fetchFileViewOptional(appid, targetDepotId, targetManifestId, targetBranch, path),
    enabled: path.length > 0,
  });

  const baseCreation =
    statusQuery.data?.find((s) => s.depot_id === depotId && s.manifest_id === manifestId)
      ?.creation_time ?? 0;
  const targetCreation =
    statusQuery.data?.find(
      (s) => s.depot_id === targetDepotId && s.manifest_id === targetManifestId,
    )?.creation_time ?? 0;

  const missing =
    (baseView.data === null ? "base" : null) ?? (targetView.data === null ? "target" : null);

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
        <ManifestSwitcher
          label={baseCreation > 0 ? formatDate(baseCreation) : "manifest"}
          appid={appid}
          depotId={depotId}
          currentManifestId={manifestId}
          currentBranch={branch}
          appInfo={appInfoQuery.data}
          extras={extraQuery.data ?? []}
          statuses={switcherStatusQuery.data}
          onSelect={(mid, br) =>
            navigate({
              params: { appid: appidParam, depotId: depotIdParam, manifestId: mid },
              search: {
                branch: br === "public" ? undefined : br,
                path,
                target_depot_id: targetDepotId,
                target_manifest_id: targetManifestId,
                target_branch: targetBranch === "public" ? undefined : targetBranch,
              },
            })
          }
          onGoToManifest={() =>
            navigate({
              to: "/apps/$appid/depots/$depotId/manifests/$manifestId",
              params: { appid: appidParam, depotId: depotIdParam, manifestId },
              search: { branch: branch === "public" ? undefined : branch },
            })
          }
        />
        <span className="text-slate-600">/</span>
        <Link
          from={Route.fullPath}
          to="/apps/$appid/depots/$depotId/manifests/$manifestId/file"
          params={{ appid: appidParam, depotId: depotIdParam, manifestId }}
          search={{ branch: branch === "public" ? undefined : branch, path }}
          className="hover:underline"
        >
          {filenameOf(path)}
        </Link>
        <span className="text-slate-600">/</span>
        <span className="font-medium text-slate-200">diff</span>
      </nav>

      <div className="mb-4">
        <h1 className="font-mono text-sm break-all">{path}</h1>
        <p className="mt-1 text-xs text-slate-500 tabular-nums">
          {/* Reads left → right as the unified diff does: `target` (old) on -, `base` (new) on +. */}
          <Link
            to="/apps/$appid/depots/$depotId/manifests/$manifestId/file"
            params={{
              appid: appidParam,
              depotId: String(targetDepotId),
              manifestId: targetManifestId,
            }}
            search={{ branch: targetBranch === "public" ? undefined : targetBranch, path }}
            className="font-mono text-rose-300 hover:underline"
          >
            {targetDepotId}/{targetManifestId}
          </Link>
          {targetCreation > 0 && (
            <span className="ml-1 text-rose-300/70">({formatDate(targetCreation)})</span>
          )}
          <span className="mx-2 text-slate-600">→</span>
          <Link
            to="/apps/$appid/depots/$depotId/manifests/$manifestId/file"
            params={{ appid: appidParam, depotId: depotIdParam, manifestId }}
            search={{ branch: branch === "public" ? undefined : branch, path }}
            className="font-mono text-emerald-300 hover:underline"
          >
            {depotId}/{manifestId}
          </Link>
          {baseCreation > 0 && (
            <span className="ml-1 text-emerald-300/70">({formatDate(baseCreation)})</span>
          )}
        </p>
      </div>

      {baseView.error && (
        <ErrorBox title="Failed to load base file" error={baseView.error as Error} />
      )}
      {targetView.error && (
        <ErrorBox title="Failed to load target file" error={targetView.error as Error} />
      )}
      {missing != null && (
        <p className="text-sm text-amber-300">
          The file is missing from the {missing} manifest — nothing to diff.
        </p>
      )}
      {missing == null && baseView.data && targetView.data && (
        <div className="flex min-h-0 flex-1 flex-col">
          <DiffBody
            base={baseView.data}
            target={targetView.data}
            baseLocator={{ appid, depotId, manifestId, branch, path }}
            targetLocator={{
              appid,
              depotId: targetDepotId,
              manifestId: targetManifestId,
              branch: targetBranch,
              path,
            }}
          />
        </div>
      )}
    </div>
  );
}

/// Pick the diff renderer based on what the backend says is available
/// for both sides: structured (tree) when both sides advertise a
/// `structured` info, text otherwise. Bytes-only files with no
/// transformer fall through to the target's plain preview.
function DiffBody({
  base,
  target,
  baseLocator,
  targetLocator,
}: {
  base: FileView;
  target: FileView;
  baseLocator: FileLocator;
  targetLocator: FileLocator;
}) {
  if (base.structured && target.structured) {
    return (
      <StructuredDiffView
        appid={baseLocator.appid}
        base={{
          depotId: baseLocator.depotId,
          manifestId: baseLocator.manifestId,
          branch: baseLocator.branch,
        }}
        target={targetLocator}
        path={baseLocator.path}
      />
    );
  }
  if (canDiffText(base) && canDiffText(target)) {
    return (
      <DiffView
        appid={baseLocator.appid}
        base={{
          depotId: baseLocator.depotId,
          manifestId: baseLocator.manifestId,
          branch: baseLocator.branch,
        }}
        target={targetLocator}
        path={baseLocator.path}
      />
    );
  }
  return (
    <FilePreview
      view={target}
      rawSrc={fileRawUrl(
        targetLocator.appid,
        targetLocator.depotId,
        targetLocator.manifestId,
        targetLocator.branch,
        targetLocator.path,
      )}
      locator={targetLocator}
      showHeader={false}
    />
  );
}

function canDiffText(view: FileView): boolean {
  if (view.kind !== "file") return false;
  return view.content_kind === "text" || view.transformer != null;
}

function filenameOf(path: string): string {
  const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return slash >= 0 ? path.slice(slash + 1) : path;
}
