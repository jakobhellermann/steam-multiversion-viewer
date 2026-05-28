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
import { formatDate } from "../lib/format";

/// File-context the file-view page hands in so the menu can hide
/// manifests whose version of this single file is identical to the
/// base. Drives a `POST /file/diff-targets` query.
export type FileContext = {
  appid: number;
  base: ManifestRef;
  path: string;
};

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

/// Build the URL-safe key used for `compare_to`. Targets in the same
/// depot as the base get a bare manifest_id, cross-depot targets carry
/// the depot prefix joined with a dash (so neither slash nor comma need
/// percent-encoding).
export function diffTargetKey(depotId: number, manifestId: string, currentDepotId: number): string {
  return depotId === currentDepotId ? manifestId : `${depotId}-${manifestId}`;
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
        const creationKey = `${d.depot_id}/${m.manifest_id}`;
        groupMap.get(d.depot_id)!.candidates.push({
          key: diffTargetKey(d.depot_id, m.manifest_id, currentDepotId),
          depotId: d.depot_id,
          manifestId: m.manifest_id,
          branch: m.branch,
          creationTime: creationByKey.get(creationKey) ?? 0,
        });
      }
    }
    for (const e of extras) {
      if (e.depot_id === currentDepotId && e.manifest_id === currentManifestId) continue;
      const group = groupMap.get(e.depot_id);
      if (!group) continue;
      if (group.candidates.some((c) => c.manifestId === e.manifest_id)) continue;
      const creationKey = `${e.depot_id}/${e.manifest_id}`;
      group.candidates.push({
        key: diffTargetKey(e.depot_id, e.manifest_id, currentDepotId),
        depotId: e.depot_id,
        manifestId: e.manifest_id,
        branch: e.branch ?? "public",
        creationTime: creationByKey.get(creationKey) ?? 0,
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

  // When the menu is bound to a specific file (file-view page), ask the
  // backend which candidates have a *different* version of that file.
  // We only enable the query while the menu is open so opening a file
  // doesn't burn a round trip per page load.
  const others = useMemo<ManifestRef[]>(() => {
    if (!fileContext) return [];
    return groups.flatMap((g) =>
      g.candidates.map((c) => ({
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
      g.candidates.map((c) => ({
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
        candidates: g.candidates.filter((c) => fileStatusByKey.get(c.key) === "different"),
      }));
      // Hide empty *other* depots but keep "this depot" — the right pane
      // can then explicitly say "no different manifests in this depot"
      // instead of the whole menu collapsing to "nothing".
      return filtered.filter((g) => g.depotId === currentDepotId || g.candidates.length > 0);
    }
    if (searchActive) {
      if (!searchKeptKeys) return [];
      const filtered = groups.map((g) => ({
        ...g,
        candidates: g.candidates.filter((c) => searchKeptKeys.has(c.key)),
      }));
      return filtered.filter((g) => g.depotId === currentDepotId || g.candidates.length > 0);
    }
    return groups;
  }, [
    groups,
    fileContext,
    fileDiff.data,
    fileStatusByKey,
    currentDepotId,
    searchActive,
    searchKeptKeys,
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
      queryKey: ["game-info", appInfo.appid, c.depotId, c.manifestId, c.branch],
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
        title="Show only files that differ from selected manifests"
      >
        Compare to{count > 0 && <span className="ml-1.5 tabular-nums">({count})</span>}
      </button>
      {open && (
        <div className="absolute top-full -right-25 z-10 mt-1 flex max-h-96 w-128 overflow-hidden rounded border border-slate-700 bg-slate-900 shadow-lg">
          <div className="w-56 overflow-auto border-r border-slate-800">
            <div className="sticky top-0 flex items-center justify-between border-b border-slate-800 bg-slate-900 px-3 py-1.5 text-xs text-slate-400">
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
                            ? "bg-slate-800 text-slate-100"
                            : "text-slate-300 hover:bg-slate-800/60"
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
                        <span className="min-w-0 flex-1">
                          <span className="block truncate">{c.branch}</span>
                          <span
                            className={`block truncate text-xs ${
                              checked ? "text-sky-400/70" : "text-slate-500"
                            }`}
                          >
                            {c.creationTime > 0 ? formatDate(c.creationTime) : "date unknown"}
                          </span>
                        </span>
                        <span
                          className={`text-xs tabular-nums ${
                            checked ? "text-sky-400/70" : "text-slate-500"
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
