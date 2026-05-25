// TODO(ai-review): review for style and correctness
import { useQuery } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";

import { fetchFileStructured, fetchStructuredNodeContent, type StructuredNode } from "../../api";
import { langForMime } from "../../lib/syntax";
import { HighlightedPre } from "./FilePreview";
import type { FileLocator } from "./types";

/// Render the backend-built structured tree for a file. Lazy: each
/// node's content is fetched on click. The tree itself comes from one
/// call to `/file/structured`; subsequent re-renders reuse the cached
/// tree thanks to react-query.
export function StructuredView({ locator }: { locator: FileLocator }) {
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

  return <Tree root={tree.data.root} locator={locator} />;
}

// DOM-id helper. Node ids can contain `:` / `.` / `<` / `>` which are
// valid in HTML id attributes but awkward to escape in querySelector;
// `CSS.escape` makes the lookup in the focus effect safe.
function rowDomId(treeUid: string, nodeId: string) {
  return `${treeUid}-${nodeId}`;
}

function Tree({ root, locator }: { root: StructuredNode; locator: FileLocator }) {
  // IMPORTANT: this component must stay format-agnostic. Don't add
  // logic that branches on a node's `kind` or `id` value — anything
  // that needs to differ per file format belongs in the backend
  // builder, expressed through generic node fields (e.g.
  // `default_collapsed`). Today's rule: expand everything by default,
  // honor the backend's collapse hint.
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

  // Flat list of visible rows in display order — collapsed subtrees
  // are skipped. Recomputed whenever expansion changes.
  const visibleRows = useMemo(() => {
    const rows: VisibleRow[] = [];
    walkVisible(root, expandedIds, 0, rows);
    return rows;
  }, [root, expandedIds]);

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
      const expanded = expandedIds.has(row.node.id);
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
    [visibleRows, focusedIdx, expandedIds, parentById, setExpanded],
  );

  const activateRow = useCallback(
    (id: string) => {
      setFocusedId(id);
      const row = visibleRows.find((r) => r.node.id === id);
      if (row && row.node.children.length > 0) {
        setExpanded(id, !expandedIds.has(id));
      }
    },
    [visibleRows, expandedIds, setExpanded],
  );

  // Fill whatever vertical space the parent gives us — the file
  // route is `flex flex-col` with a viewport-fixed height, so this
  // collapses to the remaining row automatically. Both panes scroll
  // internally below.
  return (
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
        className="h-full overflow-auto rounded border border-slate-800 bg-slate-950 p-2 focus:outline-none focus:ring-1 focus:ring-sky-600/40"
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
                  expanded={expandedIds.has(row.node.id)}
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
  depth: number,
  out: VisibleRow[],
) {
  out.push({ node, depth });
  if (!expanded.has(node.id)) return;
  for (const c of node.children) walkVisible(c, expanded, depth + 1, out);
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
