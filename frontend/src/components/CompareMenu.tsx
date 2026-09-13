// TODO(ai-review): review for style and correctness
import { useQueries, useQuery } from "@tanstack/react-query";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  fetchFileDiffTargets,
  fetchGameInfo,
  fetchManifestDiffTargets,
  type AppInfo,
  type ExtraManifestEntry,
  type GameInfo,
  type ManifestRef,
  type ManifestStatusEntry,
} from "../api";
import { BranchFilterList } from "./BranchFilterList";
import {
  buildDepotGroups,
  diffTargetKey,
  filterGroupsByBranch,
  type CompareCandidate,
  type DepotGroup,
} from "./compareCandidates";
import { formatDate } from "../lib/format";
import { useBranchFilter } from "../lib/useBranchFilter";

// Re-exported for the route files that import it alongside the component.
export { diffTargetKey };

/// File-context the file-view page hands in so the menu can hide
/// manifests whose version of this single file is identical to the
/// base. Drives a `POST /file/diff-targets` query.
export type FileContext = {
  appid: number;
  base: ManifestRef;
  path: string;
};

/// URL of the page that opens this candidate in isolation. For
/// file-context calls we stay on the file-detail page (same `path`);
/// otherwise we land on the manifest detail page. Used as the `href`
/// on the dropdown rows so middle-click / ctrl-click works as
/// expected.
function candidateHref(
  appid: number,
  c: CompareCandidate,
  fileContext: FileContext | undefined,
): string {
  const params = new URLSearchParams();
  if (c.branch !== "public") params.set("branch", c.branch);
  if (fileContext) {
    params.set("path", fileContext.path);
    return `/apps/${appid}/depots/${c.depotId}/manifests/${c.manifestId}/file?${params.toString()}`;
  }
  const qs = params.toString();
  return `/apps/${appid}/depots/${c.depotId}/manifests/${c.manifestId}${qs ? `?${qs}` : ""}`;
}

