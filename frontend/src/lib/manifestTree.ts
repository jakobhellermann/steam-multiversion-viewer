// TODO(ai-review): review for style and correctness
import type { ManifestDiffEntry, ManifestFile } from "../api";

/// One node in the tree built from the flat manifest file list. `file` is
/// set on leaves (files); for directories it stays null. `children` is
/// keyed by name segment (sorted on flatten, not here).
export type TreeNode = {
  name: string;
  path: string;
  children: Map<string, TreeNode>;
  file: ManifestFile | null;
  /// Leaf that exists only in the compare targets: `file` is
  /// synthesized from the target side.
  removed: boolean;
  // Aggregate size of all leaves under this node — including the file
  // itself for leaves and `removed` leaves. Computed during build/merge.
  size: number;
  // Number of file leaves under this node (1 if this node is itself a
  // file), including `removed` leaves.
  fileCount: number;
};

// Sentinel for "file has no extension". Has to be non-empty (and not a
// plausible real extension) so URL serialization round-trips it — the
// previous "" got dropped by `split(",").filter(Boolean)` and the UI
// chip became unselectable as a result.
export const NO_EXT = "__none__";

export function fileExtension(path: string): string {
  // Only look at the last segment so a "." in a dir name doesn't count.
  const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  const name = slash >= 0 ? path.slice(slash + 1) : path;
  // Unity serialized scenes (`level0`, `level42`, …) have no extension
  // by convention but conceptually share a type. Bucket them as `level`
  // so the extension filter treats them as a group.
  if (/^level\d+$/.test(name)) return "level";
  // Strip trailing version suffixes (libfoo.so.1, libfoo.so.1.2) before
  // picking the extension so versioned shared libs bucket as "so".
  const stripped = name.replace(/(?:\.\d+)+$/, "");
  const dot = stripped.lastIndexOf(".");
  if (dot <= 0) return NO_EXT;
  return stripped.slice(dot + 1);
}

export function buildTree(files: ManifestFile[]): TreeNode {
  const root: TreeNode = {
    name: "",
    path: "",
    children: new Map(),
    file: null,
    removed: false,
    size: 0,
    fileCount: 0,
  };
  for (const f of files) {
    const parts = f.path.split("/");
    let node = root;
    for (let i = 0; i < parts.length; i++) {
      const name = parts[i];
      const isLeaf = i === parts.length - 1;
      let child = node.children.get(name);
      if (!child) {
        child = {
          name,
          path: parts.slice(0, i + 1).join("/"),
          children: new Map(),
          file: null,
          removed: false,
          size: 0,
          fileCount: 0,
        };
        node.children.set(name, child);
      }
      if (isLeaf) child.file = f;
      node = child;
    }
  }
  // Aggregate size + fileCount bottom-up.
  const visit = (n: TreeNode) => {
    if (n.file) {
      n.size = n.file.size;
      n.fileCount = 1;
      return;
    }
    for (const c of n.children.values()) {
      visit(c);
      n.size += c.size;
      n.fileCount += c.fileCount;
    }
  };
  visit(root);
  return root;
}

/// Merge `removed` diff rows into a base-manifest tree as target-only
/// leaves: existing dir nodes are reused, missing ones created, so a
/// directory the base manifest lacks becomes visible.
export function mergeRemovedFiles(root: TreeNode, removed: ManifestDiffEntry[]): void {
  for (const entry of removed) {
    const parts = entry.path.split("/");
    const chain: TreeNode[] = [];
    let node = root;
    for (let i = 0; i < parts.length; i++) {
      const name = parts[i];
      let child = node.children.get(name);
      if (!child) {
        child = {
          name,
          path: parts.slice(0, i + 1).join("/"),
          children: new Map(),
          file: null,
          removed: false,
          size: 0,
          fileCount: 0,
        };
        node.children.set(name, child);
      }
      chain.push(child);
      node = child;
    }
    const size = entry.size ?? 0;
    node.file = {
      path: entry.path,
      size,
      kind: "file",
      chunk_count: 0,
      linktarget: null,
    };
    node.removed = true;
    node.size = size;
    node.fileCount = 1;
    for (const n of chain.slice(0, -1)) {
      n.size += size;
      n.fileCount += 1;
    }
  }
}

export type FlatRow = {
  node: TreeNode;
  depth: number;
  expanded: boolean;
  hasChildren: boolean;
};

/// Walks the tree producing visible rows. Two modes:
/// - No search (`matches == null`): dirs are expanded iff they're in
///   `expanded`. Default collapsed.
/// - Search (`matches != null`): only nodes leading to a match are returned,
///   dirs along match paths are expanded by default, but `collapsed` (an
///   explicit user collapse during search) opts out per-path.
export function flattenTree(
  root: TreeNode,
  expanded: Set<string>,
  collapsed: Set<string>,
  matches: Set<string> | null,
): FlatRow[] {
  const out: FlatRow[] = [];
  // Auto-expand chains: a dir with exactly one child-dir as its only
  // entry opens that child along with itself. The `collapsed` set lets
  // the user opt out of auto-expansion for a specific path.
  const walk = (node: TreeNode, depth: number, autoExpand: boolean) => {
    const isRoot = depth < 0;
    const hasChildren = node.children.size > 0;
    const sorted = [...node.children.values()].sort((a, b) => {
      const ad = a.file == null;
      const bd = b.file == null;
      if (ad !== bd) return ad ? -1 : 1;
      // Numeric-aware so `level24` sorts before `level231` instead of
      // lexicographically after it.
      return a.name.localeCompare(b.name, undefined, { numeric: true });
    });

    if (!isRoot) {
      if (matches && !nodeContainsMatch(node, matches)) return;
      const userCollapsed = collapsed.has(node.path);
      const isExpanded = matches
        ? !userCollapsed
        : userCollapsed
          ? false
          : autoExpand || expanded.has(node.path);
      out.push({ node, depth, expanded: isExpanded, hasChildren });
      if (!hasChildren) return;
      if (!isExpanded) return;
    }

    const onlyChildAutoOpens = sorted.length === 1 && sorted[0].file == null;
    for (const c of sorted) walk(c, depth + 1, onlyChildAutoOpens);
  };
  walk(root, -1, false);
  return out;
}

/// True iff `node` is a match itself or has any descendant in `matches`.
/// Walking each subtree is cheap because `matches` is the filtered file
/// set; we walk paths that are already known to land somewhere useful.
export function nodeContainsMatch(node: TreeNode, matches: Set<string>): boolean {
  if (node.file && matches.has(node.path)) return true;
  for (const c of node.children.values()) {
    if (nodeContainsMatch(c, matches)) return true;
  }
  return false;
}
