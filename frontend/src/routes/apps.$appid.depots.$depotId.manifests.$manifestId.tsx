// TODO(ai-review): review for style and correctness
import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useWindowVirtualizer } from "@tanstack/react-virtual";
import { memo, useCallback, useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import { Loader2 } from "lucide-react";
import {
  downloadManifest,
  fetchAppInfo,
  fetchExtraManifests,
  fetchGameInfo,
  fetchManifestDiff,
  fetchManifestDiffDeep,
  fetchManifestFiles,
  fetchManifestInfo,
  fetchManifestStatuses,
  type AppInfo,
  type EnqueueSummary,
  type ExtraManifestEntry,
  type GameInfo,
  type ManifestDiffStatus,
  type ManifestFile,
  type ManifestInfo,
  type ManifestRef,
  type ManifestStatusEntry,
} from "../api";
import { Bytes } from "../components/Bytes";
import { CompareMenu, diffTargetKey } from "../components/CompareMenu";
import { ErrorBox } from "../components/ErrorBox";
import { formatBytes, formatDate } from "../lib/format";
import { markShowImmediately } from "../lib/downloadsUiSignal";
import { pinScroll } from "../lib/pinScroll";

type Search = {
  /// `undefined` means "default" (which is `public`). Keeping default
  /// out of the type lets us drop `?branch=public` from URLs.
  branch?: string;
  /// Diff targets, persisted in the URL so the filter survives reloads
  /// and is shareable. Comma-separated string of "depot/manifest" or
  /// bare "manifest" entries (depot resolved from app_info + extras).
  /// Stored as a string instead of an array so tanstack-router doesn't
  /// JSON-encode it to `?compare_to=["..."]`.
  compare_to?: string;
  /// Extension whitelist for the file tree. Comma-separated, lowercase,
  /// without leading dots. Empty = no filter. URL form for the same
  /// reason as `compare_to`.
  ext?: string;
  /// Deep-compare toggle. In the URL so it survives the round-trip to a
  /// file/diff page and back (local state would reset on remount).
  /// Omitted when off.
  deep?: boolean;
};

export const Route = createFileRoute("/apps/$appid/depots/$depotId/manifests/$manifestId")({
  validateSearch: (search: Record<string, unknown>): Search => ({
    branch:
      typeof search.branch === "string" && search.branch !== "public" ? search.branch : undefined,
    compare_to:
      typeof search.compare_to === "string" && search.compare_to ? search.compare_to : undefined,
    ext: typeof search.ext === "string" && search.ext ? search.ext : undefined,
    deep: search.deep === true || search.deep === "true" ? true : undefined,
  }),
  component: ManifestDetail,
});

function ManifestDetail() {
  const { appid: appidParam, depotId: depotIdParam, manifestId } = Route.useParams();
  const search = Route.useSearch();
  const branch = search.branch ?? "public";
  const compare_to = search.compare_to;
  const navigate = useNavigate({ from: Route.fullPath });
  const appid = Number(appidParam);
  const depotId = Number(depotIdParam);
  // Diff targets live in the URL so they're shareable + survive reloads.
  // The CompareMenu uses the local Set form for fast O(1) lookup; we
  // re-derive it on every render from the comma-separated URL value.
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

  const info = useQuery({
    queryKey: ["manifest-info", appid, depotId, manifestId, branch],
    queryFn: () => fetchManifestInfo(appid, depotId, manifestId, branch),
  });
  const gameInfo = useQuery({
    queryKey: ["game-info", appid, depotId, manifestId, branch],
    queryFn: () => fetchGameInfo(appid, depotId, manifestId, branch),
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
    // Wait until both inputs to `compareRefs` have settled — see the
    // file-page route for the same rationale.
    enabled: compareRefs.length > 0 && extraQuery.isSuccess,
  });

  const downloadAll = useMutation({
    mutationFn: () => downloadManifest(appid, depotId, manifestId, { branch }),
  });
  // This manifest's own cache status, pulled out of the batch we already
  // fetch for the compare menu. Drives the download button so a manifest
  // that's fully on disk doesn't offer a pointless "Download all".
  const selfStatus = useMemo(
    () => statusQuery.data?.find((s) => s.depot_id === depotId && s.manifest_id === manifestId),
    [statusQuery.data, depotId, manifestId],
  );

  return (
    <div className="mx-auto max-w-6xl p-8">
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
        <span className="font-medium text-slate-200">
          {info.data ? formatDate(info.data.creation_time) : "manifest"}
        </span>
      </nav>

      {info.isPending && <p className="text-slate-400">Loading manifest…</p>}
      {info.error && <ErrorBox title="Failed to load manifest" error={info.error as Error} />}
      {info.data && (
        <ManifestHeader
          info={info.data}
          gameInfo={gameInfo.data}
          gameInfoPending={gameInfo.isPending}
          branch={branch}
          onDownload={() => {
            markShowImmediately();
            downloadAll.mutate();
          }}
          downloadPending={downloadAll.isPending}
          downloadResult={downloadAll.data}
          downloadError={downloadAll.error as Error | null}
          cachedStatus={selfStatus}
        />
      )}

      <section className="mt-8">
        {files.error && <ErrorBox title="Failed to load files" error={files.error as Error} />}
        {files.isPending && <p className="text-sm text-slate-400">Loading files…</p>}
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
            diffTargets={diffTargets}
            setDiffTargets={setDiffTargets}
          />
        )}
      </section>
    </div>
  );
}

