// TODO(ai-review): review for style and correctness
import { useQuery } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

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
  // Bring the focused row into view when arrow-keys move it.
  useEffect(() => {
    const el = treeRef.current?.querySelector<HTMLElement>(
      `[data-node-id="${cssEscape(focusedId)}"]`,
    );
    el?.scrollIntoView({ block: "nearest" });
  }, [focusedId]);

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLDivElement>) => {
      // Follow the WAI-ARIA tree keyboard pattern:
      // https://www.w3.org/WAI/ARIA/apg/patterns/treeview/
      const idx = visibleRows.findIndex((r) => r.node.id === focusedId);
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
    [visibleRows, focusedId, expandedIds, parentById, setExpanded],
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
        tabIndex={-1}
        onKeyDown={onKeyDown}
        className="h-full overflow-auto rounded border border-slate-800 bg-slate-950 p-2 focus:outline-none"
      >
        {visibleRows.map((row) => (
          <TreeRow
            key={row.node.id}
            row={row}
            expanded={expandedIds.has(row.node.id)}
            focused={row.node.id === focusedId}
            selected={row.node.id === selectedId}
            onActivate={activateRow}
            onFocus={setFocusedId}
          />
        ))}
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
  onActivate,
  onFocus,
}: {
  row: VisibleRow;
  expanded: boolean;
  focused: boolean;
  selected: boolean;
  onActivate: (id: string) => void;
  onFocus: (id: string) => void;
}) {
  const { node, depth } = row;
  const hasChildren = node.children.length > 0;
  return (
    <div
      role="treeitem"
      // Roving tabindex: only the focused row participates in the page
      // tab order so screen readers and keyboard users land on the
      // user's last position when re-entering the tree.
      tabIndex={focused ? 0 : -1}
      aria-level={depth + 1}
      aria-expanded={hasChildren ? expanded : undefined}
      aria-selected={selected}
      data-node-id={node.id}
      onFocus={() => onFocus(node.id)}
      onClick={() => onActivate(node.id)}
      className={`flex w-full cursor-default items-baseline gap-1 text-sm leading-6 select-none hover:text-sky-300 focus:outline-none ${
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

/// Escape an id for use in a CSS attribute selector. The structured-id
/// scheme uses `:` which is a CSS combinator if unescaped.
function cssEscape(id: string): string {
  if (typeof CSS !== "undefined" && typeof CSS.escape === "function") {
    return CSS.escape(id);
  }
  return id.replace(/(["\\:.])/g, "\\$1");
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
