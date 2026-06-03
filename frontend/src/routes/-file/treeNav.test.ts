// TODO(ai-review): review for style and correctness
import { describe, expect, test } from "vitest";

import { treeKeyAction, type TreeNavState, type VisibleRow } from "./StructuredView";
import type { StructuredNode } from "../../api";

// Minimal node — only the fields the nav logic reads.
function node(id: string, children: StructuredNode[] = []): StructuredNode {
  return { id, label: id, kind: "x", children };
}

function row(n: StructuredNode, depth: number, hasVisibleChildren: boolean): VisibleRow {
  return { node: n, depth, hasVisibleChildren };
}

// Tree:
//   root (expanded)
//   ├─ A  (leaf)
//   └─ B  (leaf)
function leafSiblingsState(focusedIdx: number, expandedRoot = true): TreeNavState {
  const a = node("A");
  const b = node("B");
  const root = node("root", [a, b]);
  const visibleRows: VisibleRow[] = [row(root, 0, true), row(a, 1, false), row(b, 1, false)];
  return {
    visibleRows,
    focusedIdx,
    expandedIds: new Set(expandedRoot ? ["root"] : []),
    parentById: new Map([
      ["A", "root"],
      ["B", "root"],
    ]),
  };
}

describe("treeKeyAction ArrowRight", () => {
  test("on a leaf with a following sibling, focuses the next sibling", () => {
    // Focus A (idx 1), a leaf. There's nothing to expand, so right-arrow
    // should jump to its next sibling B instead of being a no-op.
    const action = treeKeyAction("ArrowRight", leafSiblingsState(1));
    expect(action).toEqual({ type: "focus", id: "B" });
  });

  test("on the last leaf sibling, does nothing", () => {
    // Focus B (idx 2), the last sibling — no next sibling to jump to.
    const action = treeKeyAction("ArrowRight", leafSiblingsState(2));
    expect(action).toBeNull();
  });

  test("on a collapsed parent, expands it (unchanged behavior)", () => {
    const state = leafSiblingsState(0, false);
    const action = treeKeyAction("ArrowRight", state);
    expect(action).toEqual({ type: "expand", id: "root", open: true });
  });

  test("on an expanded parent, focuses its first child (unchanged behavior)", () => {
    const action = treeKeyAction("ArrowRight", leafSiblingsState(0, true));
    expect(action).toEqual({ type: "focus", id: "A" });
  });

  test("a leaf whose next sibling has expanded children jumps to that sibling, not into its children", () => {
    // Tree (all expanded):
    //   root
    //   ├─ A   (leaf)            <- focus here
    //   └─ B   (expanded)
    //      └─ B1 (leaf)
    // ArrowRight on A must land on B (its sibling), skipping nothing
    // since B is the very next row — but the guard must still treat B's
    // child B1 as out of the sibling group.
    const b1 = node("B1");
    const b = node("B", [b1]);
    const a = node("A");
    const root = node("root", [a, b]);
    const visibleRows: VisibleRow[] = [
      row(root, 0, true),
      row(a, 1, false),
      row(b, 1, true),
      row(b1, 2, false),
    ];
    const state: TreeNavState = {
      visibleRows,
      focusedIdx: 1, // A
      expandedIds: new Set(["root", "B"]),
      parentById: new Map([
        ["A", "root"],
        ["B", "root"],
        ["B1", "B"],
      ]),
    };
    expect(treeKeyAction("ArrowRight", state)).toEqual({ type: "focus", id: "B" });

    // And ArrowRight on the deepest leaf B1 (idx 3) has no later sibling.
    expect(treeKeyAction("ArrowRight", { ...state, focusedIdx: 3 })).toBeNull();
  });

  test("a leaf that is the last child descends out to an ancestor's next sibling", () => {
    // Tree (all expanded):
    //   root
    //   ├─ A
    //   │  └─ A1
    //   │     └─ A2  (leaf, last child all the way up)  <- focus here
    //   └─ B
    // ArrowRight on A2 has no sibling of its own, but B is the next row
    // that isn't a descendant of A2 — focus should jump there.
    const a2 = node("A2");
    const a1 = node("A1", [a2]);
    const a = node("A", [a1]);
    const b = node("B");
    const root = node("root", [a, b]);
    const visibleRows: VisibleRow[] = [
      row(root, 0, true),
      row(a, 1, true),
      row(a1, 2, true),
      row(a2, 3, false),
      row(b, 1, false),
    ];
    const state: TreeNavState = {
      visibleRows,
      focusedIdx: 3, // A2
      expandedIds: new Set(["root", "A", "A1"]),
      parentById: new Map([
        ["A", "root"],
        ["A1", "A"],
        ["A2", "A1"],
        ["B", "root"],
      ]),
    };
    expect(treeKeyAction("ArrowRight", state)).toEqual({ type: "focus", id: "B" });
  });
});

describe("treeKeyAction ArrowLeft", () => {
  // Tree (A and A3 expanded):
  //   A
  //   ├─ A1
  //   ├─ A2
  //   └─ A3
  //      ├─ B1   <- focus here
  //      ├─ B2
  //      └─ B3
  function nestedState(focusedIdx: number, expanded: string[]): TreeNavState {
    const b1 = node("B1");
    const b2 = node("B2");
    const b3 = node("B3");
    const a1 = node("A1");
    const a2 = node("A2");
    const a3 = node("A3", [b1, b2, b3]);
    const a = node("A", [a1, a2, a3]);
    const visibleRows: VisibleRow[] = [
      row(a, 0, true),
      row(a1, 1, false),
      row(a2, 1, false),
      row(a3, 1, true),
      row(b1, 2, false),
      row(b2, 2, false),
      row(b3, 2, false),
    ];
    return {
      visibleRows,
      focusedIdx,
      expandedIds: new Set(expanded),
      parentById: new Map([
        ["A1", "A"],
        ["A2", "A"],
        ["A3", "A"],
        ["B1", "A3"],
        ["B2", "A3"],
        ["B3", "A3"],
      ]),
    };
  }

  test("on a leaf child, focuses its parent", () => {
    // B1 (idx 4) is a leaf → ArrowLeft jumps up to A3.
    expect(treeKeyAction("ArrowLeft", nestedState(4, ["A", "A3"]))).toEqual({
      type: "focus",
      id: "A3",
    });
  });

  test("on an expanded node, collapses it; collapsed node then focuses parent", () => {
    // A3 (idx 3) expanded → ArrowLeft collapses it.
    expect(treeKeyAction("ArrowLeft", nestedState(3, ["A", "A3"]))).toEqual({
      type: "expand",
      id: "A3",
      open: false,
    });
    // A3 collapsed → ArrowLeft jumps to its parent A.
    expect(treeKeyAction("ArrowLeft", nestedState(3, ["A"]))).toEqual({
      type: "focus",
      id: "A",
    });
  });
});
