// TODO(ai-review): review for style and correctness
import { useEffect, useMemo, useRef, useState } from "react";
import type { AppInfo, ExtraManifestEntry, ManifestStatusEntry } from "./api";
import { formatDate } from "./format";

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
}: {
  appInfo: AppInfo;
  extras: ExtraManifestEntry[];
  statuses: ManifestStatusEntry[] | undefined;
  currentDepotId: number;
  currentManifestId: string;
  selected: Set<string>;
  onChange: (next: Set<string>) => void;
  error?: Error | null;
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
