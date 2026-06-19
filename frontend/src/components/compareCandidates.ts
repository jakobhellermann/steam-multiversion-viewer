// TODO(ai-review): review for style and correctness
//! Pure candidate-list logic for the compare menu: collapse every known
//! manifest (official + tracked) into one entry per gid, grouped by depot,
//! and filter that list by the active branch selection. Kept out of the
//! component so it can be unit-tested.
import type { AppInfo, ExtraManifestEntry, ManifestStatusEntry } from "../api";

export type CompareCandidate = {
  key: string;
  depotId: number;
  manifestId: string;
  /// Representative branch (display label + the branch passed to fetches).
  /// Swapped to a visible one when the filter hides the first-listed one.
  branch: string;
  /// Every branch in this depot pointing at this gid. The same content can
  /// be reachable via several branches (e.g. public == public-beta when no
  /// beta is active); we keep the gid once but filter on all of them.
  branches: string[];
  creationTime: number;
  /// The manifest currently open. Shown disabled in the list as a "you
  /// are here" anchor; never selectable or used as a diff target.
  isCurrent: boolean;
};

export type DepotGroup = {
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

/// Group every known manifest (official + tracked) under its depot, one
/// entry per gid. Each entry collects all branches that point at it and
/// its creation_time (0 if unknown), sorted newest-first per depot;
/// depots sorted current-first then by id, empty depots dropped.
export function buildDepotGroups(
  appInfo: AppInfo,
  extras: ExtraManifestEntry[],
  statuses: ManifestStatusEntry[] | undefined,
  currentDepotId: number,
  currentManifestId: string,
): DepotGroup[] {
  const creationByKey = new Map<string, number>();
  for (const s of statuses ?? []) {
    creationByKey.set(`${s.depot_id}/${s.manifest_id}`, s.creation_time);
  }
  const groupMap = new Map<number, DepotGroup>();
  const byKey = new Map<string, CompareCandidate>();
  const add = (depotId: number, manifestId: string, branch: string) => {
    const group = groupMap.get(depotId);
    if (!group) return;
    const mk = `${depotId}/${manifestId}`;
    let cand = byKey.get(mk);
    if (!cand) {
      cand = {
        key: diffTargetKey(depotId, manifestId, currentDepotId),
        depotId,
        manifestId,
        branch,
        branches: [],
        creationTime: creationByKey.get(mk) ?? 0,
        isCurrent: depotId === currentDepotId && manifestId === currentManifestId,
      };
      byKey.set(mk, cand);
      group.candidates.push(cand);
    }
    if (!cand.branches.includes(branch)) cand.branches.push(branch);
  };
  for (const d of appInfo.depots) {
    const tag = [d.oslist, d.osarch, d.language].filter(Boolean).join(" · ");
    const label = tag ? `depot ${d.depot_id} · ${tag}` : `depot ${d.depot_id}`;
    groupMap.set(d.depot_id, { depotId: d.depot_id, label, candidates: [] });
    for (const m of d.manifests) add(d.depot_id, m.manifest_id, m.branch);
  }
  for (const e of extras) add(e.depot_id, e.manifest_id, e.branch ?? "public");
  for (const g of groupMap.values()) {
    g.candidates.sort((a, b) => {
      if (b.creationTime !== a.creationTime) return b.creationTime - a.creationTime;
      return a.manifestId.localeCompare(b.manifestId);
    });
  }
  return [...groupMap.values()]
    .filter((g) => g.candidates.length > 0)
    .sort((a, b) => {
      if (a.depotId === currentDepotId) return -1;
      if (b.depotId === currentDepotId) return 1;
      return a.depotId - b.depotId;
    });
}

/// Drop branches the user filtered out, then prune now-empty depots
/// (keeping "this depot" so the right pane can explain the emptiness).
/// The current manifest is always kept as a "you are here" anchor.
export function filterGroupsByBranch(
  groups: DepotGroup[],
  hidden: Set<string>,
  currentDepotId: number,
): DepotGroup[] {
  if (hidden.size === 0) return groups;
  return groups
    .map((g) => ({
      ...g,
      candidates: g.candidates
        // Keep a gid if *any* of its branches is visible — filtering only
        // the representative branch would drop a gid that's also reachable
        // via a visible branch (e.g. shown as public-beta but also public).
        .filter((c) => c.isCurrent || c.branches.some((b) => !hidden.has(b)))
        // …and label/link it with a visible branch so the row makes sense
        // under the current filter.
        .map((c) =>
          c.isCurrent || !hidden.has(c.branch)
            ? c
            : { ...c, branch: c.branches.find((b) => !hidden.has(b)) ?? c.branch },
        ),
    }))
    .filter((g) => g.depotId === currentDepotId || g.candidates.length > 0);
}
