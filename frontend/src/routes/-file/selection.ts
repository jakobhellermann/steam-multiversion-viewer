// TODO(ai-review): review for style and correctness
//
// Shared selection helpers for the value-preview panes. Both the
// non-diff (`NodeContentPanel`) and diff (`DiffContentPane`) renderers
// wire these up so ⌘A scopes to the preview instead of the whole page,
// and a click focuses the pane so that ⌘A actually lands there.

/// Minimal shape of the keyboard event fields we read — lets the
/// helper be called with a React `KeyboardEvent` and unit-tested with a
/// plain object.
type SelectAllEvent = {
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  key: string;
  preventDefault: () => void;
  currentTarget: Node;
};

/// ⌘A / Ctrl+A inside a content pane should select only that pane's
/// contents, not the whole document. Returns true when it handled (and
/// prevented) the event. Plain `a`, or with Alt/Shift, is left alone.
export function scopeSelectAll(e: SelectAllEvent, selection: Selection | null): boolean {
  const isSelectAll =
    (e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey && (e.key === "a" || e.key === "A");
  if (!isSelectAll) return false;
  e.preventDefault();
  selection?.selectAllChildren(e.currentTarget);
  return true;
}

/// Focus `target` so a following ⌘A is scoped to it — but not while the
/// user is mid-selection: in Firefox `focus()` collapses a live
/// selection, so a click that ends a drag-select would lose it.
export function focusUnlessSelecting(target: HTMLElement, selection: Selection | null): void {
  const isSelecting =
    selection != null && !selection.isCollapsed && selection.toString().length > 0;
  if (!isSelecting) target.focus({ preventScroll: true });
}
