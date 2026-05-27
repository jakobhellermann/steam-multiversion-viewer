// TODO(ai-review): review for style and correctness
import { useQuery } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useNavigate, useRouter, useSearch } from "@tanstack/react-router";
import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";

import {
  fetchFileStructured,
  fetchStructuredNodeContent,
  type NodeStatus,
  type StructuredNode,
} from "../../api";
import { langForMime } from "../../lib/syntax";
import { HighlightedPre } from "./FilePreview";
import { makePostProcess } from "./markers";
import type { FileLocator } from "./types";

/// Render the backend-built structured tree for a file. Lazy: each
/// node's content is fetched on click. The tree itself comes from one
/// call to `/file/structured`; subsequent re-renders reuse the cached
/// tree thanks to react-query.
export function StructuredView({
  locator,
  showHeader = true,
}: {
  locator: FileLocator;
  showHeader?: boolean;
}) {
  const tree = useQuery({
    queryKey: [
      "file-structured",
      locator.appid,
      locator.depotId,
      locator.manifestId,
      locator.branch,
      locator.path,
    ],
    queryFn: () =>
      fetchFileStructured(
        locator.appid,
        locator.depotId,
        locator.manifestId,
        locator.branch,
        locator.path,
      ),
    // Backend sends `Cache-Control: immutable`, so any re-fetch after
    // react-query's default GC hits the browser disk cache. Default
    // `gcTime` (5 min) is fine.
    staleTime: Infinity,
  });

  if (tree.isPending) {
    return <p className="text-sm text-slate-500">Building structured view…</p>;
  }
  if (tree.error) {
    return (
      <p className="text-sm text-red-300">
        Structured view failed:{" "}
        {tree.error instanceof Error ? tree.error.message : String(tree.error)}
      </p>
    );
  }

  // Rebuild the tree component when the file under us changes —
  // expanded-set, focus, virtualizer scroll position etc are all
  // per-file, so a fresh React instance is the right semantics.
  // Otherwise tanstack-router happily keeps the previous file's state
  // around when only the `?path=` search param changes.
  return (
    <Tree
      key={locator.path}
      root={tree.data.root}
      showHeader={showHeader}
      mountKey={locator.path}
      renderContent={({ node, isInTree, onHashTarget }) => (
        <NodeContentPanel
          locator={locator}
          nodeId={node.id}
          isInTree={isInTree}
          onHashTarget={onHashTarget}
        />
      )}
    />
  );
}

// DOM-id helper. Node ids can contain `:` / `.` / `<` / `>` which are
// valid in HTML id attributes but awkward to escape in querySelector;
// `CSS.escape` makes the lookup in the focus effect safe.
function rowDomId(treeUid: string, nodeId: string) {
  return `${treeUid}-${nodeId}`;
}

/// Args passed to the `renderContent` prop. The tree component owns
/// selection state and threads it through so the content pane stays
/// in lockstep with what's focused in the tree, without the tree
/// itself knowing how the content is fetched / rendered.
export type TreeContentRenderArgs = {
  node: StructuredNode;
  isInTree: (id: string) => boolean;
  /// Called by the content pane when it wants to deep-link to another
  /// row (e.g. a pptr click). Mirrors what the hashchange listener
  /// does — expand ancestors + focus the target.
  onHashTarget: (id: string) => void;
};

