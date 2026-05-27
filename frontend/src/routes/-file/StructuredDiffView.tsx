// TODO(ai-review): review for style and correctness
import { useQuery } from "@tanstack/react-query";
import { useRef } from "react";

import { fetchStructuredDiff, fetchStructuredDiffNode, type StructuredNode } from "../../api";
import { HighlightedPre } from "./FilePreview";
import { Tree } from "./StructuredView";
import type { FileLocator } from "./types";

/// Tree-based diff view for files with structured representations.
/// Reuses the same `Tree` component as the non-diff view — the diff
/// signal travels on `StructuredNode.status` / `target_id`, so the
/// only thing diff-specific here is the content-pane renderer that
/// pairs both sides through `/file/structured-diff/node`.
export function StructuredDiffView({
  appid,
  base,
  target,
  path,
}: {
  appid: number;
  base: { depotId: number; manifestId: string; branch: string };
  target: FileLocator;
  path: string;
}) {
  const tree = useQuery({
    queryKey: [
      "structured-diff",
      appid,
      base.depotId,
      base.manifestId,
      base.branch,
      target.depotId,
      target.manifestId,
      target.branch,
      path,
    ],
    queryFn: () =>
      fetchStructuredDiff(
        appid,
        base.depotId,
        base.manifestId,
        base.branch,
        { depot_id: target.depotId, manifest_id: target.manifestId, branch: target.branch },
        path,
      ),
    staleTime: Infinity,
    gcTime: Infinity,
  });

  if (tree.isPending) {
    return <p className="text-sm text-slate-500">Computing structured diff…</p>;
  }
  if (tree.error) {
    return (
      <p className="text-sm text-red-300">
        Diff failed: {tree.error instanceof Error ? tree.error.message : String(tree.error)}
      </p>
    );
  }
  if (tree.data == null) {
    return (
      <p className="text-sm text-slate-500">
        No structured diff available for this file. Falling back to text diff.
      </p>
    );
  }
  if (tree.data.root.status === "unchanged" && tree.data.root.children.length === 0) {
    return <p className="text-sm text-slate-500">No structured differences.</p>;
  }
  // Identity for `mountKey`: rerun the tree's focus + state on a new
  // (base, target, path) tuple. Same role `locator.path` plays in the
  // non-diff renderer.
  const mountKey = `${base.depotId}/${base.manifestId}|${target.depotId}/${target.manifestId}|${path}`;
  return (
    <Tree
      root={tree.data.root}
      showHeader={false}
      mountKey={mountKey}
      renderContent={({ node }) => (
        <DiffNodeBody appid={appid} base={base} target={target} path={path} node={node} />
      )}
    />
  );
}

function DiffNodeBody({
  appid,
  base,
  target,
  path,
  node,
}: {
  appid: number;
  base: { depotId: number; manifestId: string; branch: string };
  target: FileLocator;
  path: string;
  node: StructuredNode;
}) {
  // Same-id matched diffs come with `target_id` unset — both sides
  // share the node id. Added-/removed-only nodes carry their single
  // side as `id`; the missing side is sent as null. `has_content`
  // gates the fetch so we don't probe section / class-stat rows.
  const baseId = node.status === "removed" ? null : node.id;
  const targetId =
    node.status === "added" ? null : node.target_id != null ? node.target_id : node.id;
  const enabled = !!node.has_content && (baseId != null || targetId != null);
  const content = useQuery({
    queryKey: [
      "structured-diff-node",
      appid,
      base.depotId,
      base.manifestId,
      base.branch,
      target.depotId,
      target.manifestId,
      target.branch,
      path,
      baseId,
      targetId,
    ],
    queryFn: () =>
      fetchStructuredDiffNode(
        appid,
        base.depotId,
        base.manifestId,
        base.branch,
        { depot_id: target.depotId, manifest_id: target.manifestId, branch: target.branch },
        path,
        baseId,
        targetId,
      ),
    enabled,
    staleTime: Infinity,
    gcTime: Infinity,
    retry: false,
  });

  // Same "keep last settled on screen" pattern the non-diff view
  // uses: react-query's cache doesn't carry across keys, so stash
  // the last successful response in a ref. Loading shimmer for a
  // sub-second fetch is noisier than just leaving the previous body
  // up.
  type Settled =
    | { kind: "ok"; lang: "diff" | "csharp" | "json"; text: string }
    | { kind: "err"; message: string };
  const lastSettledRef = useRef<Settled | null>(null);
  if (!enabled) {
    // Section / group / namespace rows have no body — drop whatever
    // we showed for the previous leaf so the pane reflects the new
    // selection.
    lastSettledRef.current = null;
    return null;
  }
  if (content.data) {
    lastSettledRef.current = {
      kind: "ok",
      lang: content.data.kind,
      text: content.data.text,
    };
  } else if (content.error) {
    lastSettledRef.current = {
      kind: "err",
      message: content.error instanceof Error ? content.error.message : String(content.error),
    };
  }
  const settled = lastSettledRef.current;
  if (!settled) {
    // First render before anything settles — keep the pane blank
    // rather than flash a spinner.
    return null;
  }
  if (settled.kind === "err") {
    return <p className="text-sm text-red-300">{settled.message}</p>;
  }
  if (settled.text.length === 0) {
    return <p className="text-sm text-slate-500">No content.</p>;
  }
  // TODO: postProcess (pptr markers → clickable links). Trickier than
  // in the non-diff view: a unified diff carries markers from both
  // sides, and each side's pptr should resolve against its own
  // manifest. Punted for now — markers render as raw sentinels.
  return <HighlightedPre code={settled.text} lang={settled.lang} />;
}
