// TODO(ai-review): review for style and correctness
import { useQuery } from "@tanstack/react-query";

import { fetchFileDiff } from "../../api";
import { HighlightedPre } from "./FilePreview";
import type { FileLocator } from "./types";

/// Render a unified text diff between `base` and `target` for the same
/// file path. Backend handles transformer resolution — `.dll` diffs are
/// against decompiled C# under the hood. Falls back to a "binary, no
/// transformer" message on HTTP 415.
export function DiffView({
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
  const diff = useQuery({
    queryKey: [
      "file-diff",
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
      fetchFileDiff(
        appid,
        base.depotId,
        base.manifestId,
        base.branch,
        { depot_id: target.depotId, manifest_id: target.manifestId, branch: target.branch },
        path,
      ),
  });

  if (diff.isPending) {
    return <p className="text-sm text-slate-500">Computing diff…</p>;
  }
  if (diff.error) {
    return (
      <p className="text-sm text-red-300">
        Diff failed: {diff.error instanceof Error ? diff.error.message : String(diff.error)}
      </p>
    );
  }
  if (diff.data == null) {
    // 415 from backend — binary file with no transformer registered.
    return (
      <p className="text-sm text-slate-500">
        Binary file with no registered transformer — no text diff available.
      </p>
    );
  }
  if (diff.data.length === 0) {
    return <p className="text-sm text-slate-500">No textual differences.</p>;
  }
  return <HighlightedPre code={diff.data} lang="diff" />;
}
