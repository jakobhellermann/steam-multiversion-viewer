// TODO(ai-review): review for style and correctness
import { useQuery } from "@tanstack/react-query";
import { Link, createFileRoute, linkOptions, useNavigate } from "@tanstack/react-router";
import { useMemo } from "react";

import { fetchAppInfo, fetchExtraManifests, fetchManifestStatuses, type ManifestRef } from "../api";
import { ManifestSwitcher } from "../components/ManifestSwitcher";
import { formatDate } from "../lib/format";
import { HistoryView } from "./-file/HistoryView";

type Search = {
  branch?: string;
  path: string;
  node_id?: string;
};

export const Route = createFileRoute(
  "/apps/$appid/depots/$depotId/manifests/$manifestId_/file_/history",
)({
  validateSearch: (search: Record<string, unknown>): Search => ({
    branch:
      typeof search.branch === "string" && search.branch !== "public" ? search.branch : undefined,
    path: typeof search.path === "string" ? search.path : "",
    node_id: typeof search.node_id === "string" ? search.node_id : undefined,
  }),
  component: FileHistoryPage,
});

function FileHistoryPage() {
  const { appid: appidParam, depotId: depotIdParam, manifestId } = Route.useParams();
  const { branch: branchSearch, path, node_id: nodeId } = Route.useSearch();
  const navigate = useNavigate({ from: Route.fullPath });
  const appid = Number(appidParam);
  const depotId = Number(depotIdParam);
  const branch = branchSearch ?? "public";

  const appInfo = useQuery({ queryKey: ["app", appid], queryFn: () => fetchAppInfo(appid) });
  const extras = useQuery({
    queryKey: ["extra-manifests", appid],
    queryFn: () => fetchExtraManifests(appid),
  });

  // Statuses for the breadcrumb's manifest label. Same key as the other
  // sub-pages so it's usually a cache hit; it also covers every manifest
  // `previous` below can contain, which the table's dates come from.
  const compareRefs = useMemo<ManifestRef[]>(() => {
    const seen = new Set<string>();
    const refs: ManifestRef[] = [];
    for (const depot of appInfo.data?.depots ?? []) {
      for (const manifest of depot.manifests) {
        const key = `${depot.depot_id}/${manifest.manifest_id}`;
        if (seen.has(key)) continue;
        seen.add(key);
        refs.push({
          depot_id: depot.depot_id,
          manifest_id: manifest.manifest_id,
          branch: manifest.branch,
        });
      }
    }
    for (const extra of extras.data ?? []) {
      const key = `${extra.depot_id}/${extra.manifest_id}`;
      if (seen.has(key)) continue;
      seen.add(key);
      refs.push({
        depot_id: extra.depot_id,
        manifest_id: extra.manifest_id,
        branch: extra.branch ?? "public",
      });
    }
    return refs;
  }, [appInfo.data, extras.data]);
  const statusQuery = useQuery({
    queryKey: ["manifest-statuses", appid, compareRefs],
    queryFn: () => fetchManifestStatuses(appid, compareRefs),
    enabled: compareRefs.length > 0 && extras.isSuccess,
  });

  // Manifests to compare against: every tracked manifest of this depot.
  const previous = useMemo<ManifestRef[]>(() => {
    const refs = new Map<string, ManifestRef>();
    refs.set(`${depotId}/${manifestId}`, { depot_id: depotId, manifest_id: manifestId, branch });
    const depot = appInfo.data?.depots.find((entry) => entry.depot_id === depotId);
    for (const manifest of depot?.manifests ?? []) {
      refs.set(`${depotId}/${manifest.manifest_id}`, {
        depot_id: depotId,
        manifest_id: manifest.manifest_id,
        branch: manifest.branch,
      });
    }
    for (const manifest of extras.data ?? []) {
      if (manifest.depot_id !== depotId) continue;
      refs.set(`${depotId}/${manifest.manifest_id}`, {
        depot_id: depotId,
        manifest_id: manifest.manifest_id,
        branch: manifest.branch ?? "public",
      });
    }
    return [...refs.values()];
  }, [appInfo.data, branch, depotId, extras.data, manifestId]);

  const currentCreation = (statusQuery.data ?? []).find(
    (status) => status.depot_id === depotId && status.manifest_id === manifestId,
  )?.creation_time;

  return (
    // Scrolls with the page like the other list pages (app overview,
    // manifest overview). The viewport-fixed shell is for the
    // interactive file/diff surfaces, not for tables.
    <main className="mx-auto max-w-6xl p-8">
      <nav className="mb-4 flex items-center gap-2 text-sm text-slate-400">
        <Link to="/apps/$appid" params={{ appid: appidParam }} className="hover:underline">
          {appInfo.data?.name ?? `App ${appid}`}
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
          label={currentCreation && currentCreation > 0 ? formatDate(currentCreation) : "manifest"}
          appid={appid}
          depotId={depotId}
          currentManifestId={manifestId}
          currentBranch={branch}
          appInfo={appInfo.data}
          extras={extras.data ?? []}
          statuses={statusQuery.data}
          onSelect={(mid, br) => {
            // Node ids are path-based, so the scoped object stays
            // meaningful across manifests; the backend tracks it.
            navigate({
              params: { appid: appidParam, depotId: depotIdParam, manifestId: mid },
              search: {
                branch: br === "public" ? undefined : br,
                path,
                node_id: nodeId,
              },
            });
          }}
          manifestLink={linkOptions({
            to: "/apps/$appid/depots/$depotId/manifests/$manifestId",
            params: { appid: appidParam, depotId: depotIdParam, manifestId },
            search: { branch: branch === "public" ? undefined : branch },
          })}
        />
        <span className="text-slate-600">/</span>
        <Link
          to="/apps/$appid/depots/$depotId/manifests/$manifestId/file"
          params={{ appid: appidParam, depotId: depotIdParam, manifestId }}
          search={{ branch: branch === "public" ? undefined : branch, path }}
          className="hover:underline"
        >
          {filenameOf(path)}
        </Link>
        <span className="text-slate-600">/</span>
        <span className="font-medium text-slate-200">history</span>
      </nav>

      {/* Header (path + scoped object + filters) and table live in
          HistoryView — the filters share its state. */}
      <HistoryView
        appid={appid}
        appidParam={appidParam}
        current={{ depot_id: depotId, manifest_id: manifestId, branch }}
        previous={previous}
        path={path}
        nodeId={nodeId}
        statuses={statusQuery.data}
        inputsReady={appInfo.isSuccess && extras.isSuccess}
      />
    </main>
  );
}

function filenameOf(path: string): string {
  const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return slash >= 0 ? path.slice(slash + 1) : path;
}
