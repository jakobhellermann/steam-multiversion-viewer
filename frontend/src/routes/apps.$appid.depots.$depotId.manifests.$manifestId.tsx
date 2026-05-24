// TODO(ai-review): review for style and correctness
import { createFileRoute, Link } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { memo, useCallback, useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import {
  downloadManifest,
  fetchAppInfo,
  fetchExtraManifests,
  fetchManifestDiff,
  fetchManifestFiles,
  fetchManifestInfo,
  fetchManifestStatuses,
  type AppInfo,
  type EnqueueSummary,
  type ExtraManifestEntry,
  type ManifestFile,
  type ManifestInfo,
  type ManifestRef,
  type ManifestStatusEntry,
} from "../api";
import { Bytes } from "../Bytes";
import { ErrorBox } from "../ErrorBox";
import { formatBytes } from "../format";
import { pinScroll } from "../pinScroll";

type Search = {
  branch: string;
};

export const Route = createFileRoute("/apps/$appid/depots/$depotId/manifests/$manifestId")({
  validateSearch: (search: Record<string, unknown>): Search => ({
    branch: typeof search.branch === "string" ? search.branch : "public",
  }),
  component: ManifestDetail,
});

function ManifestDetail() {
  const { appid: appidParam, depotId: depotIdParam, manifestId } = Route.useParams();
  const { branch } = Route.useSearch();
  const appid = Number(appidParam);
  const depotId = Number(depotIdParam);

  const info = useQuery({
    queryKey: ["manifest-info", appid, depotId, manifestId, branch],
    queryFn: () => fetchManifestInfo(appid, depotId, manifestId, branch),
  });
  const files = useQuery({
    queryKey: ["manifest-files", appid, depotId, manifestId, branch],
    queryFn: () => fetchManifestFiles(appid, depotId, manifestId, branch),
  });
  // Needed by the "compare to…" menu so the user can pick any other
  // manifest in the app (official or user-tracked) as a diff target.
  const appInfoQuery = useQuery({
    queryKey: ["app", appid],
    queryFn: () => fetchAppInfo(appid),
  });
  const extraQuery = useQuery({
    queryKey: ["extra-manifests", appid],
    queryFn: () => fetchExtraManifests(appid),
  });
  // Statuses give us creation_time per manifest so the compare-with
  // submenu can sort by date. We reuse the same query key as the app
  // detail page so the cache is shared.
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
    enabled: compareRefs.length > 0,
  });

  const downloadAll = useMutation({
    mutationFn: () => downloadManifest(appid, depotId, manifestId, { branch }),
  });

  return (
    <div className="p-8 max-w-6xl mx-auto">
      <nav className="text-sm text-slate-400 mb-4">
        <Link to="/apps/$appid" params={{ appid: appidParam }} className="hover:underline">
          ← App {appid}
        </Link>
      </nav>

      {info.isPending && <p className="text-slate-400">Loading manifest…</p>}
      {info.error && <ErrorBox title="Failed to load manifest" error={info.error as Error} />}
      {info.data && (
        <ManifestHeader
          info={info.data}
          branch={branch}
          onDownload={() => downloadAll.mutate()}
          downloadPending={downloadAll.isPending}
          downloadResult={downloadAll.data}
          downloadError={downloadAll.error as Error | null}
        />
      )}

      <section className="mt-8">
        {files.error && <ErrorBox title="Failed to load files" error={files.error as Error} />}
        {files.isPending && <p className="text-slate-400 text-sm">Loading files…</p>}
        {files.data && (
          <FilesPanel
            allFiles={files.data.files}
            appid={appidParam}
            depotId={depotIdParam}
            manifestId={manifestId}
            branch={branch}
            appInfo={appInfoQuery.data}
            extras={extraQuery.data ?? []}
            statuses={statusQuery.data}
          />
        )}
      </section>
    </div>
  );
}

