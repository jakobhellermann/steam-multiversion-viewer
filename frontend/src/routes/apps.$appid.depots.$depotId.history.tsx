// TODO(ai-review): review for style and correctness
import { useQueries, useQuery, useQueryClient, type UseQueryResult } from "@tanstack/react-query";
import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { CircleAlert, Loader2 } from "lucide-react";
import { useEffect, useMemo, useRef, type ReactNode } from "react";

import {
  fetchGameInfo,
  fetchManifestDiffDeep,
  fetchManifestHistory,
  manifestDiffDeepKey,
  type ManifestDiffEntry,
  type ManifestHistoryEntry,
} from "../api";
import { BranchFilter } from "../components/BranchFilter";
import { ErrorBox } from "../components/ErrorBox";
import { formatDate } from "../lib/format";
import { useBranchFilter } from "../lib/useBranchFilter";
import { manifestRefOf, useDepotManifests } from "../lib/useDepotManifests";

type Search = {
  /// Deep-compare toggle. In the URL so it survives reloads and links.
  deep?: boolean;
};

export const Route = createFileRoute("/apps/$appid/depots/$depotId/history")({
  validateSearch: (search: Record<string, unknown>): Search => ({
    deep: search.deep === true || search.deep === "true" ? true : undefined,
  }),
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
  // public → beta → public and report phantom changes. A version
  // participates while ANY of its branches is visible — a build the
  // public and beta heads point at together is a public version.
  const branches = useMemo(
    () => [...new Set(manifests.flatMap((manifest) => manifest.branches))].sort(),
    [manifests],
  );
  const branchFilter = useBranchFilter(appid, branches);
  const filtered = useMemo(
    () =>
      manifests.filter((manifest) =>
        manifest.branches.some((branch) => !branchFilter.hidden.has(branch)),
      ),
    [branchFilter.hidden, manifests],
  );

  const historyQuery = useQuery({
    queryKey: [
      "manifest-history",
      appid,
      filtered.map((manifest) => `${manifest.depot_id}/${manifest.manifest_id}`).join(","),
    ],
    queryFn: () => fetchManifestHistory(appid, filtered.map(manifestRefOf)),
    enabled: ready && filtered.length > 0,
    staleTime: Infinity,
  });

  const entries = useMemo(() => historyQuery.data ?? [], [historyQuery.data]);

  const navigate = useNavigate({ from: Route.fullPath });
  const deepCompare = Route.useSearch().deep ?? false;
  // Set synchronously here (the driver's queryFn starts before any
  // effect could update it) and re-synced by the effect below for
  // URL-driven changes (back/forward).
  const deepPausedRef = useRef(!deepCompare);
  const setDeepCompare = (next: boolean) => {
    deepPausedRef.current = !next;
    navigate({
      search: (prev) => ({ ...prev, deep: next ? true : undefined }),
      replace: true,
      resetScroll: false,
    });
  };

  // Deep compare refines only `changed` — added/removed rows pass
  // through the sweep untouched.
  const deepPairs = useMemo(
    () =>
      entries.flatMap((entry) => {
        if (entry.previous == null || entry.changed === 0) return [];
        return [
          {
            base: {
              depot_id: entry.depot_id,
              manifest_id: entry.manifest_id,
              branch: entry.branch,
            },
            target: entry.previous,
          },
        ];
      }),
    [entries],
  );
  const deepPairIndex = useMemo(() => {
    const m = new Map<string, number>();
    deepPairs.forEach((pair, i) => m.set(`${pair.base.manifest_id}/${pair.target.manifest_id}`, i));
    return m;
  }, [deepPairs]);

  const queryClient = useQueryClient();
  // One pair at a time: each sweep downloads both sides of its
  // changed files and holds a private rabex env pair on the backend,
  // so parallel pairs would multiply memory and CDN load.
  useQuery({
    queryKey: [
      "history-deep-sweep",
      appid,
      deepPairs.map((p) => `${p.base.manifest_id}/${p.target.manifest_id}`).join(","),
    ],
    queryFn: async ({ signal }) => {
      for (const pair of deepPairs) {
        if (deepPausedRef.current) return;
        try {
          await queryClient.fetchQuery({
            queryKey: manifestDiffDeepKey(appid, pair.base, pair.target),
            queryFn: () => fetchManifestDiffDeep(appid, pair.base, pair.target, signal),
            staleTime: Infinity,
          });
        } catch {
          // One failed pair doesn't stop the chain; its row flags the error.
          if (signal.aborted) return;
        }
      }
    },
    enabled: deepCompare && deepPairs.length > 0,
    staleTime: Infinity,
  });
  // A stopped driver counts as fresh — switching deep compare back on
  // has to kick it explicitly.
  useEffect(() => {
    deepPausedRef.current = !deepCompare;
    if (deepCompare) {
      queryClient.invalidateQueries({ queryKey: ["history-deep-sweep", appid] });
    }
  }, [deepCompare, appid, queryClient]);
  // Rows only subscribe — the driver owns which pair fetches when.
  const deepResults = useQueries({
    queries: deepPairs.map((pair) => ({
      queryKey: manifestDiffDeepKey(appid, pair.base, pair.target),
      queryFn: () => fetchManifestDiffDeep(appid, pair.base, pair.target),
      enabled: false,
      staleTime: Infinity,
    })),
  });
  const deepDone = deepResults.filter((r) => r.data != null || r.isError).length;
  const deepRunning = deepCompare && deepDone < deepResults.length;

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
          {deepPairs.length > 0 && (
            <label className="flex cursor-pointer items-center gap-1.5 rounded border border-slate-700 px-3 py-1.5 text-sm whitespace-nowrap text-slate-300 select-none">
              <input
                type="checkbox"
                checked={deepCompare}
                onChange={(e) => setDeepCompare(e.target.checked)}
                className="accent-sky-500"
              />
              Deep compare
              {deepRunning && (
                <>
                  <Loader2 size={13} className="animate-spin text-slate-500" />
                  <span className="text-slate-500 tabular-nums">
                    {deepDone}/{deepResults.length}
                  </span>
                </>
              )}
            </label>
          )}
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
              const pairIndex =
                entry.previous != null
                  ? deepPairIndex.get(`${entry.manifest_id}/${entry.previous.manifest_id}`)
                  : undefined;
              const deep = deepTransition(
                entry,
                deepCompare,
                pairIndex,
                pairIndex != null ? deepResults[pairIndex] : undefined,
              );
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
                      deep={deepCompare}
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
                      deep={deepCompare}
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
                      deep={deepCompare}
                      primary
                      className="tabular-nums"
                    >
                      <Transition entry={entry} deep={deep} />
                    </RowLink>
                  </td>
                  {showBranchColumn && (
                    <td className="p-0">
                      <RowLink
                        entry={entry}
                        appidParam={appidParam}
                        depotIdParam={depotIdParam}
                        deep={deepCompare}
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

/// Deep-compare state of one history row.
function deepTransition(
  entry: ManifestHistoryEntry,
  active: boolean,
  pairIndex: number | undefined,
  result: UseQueryResult<ManifestDiffEntry[]> | undefined,
): { phase: "done"; changed: number } | { phase: "pending" } | { phase: "failed" } {
  if (!active || entry.previous == null || pairIndex == null) {
    return { phase: "done", changed: entry.changed };
  }
  if (result?.isError) return { phase: "failed" };
  if (result?.data != null) {
    return {
      phase: "done",
      changed: result.data.filter((e) => e.status === "changed").length,
    };
  }
  return { phase: "pending" };
}

/// The changes cell: `+added −removed changed` with per-kind colours,
/// zero components omitted; `initial` for the oldest tracked version,
/// `unchanged` for one with no changes against its predecessor. Deep
/// compare replaces the `changed` count in place. `whitespace-nowrap`
/// keeps the row height stable across states — without it the pending
/// spinner's width can wrap the cell.
function Transition({
  entry,
  deep,
}: {
  entry: ManifestHistoryEntry;
  deep: ReturnType<typeof deepTransition>;
}) {
  if (!entry.previous) return <span className="text-slate-500">initial</span>;
  const changed = deep.phase === "done" ? deep.changed : entry.changed;
  if (entry.added === 0 && entry.removed === 0 && changed === 0) {
    return <span className="text-slate-500">unchanged</span>;
  }
  const parts: ReactNode[] = [];
  // added/removed are final under deep compare; dimming them anyway
  // marks the cell as one pending unit, not just `changed`.
  const dimmed = deep.phase === "pending";
  if (deep.phase === "pending") {
    parts.push(
      <span key="changed" className="text-slate-500">
        {entry.changed}
      </span>,
    );
  } else if (deep.phase === "failed") {
    parts.push(
      <span key="changed" className="text-amber-300">
        {entry.changed}
      </span>,
    );
  } else if (changed > 0) {
    parts.push(
      <span key="changed" className="text-amber-300">
        {changed}
      </span>,
    );
  }
  if (entry.added > 0) {
    parts.push(
      <span key="added" className={dimmed ? "text-slate-500" : "text-emerald-300"}>
        +{entry.added}
      </span>,
    );
  }
  if (entry.removed > 0) {
    parts.push(
      <span key="removed" className={dimmed ? "text-slate-500" : "text-rose-300"}>
        −{entry.removed}
      </span>,
    );
  }
  return (
    <span className="whitespace-nowrap tabular-nums">
      {parts.flatMap((part, index) => (index === 0 ? [part] : [" ", part]))}
      {deep.phase === "pending" && (
        <>
          {" "}
          <Loader2 size={11} className="inline-block animate-spin text-slate-500 align-[-2px]" />
        </>
      )}
      {deep.phase === "failed" && (
        <>
          {" "}
          <CircleAlert size={11} className="inline-block text-rose-400 align-[-2px]">
            <title>deep compare failed for this transition</title>
          </CircleAlert>
        </>
      )}
    </span>
  );
}

/// One cell's link in a history row. Every cell links to the same
/// destination (library-table pattern: one tab stop per row): versions
/// with changes open the manifest page with `compare_to` set to the
/// previous version — the diff view —, unchanged and initial ones the
/// manifest page itself, where a diff against an identical predecessor
/// would show nothing. The manifest id rides along as the tooltip.
/// `deep` arms deep compare on the target page.
function RowLink({
  entry,
  appidParam,
  depotIdParam,
  deep = false,
  primary = false,
  className = "",
  children,
}: {
  entry: ManifestHistoryEntry;
  appidParam: string;
  depotIdParam: string;
  deep?: boolean;
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
        deep: deep ? true : undefined,
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
