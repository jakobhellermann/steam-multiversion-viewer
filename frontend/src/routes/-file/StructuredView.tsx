// TODO(ai-review): review for style and correctness
import { useQuery } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { getRouteApi } from "@tanstack/react-router";
import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";

import { fetchFileStructured, fetchStructuredNodeContent, type StructuredNode } from "../../api";
import { langForMime } from "../../lib/syntax";
import { HighlightedPre } from "./FilePreview";
import type { FileLocator } from "./types";

const fileRoute = getRouteApi("/apps/$appid/depots/$depotId/manifests/$manifestId_/file");

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

  return <Tree root={tree.data.root} locator={locator} showHeader={showHeader} />;
}

// DOM-id helper. Node ids can contain `:` / `.` / `<` / `>` which are
// valid in HTML id attributes but awkward to escape in querySelector;
// `CSS.escape` makes the lookup in the focus effect safe.
function rowDomId(treeUid: string, nodeId: string) {
  return `${treeUid}-${nodeId}`;
}

function Tree({
  root,
  locator,
  showHeader,
}: {
  root: StructuredNode;
  locator: FileLocator;
  showHeader: boolean;
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
  // Selection (what drives the right pane) tracks focus 1:1 — moving
  // through the tree previews each row's content as you go.
  const [focusedId, setFocusedId] = useState<string>(root.id);
  const selectedId: string | null = focusedId;

  // --- Search + facet filter -------------------------------------------------
  const search = fileRoute.useSearch();
  const navigate = fileRoute.useNavigate();
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
  const setQuery = useCallback(
    (next: string) => {
      navigate({
        search: (prev) => ({ ...prev, q: next.trim() === "" ? undefined : next }),
        replace: true,
      });
    },
    [navigate],
  );
  const setFacet = useCallback(
    (key: string, values: Set<string>) => {
      const param = `f.${key}` as const;
      navigate({
        search: (prev) => ({
          ...prev,
          [param]: values.size === 0 ? undefined : [...values].sort().join(","),
        }),
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

  // Per-node "passes all filters" set. `null` means "no filter is
  // active, show everything". Otherwise: a node passes when its own
  // label/facets match AND every facet whitelist allows it.
  const directMatches = useMemo(() => {
    if (query.trim() === "" && facetWhitelist.size === 0) return null;
    const tokens = query.toLowerCase().split(/\s+/).filter(Boolean);
    const set = new Set<string>();
    walk(root, (n) => {
      if (nodeMatches(n, tokens, facetWhitelist)) set.add(n.id);
    });
    return set;
  }, [root, query, facetWhitelist]);

  // Visibility set: a node is visible if it matches, or any descendant
  // matches (so its row stays as a navigable parent). Computed bottom-up
  // in one walk.
  const visibleSet = useMemo(() => {
    if (directMatches == null) return null;
    const out = new Set<string>();
    collectVisible(root, directMatches, out);
    return out;
  }, [root, directMatches]);

  // While filtering, auto-expand every still-visible parent so users
  // don't have to click through to see matches. User's explicit
  // expand-state is preserved separately; we OR the two together.
  const effectiveExpanded = useMemo(() => {
    if (visibleSet == null) return expandedIds;
    const out = new Set(expandedIds);
    walk(root, (n) => {
      if (visibleSet.has(n.id) && n.children.length > 0) out.add(n.id);
    });
    return out;
  }, [expandedIds, visibleSet, root]);

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

  const parentById = useMemo(() => {
    const map = new Map<string, string>();
    walk(root, (n) => {
      for (const c of n.children) map.set(c.id, n.id);
    });
    return map;
  }, [root]);

  const setExpanded = useCallback((id: string, next: boolean) => {
    setExpandedIds((prev) => {
      const out = new Set(prev);
      if (next) out.add(id);
      else out.delete(id);
      return out;
    });
  }, []);

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
  }, [locator.path]);

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLDivElement>) => {
      // Follow the WAI-ARIA tree keyboard pattern:
      // https://www.w3.org/WAI/ARIA/apg/patterns/treeview/
      const idx = focusedIdx;
      if (idx < 0) return;
      const row = visibleRows[idx];
      const hasChildren = row.node.children.length > 0;
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
      setFocusedId(id);
      const row = visibleRows.find((r) => r.node.id === id);
      if (row && row.node.children.length > 0) {
        setExpanded(id, !effectiveExpanded.has(id));
      }
    },
    [visibleRows, effectiveExpanded, setExpanded],
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
    <div className="grid min-h-0 flex-1 grid-cols-1 gap-4 lg:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]">
      <div className="flex min-h-0 flex-col gap-2">
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
          className="min-h-0 flex-1 overflow-auto rounded border border-slate-800 bg-slate-950 p-2 focus:outline-none focus:ring-1 focus:ring-sky-600/40"
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
      </div>
      <div className="h-full overflow-auto rounded border border-slate-800 bg-slate-950 p-3">
        {selectedId ? (
          <NodeContentPanel locator={locator} nodeId={selectedId} />
        ) : (
          <p className="text-sm text-slate-500">Pick a node to inspect.</p>
        )}
      </div>
    </div>
  );
}

type VisibleRow = { node: StructuredNode; depth: number };

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
  out.push({ node, depth });
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
/// its descendants is visible. Returns whether `node` was added so
/// callers can short-circuit.
function collectVisible(node: StructuredNode, direct: Set<string>, out: Set<string>): boolean {
  let anyChild = false;
  for (const c of node.children) {
    if (collectVisible(c, direct, out)) anyChild = true;
  }
  if (anyChild || direct.has(node.id)) {
    out.add(node.id);
    return true;
  }
  return false;
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
  const { node, depth } = row;
  const hasChildren = node.children.length > 0;
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
      className={`flex w-full cursor-default items-baseline gap-1 text-sm leading-6 select-none hover:text-sky-300 ${
        selected ? "rounded bg-slate-800 text-sky-200" : ""
      } ${focused ? "ring-1 ring-sky-600/60 ring-inset" : ""}`}
      style={{ paddingLeft: `${depth * 12}px` }}
    >
      <span
        aria-hidden="true"
        className={`inline-block w-3 text-slate-500 ${hasChildren ? "" : "opacity-30"}`}
      >
        {hasChildren ? (expanded ? "▼︎" : "▶︎") : "·"}
      </span>
      <span className="flex-1 truncate">
        <span>{node.label}</span>
        {node.badge && <span className="ml-2 text-slate-500">{node.badge}</span>}
      </span>
    </div>
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
        <div className="absolute top-full right-0 z-10 mt-1 max-h-80 w-56 overflow-auto rounded border border-slate-700 bg-slate-900 shadow-lg">
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

function NodeContentPanel({ locator, nodeId }: { locator: FileLocator; nodeId: string }) {
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
    staleTime: Infinity,
    // Keep the previous node's text on screen while a new node loads —
    // avoids a "Loading…" flash for the typical sub-frame fetch.
    placeholderData: (prev) => prev,
  });

  if (content.error) {
    return (
      <p className="text-sm text-red-300">
        {content.error instanceof Error ? content.error.message : String(content.error)}
      </p>
    );
  }
  if (!content.data || content.data.text.length === 0) {
    return null;
  }
  return <HighlightedPre code={content.data.text} lang={langForMime(content.data.mime)} />;
}