function ManifestHeader({
  info,
  onDownload,
  downloadPending,
  downloadResult,
  downloadError,
}: {
  info: ManifestInfo;
  branch: string;
  onDownload: () => void;
  downloadPending: boolean;
  downloadResult: EnqueueSummary | undefined;
  downloadError: Error | null;
}) {
  return (
    <div>
      <div className="flex items-start gap-4">
        <h1 className="text-2xl font-bold">Manifest</h1>
        <button
          type="button"
          onClick={onDownload}
          disabled={downloadPending}
          className="ml-auto px-3 py-1.5 text-sm border border-sky-700 bg-sky-950/40 rounded hover:bg-sky-900/40 disabled:opacity-50"
        >
          {downloadPending ? "Enqueuing…" : "Download all"}
        </button>
      </div>
      {downloadResult && (
        <p className="mt-2 text-xs text-slate-400">
          {downloadResult.enqueued_chunks > 0
            ? `Enqueued ${downloadResult.enqueued_chunks.toLocaleString()} chunks (${formatBytes(downloadResult.enqueued_bytes)} compressed).`
            : "Nothing to download — everything is already cached."}
          {downloadResult.already_present_chunks > 0 && (
            <span> {downloadResult.already_present_chunks.toLocaleString()} already on disk.</span>
          )}
        </p>
      )}
      {downloadError && (
        <p className="mt-2 text-xs text-red-300">Download failed: {downloadError.message}</p>
      )}
      <dl className="mt-4 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
        <dt className="text-slate-400">Manifest ID</dt>
        <dd className="font-mono tabular-nums break-all">{info.manifest_id}</dd>
        <dt className="text-slate-400">Depot</dt>
        <dd className="tabular-nums">{info.depot_id}</dd>
        <dt className="text-slate-400">Created</dt>
        <dd className="tabular-nums">{formatTime(info.creation_time)}</dd>
        <dt className="text-slate-400">Files</dt>
        <dd className="tabular-nums">{info.file_count.toLocaleString()}</dd>
        <dt className="text-slate-400">Size (uncompressed)</dt>
        <dd className="tabular-nums">
          <Bytes value={info.size_uncompressed} />
        </dd>
        <dt className="text-slate-400">Size (compressed)</dt>
        <dd className="tabular-nums">
          <Bytes value={info.size_compressed} />
        </dd>
      </dl>
    </div>
  );
}

/// One node in the tree built from the flat manifest file list. `file` is
/// set on leaves (files); for directories it stays null. `children` is
/// keyed by name segment (sorted on flatten, not here).
type TreeNode = {
  name: string;
  path: string;
  children: Map<string, TreeNode>;
  file: ManifestFile | null;
  // Aggregate size of all files under this node — including the file
  // itself for leaves. Computed once during build.
  size: number;
  // Number of file leaves under this node (1 if this node is itself a file).
  fileCount: number;
};

const NO_EXT = "";

function fileExtension(path: string): string {
  // Only look at the last segment so a "." in a dir name doesn't count.
  const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  const name = slash >= 0 ? path.slice(slash + 1) : path;
  // Strip trailing version suffixes (libfoo.so.1, libfoo.so.1.2) before
  // picking the extension so versioned shared libs bucket as "so".
  const stripped = name.replace(/(?:\.\d+)+$/, "");
  const dot = stripped.lastIndexOf(".");
  if (dot <= 0) return NO_EXT;
  return stripped.slice(dot + 1);
}

function buildTree(files: ManifestFile[]): TreeNode {
  const root: TreeNode = {
    name: "",
    path: "",
    children: new Map(),
    file: null,
    size: 0,
    fileCount: 0,
  };
  for (const f of files) {
    const parts = f.path.split("/");
    let node = root;
    for (let i = 0; i < parts.length; i++) {
      const name = parts[i];
      const isLeaf = i === parts.length - 1;
      let child = node.children.get(name);
      if (!child) {
        child = {
          name,
          path: parts.slice(0, i + 1).join("/"),
          children: new Map(),
          file: null,
          size: 0,
          fileCount: 0,
        };
        node.children.set(name, child);
      }
      if (isLeaf) child.file = f;
      node = child;
    }
  }
  // Aggregate size + fileCount bottom-up.
  const visit = (n: TreeNode) => {
    if (n.file) {
      n.size = n.file.size;
      n.fileCount = 1;
      return;
    }
    for (const c of n.children.values()) {
      visit(c);
      n.size += c.size;
      n.fileCount += c.fileCount;
    }
  };
  visit(root);
  return root;
}

type FlatRow = {
  node: TreeNode;
  depth: number;
  expanded: boolean;
  hasChildren: boolean;
};

/// Walks the tree producing visible rows. Two modes:
/// - No search (`matches == null`): dirs are expanded iff they're in
///   `expanded`. Default collapsed.
/// - Search (`matches != null`): only nodes leading to a match are returned,
///   dirs along match paths are expanded by default, but `collapsed` (an
///   explicit user collapse during search) opts out per-path.
function flattenTree(
  root: TreeNode,
  expanded: Set<string>,
  collapsed: Set<string>,
  matches: Set<string> | null,
): FlatRow[] {
  const out: FlatRow[] = [];
  // Auto-expand chains: a dir with exactly one child-dir as its only
  // entry opens that child along with itself. The `collapsed` set lets
  // the user opt out of auto-expansion for a specific path.
  const walk = (node: TreeNode, depth: number, autoExpand: boolean) => {
    const isRoot = depth < 0;
    const hasChildren = node.children.size > 0;
    const sorted = [...node.children.values()].sort((a, b) => {
      const ad = a.file == null;
      const bd = b.file == null;
      if (ad !== bd) return ad ? -1 : 1;
      return a.name.localeCompare(b.name);
    });

    if (!isRoot) {
      if (matches && !nodeContainsMatch(node, matches)) return;
      const userCollapsed = collapsed.has(node.path);
      const isExpanded = matches
        ? !userCollapsed
        : userCollapsed
          ? false
          : autoExpand || expanded.has(node.path);
      out.push({ node, depth, expanded: isExpanded, hasChildren });
      if (!hasChildren) return;
      if (!isExpanded) return;
    }

    const onlyChildAutoOpens = sorted.length === 1 && sorted[0].file == null;
    for (const c of sorted) walk(c, depth + 1, onlyChildAutoOpens);
  };
  walk(root, -1, false);
  return out;
}

