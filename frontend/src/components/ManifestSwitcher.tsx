// TODO(ai-review): review for style and correctness
import { useQueries } from "@tanstack/react-query";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  fetchGameInfo,
  type AppInfo,
  type ExtraManifestEntry,
  type GameInfo,
  type ManifestStatusEntry,
} from "../api";
import { formatDate } from "../lib/format";
import { useBranchFilter } from "../lib/useBranchFilter";

/// Dropdown next to the manifest breadcrumb to jump to another manifest
/// in the same depot without going back to the app page. Lists official
/// + tracked manifests, newest first. The caller decides what happens
/// on selection via `onSelect` — typically staying on the current
/// sub-page (file, diff) or jumping to the manifest overview.
///
/// `onGoToManifest` (optional) makes the label a clickable link back to
/// the manifest overview — used on sub-pages where the breadcrumb should
/// still navigate "up" to the manifest detail page.
export function ManifestSwitcher({
  label,
  appid,
  depotId,
  currentManifestId,
  currentBranch,
  appInfo,
  extras,
  statuses,
  onSelect,
  onGoToManifest,
}: {
  label: string;
  appid: number;
  depotId: number;
  currentManifestId: string;
  currentBranch: string;
  appInfo: AppInfo | undefined;
  extras: ExtraManifestEntry[];
  statuses: ManifestStatusEntry[] | undefined;
  /// Called when the user picks a manifest from the dropdown.
  onSelect: (manifestId: string, branch: string) => void;
  /// Optional: makes the label a link back to the manifest overview.
  /// Omit on the manifest page itself (the label is already the current page).
  onGoToManifest?: () => void;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
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
  const manifests = useMemo(() => {
    if (!appInfo) return [];
    const creation = new Map<string, number>();
    for (const s of statuses ?? []) creation.set(`${s.depot_id}/${s.manifest_id}`, s.creation_time);
    const seen = new Set<string>();
    const out: { manifestId: string; branch: string; creationTime: number }[] = [];
    const add = (manifestId: string, branch: string) => {
      const key = `${manifestId}|${branch}`;
      if (seen.has(key)) return;
      seen.add(key);
      out.push({ manifestId, branch, creationTime: creation.get(`${depotId}/${manifestId}`) ?? 0 });
    };
    for (const m of appInfo.depots.find((d) => d.depot_id === depotId)?.manifests ?? []) {
      add(m.manifest_id, m.branch);
    }
    for (const e of extras) {
      if (e.depot_id === depotId) add(e.manifest_id, e.branch ?? "public");
    }
    out.sort((a, b) => b.creationTime - a.creationTime || a.manifestId.localeCompare(b.manifestId));
    return out;
  }, [appInfo, extras, statuses, depotId]);
  // Follow the shared branch filter, but always keep the current manifest
  // visible so the dropdown still reflects where you are.
  const { hidden } = useBranchFilter(
    appid,
    manifests.map((m) => m.branch),
  );
  const visible = useMemo(
    () =>
      manifests.filter(
        (m) =>
          !hidden.has(m.branch) ||
          (m.manifestId === currentManifestId && m.branch === currentBranch),
      ),
    [manifests, hidden, currentManifestId, currentBranch],
  );
  // Show the Unity game version instead of the raw manifest id when we can
  // detect it — same `/game_info` query shape as elsewhere, so it's a cache
  // hit. The current manifest is always fetched (it feeds the breadcrumb
  // label); the rest only while the dropdown is open.
  const gameInfoQueries = useQueries({
    queries: visible.map((m) => ({
      // Branch omitted from the key on purpose — shared with the page +
      // compare menu (see the gameInfo query above).
      queryKey: ["game-info", appid, depotId, m.manifestId],
      queryFn: () => fetchGameInfo(appid, depotId, m.manifestId, m.branch),
      enabled: open || (m.manifestId === currentManifestId && m.branch === currentBranch),
      staleTime: Infinity,
      gcTime: 30 * 60 * 1000,
    })),
  });
  const versionByKey = useMemo(() => {
    const map = new Map<string, string>();
    visible.forEach((m, i) => {
      const data = gameInfoQueries[i]?.data as GameInfo | undefined;
      const v = data?.engine?.engine === "unity" ? data.engine.data.bundle_version : undefined;
      if (v) map.set(`${m.manifestId}|${m.branch}`, v);
    });
    return map;
  }, [visible, gameInfoQueries]);
  // Prefer the detected game version in the breadcrumb, falling back to the
  // date-or-"manifest" label the caller passed in.
  const displayLabel = versionByKey.get(`${currentManifestId}|${currentBranch}`) ?? label;

  if (visible.length <= 1) {
    return onGoToManifest ? (
      <button type="button" onClick={onGoToManifest} className="hover:underline">
        {displayLabel}
      </button>
    ) : (
      <span className="font-medium text-slate-200">{displayLabel}</span>
    );
  }
  return (
    <div ref={rootRef} className="relative flex items-center gap-0.5">
      {onGoToManifest ? (
        <>
          <button type="button" onClick={onGoToManifest} className="hover:underline">
            {displayLabel}
          </button>
          <button
            type="button"
            onClick={() => setOpen((o) => !o)}
            aria-haspopup="listbox"
            aria-expanded={open}
            title="Switch to another manifest"
            className="text-xs text-slate-500 hover:text-white"
          >
            ▾
          </button>
        </>
      ) : (
        <button
          type="button"
          onClick={() => setOpen((o) => !o)}
          aria-haspopup="listbox"
          aria-expanded={open}
          title="Switch to another manifest"
          className="flex items-center gap-1 font-medium text-slate-200 hover:text-white"
        >
          {displayLabel}
          <span className="text-xs text-slate-500">▾</span>
        </button>
      )}
      {open && (
        <div className="absolute top-full left-0 z-20 mt-1 max-h-80 w-72 overflow-auto rounded border border-slate-700 bg-slate-900 shadow-lg">
          <ul>
            {visible.map((m) => {
              const isCurrent = m.manifestId === currentManifestId && m.branch === currentBranch;
              return (
                <li key={`${m.manifestId}|${m.branch}`}>
                  <a
                    role="button"
                    tabIndex={0}
                    onClick={(e) => {
                      e.preventDefault();
                      onSelect(m.manifestId, m.branch);
                      setOpen(false);
                    }}
                    className={`flex items-center gap-2 px-3 py-1.5 text-sm ${
                      isCurrent
                        ? "bg-slate-800 text-slate-100"
                        : "text-slate-300 hover:bg-slate-800/60"
                    }`}
                  >
                    <span className="inline-block w-3 text-center text-sky-400">
                      {isCurrent ? "✓" : ""}
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="block truncate">{m.branch}</span>
                      <span className="block truncate text-xs text-slate-500">
                        {m.creationTime > 0 ? formatDate(m.creationTime) : "date unknown"}
                      </span>
                    </span>
                    {(() => {
                      const v = versionByKey.get(`${m.manifestId}|${m.branch}`);
                      return (
                        <span
                          className={`text-xs text-slate-500 tabular-nums ${v ? "" : "font-mono"}`}
                          title={v ? m.manifestId : undefined}
                        >
                          {v ?? `${m.manifestId.slice(0, 8)}…`}
                        </span>
                      );
                    })()}
                  </a>
                </li>
              );
            })}
          </ul>
        </div>
      )}
    </div>
  );
}
