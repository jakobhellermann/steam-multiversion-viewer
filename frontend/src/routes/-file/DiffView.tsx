// TODO(ai-review): review for style and correctness
import { useQuery } from "@tanstack/react-query";

import { fetchFileDiff } from "../../api";
import { highlight } from "../../lib/syntax";
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

  // Run shiki once we have the text. Diff highlighting comes free with
  // shiki's `diff` grammar — +/- lines get green/red, hunk headers blue.
  const html = useQuery({
    queryKey: ["syntax-highlight", "diff", diff.data?.length ?? 0, diff.data?.slice(0, 64) ?? ""],
    queryFn: () => (diff.data ? highlight(diff.data, "diff") : Promise.resolve(null)),
    enabled: diff.data != null && diff.data.length > 0,
    staleTime: Infinity,
    gcTime: Infinity,
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
  if (html.data) {
    return (
      <div
        className="overflow-x-auto rounded border border-slate-800 text-xs [&_pre]:m-0! [&_pre]:bg-slate-950! [&_pre]:p-3!"
        dangerouslySetInnerHTML={{ __html: html.data }}
      />
    );
  }
  return (
    <pre className="overflow-x-auto rounded border border-slate-800 bg-slate-950 p-3 font-mono text-xs wrap-break-word whitespace-pre-wrap">
      {diff.data}
    </pre>
  );
}