/// True iff `node` is a match itself or has any descendant in `matches`.
/// Walking each subtree is cheap because `matches` is the filtered file
/// set; we walk paths that are already known to land somewhere useful.
function nodeContainsMatch(node: TreeNode, matches: Set<string>): boolean {
  if (node.file && matches.has(node.path)) return true;
  for (const c of node.children.values()) {
    if (nodeContainsMatch(c, matches)) return true;
  }
  return false;
}

/// Pin scroll across a tree mutation that may shrink the page.
///
/// Browsers clamp scrollY to (scrollHeight - innerHeight) the moment a
/// layout shrinks, so collapsing a deep subtree near the bottom jumps
/// the page to the top before any post-commit code can compensate.
/// We pad the body to the pre-commit height, keep scrollY where it
/// was, and (optionally, when an anchor element is provided) restore
/// the anchor's viewport position after the new layout settles. The
/// pad shrinks back to nothing as the user scrolls up; once scrollY +
/// viewport fits within the natural height we drop it entirely.
function FilesPanel({
  allFiles,
  appid,
  depotId,
  manifestId,
  branch,
  appInfo,
  extras,
  statuses,
}: {
  allFiles: ManifestFile[];
  appid: string;
  depotId: string;
  manifestId: string;
  branch: string;
  appInfo: AppInfo | undefined;
  extras: ExtraManifestEntry[];
  statuses: ManifestStatusEntry[] | undefined;
}) {
  // Filter and expanded-dirs are per-manifest UI state we want to
  // survive the round-trip to file detail and back. Park both in the
  // query client so they persist across remounts but stay in memory
  // (no URL pollution).
  const queryClient = useQueryClient();
  const queryKey = useMemo(() => ["tree-query", depotId, manifestId], [depotId, manifestId]);
  const extFilterKey = useMemo(
    () => ["tree-ext-filter", depotId, manifestId],
    [depotId, manifestId],
  );
  const diffTargetsKey = useMemo(
    () => ["tree-diff-targets", depotId, manifestId],
    [depotId, manifestId],
  );
  const expandedKey = useMemo(() => ["tree-expanded", depotId, manifestId], [depotId, manifestId]);
  // Search-mode default-expanded; this set tracks paths the user explicitly
  // collapsed during search so toggle still has an effect there.
  const collapsedKey = useMemo(
    () => ["tree-collapsed", depotId, manifestId],
    [depotId, manifestId],
  );
  // Filter input is plain useState so native undo (ctrl+z) works as
  // expected — query-client roundtrips re-render the controlled value
  // mid-typing and break the input's internal undo stack. Initial value
  // is hydrated from the cache, and every change writes back so the
  // filter survives the round-trip to file detail and back.
  const [query, setQueryLocal] = useState<string>(
    () => queryClient.getQueryData<string>(queryKey) ?? "",
  );
  const setQuery = (next: string) => {
    setQueryLocal(next);
    queryClient.setQueryData(queryKey, next);
    // Search-only "collapsed" set has no meaning once there's no search.
    if (!next.trim()) {
      queryClient.setQueryData(collapsedKey, new Set<string>());
    }
  };
  // Active extension filter (whitelist). Empty = no filter (show all).
  // Hydrated from + synced to the query client so it survives the
  // round-trip to file detail and back, like the path filter.
  const [extFilter, setExtFilterLocal] = useState<Set<string>>(
    () => queryClient.getQueryData<Set<string>>(extFilterKey) ?? new Set(),
  );
  const setExtFilter = (next: Set<string>) => {
    setExtFilterLocal(next);
    queryClient.setQueryData(extFilterKey, next);
  };
  // Active diff targets. Each entry is "depot_id/manifest_id". Empty =
  // no diff filter active. Same cache-hydration pattern as extFilter.
  const [diffTargets, setDiffTargetsLocal] = useState<Set<string>>(
    () => queryClient.getQueryData<Set<string>>(diffTargetsKey) ?? new Set(),
  );
  const setDiffTargets = (next: Set<string>) => {
    setDiffTargetsLocal(next);
    queryClient.setQueryData(diffTargetsKey, next);
  };
  const diffRefs = useMemo<ManifestRef[]>(() => {
    if (!appInfo || diffTargets.size === 0) return [];
    const out: ManifestRef[] = [];
    const seen = new Set<string>();
    for (const d of appInfo.depots) {
      for (const m of d.manifests) {
        const key = `${d.depot_id}/${m.manifest_id}`;
        if (diffTargets.has(key) && !seen.has(key)) {
          seen.add(key);
          out.push({ depot_id: d.depot_id, manifest_id: m.manifest_id, branch: m.branch });
        }
      }
    }
    for (const e of extras) {
      const key = `${e.depot_id}/${e.manifest_id}`;
      if (diffTargets.has(key) && !seen.has(key)) {
        seen.add(key);
        out.push({
          depot_id: e.depot_id,
          manifest_id: e.manifest_id,
          branch: e.branch ?? "public",
        });
      }
    }
    return out;
  }, [appInfo, extras, diffTargets]);
  const diffQuery = useQuery({
    queryKey: [
      "manifest-diff",
      appid,
      depotId,
      manifestId,
      branch,
      diffRefs.map((r) => `${r.depot_id}/${r.manifest_id}`).join(","),
    ],
    queryFn: () =>
      fetchManifestDiff(
        Number(appid),
        { depot_id: Number(depotId), manifest_id: manifestId, branch },
        diffRefs,
      ),
    enabled: diffRefs.length > 0,
    staleTime: Infinity,
    gcTime: 10 * 60 * 1000,
    // Keep the previous diff visible while a new selection refetches so
    // the tree doesn't flash back to "all files" in between.
    placeholderData: (prev) => prev,
  });
  const diffPaths = useMemo<Set<string> | null>(() => {
    if (diffTargets.size === 0) return null;
    if (!diffQuery.data) return null;
    return new Set(diffQuery.data);
  }, [diffTargets, diffQuery.data]);
  const expanded =
    useQuery({
      queryKey: expandedKey,
      queryFn: () => new Set<string>(),
      staleTime: Infinity,
      gcTime: Infinity,
    }).data ?? new Set<string>();
  const collapsed =
    useQuery({
      queryKey: collapsedKey,
      queryFn: () => new Set<string>(),
      staleTime: Infinity,
      gcTime: Infinity,
    }).data ?? new Set<string>();
  const deferred = useDeferredValue(query);

  const tree = useMemo(() => buildTree(allFiles), [allFiles]);

  // Extension → count over all files, sorted by count desc. Files without
  // a dot in their last segment get bucketed under "(no ext)".
  const extCounts = useMemo(() => {
    const counts = new Map<string, number>();
    for (const f of allFiles) {
      const ext = fileExtension(f.path);
      counts.set(ext, (counts.get(ext) ?? 0) + 1);
    }
    return [...counts.entries()].sort((a, b) => b[1] - a[1]);
  }, [allFiles]);

  const matches = useMemo<Set<string> | null>(() => {
    const tokens = deferred.toLowerCase().split(/\s+/).filter(Boolean);
    const hasExtFilter = extFilter.size > 0;
    const hasDiffFilter = diffPaths != null;
    if (tokens.length === 0 && !hasExtFilter && !hasDiffFilter) return null;
    const m = new Set<string>();
    for (const f of allFiles) {
      const path = f.path.toLowerCase();
      if (tokens.length > 0 && !tokens.every((t) => path.includes(t))) continue;
      if (hasExtFilter && !extFilter.has(fileExtension(f.path))) continue;
      if (hasDiffFilter && !diffPaths!.has(f.path)) continue;
      m.add(f.path);
    }
    return m;
  }, [allFiles, deferred, extFilter, diffPaths]);

  const rows = useMemo(
    () => flattenTree(tree, expanded, collapsed, matches),
    [tree, expanded, collapsed, matches],
  );

  const toggleDir = useCallback(
    (path: string, currentlyExpanded: boolean, anchor: HTMLElement | null) => {
      const restore = pinScroll(anchor);
      // What we update depends on what the user sees (currentlyExpanded)
      // rather than what's in either set — auto-expanded chain rows and
      // search-default-open rows aren't in `expanded` but are visibly open.
      if (currentlyExpanded) {
        // User wants to close. Drop from expanded; record in collapsed
        // so the override beats both search-default and auto-chain.
        queryClient.setQueryData<Set<string>>(expandedKey, (prev) => {
          if (!prev?.has(path)) return prev ?? new Set<string>();
          const next = new Set(prev);
          next.delete(path);
          return next;
        });
        queryClient.setQueryData<Set<string>>(collapsedKey, (prev) => {
          const next = new Set(prev ?? []);
          next.add(path);
          return next;
        });
      } else {
        // User wants to open. Add to expanded; drop from collapsed.
        queryClient.setQueryData<Set<string>>(expandedKey, (prev) => {
          const next = new Set(prev ?? []);
          next.add(path);
          return next;
        });
        queryClient.setQueryData<Set<string>>(collapsedKey, (prev) => {
          if (!prev?.has(path)) return prev ?? new Set<string>();
          const next = new Set(prev);
          next.delete(path);
          return next;
        });
      }
      restore();
    },
    [queryClient, expandedKey, collapsedKey],
  );

  const matchedCount = matches?.size ?? null;

  // Dir paths that are currently visible as open rows in the tree —
  // distinct from `expanded` because search-mode default-opens match
  // paths and the chain auto-expand opens single-child dirs.
  const openDirPaths = useMemo(() => {
    const set = new Set<string>();
    for (const r of rows) {
      if (r.node.file == null && r.expanded) set.add(r.node.path);
    }
    return set;
  }, [rows]);
  const hasOpenDirs = openDirPaths.size > 0;

  const collapseAll = () => {
    const restore = pinScroll(null);
    if (matches) {
      // Under search, override every visibly-open match path.
      queryClient.setQueryData(collapsedKey, new Set(openDirPaths));
    } else {
      queryClient.setQueryData(expandedKey, new Set<string>());
      queryClient.setQueryData(collapsedKey, new Set<string>());
    }
    restore();
  };
  const expandAll = () => {
    const restore = pinScroll(null);
    if (matches) {
      // Clearing the override is enough — search default-opens everything.
      queryClient.setQueryData(collapsedKey, new Set<string>());
    } else {
      // Walk the tree once to collect every dir path. With 6k+ files
      // this is fast and only runs on the explicit click.
      const all = new Set<string>();
      const visit = (n: TreeNode) => {
        for (const c of n.children.values()) {
          if (c.file == null) {
            all.add(c.path);
            visit(c);
          }
        }
      };
      visit(tree);
      queryClient.setQueryData(expandedKey, all);
      queryClient.setQueryData(collapsedKey, new Set<string>());
    }
    restore();
  };

  return (
    <>
      <div className="flex items-baseline gap-4 mb-3">
        <h2 className="text-xl font-semibold">Files</h2>
        <span className="text-sm text-slate-400 tabular-nums">
          {matchedCount != null ? (
            <>
              {matchedCount.toLocaleString()}
              <span className="text-slate-600"> / {allFiles.length.toLocaleString()}</span>
            </>
          ) : (
            allFiles.length.toLocaleString()
          )}
        </span>
        <button
          type="button"
          onClick={hasOpenDirs ? collapseAll : expandAll}
          className="text-xs text-slate-500 hover:text-sky-400 hover:underline"
        >
          {hasOpenDirs ? "collapse all" : "expand all"}
        </button>
      </div>
      <div className="flex gap-2 mb-3">
        <div className="relative flex-1">
          <input
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Filter by path…"
            spellCheck={false}
            autoCorrect="off"
            autoCapitalize="off"
            className="w-full px-3 py-1.5 pr-8 text-sm bg-slate-900 border border-slate-700 rounded focus:outline-none focus:border-sky-700 [&::-webkit-search-cancel-button]:hidden"
          />
          {query && (
            <button
              type="button"
              onClick={() => setQuery("")}
              aria-label="Clear filter"
              className="absolute right-1.5 top-1/2 -translate-y-1/2 px-1.5 text-slate-500 hover:text-slate-200 text-lg leading-none"
            >
              ×
            </button>
          )}
        </div>
        {appInfo && (
          <CompareMenu
            appInfo={appInfo}
            extras={extras}
            statuses={statuses}
            currentDepotId={Number(depotId)}
            currentManifestId={manifestId}
            selected={diffTargets}
            onChange={setDiffTargets}
            loading={diffQuery.isFetching}
            error={diffQuery.error as Error | null}
          />
        )}
        <ExtensionFilter extCounts={extCounts} selected={extFilter} onChange={setExtFilter} />
      </div>
      <TreeList
        rows={rows}
        appid={appid}
        depotId={depotId}
        manifestId={manifestId}
        branch={branch}
        onToggle={toggleDir}
      />
    </>
  );
}

