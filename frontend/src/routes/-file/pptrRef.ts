// TODO(ai-review): review for style and correctness
//
// Pure helpers around diff node ids. Two layers:
// - the side grammar every diff builder shares: `base:<inner>` /
//   `target:<inner>` / `mod:<base>,<target>` / bare `<inner>`
//   (matched, same id on both sides), optionally under an
//   `archive:<entry>/` prefix — [`splitDiffId`] and the projections
//   built on it are generic over all formats;
// - the unity object refinement (`obj:<n>`) that pptr refs use —
//   [`pptrNodeKeys`]/[`parsePptrRef`] stay strict to it.

/// A structured-tree node id (also the shape carried in the URL hash).
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
  if ((m = /^(obj:-?\d+)$/.exec(inner))) {
    // Matched on both sides with the same path id — answers to either.
    const key = prefix + m[1];
    return [
      { side: "base", key },
      { side: "target", key },
    ];
  }
  if ((m = /^base:(obj:-?\d+)$/.exec(inner))) return [{ side: "base", key: prefix + m[1] }];
  if ((m = /^target:(obj:-?\d+)$/.exec(inner))) return [{ side: "target", key: prefix + m[1] }];
  if ((m = /^mod:(obj:-?\d+),(obj:-?\d+)$/.exec(inner))) {
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
  if ((m = /^base:(obj:-?\d+)$/.exec(inner))) return { side: "base", key: prefix + m[1] };
  if ((m = /^target:(obj:-?\d+)$/.exec(inner))) return { side: "target", key: prefix + m[1] };
  if ((m = /^(obj:-?\d+)$/.exec(inner))) return { side: "either", key: prefix + m[1] };
  return null;
}

/// Split a diff node id into its per-side inner ids over the side
/// grammar every diff builder shares, mirroring the backend's
/// `split_diff_id`: `base:`/`target:` keep their side, a `mod:` pair
/// splits at the first comma (inner ids must not contain one), and a
/// bare id is matched — it answers to both sides. The bundle
/// `archive:<entry>/` prefix passes through onto the inner ids.
export function splitDiffId(id: NodeId): {
  prefix: NodeId;
  base?: NodeId;
  target?: NodeId;
} {
  const [prefix, inner] = splitArchivePrefix(id);
  if (inner.startsWith("base:")) return { prefix, base: inner.slice(5) };
  if (inner.startsWith("target:")) return { prefix, target: inner.slice(7) };
  if (inner.startsWith("mod:")) {
    const rest = inner.slice(4);
    const comma = rest.indexOf(",");
    if (comma >= 0) {
      return {
        prefix,
        base: rest.slice(0, comma),
        target: rest.slice(comma + 1),
      };
    }
  }
  return { prefix, base: inner, target: inner };
}

/// Project a diff node id onto one side, yielding the node id that
/// side's single-file structured view carries for the same node —
/// `undefined` when the node has no counterpart on that side (a
/// one-sided added/removed row).
export function projectRefToSide(nodeId: NodeId, side: "base" | "target"): NodeId | undefined {
  const { prefix, ...sides } = splitDiffId(nodeId);
  const inner = sides[side];
  return inner === undefined ? undefined : prefix + inner;
}

/// Like [`projectRefToSide`], but keeps the `base:`/`target:` tag so the
/// ref stays unambiguous inside *another diff* that retains this side.
/// The bare `obj:N` a single-file view wants would be dangerous there —
/// path ids collide across manifests, so a bare ref could resolve to the
/// *other* side's object with the same number. Tagging pins it to the
/// retained side's index. `undefined` when the object isn't on `side`
/// (the replaced-side case — caller should carry no hash rather than a
/// wrong one).
export function qualifyRefForSide(nodeId: NodeId, side: "base" | "target"): NodeId | undefined {
  const { prefix, ...sides } = splitDiffId(nodeId);
  const inner = sides[side];
  return inner === undefined ? undefined : `${prefix}${side}:${inner}`;
}
