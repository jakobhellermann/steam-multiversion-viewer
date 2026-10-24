// TODO(ai-review): review for style and correctness
import { Link, createFileRoute, linkOptions, useNavigate } from "@tanstack/react-router";

import { ManifestSwitcher } from "../components/ManifestSwitcher";
import { useAppManifestStatuses, useManifestStatus } from "../lib/appManifests";
import { formatDate } from "../lib/format";
import { useDepotManifests } from "../lib/useDepotManifests";
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

  const {
    appInfo,
    extras,
    manifests: previous,
    ready,
  } = useDepotManifests(appid, depotId, {
    manifest_id: manifestId,
    branch,
  });

  // Statuses for the breadcrumb's manifest label; they also cover every manifest `previous`
  // can contain, which the table's dates come from.
  const { statuses: statusQuery } = useAppManifestStatuses(appid);

  const currentCreation = useManifestStatus(
    appid,
    { depot_id: depotId, manifest_id: manifestId, branch },
    statusQuery.data,
  ).data?.creation_time;

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
        inputsReady={ready}
      />
    </main>
  );
}

function filenameOf(path: string): string {
  const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return slash >= 0 ? path.slice(slash + 1) : path;
}