export function Tree({
  root,
  showHeader,
  renderContent,
  mountKey,
}: {
  root: StructuredNode;
  showHeader: boolean;
  /// Renders the right-hand content pane for the currently-selected
  /// row. Default `StructuredView` wires this up to
  /// `NodeContentPanel` (locator-based fetch); the diff variant uses
  /// its own renderer that pairs both sides.
  renderContent: (args: TreeContentRenderArgs) => React.ReactNode;
  /// Identifier that changes whenever the file under the tree
  /// changes. Used to re-focus the tree container — `Tree` itself
  /// has no notion of "the current file", just a root.
  mountKey: string;
}) {
  // IMPORTANT: this component must stay format-agnostic. Don't add
  // logic that branches on a node's `kind` or `id` value — anything
  // that needs to differ per file format belongs in the backend
  // builder, expressed through generic node fields (`default_collapsed`,
  // `facets`). Today's rule: expand everything by default, honor the
  // backend's collapse hint, prune by search/facets when active.
  const initiallyExpanded = useMemo(() => {
    const ids = new Set<string>();
    walk(root, (n) => {
      if (!n.default_collapsed) ids.add(n.id);
    });
    return ids;
  }, [root]);

  const [expandedIds, setExpandedIds] = useState<Set<string>>(initiallyExpanded);
  // User-driven collapse override during filtering. The filter
  // auto-expands every parent of a match (see `effectiveExpanded`),
  // so plain `setExpanded(id, false)` would be a no-op. Entries here
  // beat that auto-expansion — the node stays closed until the user
  // opens it explicitly or clears the filter.
  const [collapseOverride, setCollapseOverride] = useState<Set<string>>(new Set());
  // A `#obj:N` deep-link target that should be visible regardless of
  // the active filter — so clicking a PPtr link to something outside
  // the current search still lands on it. Cleared by clearing the
  // hash or selecting a different row.
  const [hashTarget, setHashTarget] = useState<string | null>(() =>
    typeof window === "undefined"
      ? null
      : decodeURIComponent(window.location.hash.replace(/^#/, "")) || null,
  );
  // Selection (what drives the right pane) tracks focus 1:1 — moving
  // through the tree previews each row's content as you go.
  const [focusedId, setFocusedId] = useState<string>(root.id);
  const selectedId: string | null = focusedId;
  // Flat lookup so `renderContent` gets the whole node object —
  // diff-mode reads `status` and `has_content` off it, the non-diff
  // renderer just needs the id.
  const nodeById = useMemo(() => {
    const map = new Map<string, StructuredNode>();
    walk(root, (n) => map.set(n.id, n));
    return map;
  }, [root]);
  const selectedNode = selectedId ? (nodeById.get(selectedId) ?? null) : null;

  // --- Search + facet filter -------------------------------------------------
  // `strict: false` lets us pull search params without binding to a
  // specific route — same component is mounted from the file route
  // and the (sibling) diff route.
  const search = useSearch({ strict: false }) as Record<string, string | undefined>;
  const navigate = useNavigate();
  const query = search.q ?? "";
  // Pull all `f.<key>` entries out of the URL search params into a
  // {key -> Set<value>} whitelist. Empty = no filter for that key.
  const facetWhitelist = useMemo(() => {
    const out = new Map<string, Set<string>>();
    for (const [k, v] of Object.entries(search)) {
      if (!k.startsWith("f.") || typeof v !== "string" || !v) continue;
      out.set(k.slice(2), new Set(v.split(",").filter(Boolean)));
    }
    return out;
  }, [search]);
  // Navigate without route binding — `to: "."` keeps us on whichever
  // route the component is mounted under (file or diff). The search
  // reducers' types collapse to `never` when the router can't pick a
  // single concrete route, so we cast the closure shape per call.
  const setQuery = useCallback(
    (next: string) => {
      navigate({
        to: ".",
        search: ((prev: Record<string, string | undefined>) => ({
          ...prev,
          q: next.trim() === "" ? undefined : next,
        })) as never,
        replace: true,
      });
    },
    [navigate],
  );
  const setFacet = useCallback(
    (key: string, values: Set<string>) => {
      const param = `f.${key}` as const;
      navigate({
        to: ".",
        search: ((prev: Record<string, string | undefined>) => ({
          ...prev,
          [param]: values.size === 0 ? undefined : [...values].sort().join(","),
        })) as never,
        replace: true,
      });
    },
    [navigate],
  );

  // Available facet keys + per-value counts, computed over *other*
  // filters active — checking off a value mustn't make all sibling
  // values vanish from the dropdown. So we reapply every filter except
  // this key, then count facet hits in what's left.
  const facetSummary = useMemo(() => {
    return computeFacetSummary(root, query, facetWhitelist);
  }, [root, query, facetWhitelist]);

  const filterActive = query.trim() !== "" || facetWhitelist.size > 0;

  // Per-node "passes all filters" set. `null` means no filter is
  // active (we still need a visibility set, see below, but match-
  // counters and auto-expand shouldn't kick in).
  const directMatches = useMemo(() => {
    if (!filterActive) return null;
    const tokens = query.toLowerCase().split(/\s+/).filter(Boolean);
    const set = new Set<string>();
    walk(root, (n) => {
      if (nodeMatches(n, tokens, facetWhitelist)) set.add(n.id);
    });
    return set;
  }, [filterActive, root, query, facetWhitelist]);

  // Visibility set, computed in two modes:
  // - Filter active: matches + every ancestor of a match.
  // - No filter: every node except those tagged `hide_unless_matched`
  //   (and their descendants — propagated by `collectHidden`).
  // The "default-hide" path is what makes nested .NET types show up
  // only when the user searches for them.
  // Pre-build a parent lookup so we can splice deep-link targets back
  // into visibility/expansion sets cheaply.
  const parentByIdEarly = useMemo(() => {
    const map = new Map<string, string>();
    walk(root, (n) => {
      for (const c of n.children) map.set(c.id, n.id);
    });
    return map;
  }, [root]);

  const visibleSet = useMemo(() => {
    const out = new Set<string>();
    if (directMatches != null) {
      collectVisible(root, directMatches, out);
    } else {
      collectDefaultVisible(root, false, out);
    }
    // Splice the deep-link target (and its ancestors) in — even if a
    // filter would otherwise hide it. Without this, navigating to a
    // PPtr from a filtered view snaps focus straight back onto the
    // first row in the (still-filtered) tree.
    if (hashTarget) {
      let cur: string | undefined = hashTarget;
      while (cur && !out.has(cur)) {
        out.add(cur);
        cur = parentByIdEarly.get(cur);
      }
    }
    return out;
  }, [root, directMatches, hashTarget, parentByIdEarly]);

  // Drop any collapse-overrides once the filter is gone — they only
  // make sense as a counter to auto-expansion.
  useEffect(() => {
    if (!filterActive && collapseOverride.size > 0) {
      setCollapseOverride(new Set());
    }
  }, [filterActive, collapseOverride.size]);

  // While filtering, auto-expand every still-visible parent so users
  // don't have to click through to see matches. User's explicit
  // collapse-overrides take precedence — those nodes stay closed.
  const effectiveExpanded = useMemo(() => {
    if (!filterActive) return expandedIds;
    const out = new Set(expandedIds);
    walk(root, (n) => {
      if (visibleSet.has(n.id) && n.children.length > 0 && !collapseOverride.has(n.id)) {
        out.add(n.id);
      }
    });
    for (const id of collapseOverride) out.delete(id);
    return out;
  }, [filterActive, expandedIds, visibleSet, root, collapseOverride]);

  // Flat list of visible rows in display order — collapsed subtrees
  // are skipped. Recomputed whenever expansion or filtering changes.
  const visibleRows = useMemo(() => {
    const rows: VisibleRow[] = [];
    walkVisible(root, effectiveExpanded, visibleSet, 0, rows);
    return rows;
  }, [root, effectiveExpanded, visibleSet]);

  // Keep focus inside the visible set — if the focused node got
  // collapsed away, jump to its nearest visible ancestor (or the root).
  useEffect(() => {
    if (!visibleRows.some((r) => r.node.id === focusedId)) {
      setFocusedId(visibleRows[0]?.node.id ?? root.id);
    }
  }, [visibleRows, focusedId, root.id]);

  // When the active filter changes (e.g. a `?q=…` deep link or a
  // toggled facet) slide focus onto the first match so the right pane
  // shows something useful straight away. Keyed on `directMatches`
  // identity so clicking a non-leaf ancestor doesn't re-trigger the
  // jump — that effect would otherwise fight the user every time.
  useEffect(() => {
    if (directMatches == null) return;
    // A `#obj:N` deep link wins over the "snap to first match" rule —
    // the user explicitly asked to land there.
    if (hashTarget) return;
    const firstMatch = visibleRows.find((r) => directMatches.has(r.node.id));
    if (firstMatch) setFocusedId(firstMatch.node.id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [directMatches]);

  const parentById = parentByIdEarly;

  // Stable across renders so memoised consumers (e.g. `makePostProcess`
  // in the preview pane) don't get a fresh function reference every
  // time the tree state ticks.
  const isInTree = useCallback(
    (id: string) => id === root.id || parentById.has(id),
    [root.id, parentById],
  );

  const setExpanded = useCallback(
    (id: string, next: boolean) => {
      // With a filter active, `effectiveExpanded` auto-includes every
      // visible parent. To actually collapse a node the user has to
      // win over that auto-expansion via `collapseOverride`; opening
      // a previously-overridden node just drops it from the override.
      if (filterActive) {
        setCollapseOverride((prev) => {
          const out = new Set(prev);
          if (next) out.delete(id);
          else out.add(id);
          return out;
        });
      }
      setExpandedIds((prev) => {
        const out = new Set(prev);
        if (next) out.add(id);
        else out.delete(id);
        return out;
      });
    },
    [filterActive],
  );

  const treeRef = useRef<HTMLDivElement | null>(null);
  // Stable prefix so each row's DOM id is unique across multiple
  // structured views on the same page (and across remounts).
  const treeUid = useId();
  // Virtualize so a fully-expanded scene (≈8k rows) doesn't try to
  // mount every <div role=treeitem> on each toggle. Row size is fixed
  // by `leading-6` on TreeRow (24px); we don't bother measuring.
  const virtualizer = useVirtualizer({
    count: visibleRows.length,
    getScrollElement: () => treeRef.current,
    estimateSize: () => 24,
    overscan: 12,
  });

  // Bring the focused row into view when arrow-keys move it.
  const focusedIdx = useMemo(
    () => visibleRows.findIndex((r) => r.node.id === focusedId),
    [visibleRows, focusedId],
  );
  useEffect(() => {
    if (focusedIdx >= 0) virtualizer.scrollToIndex(focusedIdx, { align: "auto" });
  }, [focusedIdx, virtualizer]);

  // Move keyboard focus to the tree container as soon as it mounts so
  // arrow keys drive navigation instead of scrolling the page.
  useEffect(() => {
    treeRef.current?.focus({ preventScroll: true });
  }, [mountKey]);

  // Snap focus + expansion onto a `#obj:<path-id>` deep-link target.
  // Used both by the hashchange listener (browser-initiated nav, e.g.
  // back/forward) and by same-file PPtr clicks in the preview, which
  // update `location.hash` via `history.replaceState` and therefore
  // never fire `hashchange` themselves.
  const jumpToHashTarget = useCallback(
    (id: string) => {
      // Track the target so `visibleSet` keeps it (and its ancestors)
      // visible even if a filter would normally hide it.
      setHashTarget(id);
      // Open every ancestor so the row is actually visible.
      setExpandedIds((prev) => {
        const out = new Set(prev);
        let cur = parentById.get(id);
        while (cur) {
          out.add(cur);
          cur = parentById.get(cur);
        }
        return out;
      });
      setFocusedId(id);
    },
    [parentById],
  );

  // Honor `#obj:<path-id>` hash links — the unity object dump turns
  // local PPtrs into anchors, and the browser's default click on those
  // updates `location.hash`. Listen and snap focus onto the target,
  // expanding ancestors as needed.
  useEffect(() => {
    const idsInTree = new Set<string>();
    walk(root, (n) => idsInTree.add(n.id));
    // Track whether we've ever observed a non-empty hash for this
    // mount — that lets us tell apart "page just loaded without a hash"
    // (do nothing) from "user navigated back from a #obj:N entry"
    // (snap to root so the preview clears).
    let everSawHash = false;
    const onHashChange = () => {
      const id = decodeURIComponent(window.location.hash.replace(/^#/, ""));
      if (!id) {
        if (!everSawHash) return;
        setHashTarget(null);
        setFocusedId(root.id);
        return;
      }
      everSawHash = true;
      if (!idsInTree.has(id)) return;
      jumpToHashTarget(id);
    };
    onHashChange();
    window.addEventListener("hashchange", onHashChange);
    return () => window.removeEventListener("hashchange", onHashChange);
  }, [root, jumpToHashTarget]);

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLDivElement>) => {
      // Follow the WAI-ARIA tree keyboard pattern:
      // https://www.w3.org/WAI/ARIA/apg/patterns/treeview/
      const idx = focusedIdx;
      if (idx < 0) return;
      const row = visibleRows[idx];
      const hasChildren = row.hasVisibleChildren;
      // Treat filter-auto-expanded rows as expanded for nav purposes
      // (arrow-right moves to child instead of opening). Collapsing
      // while a filter is active is a no-op — clear the filter to
      // explore structure.
      const expanded = effectiveExpanded.has(row.node.id);
      switch (e.key) {
        case "ArrowDown": {
          if (idx + 1 < visibleRows.length) setFocusedId(visibleRows[idx + 1].node.id);
          break;
        }
        case "ArrowUp": {
          if (idx > 0) setFocusedId(visibleRows[idx - 1].node.id);
          break;
        }
        case "ArrowRight": {
          if (hasChildren && !expanded) {
            setExpanded(row.node.id, true);
          } else if (hasChildren && expanded && idx + 1 < visibleRows.length) {
            setFocusedId(visibleRows[idx + 1].node.id);
          }
          break;
        }
        case "ArrowLeft": {
          if (hasChildren && expanded) {
            setExpanded(row.node.id, false);
          } else {
            const parent = parentById.get(row.node.id);
            if (parent) setFocusedId(parent);
          }
          break;
        }
        case "Home": {
          setFocusedId(visibleRows[0].node.id);
          break;
        }
        case "End": {
          setFocusedId(visibleRows[visibleRows.length - 1].node.id);
          break;
        }
        case "Enter":
        case " ": {
          if (hasChildren) setExpanded(row.node.id, !expanded);
          break;
        }
        default:
          return;
      }
      e.preventDefault();
    },
    [visibleRows, focusedIdx, effectiveExpanded, parentById, setExpanded],
  );

  const activateRow = useCallback(
    (id: string) => {
      const row = visibleRows.find((r) => r.node.id === id);
      const hasChildren = row != null && row.hasVisibleChildren;
      const wasSelected = id === focusedId;
      const isExpanded = effectiveExpanded.has(id);
      // Reflect the selection in the URL hash so the page is shareable
      // and survives reloads, but use replaceState — every click in the
      // tree producing a history entry was too noisy on back/forward.
      if (window.location.hash !== `#${id}`) {
        history.replaceState(null, "", `#${id}`);
        setHashTarget(id);
      }
      setFocusedId(id);
      if (!hasChildren) return;
      // Click semantics: collapsed → open. Expanded but not yet
      // selected → just select (so users can preview a header without
      // losing its open state). Expanded *and* already selected →
      // collapse (second click on the same header closes it).
      if (!isExpanded) {
        setExpanded(id, true);
      } else if (wasSelected) {
        setExpanded(id, false);
      }
    },
    [visibleRows, effectiveExpanded, focusedId, setExpanded],
  );

  const matchedCount = directMatches?.size ?? null;
  const totalCount = useMemo(() => {
    let n = 0;
    walk(root, () => {
      n += 1;
    });
    return n;
  }, [root]);

  // Fill whatever vertical space the parent gives us — the file
  // route is `flex flex-col` with a viewport-fixed height, so this
  // collapses to the remaining row automatically. Both panes scroll
  // internally below.
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2">
      {showHeader && (
        <div className="flex items-baseline gap-3">
          <h2 className="text-sm font-semibold text-slate-400">Preview</h2>
          <span className="text-xs text-slate-500 tabular-nums">
            {matchedCount != null ? (
              <>
                {matchedCount.toLocaleString()}
                <span className="text-slate-700"> / {totalCount.toLocaleString()}</span>
              </>
            ) : (
              totalCount.toLocaleString()
            )}
          </span>
        </div>
      )}
      <div className="flex items-center gap-2">
        <div className="relative flex-1">
          <input
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Filter by label…"
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
        {facetSummary.map(({ key, counts }) => (
          <FacetDropdown
            key={key}
            facetKey={key}
            counts={counts}
            selected={facetWhitelist.get(key) ?? EMPTY_SET}
            onChange={(next) => setFacet(key, next)}
          />
        ))}
      </div>
      <div className="grid min-h-0 flex-1 grid-cols-1 gap-4 lg:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]">
        <div
          ref={treeRef}
          role="tree"
          aria-label="Structured file contents"
          // Container holds focus; rows are addressed via
          // aria-activedescendant. This way arrow keys reach onKeyDown
          // even when the user hasn't clicked into the tree yet — focus
          // moves cleanly from <body> to here on first tab/click.
          tabIndex={0}
          aria-activedescendant={focusedId ? rowDomId(treeUid, focusedId) : undefined}
          onKeyDown={onKeyDown}
          className="h-full overflow-auto rounded border border-slate-800 bg-slate-950 p-2 focus:ring-1 focus:ring-sky-600/40 focus:outline-none"
        >
          {/* Spacer keeps the scroll thumb honest while we render only
              the rows in view. Items are absolute-positioned by the
              virtualizer's reported offset. */}
          <div className="relative" style={{ height: virtualizer.getTotalSize() }}>
            {virtualizer.getVirtualItems().map((vi) => {
              const row = visibleRows[vi.index];
              return (
                <div
                  key={row.node.id}
                  ref={virtualizer.measureElement}
                  data-index={vi.index}
                  style={{
                    position: "absolute",
                    top: 0,
                    left: 0,
                    right: 0,
                    transform: `translateY(${vi.start}px)`,
                  }}
                >
                  <TreeRow
                    row={row}
                    expanded={effectiveExpanded.has(row.node.id)}
                    focused={row.node.id === focusedId}
                    selected={row.node.id === selectedId}
                    domId={rowDomId(treeUid, row.node.id)}
                    onActivate={activateRow}
                  />
                </div>
              );
            })}
          </div>
        </div>
        <div className="h-full overflow-auto rounded border border-slate-800 bg-slate-950 p-3">
          {selectedNode ? (
            renderContent({
              node: selectedNode,
              isInTree,
              onHashTarget: jumpToHashTarget,
            })
          ) : (
            <p className="text-sm text-slate-500">Pick a node to inspect.</p>
          )}
        </div>
      </div>
    </div>
  );
}

type VisibleRow = {
  node: StructuredNode;
  depth: number;
  /// True iff *at least one* child of `node` survives the current
  /// visibility filter — drives the chevron, the expand/collapse
  /// keyboard handlers, and the click-to-toggle. Rows with raw
  /// `children.length > 0` but no visible ones (e.g. an outer .NET
  /// type whose nested members are hidden) get a leaf-style chevron.
  hasVisibleChildren: boolean;
};

function walk(node: StructuredNode, visit: (n: StructuredNode) => void) {
  visit(node);
  for (const c of node.children) walk(c, visit);
}

function walkVisible(
  node: StructuredNode,
  expanded: Set<string>,
  visibleSet: Set<string> | null,
  depth: number,
  out: VisibleRow[],
) {
  if (visibleSet != null && !visibleSet.has(node.id)) return;
  const hasVisibleChildren = node.children.some((c) => visibleSet == null || visibleSet.has(c.id));
  out.push({ node, depth, hasVisibleChildren });
  if (!expanded.has(node.id)) return;
  for (const c of node.children) walkVisible(c, expanded, visibleSet, depth + 1, out);
}

/// True iff `node` matches the search tokens and survives every active
/// facet whitelist. Tokens are matched AND-wise as substrings against
/// the lowercased `label` + `badge`. Each facet whitelist checks the
/// node's own `facets[key]` against the allowed value set; nodes
/// missing the key do NOT pass a whitelist for that key. Both passing
/// conditions are ANDed together.
function nodeMatches(
  node: StructuredNode,
  tokens: string[],
  whitelist: Map<string, Set<string>>,
): boolean {
  if (tokens.length > 0) {
    const hay = `${node.label.toLowerCase()} ${node.badge?.toLowerCase() ?? ""}`;
    if (!tokens.every((t) => hay.includes(t))) return false;
  }
  if (whitelist.size > 0) {
    for (const [key, allowed] of whitelist) {
      const v = node.facets?.[key];
      if (v == null || !allowed.has(v)) return false;
    }
  }
  return true;
}

/// Bottom-up: a node is visible if it directly matches or if any of
/// its descendants is visible. Container-shaped matches (those with
/// `include_descendants_on_match`) additionally pull their whole
/// subtree in so the user sees "the thing and what's inside it".
/// Returns whether `node` was added so callers can short-circuit.
function collectVisible(node: StructuredNode, direct: Set<string>, out: Set<string>): boolean {
  const selfMatched = direct.has(node.id);
  if (selfMatched && node.include_descendants_on_match) {
    walk(node, (n) => out.add(n.id));
    return true;
  }
  let anyChild = false;
  for (const c of node.children) {
    if (collectVisible(c, direct, out)) anyChild = true;
  }
  if (anyChild || selfMatched) {
    out.add(node.id);
    return true;
  }
  return false;
}

/// No-filter visibility: include every node except those tagged
/// `hide_unless_matched` (and everything under them). `hiddenAncestor`
/// is true once we've entered a hidden subtree, so the entire branch
/// stays out without re-checking the flag per node.
function collectDefaultVisible(node: StructuredNode, hiddenAncestor: boolean, out: Set<string>) {
  const hidden = hiddenAncestor || node.hide_unless_matched === true;
  if (!hidden) out.add(node.id);
  for (const c of node.children) collectDefaultVisible(c, hidden, out);
}

/// Per-facet-key value distribution shown in the filter dropdowns.
/// Counts are computed *excluding* this facet key's own whitelist —
/// otherwise checking off a value would make all sibling values vanish.
/// Tokens and all *other* facet whitelists still apply, so a paste-
/// restricted search shrinks the menu too.
type FacetSummary = { key: string; counts: [string, number][] }[];

function computeFacetSummary(
  root: StructuredNode,
  query: string,
  whitelist: Map<string, Set<string>>,
): FacetSummary {
  const tokens = query.toLowerCase().split(/\s+/).filter(Boolean);
  const keys = new Set<string>();
  walk(root, (n) => {
    if (n.facets) for (const k of Object.keys(n.facets)) keys.add(k);
  });
  const out: FacetSummary = [];
  for (const key of [...keys].sort()) {
    const restricted = new Map(whitelist);
    restricted.delete(key);
    const counts = new Map<string, number>();
    walk(root, (n) => {
      const v = n.facets?.[key];
      if (v == null) return;
      if (!nodeMatches(n, tokens, restricted)) return;
      counts.set(v, (counts.get(v) ?? 0) + 1);
    });
    out.push({
      key,
      counts: [...counts.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0])),
    });
  }
  return out;
}

function TreeRow({
  row,
  expanded,
  focused,
  selected,
  domId,
  onActivate,
}: {
  row: VisibleRow;
  expanded: boolean;
  focused: boolean;
  selected: boolean;
  domId: string;
  onActivate: (id: string) => void;
}) {
  const { node, depth, hasVisibleChildren: hasChildren } = row;
  // Status colouring is only set in diff trees — `node.status` stays
  // undefined elsewhere. `changed` rows are unstyled: the diff tree
  // already prunes unchanged subtrees, so every leaf is by default a
  // change — the bulk of rows being yellow would be visual noise.
  // Reserve colour + glyph for the structurally distinct cases
  // (added / removed) where the call-out is actually informative.
  const statusLabelClass =
    node.status === "added" ? "text-emerald-300" : node.status === "removed" ? "text-rose-300" : "";
  return (
    <div
      id={domId}
      role="treeitem"
      // Focus lives on the tree container; rows are referenced via
      // aria-activedescendant. No per-row tabindex.
      aria-level={depth + 1}
      aria-expanded={hasChildren ? expanded : undefined}
      aria-selected={selected}
      data-node-id={node.id}
      onClick={() => onActivate(node.id)}
      // Native browser tooltip surfaces the raw node id (e.g.
      // `obj:894`, `type:Foo.Bar`) — handy when reproducing an issue
      // against the backend directly.
      title={node.id}
      className={`flex w-full cursor-default items-baseline gap-1 rounded text-sm leading-6 select-none ${
        selected ? "bg-slate-800 text-sky-200" : "hover:bg-slate-800/50"
      } ${focused ? "ring-1 ring-sky-600/60 ring-inset" : ""}`}
      style={{ paddingLeft: `${depth * 12}px` }}
    >
      <span
        aria-hidden="true"
        className={`inline-block w-3 text-slate-500 ${hasChildren ? "" : "opacity-30"}`}
      >
        {hasChildren ? (expanded ? "▼︎" : "▶︎") : "·"}
      </span>
      <DiffStatusGlyph status={node.status} />
      <span className={`flex-1 truncate ${statusLabelClass}`}>
        <span>{node.label}</span>
        {node.badge && <span className="ml-2 text-slate-500">{node.badge}</span>}
      </span>
    </div>
  );
}

/// Inline `+` / `−` glyph in diff trees — fixed-width container so
/// labels line up across rows. Returns an empty (but space-holding)
/// span for `changed` / `unchanged` / non-diff trees so column
/// alignment is preserved while the row stays visually quiet.
function DiffStatusGlyph({ status }: { status: NodeStatus | undefined }) {
  const glyph = status === "added" ? "+" : status === "removed" ? "−" : "";
  const cls = status === "added" ? "text-emerald-400" : status === "removed" ? "text-rose-400" : "";
  return (
    <span aria-hidden="true" className={`inline-block w-3 text-center ${cls}`}>
      {glyph}
    </span>
  );
}

const EMPTY_SET: ReadonlySet<string> = new Set();

function FacetDropdown({
  facetKey,
  counts,
  selected,
  onChange,
}: {
  facetKey: string;
  counts: [string, number][];
  selected: ReadonlySet<string>;
  onChange: (next: Set<string>) => void;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  // Stash the popup's scroll offset whenever it closes so reopening
  // brings the user back to where they were in a long value list.
  const scrollTopRef = useRef(0);
  const popupRef = useCallback((el: HTMLDivElement | null) => {
    if (el) el.scrollTop = scrollTopRef.current;
  }, []);
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
  const toggle = (v: string) => {
    const next = new Set(selected);
    if (next.has(v)) next.delete(v);
    else next.add(v);
    onChange(next);
  };
  const count = selected.size;
  return (
    <div ref={rootRef} className="relative">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className={`rounded border px-3 py-1.5 text-sm whitespace-nowrap capitalize ${
          count > 0
            ? "border-sky-700 bg-sky-950/40 text-sky-200 hover:bg-sky-900/40"
            : "border-slate-700 bg-slate-900 text-slate-300 hover:border-slate-600"
        }`}
        aria-haspopup="dialog"
        aria-expanded={open}
      >
        {facetKey}
        {count > 0 && <span className="ml-1.5 tabular-nums">({count})</span>}
      </button>
      {open && (
        <div
          ref={popupRef}
          onScroll={(e) => {
            scrollTopRef.current = e.currentTarget.scrollTop;
          }}
          className="absolute top-full right-0 z-10 mt-1 max-h-80 w-56 overflow-auto rounded border border-slate-700 bg-slate-900 shadow-lg"
        >
          <div className="sticky top-0 flex items-center justify-between border-b border-slate-800 bg-slate-900 px-3 py-1.5 text-xs text-slate-400">
            <span className="tabular-nums">{counts.length} values</span>
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
            {counts.map(([v, n]) => {
              const checked = selected.has(v);
              return (
                <li key={v}>
                  <label
                    onMouseDown={(e) => e.preventDefault()}
                    className={`flex cursor-pointer items-center gap-2 px-3 py-1 text-sm select-none hover:bg-slate-800/60 ${
                      checked ? "text-sky-200" : "text-slate-300"
                    }`}
                  >
                    <input
                      type="checkbox"
                      checked={checked}
                      onChange={() => toggle(v)}
                      className="accent-sky-500"
                    />
                    <span className="flex-1 truncate">{v}</span>
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

export function NodeContentPanel({
  locator,
  nodeId,
  isInTree,
  onHashTarget,
}: {
  locator: FileLocator;
  nodeId: string;
  isInTree: (id: string) => boolean;
  onHashTarget: (id: string) => void;
}) {
  const content = useQuery({
    queryKey: [
      "file-structured-node",
      locator.appid,
      locator.depotId,
      locator.manifestId,
      locator.branch,
      locator.path,
      nodeId,
    ],
    queryFn: () =>
      fetchStructuredNodeContent(
        locator.appid,
        locator.depotId,
        locator.manifestId,
        locator.branch,
        locator.path,
        nodeId,
      ),
    // Same rationale as the tree query — backend's immutable
    // Cache-Control makes a re-fetch cheap.
    staleTime: Infinity,
    // No `placeholderData` — we keep prior content on screen ourselves
    // via `lastSettledRef`, which works across node changes too.
  });

  // We render whatever payload the most recent *settled* query
  // produced — data on success, message on error — and leave it
  // alone while a new node is still in-flight. react-query's per-key
  // cache doesn't carry across nodes, so stash the last settled
  // result ourselves.
  type Settled = { kind: "ok"; text: string; mime: string } | { kind: "err"; message: string };
  const lastSettledRef = useRef<Settled | null>(null);
  if (content.data) {
    lastSettledRef.current = {
      kind: "ok",
      text: content.data.text,
      mime: content.data.mime,
    };
  } else if (content.error) {
    lastSettledRef.current = {
      kind: "err",
      message: content.error instanceof Error ? content.error.message : String(content.error),
    };
  }
  const settled = lastSettledRef.current;
  const text = settled?.kind === "ok" ? settled.text : "";
  const mime = settled?.kind === "ok" ? settled.mime : "";
  const postProcess = useMemo(() => makePostProcess(locator, isInTree), [locator, isInTree]);
  const router = useRouter();
  // Delegated click — pptr anchors are either local hash refs (no
  // `href`, just `data-pptr-ref`) or full route links (`href` set to
  // the destination file + `#obj:N` hash). For the latter we route
  // through tanstack-router so the destination's loader runs and the
  // previous page stays mounted until data is ready.
  const onClick = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      const t = e.target as HTMLElement | null;
      if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || e.button !== 0) return;
      const a = t?.closest("a[data-pptr-ref]") as HTMLAnchorElement | null;
      if (!a) return;
      e.preventDefault();
      const ref = a.getAttribute("data-pptr-ref") ?? "";
      // External refs carry `data-pptr-file` + a route href; everything
      // else is a same-file hash jump. The `href` exists in both cases
      // (so hover/middle-click work) but we can't dispatch on it alone.
      if (a.hasAttribute("data-pptr-file")) {
        const href = a.getAttribute("href");
        if (href) {
          // SPA navigation via the router's history primitive — the
          // typed `navigate({to: rawPath})` path silently falls back
          // to a full reload when called from a non-route-bound
          // context (this component is mounted under both `/file`
          // and `/diff`, so we can't bind it to a single route id).
          router.history.push(href);
        }
        return;
      }
      // Same-file ref: only navigate the hash if the target actually
      // lives in the tree. Otherwise the click is a no-op — the
      // backend filters some pptrs out (self-transforms, etc) but
      // they still appear as `__PPTR__` sentinels in the json dump.
      if (!isInTree(ref)) return;
      if (window.location.hash !== `#${ref}`) {
        // Push (not replace) — a pptr click is an explicit jump the
        // user should be able to undo with the back button. Arrow-key
        // tree nav still uses replaceState in `activateRow` to avoid
        // History spam on every row.
        history.pushState(null, "", `#${ref}`);
        onHashTarget(ref);
      }
    },
    [router, isInTree, onHashTarget],
  );

  // Scope ⌘A / ctrl-A to the preview contents instead of letting the
  // browser select the whole page. Needs `tabIndex` so the wrapper can
  // receive focus + keyboard events; the click handler grabs focus on
  // any interaction inside the preview.
  const handleKeyDown = useCallback((e: React.KeyboardEvent<HTMLDivElement>) => {
    if ((e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey && e.key === "a") {
      e.preventDefault();
      window.getSelection()?.selectAllChildren(e.currentTarget);
    }
  }, []);
  const handleClick = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      // Focus the wrapper so the next ⌘A lands in our keydown handler;
      // `preventScroll` keeps clicks from jumping the long preview.
      // Skip if the user just made a text selection — in Firefox,
      // `element.focus()` collapses the live selection, so clicks that
      // end a drag-select would lose what was just highlighted.
      const sel = window.getSelection();
      const isSelecting = sel != null && !sel.isCollapsed && sel.toString().length > 0;
      if (!isSelecting) e.currentTarget.focus({ preventScroll: true });
      onClick(e);
    },
    [onClick],
  );

  if (!settled) return null;
  if (settled.kind === "err") {
    return <p className="text-sm text-red-300">{settled.message}</p>;
  }
  if (settled.text.length === 0) return null;
  return (
    <div tabIndex={-1} onClick={handleClick} onKeyDown={handleKeyDown} className="outline-none">
      <HighlightedPre code={text} lang={langForMime(mime)} bare postProcess={postProcess} />
    </div>
  );
}