export function CompareMenu({
  appInfo,
  extras,
  statuses,
  currentDepotId,
  currentManifestId,
  selected,
  onChange,
  error,
  fileContext,
  searchQuery,
  searchBase,
}: {
  appInfo: AppInfo;
  extras: ExtraManifestEntry[];
  statuses: ManifestStatusEntry[] | undefined;
  currentDepotId: number;
  currentManifestId: string;
  selected: Set<string>;
  onChange: (next: Set<string>) => void;
  error?: Error | null;
  /// When set, the menu filters out manifests where the file at
  /// `fileContext.path` is identical to the base — only `different`
  /// and `missing` candidates remain.
  fileContext?: FileContext;
  /// When set (and `fileContext` is not), the menu filters out
  /// manifests where no path matching `searchQuery` differs from
  /// `searchBase`. Used by the manifest-detail page so an active
  /// search restricts the compare-to dropdown to targets that
  /// actually have changes inside the search results.
  searchQuery?: string;
  searchBase?: ManifestRef;
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
  const groups = useMemo<DepotGroup[]>(
    () => buildDepotGroups(appInfo, extras, statuses, currentDepotId, currentManifestId),
    [appInfo, extras, statuses, currentDepotId, currentManifestId],
  );

  // Distinct branches across all candidates, in encounter order. Drives
  // the branch filter at the bottom of the depot column.
  const allCandidateBranches = useMemo(() => {
    const ordered: string[] = [];
    const seen = new Set<string>();
    for (const g of groups) {
      for (const c of g.candidates) {
        if (c.isCurrent) continue;
        for (const b of c.branches) {
          if (!seen.has(b)) {
            seen.add(b);
            ordered.push(b);
          }
        }
      }
    }
    return ordered;
  }, [groups]);
  // Which branches to hide from the candidate list. Shared with the
  // app-overview page (persisted per appid) and defaulting to public-only.
  const {
    hidden: hiddenBranches,
    toggle: toggleBranch,
    showAll: showAllBranches,
    hideAll: hideAllBranches,
    only: onlyBranch,
  } = useBranchFilter(appInfo.appid, allCandidateBranches);
  // The branch filter is a popover, not an inline list — the default
  // (public only) is what you want almost all the time, so it stays out
  // of the way until clicked. It lives at the top of the depot column so
  // its anchor stays put as the candidate list (and the dropdown) grows
  // downward.
  const [branchMenuOpen, setBranchMenuOpen] = useState(false);
  useEffect(() => {
    if (!open) setBranchMenuOpen(false);
  }, [open]);

  // When the menu is bound to a specific file (file-view page), ask the
  // backend which candidates have a *different* version of that file.
  // We only enable the query while the menu is open so opening a file
  // doesn't burn a round trip per page load.
  const others = useMemo<ManifestRef[]>(() => {
    if (!fileContext) return [];
    return groups.flatMap((g) =>
      g.candidates
        .filter((c) => !c.isCurrent)
        .map((c) => ({
          depot_id: c.depotId,
          manifest_id: c.manifestId,
          branch: c.branch,
        })),
    );
  }, [fileContext, groups]);
  const fileDiff = useQuery({
    queryKey: [
      "file-diff-targets",
      fileContext?.appid,
      fileContext?.base.depot_id,
      fileContext?.base.manifest_id,
      fileContext?.path,
      // Sort the candidate list so ordering changes (e.g. once the
      // manifests-status query lands and we re-sort by creation_time)
      // don't fragment the cache into "same content, different key".
      others
        .map((r) => `${r.depot_id}/${r.manifest_id}`)
        .sort()
        .join(","),
    ],
    queryFn: () =>
      fetchFileDiffTargets(fileContext!.appid, fileContext!.base, others, fileContext!.path),
    // Prefetch — independent of `open` — so opening the menu has the
    // filter ready instead of flashing "Checking…". Cheap when the
    // candidate manifests are already in the backend's cache.
    enabled: fileContext != null && others.length > 0,
    staleTime: Infinity,
    // POST request → browser http-cache won't store it; rely on react-
    // query's in-memory store instead. Manifests are immutable so a
    // long gcTime is safe.
    gcTime: 60 * 60 * 1000,
    // Closing + reopening the menu would otherwise reset `data` to
    // undefined and flash the "Checking…" empty state again.
    placeholderData: (prev) => prev,
  });
  const fileStatusByKey = useMemo(() => {
    const map = new Map<string, "same" | "different" | "missing">();
    if (!fileDiff.data) return map;
    for (const s of fileDiff.data) {
      const key = diffTargetKey(s.depot_id, s.manifest_id, currentDepotId);
      map.set(key, s.status);
    }
    return map;
  }, [fileDiff.data, currentDepotId]);

  // Manifest-detail search variant: hide candidates that have no diff
  // in any path matching the user's current search box. Mutually
  // exclusive with `fileContext` — the file-detail page never has a
  // free-form search box.
  const searchActive =
    !fileContext && !!searchBase && !!searchQuery && searchQuery.trim().length > 0;
  const searchOthers = useMemo<ManifestRef[]>(() => {
    if (!searchActive) return [];
    return groups.flatMap((g) =>
      g.candidates
        .filter((c) => !c.isCurrent)
        .map((c) => ({
          depot_id: c.depotId,
          manifest_id: c.manifestId,
          branch: c.branch,
        })),
    );
  }, [searchActive, groups]);
  const searchDiff = useQuery({
    queryKey: [
      "manifest-diff-targets",
      searchBase?.depot_id,
      searchBase?.manifest_id,
      searchBase?.branch,
      searchQuery?.trim() ?? "",
      searchOthers
        .map((r) => `${r.depot_id}/${r.manifest_id}`)
        .sort()
        .join(","),
    ],
    queryFn: () =>
      fetchManifestDiffTargets(appInfo.appid, searchBase!, searchOthers, searchQuery!.trim()),
    enabled: searchActive && searchOthers.length > 0,
    staleTime: Infinity,
    gcTime: 60 * 60 * 1000,
    placeholderData: (prev) => prev,
  });
  const searchKeptKeys = useMemo<Set<string> | null>(() => {
    if (!searchActive) return null;
    if (!searchDiff.data) return null;
    const out = new Set<string>();
    for (const t of searchDiff.data) {
      out.add(diffTargetKey(t.depot_id, t.manifest_id, currentDepotId));
    }
    return out;
  }, [searchActive, searchDiff.data, currentDepotId]);

  const visibleGroups = useMemo<DepotGroup[]>(() => {
    const filterBranches = (gs: DepotGroup[]) =>
      filterGroupsByBranch(gs, hiddenBranches, currentDepotId);
    if (fileContext) {
      // File-detail variant — original `/file/diff-targets` filter.
      // Until the diff-targets query returns, don't render anything —
      // otherwise the menu briefly shows every manifest and then collapses
      // to the filtered subset, which looks like a flicker.
      if (!fileDiff.data) return [];
      const filtered = groups.map((g) => ({
        ...g,
        // Keep only manifests where the file still exists *and* differs —
        // skip both `same` (no change to show) and `missing` (deleted in
        // that version, nothing to compare to).
        candidates: g.candidates.filter(
          (c) => c.isCurrent || fileStatusByKey.get(c.key) === "different",
        ),
      }));
      // Hide empty *other* depots but keep "this depot" — the right pane
      // can then explicitly say "no different manifests in this depot"
      // instead of the whole menu collapsing to "nothing".
      return filterBranches(
        filtered.filter((g) => g.depotId === currentDepotId || g.candidates.length > 0),
      );
    }
    if (searchActive) {
      if (!searchKeptKeys) return [];
      const filtered = groups.map((g) => ({
        ...g,
        candidates: g.candidates.filter((c) => c.isCurrent || searchKeptKeys.has(c.key)),
      }));
      return filterBranches(
        filtered.filter((g) => g.depotId === currentDepotId || g.candidates.length > 0),
      );
    }
    return filterBranches(groups);
  }, [
    groups,
    fileContext,
    fileDiff.data,
    fileStatusByKey,
    currentDepotId,
    searchActive,
    searchKeptKeys,
    hiddenBranches,
  ]);

  // If our previously-active depot stopped having candidates, fall back
  // to the first available group.
  useEffect(() => {
    if (!open) return;
    if (!visibleGroups.find((g) => g.depotId === activeDepotId)) {
      setActiveDepotId(visibleGroups[0]?.depotId ?? null);
    }
  }, [open, visibleGroups, activeDepotId]);

  // Game version per candidate. Read from the same `/game_info` query
  // key shape that AppDetail and ManifestDetail use, so cache hits
  // make the lookup free here. Only enabled while the menu is open so
  // a passive file-detail page doesn't kick off N requests at mount.
  const candidates = useMemo(() => visibleGroups.flatMap((g) => g.candidates), [visibleGroups]);
  const gameInfoQueries = useQueries({
    queries: candidates.map((c) => ({
      // Branch omitted from the key on purpose: game_info is content-
      // addressed by manifest_id, so the same gid shares one entry across
      // branches and with the manifest page + switcher. Branch is still
      // passed to the fetch so the backend can open the manifest.
      queryKey: ["game-info", appInfo.appid, c.depotId, c.manifestId],
      queryFn: () => fetchGameInfo(appInfo.appid, c.depotId, c.manifestId, c.branch),
      enabled: open,
      staleTime: Infinity,
      gcTime: 30 * 60 * 1000,
    })),
  });
  const bundleVersionByKey = useMemo(() => {
    const map = new Map<string, string>();
    candidates.forEach((c, i) => {
      const data = gameInfoQueries[i]?.data as GameInfo | undefined;
      const v = data?.engine?.engine === "unity" ? data.engine.data.bundle_version : undefined;
      if (v) map.set(c.key, v);
    });
    return map;
  }, [candidates, gameInfoQueries]);

  const activeGroup = visibleGroups.find((g) => g.depotId === activeDepotId) ?? visibleGroups[0];
  const toggle = (key: string) => {
    const next = new Set(selected);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    onChange(next);
  };
  const count = selected.size;
  // Compact summary for the branch-filter button: the single branch name
  // when exactly one is shown (the common "public" case), else a count.
  const shownBranches = allCandidateBranches.filter((b) => !hiddenBranches.has(b));
  const branchLabel =
    hiddenBranches.size === 0
      ? "all"
      : shownBranches.length === 0
        ? "none"
        : shownBranches.length === 1
          ? shownBranches[0]
          : `${shownBranches.length} of ${allCandidateBranches.length}`;
  return (
    <div ref={rootRef} className="relative">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className={`dropdown-trigger ${
          count > 0
            ? "border-sky-700 bg-sky-950/40 text-sky-200 hover:bg-sky-900/40"
            : "border-slate-700 text-slate-300 hover:border-slate-600"
        }`}
        aria-haspopup="dialog"
        aria-expanded={open}
        title="Show only files that differ from selected manifests"
      >
        Compare to{count > 0 && <span className="ml-1.5 tabular-nums">({count})</span>}
      </button>
      {open && (
        <div className="dropdown-panel -right-25 z-10 flex max-h-96 w-128 surface-float">
          <div className="flex w-56 flex-col border-r border-slate-800">
            {allCandidateBranches.length > 1 && (
              <div className="relative z-20 shrink-0 border-b border-slate-800">
                <button
                  type="button"
                  onClick={() => setBranchMenuOpen((o) => !o)}
                  aria-expanded={branchMenuOpen}
                  className="flex w-full items-center justify-between gap-2 px-3 py-1.5 text-xs text-slate-400 hover:bg-slate-700/60"
                >
                  <span className="truncate">
                    Branches: <span className="text-slate-300">{branchLabel}</span>
                  </span>
                  <span className="text-slate-500">{branchMenuOpen ? "▲" : "▼"}</span>
                </button>
                {branchMenuOpen && (
                  <div className="dropdown-panel left-0 z-20 flex max-h-72 w-48 flex-col overflow-hidden surface-float">
                    <BranchFilterList
                      branches={allCandidateBranches}
                      hidden={hiddenBranches}
                      onToggle={toggleBranch}
                      onShowAll={showAllBranches}
                      onHideAll={hideAllBranches}
                      onOnly={onlyBranch}
                    />
                  </div>
                )}
              </div>
            )}
            <div className="flex shrink-0 items-center justify-between border-b border-slate-800 px-3 py-1.5 text-xs text-slate-400">
              <span className="tabular-nums">compare to...</span>
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
            <div className="flex-1 overflow-auto">
              {visibleGroups.length === 0 ? (
                <p className="px-3 py-2 text-sm text-slate-500">
                  {fileContext && fileDiff.isFetching
                    ? "Checking which manifests differ…"
                    : fileContext
                      ? "No manifests where this file differs."
                      : searchActive && searchDiff.isFetching
                        ? "Checking which manifests differ…"
                        : searchActive
                          ? "No manifests with changes in the search results."
                          : "No other manifests."}
                </p>
              ) : (
                <ul>
                  {visibleGroups.map((g) => {
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
                          className={`flex w-full items-center gap-2 px-3 py-1.5 text-left text-sm ${
                            isActive
                              ? "bg-slate-700 text-slate-100"
                              : "text-slate-300 hover:bg-slate-700/60"
                          }`}
                        >
                          <span className="min-w-0 flex-1">
                            <span className="block truncate">
                              {g.depotId === currentDepotId ? (
                                <span>This depot</span>
                              ) : (
                                <span>depot {g.depotId}</span>
                              )}
                            </span>
                            <span className="block truncate text-xs text-slate-500">{g.label}</span>
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
          </div>
          <div className="flex-1 overflow-auto">
            {error && (
              <p className="border-b border-slate-800 px-3 py-2 text-xs text-red-300">
                Diff failed: {error.message}
              </p>
            )}
            {!activeGroup ? (
              <p className="px-3 py-2 text-sm text-slate-500">Pick a depot on the left.</p>
            ) : activeGroup.candidates.length === 0 ? (
              <p className="px-3 py-2 text-sm text-slate-500">
                {fileContext
                  ? "No manifests where this file differs."
                  : searchActive
                    ? "No manifests with changes in the search results."
                    : "No manifests in this depot."}
              </p>
            ) : (
              <ul>
                {activeGroup.candidates.map((c) => {
                  const checked = selected.has(c.key);
                  // The manifest you're on: same layout as the others, but a
                  // disabled "you are here" anchor (• instead of the checkmark)
                  // so its date places it among them. Not selectable.
                  if (c.isCurrent) {
                    return (
                      <li key={c.key}>
                        <div
                          aria-disabled="true"
                          className="flex w-full items-center gap-2 px-3 py-1 text-left text-sm text-slate-500"
                        >
                          <span aria-hidden="true" className="inline-block w-3 text-center">
                            •
                          </span>
                          <span className="min-w-0 flex-1">
                            <span className="block truncate">{c.branch}</span>
                            <span className="block truncate text-xs text-slate-600">
                              {c.creationTime > 0 ? formatDate(c.creationTime) : "date unknown"}
                            </span>
                          </span>
                          <span
                            className={`text-xs tabular-nums ${
                              bundleVersionByKey.has(c.key) ? "" : "font-mono"
                            }`}
                            title={bundleVersionByKey.has(c.key) ? c.manifestId : undefined}
                          >
                            {bundleVersionByKey.get(c.key) ?? `${c.manifestId.slice(0, 8)}…`}
                          </span>
                        </div>
                      </li>
                    );
                  }
                  return (
                    <li key={c.key}>
                      {/*
                        `<a>` rather than `<button>` so middle-click /
                        ctrl-click open the candidate in a new tab. We
                        intercept the plain left-click to toggle the
                        compare-set inline; modifier-clicks fall through
                        to the browser's default navigation.
                      */}
                      <a
                        href={candidateHref(appInfo.appid, c, fileContext)}
                        onClick={(e) => {
                          // Only let ctrl/cmd/alt (new tab/window) fall
                          // through to the browser's default navigation;
                          // shift is reserved for a future range-select.
                          if (e.button !== 0 || e.metaKey || e.ctrlKey || e.altKey) return;
                          e.preventDefault();
                          toggle(c.key);
                        }}
                        aria-pressed={checked}
                        className={`flex w-full cursor-pointer items-center gap-2 px-3 py-1 text-left text-sm select-none ${
                          checked
                            ? "bg-sky-950/40 text-slate-100 hover:bg-sky-900/40"
                            : "text-slate-300 hover:bg-slate-700/60"
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
                        <span className="min-w-0 flex-1">
                          <span className="block truncate">{c.branch}</span>
                          <span
                            className={`block truncate text-xs ${
                              checked ? "text-slate-400" : "text-slate-500"
                            }`}
                          >
                            {c.creationTime > 0 ? formatDate(c.creationTime) : "date unknown"}
                          </span>
                        </span>
                        <span
                          className={`text-xs tabular-nums ${
                            checked ? "text-slate-400" : "text-slate-500"
                          } ${bundleVersionByKey.has(c.key) ? "" : "font-mono"}`}
                          title={bundleVersionByKey.has(c.key) ? c.manifestId : undefined}
                        >
                          {bundleVersionByKey.get(c.key) ?? `${c.manifestId.slice(0, 8)}…`}
                        </span>
                      </a>
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
