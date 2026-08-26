// TODO(ai-review): review for style and correctness
//
// Pure helpers for translating between diff node ids and pptr refs.
// Extracted from `StructuredView` so the diff route can project a
// selection onto one side without importing the whole component module.

/// A structured-tree node id (also the shape carried in the URL hash).
/// Documentation only — structurally a `string`; the grammar is a
/// runtime contract the regexes below enforce, not a compile-time one.
///
/// Object-shaped rows (what these helpers act on):
///   - `obj:<n>`                 matched on both sides, same path id
///   - `base:obj:<n>`            object only on the base side (added)
///   - `target:obj:<n>`          object only on the target side (removed)
///   - `mod:obj:<b>,obj:<t>`     matched pair whose path id renumbered
/// Any of the above may be prefixed with `archive:<entry>/` inside a
/// bundle. Non-object rows (`file:<path>`, `section:<name>`,
/// `class:<id>`, …) carry no pptr and yield nothing.
export type NodeId = string;

/// Split a node id / ref into its bundle `archive:<entry>/` prefix (or
/// `""` outside bundles) and the inner part.
function splitArchivePrefix(id: NodeId): [string, NodeId] {
  if (id.startsWith("archive:")) {
    const slash = id.indexOf("/");
    if (slash >= 0) return [id.slice(0, slash + 1), id.slice(slash + 1)];
  }
  return ["", id];
}

/// The (side, path-id key) entries this node id should be indexed under.
/// A pptr ref carries the diff side it was dumped from but only a bare
/// path id; the node carries the pairing-derived prefix. Indexing per
/// side by `[archive:X/]<path-id>` lets [`resolveRef`] bridge them
/// without the bare-`obj:N` ambiguity (a path id is unique within one
/// side's file). Non-object rows (sections, class-stats, blobs) yield
/// nothing.
export function pptrNodeKeys(id: NodeId): Array<{ side: "base" | "target"; key: NodeId }> {
  const [prefix, inner] = splitArchivePrefix(id);
  let m: RegExpExecArray | null;
  if ((m = /^(obj:\d+)$/.exec(inner))) {
    // Matched on both sides with the same path id — answers to either.
    const key = prefix + m[1];
    return [
      { side: "base", key },
      { side: "target", key },
    ];
  }
  if ((m = /^base:(obj:\d+)$/.exec(inner))) return [{ side: "base", key: prefix + m[1] }];
  if ((m = /^target:(obj:\d+)$/.exec(inner))) return [{ side: "target", key: prefix + m[1] }];
  if ((m = /^mod:(obj:\d+),(obj:\d+)$/.exec(inner))) {
    // `mod:obj:<base>,obj:<target>` — each side keys under its own id.
    return [
      { side: "base", key: prefix + m[1] },
      { side: "target", key: prefix + m[2] },
    ];
  }
  return [];
}

/// Parse an incoming pptr ref into the index side + key to look up.
/// `"either"` is a bare `obj:N` with no side hint (single-file view, or
/// a hand-typed hash) — callers fall back across both sides.
export function parsePptrRef(
  ref: NodeId,
): { side: "base" | "target" | "either"; key: NodeId } | null {
  const [prefix, inner] = splitArchivePrefix(ref);
  let m: RegExpExecArray | null;
  if ((m = /^base:(obj:\d+)$/.exec(inner))) return { side: "base", key: prefix + m[1] };
  if ((m = /^target:(obj:\d+)$/.exec(inner))) return { side: "target", key: prefix + m[1] };
  if ((m = /^(obj:\d+)$/.exec(inner))) return { side: "either", key: prefix + m[1] };
  return null;
}

/// Project a diff node id onto one side, yielding the node id that
/// side's single-file structured view carries for the same object — or
/// `undefined` when the object has no counterpart on that side (a
/// one-sided added/removed row, or a non-object node). A bare/matched
/// `obj:N` answers to either side; `mod:obj:<base>,obj:<target>` hands
/// back the requested side's path id.
export function projectRefToSide(nodeId: NodeId, side: "base" | "target"): NodeId | undefined {
  return pptrNodeKeys(nodeId).find((k) => k.side === side)?.key;
}
