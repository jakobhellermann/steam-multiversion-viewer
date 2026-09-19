// TODO(ai-review): review for style and correctness
import { useQueries, useQuery } from "@tanstack/react-query";
import { Link, createFileRoute } from "@tanstack/react-router";
import { useMemo, type ReactNode } from "react";

import { fetchGameInfo, fetchManifestHistory, type ManifestHistoryEntry } from "../api";
import { BranchFilter } from "../components/BranchFilter";
import { ErrorBox } from "../components/ErrorBox";
import { formatDate } from "../lib/format";
import { useBranchFilter } from "../lib/useBranchFilter";
import { useDepotManifests } from "../lib/useDepotManifests";

export const Route = createFileRoute("/apps/$appid/depots/$depotId/history")({
  component: DepotHistoryPage,
});

function DepotHistoryPage() {
  const { appid: appidParam, depotId: depotIdParam } = Route.useParams();
  const appid = Number(appidParam);
  const depotId = Number(depotIdParam);

  const { appInfo, manifests, ready } = useDepotManifests(appid, depotId);

  // The branch filter decides which tracked versions take part in the
  // walk (it filters the request, not just the rows): without it, a
  // beta between two public builds would chain the transitions
  // public → beta → public and report phantom changes.
  const branches = useMemo(
    () => [...new Set(manifests.map((manifest) => manifest.branch))].sort(),
    [manifests],
  );
  const branchFilter = useBranchFilter(appid, branches);
  const filtered = useMemo(
    () => manifests.filter((manifest) => !branchFilter.hidden.has(manifest.branch)),
    [branchFilter.hidden, manifests],
  );

  const historyQuery = useQuery({
    queryKey: [
      "manifest-history",
      appid,
      filtered.map((manifest) => `${manifest.depot_id}/${manifest.manifest_id}`).join(","),
    ],
    queryFn: () => fetchManifestHistory(appid, filtered),
    enabled: ready && filtered.length > 0,
    staleTime: Infinity,
  });

  const entries = historyQuery.data ?? [];
  const versions = useQueries({
    queries: entries.map((entry) => ({
      // Content-addressed key (same gid → same info), shared with the
      // switcher and the compare menu; `branch` still goes to the fetch.
      queryKey: ["game-info", appid, entry.depot_id, entry.manifest_id],
      queryFn: () => fetchGameInfo(appid, entry.depot_id, entry.manifest_id, entry.branch),
      staleTime: Infinity,
    })),
  });

  // Branch column only when the rows actually differ — a column of
  // identical values is noise. The toolbar filter instead keys off the
  // tracked set, so it stays reachable even while its own filter has
  // removed those branches from the rows.
  const entryBranches = new Set(entries.map((entry) => entry.branch));
  const showBranchColumn = entryBranches.size > 1;
  const showBranchFilter = branches.length > 1;

  return (
    // Scrolls with the page like the other list pages (app overview,
    // file history); tables are no fixed-pane interactive surface.
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
          {depotId}
        </Link>
        <span className="text-slate-600">/</span>
        <span className="font-medium text-slate-200">history</span>
      </nav>

      <div className="mb-3 flex items-center gap-3">
        <div className="min-w-0 flex-1">
          <h1 className="text-xl font-semibold">Manifest history</h1>
        </div>
        {/* The filter anchors to the header line's right edge, not to
            the table: the table's width changes when the filter
            refetches with a different version set, so anything
            attached to it would shift under the cursor. */}
        <div className="flex shrink-0 items-center gap-2">
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

      {ready && manifests.length === 0 && (
        <p className="mb-3 text-sm text-slate-500">No tracked manifests for this depot.</p>
      )}
      {ready && filtered.length > 0 && historyQuery.isPending && (
        <p className="text-sm text-slate-400">Loading…</p>
      )}
      {historyQuery.error && (
        <ErrorBox title="History failed" error={historyQuery.error as Error} />
      )}

      {entries.length > 0 && (
        <table className="text-left text-sm">
          <thead>
            <tr className="border-b border-slate-700">
              <th className="px-3 py-2">Created</th>
              <th className="px-3 py-2">Version</th>
              <th className="px-3 py-2">Changes</th>
              {showBranchColumn && <th className="px-3 py-2">Branch</th>}
            </tr>
          </thead>
          <tbody>
            {entries.map((entry, index) => {
              const gameVersion = versions[index]?.data?.engine?.data.bundle_version;
              const created = formatDate(entry.creation_time);
              return (
                <tr
                  key={`${entry.depot_id}-${entry.manifest_id}`}
                  className="border-b border-slate-800 hover:bg-slate-800"
                >
                  <td className="p-0">
                    <RowLink
                      entry={entry}
                      appidParam={appidParam}
                      depotIdParam={depotIdParam}
                      className="text-slate-500 tabular-nums"
                    >
                      {created}
                    </RowLink>
                  </td>
                  <td className="p-0">
                    <RowLink
                      entry={entry}
                      appidParam={appidParam}
                      depotIdParam={depotIdParam}
                      className={gameVersion ? "text-sky-300" : "text-slate-600"}
                    >
                      {gameVersion ? `v${gameVersion}` : "—"}
                    </RowLink>
                  </td>
                  <td className="p-0">
                    <RowLink
                      entry={entry}
                      appidParam={appidParam}
                      depotIdParam={depotIdParam}
                      primary
                      className="tabular-nums"
                    >
                      <Transition entry={entry} />
                    </RowLink>
                  </td>
                  {showBranchColumn && (
                    <td className="p-0">
                      <RowLink
                        entry={entry}
                        appidParam={appidParam}
                        depotIdParam={depotIdParam}
                        className="text-slate-500"
                      >
                        {entry.branch}
                      </RowLink>
                    </td>
                  )}
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </main>
  );
}

/// A version with any path-level difference against its `previous`.
function hasChanges(entry: ManifestHistoryEntry): boolean {
  return entry.added > 0 || entry.removed > 0 || entry.changed > 0;
}

/// The changes cell: `+added −removed changed` with per-kind colours,
/// zero components omitted; `initial` for the oldest tracked version,
/// `unchanged` for one with no changes against its predecessor.
function Transition({ entry }: { entry: ManifestHistoryEntry }) {
  if (!entry.previous) return <span className="text-slate-500">initial</span>;
  if (!hasChanges(entry)) return <span className="text-slate-500">unchanged</span>;
  return (
    <span className="tabular-nums">
      {entry.added > 0 && <span className="text-emerald-300">+{entry.added} </span>}
      {entry.removed > 0 && <span className="text-rose-300">−{entry.removed} </span>}
      {entry.changed > 0 && <span className="text-amber-300">~{entry.changed}</span>}
    </span>
  );
}

/// One cell's link in a history row. Every cell links to the same
/// destination (library-table pattern: one tab stop per row): versions
/// with changes open the manifest page with `compare_to` set to the
/// previous version — the diff view —, unchanged and initial ones the
/// manifest page itself, where a diff against an identical predecessor
/// would show nothing. The manifest id rides along as the tooltip.
function RowLink({
  entry,
  appidParam,
  depotIdParam,
  primary = false,
  className = "",
  children,
}: {
  entry: ManifestHistoryEntry;
  appidParam: string;
  depotIdParam: string;
  primary?: boolean;
  className?: string;
  children: ReactNode;
}) {
  const diffTarget = entry.previous != null && hasChanges(entry) ? entry.previous : undefined;
  return (
    <Link
      to="/apps/$appid/depots/$depotId/manifests/$manifestId"
      params={{
        appid: appidParam,
        depotId: depotIdParam,
        manifestId: entry.manifest_id,
      }}
      search={{
        branch: entry.branch === "public" ? undefined : entry.branch,
        compare_to: diffTarget?.manifest_id,
      }}
      title={`manifest ${entry.manifest_id}`}
      tabIndex={primary ? undefined : -1}
      aria-hidden={primary ? undefined : "true"}
      className={`block px-3 py-2 ${className}`}
    >
      {children}
    </Link>
  );
}