function TreeList({
  rows,
  appid,
  depotId,
  manifestId,
  branch,
  onToggle,
}: {
  rows: FlatRow[];
  appid: string;
  depotId: string;
  manifestId: string;
  branch: string;
  onToggle: (path: string, currentlyExpanded: boolean, anchor: HTMLElement | null) => void;
}) {
  if (rows.length === 0) {
    return <p className="text-slate-500 text-sm">No matches.</p>;
  }
  return (
    <div className="text-sm border border-slate-800 rounded overflow-hidden">
      <div className="flex items-baseline gap-1 py-1.5 px-3 border-b border-slate-800 font-semibold text-slate-400">
        <span>Name</span>
        <span className="ml-auto w-14 text-right">Files</span>
        <span className="w-20 text-right">Size</span>
      </div>
      <ul>
        {rows.map((row) => (
          <TreeRow
            key={row.node.path}
            node={row.node}
            depth={row.depth}
            expanded={row.expanded}
            appid={appid}
            depotId={depotId}
            manifestId={manifestId}
            branch={branch}
            onToggle={onToggle}
          />
        ))}
      </ul>
    </div>
  );
}

// React.memo on TreeRow lets unchanged rows skip rerenders when other
// dirs expand/collapse. The props are deliberately flat primitives or
// stable references (node identity comes from the once-built tree) so
// the default shallow comparison is a hit on the rows that didn't change.
const TreeRow = memo(function TreeRow({
  node,
  depth,
  expanded,
  appid,
  depotId,
  manifestId,
  branch,
  onToggle,
}: {
  node: TreeNode;
  depth: number;
  expanded: boolean;
  appid: string;
  depotId: string;
  manifestId: string;
  branch: string;
  onToggle: (path: string, currentlyExpanded: boolean, anchor: HTMLElement | null) => void;
}) {
  const isDir = node.file == null;
  // 16px per nesting level for the row, plus 16px for the chevron column
  // (files have no chevron; we reserve the same slot so names align).
  const indentPx = 12 + depth * 16;

  if (isDir) {
    return (
      <li className="border-b border-slate-800 last:border-b-0">
        <button
          type="button"
          onClick={(e) => onToggle(node.path, expanded, e.currentTarget)}
          className="w-full flex items-baseline gap-1 py-1.5 pr-3 text-left hover:bg-slate-800/40"
          style={{ paddingLeft: indentPx }}
        >
          <span className="inline-block w-3 text-slate-500 text-xs">{expanded ? "▾" : "▸"}</span>
          <span className="text-sm text-slate-200">{node.name}</span>
          <span className="ml-auto w-14 text-right text-xs text-slate-500 tabular-nums whitespace-nowrap">
            {node.fileCount.toLocaleString()}
          </span>
          <span className="w-20 text-right text-xs text-slate-500 tabular-nums whitespace-nowrap">
            <Bytes value={node.size} />
          </span>
        </button>
      </li>
    );
  }

  // File row — clickable link. Add the chevron-slot width to indent so
  // file names line up with sibling dir names (which have a chevron).
  const file = node.file!;
  return (
    <li className="border-b border-slate-800 last:border-b-0 hover:bg-slate-800/40">
      <Link
        to="/apps/$appid/depots/$depotId/manifests/$manifestId/file"
        params={{ appid, depotId, manifestId }}
        search={{ branch, path: file.path }}
        className="flex items-baseline gap-1 py-1.5 pr-3 text-sky-400"
        style={{ paddingLeft: indentPx + 16 }}
      >
        <span className="text-sm break-all">{node.name}</span>
        {file.linktarget && (
          <span className="font-mono text-xs text-slate-500"> → {file.linktarget}</span>
        )}
        <span className="ml-auto w-14" aria-hidden="true" />
        <span className="w-20 text-right text-xs text-slate-400 tabular-nums whitespace-nowrap">
          <Bytes value={file.size} />
        </span>
      </Link>
    </li>
  );
});

