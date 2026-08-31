// TODO(ai-review): review for style and correctness
import { useQuery } from "@tanstack/react-query";
import { useRouter } from "@tanstack/react-router";
import { useCallback, useRef } from "react";

import { fetchStructuredDiff, fetchStructuredDiffNode, type StructuredNode } from "../../api";
import { type Lang, langForMime } from "../../lib/syntax";
import { useDelayedFlag } from "../../lib/useDelayedFlag";
import { HighlightedPre } from "./FilePreview";
import { makeDiffPostProcess, makePostProcess } from "./markers";
import { focusUnlessSelecting, scopeSelectAll } from "./selection";
import { NODE_SPINNER_DELAY_MS, NodePanelSpinner, Tree } from "./StructuredView";
import type { FileLocator } from "./types";

/// Tree-based diff view for files with structured representations.
/// Reuses the same `Tree` component as the non-diff view — the diff
/// signal travels on `StructuredNode.status` and the id-prefix, so the
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
  // Backend reads which side(s) to dump out of the id's prefix — the
  // frontend just hands the raw tree id over. `has_content` gates the
  // fetch so we don't probe section / class-stat rows.
  const enabled = !!node.has_content;
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
      node.id,
    ],
    queryFn: () =>
      fetchStructuredDiffNode(
        appid,
        base.depotId,
        base.manifestId,
        base.branch,
        { depot_id: target.depotId, manifest_id: target.manifestId, branch: target.branch },
        path,
        node.id,
      ),
    enabled,
    staleTime: Infinity,
    gcTime: Infinity,
    retry: false,
  });

  // Keep the previous body up for a short fetch; swap to a spinner only
  // once the new node has been in-flight past the flash threshold.
  const showSpinner = useDelayedFlag(enabled && content.isFetching, NODE_SPINNER_DELAY_MS);

  // Same "keep last settled on screen" pattern the non-diff view
  // uses: react-query's cache doesn't carry across keys, so stash
  // the last successful response in a ref. Loading shimmer for a
  // sub-second fetch is noisier than just leaving the previous body
  // up.
  type Settled = { kind: "ok"; lang: Lang | null; text: string } | { kind: "err"; message: string };
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
      lang: langForMime(content.data.mime),
      text: content.data.text,
    };
  } else if (content.error) {
    lastSettledRef.current = {
      kind: "err",
      message: content.error instanceof Error ? content.error.message : String(content.error),
    };
  }
  const settled = lastSettledRef.current;
  if (showSpinner) return <NodePanelSpinner />;
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
  // Both diff sides emit identical pptr markers (the side isn't baked
  // into the marker shape), so links route per line off the unified-diff
  // `+`/`-` gutter: a removed line points into the target manifest, an
  // added/context line into the base.
  const baseLocator: FileLocator = {
    appid,
    depotId: base.depotId,
    manifestId: base.manifestId,
    branch: base.branch,
    path,
  };
  const targetLocator: FileLocator = {
    appid,
    depotId: target.depotId,
    manifestId: target.manifestId,
    branch: target.branch,
    path,
  };
  // A two-sided body (`diff`) carries the side per line in its `+`/`-`
  // gutter. A one-sided body (added/removed node → plain json/csharp)
  // has no gutter, so the whole dump belongs to one side: a removed
  // node is target-only, everything else base. Tag accordingly and
  // resolve external refs against that side's manifest.
  const postProcess =
    settled.lang === "diff"
      ? makeDiffPostProcess(baseLocator, targetLocator, () => true)
      : node.status === "removed"
        ? makePostProcess(targetLocator, () => true, "target")
        : makePostProcess(baseLocator, () => true, "base");
  return (
    <DiffContentPane>
      <HighlightedPre code={settled.text} lang={settled.lang} bare postProcess={postProcess} />
    </DiffContentPane>
  );
}

/// Wrapper that intercepts clicks on rendered pptr links so cross-file
/// navigation stays SPA. Without this, the raw `<a href>` in the
/// post-processed HTML falls back to a browser-level full reload — the
/// non-diff `NodeContentPanel` has the same delegated handler.
function DiffContentPane({ children }: { children: React.ReactNode }) {
  const router = useRouter();
  const onClick = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      // Focus the pane so a following ⌘A is scoped to it, not the page.
      focusUnlessSelecting(e.currentTarget, window.getSelection());
      if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || e.button !== 0) return;
      const a = (e.target as HTMLElement | null)?.closest(
        "a[data-pptr-file]",
      ) as HTMLAnchorElement | null;
      if (!a) return;
      const href = a.getAttribute("href");
      if (!href) return;
      e.preventDefault();
      router.history.push(href);
    },
    [router],
  );
  const onKeyDown = useCallback((e: React.KeyboardEvent<HTMLDivElement>) => {
    scopeSelectAll(e, window.getSelection());
  }, []);
  return (
    <div
      tabIndex={-1}
      onClick={onClick}
      onKeyDown={onKeyDown}
      className="w-max min-w-full outline-none"
    >
      {children}
    </div>
  );
}
