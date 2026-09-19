// TODO(ai-review): review for style and correctness
import { describe, expect, it } from "vitest";
import type { ManifestFile } from "../api";
import {
  buildTree,
  fileExtension,
  flattenTree,
  mergeRemovedFiles,
  NO_EXT,
  nodeContainsMatch,
  type TreeNode,
} from "./manifestTree";

function file(path: string, size = 1): ManifestFile {
  return { path, size, kind: "file", chunk_count: 0, linktarget: null };
}

function removedEntry(path: string, size = 1): { path: string; status: "removed"; size: number } {
  return { path, status: "removed", size };
}

function child(node: TreeNode, name: string): TreeNode {
  const c = node.children.get(name);
  if (!c) throw new Error(`missing child ${name} in ${node.path}`);
  return c;
}

describe("buildTree + mergeRemovedFiles", () => {
  it("merges a removed leaf into an existing dir and counts it in the aggregates", () => {
    const tree = buildTree([file("Content/Dialog/English.txt", 100)]);
    mergeRemovedFiles(tree, [removedEntry("Content/Dialog/english.txt", 50)]);

    const dialog = child(child(tree, "Content"), "Dialog");
    expect(dialog.fileCount).toBe(2);
    expect(dialog.size).toBe(150);
    const english = child(dialog, "english.txt");
    expect(english.removed).toBe(true);
    expect(english.file?.size).toBe(50);
    expect(english.fileCount).toBe(1);
    expect(child(dialog, "English.txt").removed).toBe(false);
  });

  it("creates dir nodes the base manifest lacks, with their own aggregates", () => {
    const tree = buildTree([file("Celeste.exe", 10)]);
    mergeRemovedFiles(tree, [
      removedEntry("lib/libfoo.so.10", 4),
      removedEntry("lib/libbar.so.10", 6),
    ]);

    const lib = child(tree, "lib");
    expect(lib.file).toBeNull();
    expect(lib.fileCount).toBe(2);
    expect(lib.size).toBe(10);
    expect(nodeContainsMatch(tree, new Set(["lib/libbar.so.10"]))).toBe(true);
  });

  it("keeps a removed case-variant next to its base twin as a distinct key", () => {
    const tree = buildTree([file("Content/Dialog/English.txt")]);
    mergeRemovedFiles(tree, [removedEntry("Content/Dialog/english.txt")]);
    const dialog = child(child(tree, "Content"), "Dialog");
    expect([...dialog.children.keys()].sort()).toEqual(["English.txt", "english.txt"]);
  });

  it("links the row body through a synthesized file at the leaf", () => {
    const tree = buildTree([]);
    mergeRemovedFiles(tree, [removedEntry("a/b.txt", 7)]);
    const leaf = child(child(tree, "a"), "b.txt");
    expect(leaf.file).toEqual({
      path: "a/b.txt",
      size: 7,
      kind: "file",
      chunk_count: 0,
      linktarget: null,
    });
  });
});

describe("flattenTree with removed leaves", () => {
  it("shows removed rows once expanded, sorted among the files", () => {
    const tree = buildTree([file("Data/a.txt"), file("Data/z.txt")]);
    mergeRemovedFiles(tree, [removedEntry("Data/m.txt")]);
    const rows = flattenTree(tree, new Set(["Data"]), new Set(), null);
    expect(rows.map((r) => r.node.path)).toEqual([
      "Data",
      "Data/a.txt",
      "Data/m.txt",
      "Data/z.txt",
    ]);
  });

  it("keeps removed rows under a search that matches them", () => {
    const tree = buildTree([file("Data/a.txt")]);
    mergeRemovedFiles(tree, [removedEntry("lib/m.txt")]);
    const rows = flattenTree(tree, new Set(), new Set(), new Set(["lib/m.txt"]));
    expect(rows.map((r) => r.node.path)).toEqual(["lib", "lib/m.txt"]);
  });
});

describe("fileExtension", () => {
  it("buckets versioned shared libs as so and level scenes as level", () => {
    expect(fileExtension("lib/libfmod.so.10")).toBe("so");
    expect(fileExtension("Data/level42")).toBe("level");
    expect(fileExtension("README")).toBe(NO_EXT);
  });
});