function ExtensionFilter({
  extCounts,
  selected,
  onChange,
}: {
  extCounts: [string, number][];
  selected: Set<string>;
  onChange: (next: Set<string>) => void;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);
  const toggle = (ext: string) => {
    const next = new Set(selected);
    if (next.has(ext)) next.delete(ext);
    else next.add(ext);
    onChange(next);
  };
  const count = selected.size;
  return (
    <div ref={rootRef} className="relative">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className={`px-3 py-1.5 text-sm border rounded whitespace-nowrap ${
          count > 0
            ? "border-sky-700 bg-sky-950/40 text-sky-200 hover:bg-sky-900/40"
            : "border-slate-700 bg-slate-900 text-slate-300 hover:border-slate-600"
        }`}
        aria-haspopup="dialog"
        aria-expanded={open}
      >
        Extensions{count > 0 && <span className="ml-1.5 tabular-nums">({count})</span>}
      </button>
      {open && (
        <div className="absolute right-0 top-full mt-1 z-10 w-48 max-h-80 overflow-auto bg-slate-900 border border-slate-700 rounded shadow-lg">
          <div className="flex items-center justify-between px-3 py-1.5 text-xs text-slate-400 border-b border-slate-800 sticky top-0 bg-slate-900">
            <span className="tabular-nums">{extCounts.length} types</span>
            {count > 0 && (
              <button
                type="button"
                onClick={() => onChange(new Set())}
                className="text-slate-500 hover:text-slate-200"
              >
                clear
              </button>
            )}
          </div>
          <ul>
            {extCounts.map(([ext, n]) => {
              const checked = selected.has(ext);
              return (
                <li key={ext}>
                  <label
                    onMouseDown={(e) => e.preventDefault()}
                    className={`flex items-center gap-2 px-3 py-1 text-sm cursor-pointer select-none hover:bg-slate-800/60 ${
                      checked ? "text-sky-200" : "text-slate-300"
                    }`}
                  >
                    <input
                      type="checkbox"
                      checked={checked}
                      onChange={() => toggle(ext)}
                      className="accent-sky-500"
                    />
                    <span className="flex-1 truncate">
                      {ext === NO_EXT ? <span className="italic text-slate-500">none</span> : ext}
                    </span>
                    <span className="text-xs text-slate-500 tabular-nums">
                      {n.toLocaleString()}
                    </span>
                  </label>
                </li>
              );
            })}
          </ul>
        </div>
      )}
    </div>
  );
}