function ManifestHeader({
  info,
  gameInfo,
  gameInfoPending,
  onDownload,
  downloadPending,
  downloadResult,
  downloadError,
  cachedStatus,
}: {
  info: ManifestInfo;
  gameInfo: GameInfo | undefined;
  gameInfoPending: boolean;
  branch: string;
  onDownload: () => void;
  downloadPending: boolean;
  downloadResult: EnqueueSummary | undefined;
  downloadError: Error | null;
  cachedStatus: ManifestStatusEntry | undefined;
}) {
  // Every chunk already on disk → nothing to download. `chunks_total > 0`
  // guards against the not-yet-loaded / errored status (both report 0).
  const fullyCached =
    cachedStatus != null &&
    cachedStatus.error == null &&
    cachedStatus.chunks_total > 0 &&
    cachedStatus.chunks_missing === 0;
  return (
    <div>
      <div className="flex items-start gap-4">
        <h1 className="text-2xl font-bold">Manifest</h1>
        {!fullyCached && (
          <button
            type="button"
            onClick={onDownload}
            disabled={downloadPending}
            className="ml-auto rounded border border-sky-700 bg-sky-950/40 px-3 py-1.5 text-sm hover:bg-sky-900/40 disabled:opacity-50"
          >
            {downloadPending ? "Enqueuing…" : "Download all"}
          </button>
        )}
      </div>
      {!downloadResult && fullyCached && cachedStatus && (
        <p className="mt-2 text-xs text-slate-400">
          Everything is already cached. {cachedStatus.chunks_total.toLocaleString()} chunks on disk.
        </p>
      )}
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
        <dd className="font-mono break-all tabular-nums">{info.manifest_id}</dd>
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
        {gameInfoPending ? (
          <>
            <dt className="text-slate-400">Engine</dt>
            <dd className="flex items-center gap-2 text-slate-500">
              <Loader2 size={14} className="animate-spin" />
            </dd>
          </>
        ) : (
          gameInfo?.engine?.engine === "unity" && (
            <>
              <dt className="text-slate-400">Engine</dt>
              <dd>Unity {gameInfo.engine.data.version}</dd>
              {gameInfo.engine.data.bundle_version && (
                <>
                  <dt className="text-slate-400">Game version</dt>
                  <dd>{gameInfo.engine.data.bundle_version}</dd>
                </>
              )}
            </>
          )
        )}
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

// Sentinel for "file has no extension". Has to be non-empty (and not a
// plausible real extension) so URL serialization round-trips it — the
// previous "" got dropped by `split(",").filter(Boolean)` and the UI
// chip became unselectable as a result.
const NO_EXT = "__none__";

function fileExtension(path: string): string {
  // Only look at the last segment so a "." in a dir name doesn't count.
  const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  const name = slash >= 0 ? path.slice(slash + 1) : path;
  // Unity serialized scenes (`level0`, `level42`, …) have no extension
  // by convention but conceptually share a type. Bucket them as `level`
  // so the extension filter treats them as a group.
  if (/^level\d+$/.test(name)) return "level";
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
      // Numeric-aware so `level24` sorts before `level231` instead of
      // lexicographically after it.
      return a.name.localeCompare(b.name, undefined, { numeric: true });
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
  diffTargets,
  setDiffTargets,
}: {
  allFiles: ManifestFile[];
  appid: string;
  depotId: string;
  manifestId: string;
  branch: string;
  appInfo: AppInfo | undefined;
  extras: ExtraManifestEntry[];
  statuses: ManifestStatusEntry[] | undefined;
  diffTargets: Set<string>;
  setDiffTargets: (next: Set<string>) => void;
}) {
  // Filter and expanded-dirs are per-manifest UI state we want to
  // survive the round-trip to file detail and back. Park both in the
  // query client so they persist across remounts but stay in memory
  // (no URL pollution).
  const queryClient = useQueryClient();
  const search = Route.useSearch();
  const navigate = useNavigate({ from: Route.fullPath });
  const queryKey = useMemo(() => ["tree-query", depotId, manifestId], [depotId, manifestId]);
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
  // Persisted in the URL so the filter survives reloads and is
  // shareable; we derive a Set on every render for O(1) lookup.
  const extFilter = useMemo(
    () => new Set(search.ext ? search.ext.split(",").filter(Boolean) : []),
    [search.ext],
  );
  const setExtFilter = useCallback(
    (next: Set<string>) => {
      navigate({
        search: (prev) => ({
          ...prev,
          ext: next.size === 0 ? undefined : [...next].sort().join(","),
        }),
        replace: true,
      });
    },
    [navigate],
  );
  // Active diff targets. Each entry is "depot_id/manifest_id". Empty =
  // no diff filter active. Same cache-hydration pattern as extFilter.
  const diffRefs = useMemo<ManifestRef[]>(() => {
    if (!appInfo || diffTargets.size === 0) return [];
    // Index every (depot, manifest) we know about so URL entries can be
    // matched either by their canonical key (bare manifest in the
    // current depot, "depot-manifest" otherwise) or as a bare
    // manifest_id (deep-link convenience across depots too).
    const all: { depot_id: number; manifest_id: string; branch: string }[] = [];
    const seenAll = new Set<string>();
    for (const d of appInfo.depots) {
      for (const m of d.manifests) {
        const k = `${d.depot_id}-${m.manifest_id}`;
        if (seenAll.has(k)) continue;
        seenAll.add(k);
        all.push({ depot_id: d.depot_id, manifest_id: m.manifest_id, branch: m.branch });
      }
    }
    for (const e of extras) {
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
      const canonical = diffTargetKey(entry.depot_id, entry.manifest_id, Number(depotId));
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
  }, [appInfo, extras, diffTargets, depotId]);
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
  // Deep compare: a 1:1 diff that structural-diffs each changed file and
  // drops the ones with no structured difference. Gated on a single
  // compare target. Slow (downloads both sides of every changed file)
  // and recomputed on each load for now — the cheap fingerprint list
  // above stays visible until this lands, then the tree narrows.
  const deepCompare = search.deep ?? false;
  const setDeepCompare = useCallback(
    (next: boolean) => {
      navigate({
        search: (prev) => ({ ...prev, deep: next ? true : undefined }),
        replace: true,
      });
    },
    [navigate],
  );
  const deepTarget = diffRefs.length === 1 ? diffRefs[0] : undefined;
  const deepQuery = useQuery({
    queryKey: [
      "manifest-diff-deep",
      appid,
      depotId,
      manifestId,
      branch,
      deepTarget ? `${deepTarget.depot_id}/${deepTarget.manifest_id}` : "",
    ],
    queryFn: () =>
      fetchManifestDiffDeep(
        Number(appid),
        { depot_id: Number(depotId), manifest_id: manifestId, branch },
        deepTarget!,
      ),
    enabled: deepCompare && deepTarget != null,
    staleTime: Infinity,
    gcTime: 10 * 60 * 1000,
  });
  // Per-path diff status from the backend. `null` when no compare
  // target is active (or the query hasn't landed yet) — callers treat
  // that as "no diff filter". `has(path)` mirrors the old Set API,
  // and `get(path)` gives the colour (added/changed) for the row. Deep
  // results replace the fingerprint ones once available; until then the
  // fingerprint list shows so there's an immediate result.
  const diffStatus = useMemo<Map<string, ManifestDiffStatus> | null>(() => {
    if (diffTargets.size === 0) return null;
    const source = deepCompare && deepQuery.data ? deepQuery.data : diffQuery.data;
    if (!source) return null;
    const m = new Map<string, ManifestDiffStatus>();
    for (const e of source) m.set(e.path, e.status);
    return m;
  }, [diffTargets, diffQuery.data, deepQuery.data, deepCompare]);
  // `initialData` (not `?? new Set()`) so the cache entry's reference is
  // stable across renders — otherwise the `flattenTree` useMemo below
  // would re-run on every render because its `expanded`/`collapsed`
  // deps would be a fresh Set each time.
  const expanded = useQuery({
    queryKey: expandedKey,
    queryFn: () => new Set<string>(),
    initialData: () => new Set<string>(),
    staleTime: Infinity,
    gcTime: Infinity,
  }).data;
  const collapsed = useQuery({
    queryKey: collapsedKey,
    queryFn: () => new Set<string>(),
    initialData: () => new Set<string>(),
    staleTime: Infinity,
    gcTime: Infinity,
  }).data;
  const deferred = useDeferredValue(query);

  const tree = useMemo(() => buildTree(allFiles), [allFiles]);

  // Extensions are counted over the files that survive the *other*
  // filters (path search + diff filter) but *not* the extension filter
  // itself — otherwise unchecking a type would make all others disappear
  // from the menu. So a paste-restricted "compare to" run narrows the
  // extension menu to just the changed types.
  const extCounts = useMemo(() => {
    const counts = new Map<string, number>();
    const tokens = deferred.toLowerCase().split(/\s+/).filter(Boolean);
    for (const f of allFiles) {
      if (diffStatus != null && !diffStatus.has(f.path)) continue;
      if (tokens.length > 0) {
        const path = f.path.toLowerCase();
        if (!tokens.every((t) => path.includes(t))) continue;
      }
      const ext = fileExtension(f.path);
      counts.set(ext, (counts.get(ext) ?? 0) + 1);
    }
    return [...counts.entries()].sort((a, b) => b[1] - a[1]);
  }, [allFiles, diffStatus, deferred]);

  const matches = useMemo<Set<string> | null>(() => {
    const tokens = deferred.toLowerCase().split(/\s+/).filter(Boolean);
    const hasExtFilter = extFilter.size > 0;
    const hasDiffFilter = diffStatus != null;
    if (tokens.length === 0 && !hasExtFilter && !hasDiffFilter) return null;
    const m = new Set<string>();
    for (const f of allFiles) {
      const path = f.path.toLowerCase();
      if (tokens.length > 0 && !tokens.every((t) => path.includes(t))) continue;
      if (hasExtFilter && !extFilter.has(fileExtension(f.path))) continue;
      if (hasDiffFilter && !diffStatus!.has(f.path)) continue;
      m.add(f.path);
    }
    return m;
  }, [allFiles, deferred, extFilter, diffStatus]);

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
      <div className="mb-3 flex items-baseline gap-4">
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
      <div className="mb-3 flex gap-2">
        <div className="relative flex-1">
          <input
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Filter by path…"
            spellCheck={false}
            autoCorrect="off"
            autoCapitalize="off"
            className="w-full rounded border border-slate-700 bg-slate-900 px-3 py-1.5 pr-8 text-sm focus:border-sky-700 focus:outline-none [&::-webkit-search-cancel-button]:hidden"
          />
          {query && (
            <button
              type="button"
              onClick={() => setQuery("")}
              aria-label="Clear filter"
              className="absolute top-1/2 right-1.5 -translate-y-1/2 px-1.5 text-lg leading-none text-slate-500 hover:text-slate-200"
            >
              ×
            </button>
          )}
        </div>
        {deepTarget && (
          <label
            className="flex cursor-pointer items-center gap-1.5 rounded border border-slate-700 bg-slate-900 px-3 py-1.5 text-sm whitespace-nowrap text-slate-300 select-none"
            title="Download and structural-diff every changed file, hiding those with no structured difference"
          >
            <input
              type="checkbox"
              checked={deepCompare}
              onChange={(e) => setDeepCompare(e.target.checked)}
              className="accent-sky-500"
            />
            Deep compare
            {deepCompare && deepQuery.isFetching && (
              <Loader2 size={13} className="animate-spin text-slate-500" />
            )}
          </label>
        )}
        {appInfo && (
          <CompareMenu
            appInfo={appInfo}
            extras={extras}
            statuses={statuses}
            currentDepotId={Number(depotId)}
            currentManifestId={manifestId}
            selected={diffTargets}
            onChange={setDiffTargets}
            error={diffQuery.error as Error | null}
            // Pass the live search box value so the menu can hide
            // candidates with no diff inside the search results. We
            // forward the un-deferred `query` here — `deferred` would
            // make the dropdown filter lag behind the box.
            searchQuery={query}
            searchBase={{ depot_id: Number(depotId), manifest_id: manifestId, branch }}
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
        compareTo={diffTargets.size > 0 ? [...diffTargets].join(",") : undefined}
        // When exactly one compare-to target is selected we promote
        // file rows to direct diff links — single 1×1 diff is what
        // the new dedicated route handles. Two+ targets fall back to
        // the file-view (with `compare_to` kept), where the user
        // picks which one to diff against from the inline list.
        singleDiffTarget={diffRefs.length === 1 ? diffRefs[0] : undefined}
        diffStatus={diffStatus}
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
  compareTo,
  singleDiffTarget,
  diffStatus,
  onToggle,
}: {
  rows: FlatRow[];
  appid: string;
  depotId: string;
  manifestId: string;
  branch: string;
  compareTo: string | undefined;
  singleDiffTarget: ManifestRef | undefined;
  diffStatus: Map<string, ManifestDiffStatus> | null;
  onToggle: (path: string, currentlyExpanded: boolean, anchor: HTMLElement | null) => void;
}) {
  // Virtualize against the window — the manifest page scrolls at the
  // document level, not inside a fixed pane, so a window virtualizer
  // keeps the existing UX while skipping render of off-screen rows.
  // With 10k+ files the unvirtualized list lagged on toggle/filter.
  //
  // Capture the row container's offset from the page top so the
  // virtualizer can map window scrollY → row index correctly. The ref
  // sits on the *inner* spacer, not the outer card, so the header row
  // above it isn't part of the virtualized coordinate space. A
  // callback ref re-measures whenever the spacer re-mounts (e.g.
  // empty state → results) instead of latching the initial value.
  const [listOffset, setListOffset] = useState(0);
  const listRef = useCallback((el: HTMLDivElement | null) => {
    if (el) setListOffset(el.offsetTop);
  }, []);
  // Row height ≈ 33px (text-sm × py-1.5 + leading) but file rows can
  // wrap on long paths because of `break-all`, so we measure each row
  // and let the virtualizer adjust the spacer accordingly.
  const virtualizer = useWindowVirtualizer({
    count: rows.length,
    estimateSize: () => 33,
    overscan: 12,
    scrollMargin: listOffset,
  });
  if (rows.length === 0) {
    return <p className="text-sm text-slate-500">No matches.</p>;
  }
  const items = virtualizer.getVirtualItems();
  const scrollMargin = virtualizer.options.scrollMargin;
  return (
    <div className="overflow-hidden rounded border border-slate-800 text-sm">
      <div className="flex items-baseline gap-1 border-b border-slate-800 px-3 py-1.5 font-semibold text-slate-400">
        <span>Name</span>
        <span className="ml-auto w-14 text-right">Files</span>
        <span className="w-20 text-right">Size</span>
      </div>
      <div ref={listRef} className="relative" style={{ height: virtualizer.getTotalSize() }}>
        {items.map((vi) => {
          const row = rows[vi.index];
          return (
            <div
              key={row.node.path}
              ref={virtualizer.measureElement}
              data-index={vi.index}
              style={{
                position: "absolute",
                top: 0,
                left: 0,
                right: 0,
                transform: `translateY(${vi.start - scrollMargin}px)`,
              }}
            >
              <TreeRow
                node={row.node}
                depth={row.depth}
                expanded={row.expanded}
                appid={appid}
                depotId={depotId}
                manifestId={manifestId}
                branch={branch}
                compareTo={compareTo}
                singleDiffTarget={singleDiffTarget}
                fileDiffStatus={row.node.file ? (diffStatus?.get(row.node.path) ?? null) : null}
                onToggle={onToggle}
              />
            </div>
          );
        })}
      </div>
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
  compareTo,
  singleDiffTarget,
  fileDiffStatus,
  onToggle,
}: {
  node: TreeNode;
  depth: number;
  expanded: boolean;
  appid: string;
  depotId: string;
  manifestId: string;
  branch: string;
  compareTo: string | undefined;
  singleDiffTarget: ManifestRef | undefined;
  /// Per-row diff status. `null` outside compare mode and for plain
  /// `changed` files keeps the default sky link colour; `"added"`
  /// recolours the row so the user can tell at a glance that the
  /// file does not exist in any compare target.
  fileDiffStatus: ManifestDiffStatus | null;
  onToggle: (path: string, currentlyExpanded: boolean, anchor: HTMLElement | null) => void;
}) {
  const isDir = node.file == null;
  // 16px per nesting level for the row, plus 16px for the chevron column
  // (files have no chevron; we reserve the same slot so names align).
  const indentPx = 12 + depth * 16;
  // Sky is the default file-link colour; emerald flags "added in base"
  // so the user can scan added vs. changed entries at a glance. We pin
  // the colour on the <Link> rather than wrapping the row so the link
  // text — including the size column — stays consistently coloured.
  const linkColorClass = fileDiffStatus === "added" ? "text-emerald-400" : "text-sky-400";
  const sizeColorClass = fileDiffStatus === "added" ? "text-emerald-400/70" : "text-slate-400";

  if (isDir) {
    return (
      <div className="border-b border-slate-800">
        <button
          type="button"
          onClick={(e) => onToggle(node.path, expanded, e.currentTarget)}
          className="flex w-full items-baseline gap-1 py-1.5 pr-3 text-left hover:bg-slate-800/40"
          style={{ paddingLeft: indentPx }}
        >
          <span className="inline-block w-3 text-xs text-slate-500">{expanded ? "▾" : "▸"}</span>
          <span className="text-sm text-slate-200">{node.name}</span>
          <span className="ml-auto w-14 text-right text-xs whitespace-nowrap text-slate-500 tabular-nums">
            {node.fileCount.toLocaleString()}
          </span>
          <span className="w-20 text-right text-xs whitespace-nowrap text-slate-500 tabular-nums">
            <Bytes value={node.size} />
          </span>
        </button>
      </div>
    );
  }

  // File row — clickable link. Add the chevron-slot width to indent so
  // file names line up with sibling dir names (which have a chevron).
  const file = node.file!;
  const fileLink = singleDiffTarget ? (
    <Link
      to="/apps/$appid/depots/$depotId/manifests/$manifestId/diff"
      params={{ appid, depotId, manifestId }}
      search={{
        branch: branch === "public" ? undefined : branch,
        path: file.path,
        target_depot_id: singleDiffTarget.depot_id,
        target_manifest_id: singleDiffTarget.manifest_id,
        target_branch: singleDiffTarget.branch === "public" ? undefined : singleDiffTarget.branch,
      }}
      className={`flex items-baseline gap-1 py-1.5 pr-3 ${linkColorClass}`}
      style={{ paddingLeft: indentPx + 16 }}
    >
      <span className="text-sm break-all">{node.name}</span>
      {file.linktarget && (
        <span className="font-mono text-xs text-slate-500"> → {file.linktarget}</span>
      )}
      <span className="ml-auto w-14" aria-hidden="true" />
      <span className={`w-20 text-right text-xs whitespace-nowrap tabular-nums ${sizeColorClass}`}>
        <Bytes value={file.size} />
      </span>
    </Link>
  ) : (
    <Link
      to="/apps/$appid/depots/$depotId/manifests/$manifestId/file"
      params={{ appid, depotId, manifestId }}
      search={{
        branch: branch === "public" ? undefined : branch,
        path: file.path,
        compare_to: compareTo,
      }}
      className={`flex items-baseline gap-1 py-1.5 pr-3 ${linkColorClass}`}
      style={{ paddingLeft: indentPx + 16 }}
    >
      <span className="text-sm break-all">{node.name}</span>
      {file.linktarget && (
        <span className="font-mono text-xs text-slate-500"> → {file.linktarget}</span>
      )}
      <span className="ml-auto w-14" aria-hidden="true" />
      <span className={`w-20 text-right text-xs whitespace-nowrap tabular-nums ${sizeColorClass}`}>
        <Bytes value={file.size} />
      </span>
    </Link>
  );
  return <div className="border-b border-slate-800 hover:bg-slate-800/40">{fileLink}</div>;
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
        className={`rounded border px-3 py-1.5 text-sm whitespace-nowrap ${
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
        <div className="absolute top-full right-0 z-10 mt-1 max-h-80 w-48 overflow-auto rounded border border-slate-700 bg-slate-900 shadow-lg">
          <div className="sticky top-0 flex items-center justify-between border-b border-slate-800 bg-slate-900 px-3 py-1.5 text-xs text-slate-400">
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
                    className={`flex cursor-pointer items-center gap-2 px-3 py-1 text-sm select-none hover:bg-slate-800/60 ${
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
                      {ext === NO_EXT ? <span className="text-slate-500 italic">none</span> : ext}
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

function formatTime(unix: number): string {
  if (!unix) return "—";
  return new Date(unix * 1000).toISOString().replace("T", " ").slice(0, 19) + " UTC";
}
