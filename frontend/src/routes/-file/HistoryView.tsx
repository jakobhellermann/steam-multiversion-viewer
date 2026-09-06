// TODO(ai-review): review for style and correctness
import { useQueries, useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useMemo, useState, type ReactNode } from "react";

import {
  fetchFileHistory,
  fetchFileStructured,
  fetchGameInfo,
  type FileHistoryEntry,
  type HistoryStatus,
  type ManifestRef,
  type ManifestStatusEntry,
  type StructuredNode,
} from "../../api";
import { ErrorBox } from "../../components/ErrorBox";
import { BranchFilter } from "../../components/BranchFilter";
import { formatDate } from "../../lib/format";
import { useBranchFilter } from "../../lib/useBranchFilter";

/// Per-status text color. Follows the diff conventions (added = new side
/// emerald, removed = old side rose); neutral states stay muted.
function statusClass(status: HistoryStatus): string {
  switch (status) {
    case "added":
      return "text-emerald-300";
    case "removed":
      return "text-rose-300";
    case "changed":
      return "text-amber-300";
    default:
      return "text-slate-500";
  }
}

export function HistoryView({
  appid,
  appidParam,
  current,
  previous,
  path,
  nodeId,
  statuses,
  inputsReady,
}: {
  appid: number;
  appidParam: string;
  current: ManifestRef;
  previous: ManifestRef[];
  path: string;
  nodeId?: string;
  statuses: ManifestStatusEntry[] | undefined;
  /// Whether `previous` is complete (app info + tracked extras have
  /// loaded). Until then the history query must not fire — it would
  /// fetch with an incomplete manifest list and then again once the
  /// list settles.
  inputsReady: boolean;
}) {
  const [onlyChanged, setOnlyChanged] = useState(false);
  const branches = useMemo(
    () => [...new Set(previous.map((manifest) => manifest.branch))].sort(),
    [previous],
  );
  const branchFilter = useBranchFilter(appid, branches);
  const filteredPrevious = useMemo(
    () => previous.filter((manifest) => !branchFilter.hidden.has(manifest.branch)),
    [branchFilter.hidden, previous],
  );
  const historyQuery = useQuery({
    queryKey: ["file-history", appid, current, filteredPrevious, path, nodeId],
    queryFn: () => fetchFileHistory(appid, current, filteredPrevious, path, nodeId),
    enabled: path.length > 0 && inputsReady,
    staleTime: Infinity,
  });
  // Resolve the scoped node's label in the current tree — normally a
  // cache hit, the file page already loaded this exact tree.
  const treeQuery = useQuery({
    queryKey: [
      "file-structured",
      appid,
      current.depot_id,
      current.manifest_id,
      current.branch,
      path,
    ],
    queryFn: () =>
      fetchFileStructured(appid, current.depot_id, current.manifest_id, current.branch, path),
    enabled: nodeId != null,
    staleTime: Infinity,
  });
  const selectedNode = nodeId && treeQuery.data ? findNode(treeQuery.data.root, nodeId) : undefined;
  // Newest first: the backend already walks oldest → newest, but the
  // manifest status list knows creation times for every ref (including
  // ones the backend skipped), so sort on that when available.
  const entries = useMemo(
    () =>
      [...(historyQuery.data ?? [])].sort((left, right) => {
        const leftTime = statuses?.find(
          (status) => status.depot_id === left.depot_id && status.manifest_id === left.manifest_id,
        )?.creation_time;
        const rightTime = statuses?.find(
          (status) =>
            status.depot_id === right.depot_id && status.manifest_id === right.manifest_id,
        )?.creation_time;
        return (rightTime ?? 0) - (leftTime ?? 0);
      }),
    [historyQuery.data, statuses],
  );
  const versions = useQueries({
    queries: entries.map((entry) => ({
      queryKey: ["game-info", appid, entry.depot_id, entry.manifest_id, entry.branch],
      queryFn: () => fetchGameInfo(appid, entry.depot_id, entry.manifest_id, entry.branch),
      staleTime: Infinity,
    })),
  });
  const visibleEntries = onlyChanged
    ? entries.filter((entry) => entry.status !== "unchanged" && entry.status !== "initial")
    : entries;
  // Branch column only when the rows actually differ — a column of
  // identical values is noise. The toolbar filter instead keys off the
  // tracked set: it must stay reachable even while its own filter has
  // removed the other branches from the current rows.
  const entryBranches = new Set(entries.map((entry) => entry.branch));
  const showBranchColumn = entryBranches.size > 1;
  const showBranchFilter = branches.length > 1;

  return (
    <section>
      <div className="mb-3 flex items-center gap-3">
        <div className="min-w-0 flex-1">
          {/* The scoped object is the last leg of the address — file path
              first, then the object inside it — not a separate chip row. */}
          <h1 className="font-mono text-sm break-all">
            {path}
            {nodeId && (
              <>
                <span className="text-slate-600">{" :: "}</span>
                <Link
                  to="/apps/$appid/depots/$depotId/manifests/$manifestId/file"
                  params={{
                    appid: appidParam,
                    depotId: String(current.depot_id),
                    manifestId: current.manifest_id,
                  }}
                  search={{
                    branch: current.branch === "public" ? undefined : current.branch,
                    path,
                  }}
                  hash={nodeId}
                  className="text-sky-300 hover:underline"
                >
                  {selectedNode?.label ?? nodeId}
                </Link>
                {selectedNode?.badge && (
                  <span className="ml-1.5 font-sans text-xs text-slate-500">
                    {selectedNode.badge}
                  </span>
                )}
              </>
            )}
          </h1>
        </div>
        {/* Filters anchor to the header line's right edge, not to the
            table: the table's width changes as rows are filtered, so
            anything attached to it would shift under the cursor. The
            match count rides along on the toggle that causes it. */}
        <div className="flex shrink-0 items-center gap-2">
          <label
            className="flex cursor-pointer items-center gap-1.5 rounded border border-slate-700 px-3 py-1.5 text-sm whitespace-nowrap text-slate-300 select-none hover:border-slate-600"
            title="Hide versions where the file didn't change"
          >
            <input
              type="checkbox"
              checked={onlyChanged}
              onChange={(event) => setOnlyChanged(event.target.checked)}
              className="accent-sky-500"
            />
            Only changed
          </label>
          {showBranchFilter && (
            <BranchFilter
              branches={branches}
              hidden={branchFilter.hidden}
              onToggle={branchFilter.toggle}
              onShowAll={branchFilter.showAll}
              onHideAll={branchFilter.hideAll}
              onOnly={branchFilter.only}
              size="md"
            />
          )}
        </div>
      </div>

      {inputsReady && filteredPrevious.length <= 1 && (
        <p className="mb-3 text-sm text-slate-500">No other tracked manifests for this depot.</p>
      )}
      {historyQuery.error && (
        <ErrorBox title="History failed" error={historyQuery.error as Error} />
      )}

      {historyQuery.data && (
        <table className="text-left text-sm">
          <thead>
            <tr className="border-b border-slate-700">
              <th className="px-3 py-2">Status</th>
              <th className="px-3 py-2">Created</th>
              <th className="px-3 py-2">Version</th>
              {showBranchColumn && <th className="px-3 py-2">Branch</th>}
            </tr>
          </thead>
          <tbody>
            {visibleEntries.length === 0 && (
              <tr>
                <td colSpan={showBranchColumn ? 4 : 3} className="px-3 py-3 text-slate-500">
                  No versions match the filters.
                </td>
              </tr>
            )}
            {visibleEntries.map((entry) => {
              const index = entries.indexOf(entry);
              const creationTime = statuses?.find(
                (status) =>
                  status.depot_id === entry.depot_id && status.manifest_id === entry.manifest_id,
              )?.creation_time;
              const gameVersion = versions[index]?.data?.engine?.data.bundle_version;
              const created =
                creationTime != null && creationTime > 0 ? formatDate(creationTime) : "—";
              const version = gameVersion ? `v${gameVersion}` : "—";
              return (
                <tr
                  key={`${entry.depot_id}-${entry.manifest_id}`}
                  className="border-b border-slate-800 hover:bg-slate-800"
                >
                  <td className="p-0">
                    <EntryLink
                      entry={entry}
                      appidParam={appidParam}
                      path={path}
                      primary
                      className={`capitalize ${statusClass(entry.status)}`}
                    >
                      {entry.status}
                    </EntryLink>
                  </td>
                  <td className="p-0">
                    <EntryLink
                      entry={entry}
                      appidParam={appidParam}
                      path={path}
                      className="text-slate-500 tabular-nums"
                    >
                      {created}
                    </EntryLink>
                  </td>
                  <td className="p-0">
                    <EntryLink
                      entry={entry}
                      appidParam={appidParam}
                      path={path}
                      className={version === "—" ? "text-slate-600" : "text-sky-300"}
                    >
                      {version}
                    </EntryLink>
                  </td>
                  {showBranchColumn && (
                    <td className="p-0">
                      <EntryLink
                        entry={entry}
                        appidParam={appidParam}
                        path={path}
                        className="text-slate-500"
                      >
                        {entry.branch}
                      </EntryLink>
                    </td>
                  )}
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </section>
  );
}

function findNode(node: StructuredNode, id: string): StructuredNode | undefined {
  if (node.id === id) return node;
  for (const child of node.children) {
    const found = findNode(child, id);
    if (found) return found;
  }
  return undefined;
}

/// One cell's link in a history row. Every cell of a row links to the
/// same destination: changed/added entries open the pairwise diff
/// against the previous manifest, everything else the file in that
/// manifest. Secondary cells (`primary` unset) are unfocusable and
/// hidden from screen readers so a row is a single tab stop
/// (library-table pattern). The manifest id rides along as the tooltip.
function EntryLink({
  entry,
  appidParam,
  path,
  primary = false,
  className = "",
  children,
}: {
  entry: FileHistoryEntry;
  appidParam: string;
  path: string;
  primary?: boolean;
  className?: string;
  children: ReactNode;
}) {
  const params = {
    appid: appidParam,
    depotId: String(entry.depot_id),
    manifestId: entry.manifest_id,
  };
  const previous =
    entry.status === "changed" || entry.status === "added" ? entry.previous : undefined;
  if (previous) {
    return (
      <Link
        to="/apps/$appid/depots/$depotId/manifests/$manifestId/diff"
        params={params}
        search={{
          branch: entry.branch === "public" ? undefined : entry.branch,
          path,
          target_depot_id: previous.depot_id,
          target_manifest_id: previous.manifest_id,
          target_branch: previous.branch === "public" ? undefined : previous.branch,
        }}
        hash={entry.diff_node_id}
        title={`manifest ${entry.manifest_id}`}
        tabIndex={primary ? undefined : -1}
        aria-hidden={primary ? undefined : "true"}
        className={`block px-3 py-2 ${className}`}
      >
        {children}
      </Link>
    );
  }
  return (
    <Link
      to="/apps/$appid/depots/$depotId/manifests/$manifestId/file"
      params={params}
      search={{ branch: entry.branch === "public" ? undefined : entry.branch, path }}
      hash={entry.node_id}
      title={`manifest ${entry.manifest_id}`}
      tabIndex={primary ? undefined : -1}
      aria-hidden={primary ? undefined : "true"}
      className={`block px-3 py-2 ${className}`}
    >
      {children}
    </Link>
  );
}