type CompareCandidate = {
  key: string;
  depotId: number;
  manifestId: string;
  branch: string;
  creationTime: number;
};

type DepotGroup = {
  depotId: number;
  label: string;
  candidates: CompareCandidate[];
};

function CompareMenu({
  appInfo,
  extras,
  statuses,
  currentDepotId,
  currentManifestId,
  selected,
  onChange,
  loading,
  error,
}: {
  appInfo: AppInfo;
  extras: ExtraManifestEntry[];
  statuses: ManifestStatusEntry[] | undefined;
  currentDepotId: number;
  currentManifestId: string;
  selected: Set<string>;
  onChange: (next: Set<string>) => void;
  loading: boolean;
  error: Error | null;
}) {
  const [open, setOpen] = useState(false);
  const [activeDepotId, setActiveDepotId] = useState<number | null>(currentDepotId);
  const rootRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);
  // Group every known manifest (official + tracked) under its depot. Per
  // entry we keep creation_time from manifest_statuses (0 if unknown)
  // so each depot's submenu can sort chronologically.
  const groups = useMemo<DepotGroup[]>(() => {
    const creationByKey = new Map<string, number>();
    for (const s of statuses ?? []) {
      creationByKey.set(`${s.depot_id}/${s.manifest_id}`, s.creation_time);
    }
    const groupMap = new Map<number, DepotGroup>();
    for (const d of appInfo.depots) {
      const tag = [d.oslist, d.osarch, d.language].filter(Boolean).join(" · ");
      const label = tag ? `depot ${d.depot_id} · ${tag}` : `depot ${d.depot_id}`;
      groupMap.set(d.depot_id, { depotId: d.depot_id, label, candidates: [] });
      const seenManifest = new Set<string>();
      for (const m of d.manifests) {
        if (seenManifest.has(m.manifest_id)) continue;
        seenManifest.add(m.manifest_id);
        if (d.depot_id === currentDepotId && m.manifest_id === currentManifestId) continue;
        const key = `${d.depot_id}/${m.manifest_id}`;
        groupMap.get(d.depot_id)!.candidates.push({
          key,
          depotId: d.depot_id,
          manifestId: m.manifest_id,
          branch: m.branch,
          creationTime: creationByKey.get(key) ?? 0,
        });
      }
    }
    for (const e of extras) {
      const key = `${e.depot_id}/${e.manifest_id}`;
      if (e.depot_id === currentDepotId && e.manifest_id === currentManifestId) continue;
      const group = groupMap.get(e.depot_id);
      if (!group) continue;
      if (group.candidates.some((c) => c.manifestId === e.manifest_id)) continue;
      group.candidates.push({
        key,
        depotId: e.depot_id,
        manifestId: e.manifest_id,
        branch: e.branch ?? "public",
        creationTime: creationByKey.get(key) ?? 0,
      });
    }
    // Sort each depot's candidates by creation_time desc; manifests we
    // haven't fetched yet (creation_time 0) bubble to the bottom.
    for (const g of groupMap.values()) {
      g.candidates.sort((a, b) => {
        if (b.creationTime !== a.creationTime) return b.creationTime - a.creationTime;
        return a.manifestId.localeCompare(b.manifestId);
      });
    }
    // Sort groups: current depot first, then by depot id.
    return [...groupMap.values()]
      .filter((g) => g.candidates.length > 0)
      .sort((a, b) => {
        if (a.depotId === currentDepotId) return -1;
        if (b.depotId === currentDepotId) return 1;
        return a.depotId - b.depotId;
      });
  }, [appInfo, extras, statuses, currentDepotId, currentManifestId]);

  // If our previously-active depot stopped having candidates, fall back
  // to the first available group.
  useEffect(() => {
    if (!open) return;
    if (!groups.find((g) => g.depotId === activeDepotId)) {
      setActiveDepotId(groups[0]?.depotId ?? null);
    }
  }, [open, groups, activeDepotId]);

  const activeGroup = groups.find((g) => g.depotId === activeDepotId) ?? groups[0];
  const toggle = (key: string) => {
    const next = new Set(selected);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    onChange(next);
  };
  const count = selected.size;
  return (
    <div ref={rootRef} className="relative">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className={`px-3 py-1.5 text-sm border rounded whitespace-nowrap ${
          count > 0
            ? "border-sky-700 bg-sky-950/40 text-sky-200 hover:bg-sky-900/40"
            : "border-slate-700 bg-slate-900 text-slate-300 hover:border-slate-600"
        }`}
        aria-haspopup="dialog"
        aria-expanded={open}
        title="Show only files that differ from selected manifests"
      >
        Compare to{count > 0 && <span className="ml-1.5 tabular-nums">({count})</span>}
        {loading && <span className="ml-2 text-xs text-slate-400">…</span>}
      </button>
      {open && (
        <div className="absolute -right-[100px] top-full mt-1 z-10 flex w-[640px] max-h-96 bg-slate-900 border border-slate-700 rounded shadow-lg overflow-hidden">
          <div className="w-56 border-r border-slate-800 overflow-auto">
            <div className="flex items-center justify-between px-3 py-1.5 text-xs text-slate-400 border-b border-slate-800 sticky top-0 bg-slate-900">
              <span>compare to…</span>
              {count > 0 && (
                <button
                  type="button"
                  onClick={() => onChange(new Set())}
                  className="text-slate-500 hover:text-slate-200"
                >
                  clear
                </button>
              )}
            </div>
            {groups.length === 0 ? (
              <p className="px-3 py-2 text-sm text-slate-500">No other manifests.</p>
            ) : (
              <ul>
                {groups.map((g) => {
                  const isActive = g.depotId === activeGroup?.depotId;
                  const selectedHere = g.candidates.reduce(
                    (n, c) => n + (selected.has(c.key) ? 1 : 0),
                    0,
                  );
                  return (
                    <li key={g.depotId}>
                      <button
                        type="button"
                        onClick={() => setActiveDepotId(g.depotId)}
                        className={`w-full px-3 py-1.5 text-left text-sm flex items-center gap-2 ${
                          isActive
                            ? "bg-slate-800 text-slate-100"
                            : "text-slate-300 hover:bg-slate-800/60"
                        }`}
                      >
                        <span className="flex-1 min-w-0">
                          <span className="block truncate">
                            {g.depotId === currentDepotId ? (
                              <span>This depot</span>
                            ) : (
                              <span>depot {g.depotId}</span>
                            )}
                          </span>
                          <span className="block text-xs text-slate-500 truncate">{g.label}</span>
                        </span>
                        <span className="text-xs text-slate-500 tabular-nums">
                          {selectedHere > 0 ? `${selectedHere}/` : ""}
                          {g.candidates.length}
                        </span>
                      </button>
                    </li>
                  );
                })}
              </ul>
            )}
          </div>
          <div className="flex-1 overflow-auto">
            {error && (
              <p className="px-3 py-2 text-xs text-red-300 border-b border-slate-800">
                Diff failed: {error.message}
              </p>
            )}
            {!activeGroup ? (
              <p className="px-3 py-2 text-sm text-slate-500">Pick a depot on the left.</p>
            ) : (
              <ul>
                {activeGroup.candidates.map((c) => {
                  const checked = selected.has(c.key);
                  return (
                    <li key={c.key}>
                      <button
                        type="button"
                        onMouseDown={(e) => e.preventDefault()}
                        onClick={() => toggle(c.key)}
                        aria-pressed={checked}
                        className={`w-full flex items-center gap-2 px-3 py-1 text-sm cursor-pointer select-none text-left ${
                          checked
                            ? "bg-sky-950/40 text-sky-200 hover:bg-sky-900/40"
                            : "text-slate-300 hover:bg-slate-800/60"
                        }`}
                      >
                        <span
                          aria-hidden="true"
                          className={`inline-block w-3 text-center ${
                            checked ? "text-sky-400" : "text-transparent"
                          }`}
                        >
                          ✓
                        </span>
                        <span className="flex-1 min-w-0">
                          <span className="block truncate">{c.branch}</span>
                          <span
                            className={`block text-xs truncate ${
                              checked ? "text-sky-400/70" : "text-slate-500"
                            }`}
                          >
                            {c.creationTime > 0 ? formatDate(c.creationTime) : "date unknown"}
                          </span>
                        </span>
                        <span
                          className={`text-xs font-mono tabular-nums ${
                            checked ? "text-sky-400/70" : "text-slate-500"
                          }`}
                        >
                          {c.manifestId.slice(0, 8)}…
                        </span>
                      </button>
                    </li>
                  );
                })}
              </ul>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

function formatTime(unix: number): string {
  if (!unix) return "—";
  return new Date(unix * 1000).toISOString().replace("T", " ").slice(0, 19) + " UTC";
}

const dateFormatter = new Intl.DateTimeFormat(undefined, { dateStyle: "medium" });

function formatDate(unix: number): string {
  if (!unix) return "—";
  return dateFormatter.format(new Date(unix * 1000));
}
