// TODO(ai-review): review for style and correctness
import { describe, expect, test, vi } from "vitest";

import { focusUnlessSelecting, scopeSelectAll } from "./selection";

const pane = { nodeName: "DIV" } as unknown as Node;

function keyEvent(over: Partial<Record<string, unknown>> = {}) {
  return {
    metaKey: false,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    key: "a",
    currentTarget: pane,
    preventDefault: vi.fn(),
    ...over,
  } as Parameters<typeof scopeSelectAll>[0] & { preventDefault: ReturnType<typeof vi.fn> };
}

function fakeSelection() {
  return {
    selectAllChildren: vi.fn(),
    isCollapsed: true,
    toString: () => "",
  } as unknown as Selection;
}

describe("scopeSelectAll", () => {
  test("⌘A selects only the pane and prevents default", () => {
    const e = keyEvent({ metaKey: true });
    const sel = fakeSelection();
    expect(scopeSelectAll(e, sel)).toBe(true);
    expect(e.preventDefault).toHaveBeenCalledOnce();
    expect(sel.selectAllChildren).toHaveBeenCalledWith(pane);
  });

  test("Ctrl+A also handled (non-mac)", () => {
    const e = keyEvent({ ctrlKey: true });
    const sel = fakeSelection();
    expect(scopeSelectAll(e, sel)).toBe(true);
    expect(sel.selectAllChildren).toHaveBeenCalledWith(pane);
  });

  test("⌘A still prevents default when there is no selection object", () => {
    const e = keyEvent({ metaKey: true });
    expect(scopeSelectAll(e, null)).toBe(true);
    expect(e.preventDefault).toHaveBeenCalledOnce();
  });

  test.each([
    ["plain a", { key: "a" }],
    ["⌘C", { metaKey: true, key: "c" }],
    ["⌘⇧A", { metaKey: true, shiftKey: true }],
    ["⌘⌥A", { metaKey: true, altKey: true }],
  ])("ignores %s", (_label, over) => {
    const e = keyEvent(over);
    const sel = fakeSelection();
    expect(scopeSelectAll(e, sel)).toBe(false);
    expect(e.preventDefault).not.toHaveBeenCalled();
    expect(sel.selectAllChildren).not.toHaveBeenCalled();
  });
});

describe("focusUnlessSelecting", () => {
  test("focuses when nothing is selected", () => {
    const target = { focus: vi.fn() } as unknown as HTMLElement;
    focusUnlessSelecting(target, { isCollapsed: true, toString: () => "" } as Selection);
    expect(target.focus).toHaveBeenCalledWith({ preventScroll: true });
  });

  test("does not steal focus mid-selection (would collapse it in Firefox)", () => {
    const target = { focus: vi.fn() } as unknown as HTMLElement;
    focusUnlessSelecting(target, { isCollapsed: false, toString: () => "abc" } as Selection);
    expect(target.focus).not.toHaveBeenCalled();
  });
});
