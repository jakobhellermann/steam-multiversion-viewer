// TODO(ai-review): review for style and correctness
import { useState } from "react";

import type { FileView, ManifestRef } from "../../api";
import { formatBytes, formatDate } from "../../lib/format";
import { FilePreview } from "./FilePreview";
import type { FileLocator } from "./types";

/// Collapsible "diff against another manifest" row. Header summarises
/// the size delta; body shows the target's full preview.
export function DiffTargetBlock({
  base,
  ref_,
  creationTime,
  query,
  rawSrc,
  locator,
}: {
  base: FileView | null;
  ref_: ManifestRef;
  creationTime: number;
  query: { data: FileView | null | undefined; isPending: boolean; error: unknown };
  rawSrc: string;
  locator: FileLocator;
}) {
  const [open, setOpen] = useState(false);
  const target = query.data;
  // fetchFileViewOptional returns null on 404 — file is missing from
  // that manifest, not an error.
  const targetMissing = !query.isPending && query.error == null && target === null;
  const delta = base != null && target != null ? target.size - base.size : 0;
  const kindChanged = base != null && target != null && base.kind !== target.kind;
  const linktargetChanged = base != null && target != null && base.linktarget !== target.linktarget;
  const { summary, summaryClass } = summaryFor(query, targetMissing, delta);
  return (
    <div className="rounded border border-slate-800">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        disabled={query.isPending || targetMissing}
        aria-expanded={open}
        className="flex w-full items-baseline gap-2 px-3 py-1.5 text-left text-sm hover:bg-slate-800/40 disabled:cursor-default disabled:hover:bg-transparent"
      >
        <span aria-hidden="true" className="inline-block w-3 text-slate-500">
          {query.isPending || targetMissing ? "" : open ? "▼︎" : "▶︎"}
        </span>
        <span className="font-medium text-slate-200">{ref_.branch}</span>
        {creationTime > 0 && (
          <span className="text-xs text-slate-500 tabular-nums">{formatDate(creationTime)}</span>
        )}
        <span className="font-mono text-xs text-slate-500 tabular-nums">
          depot {ref_.depot_id} · {ref_.manifest_id.slice(0, 12)}…
        </span>
        <span className={`ml-auto text-xs tabular-nums ${summaryClass}`}>{summary}</span>
      </button>
      {open && (
        <div className="space-y-2 border-t border-slate-800 px-3 py-2">
          {query.error != null && (
            <p className="text-sm text-red-300">
              Failed: {query.error instanceof Error ? query.error.message : String(query.error)}
            </p>
          )}
          {kindChanged && target && base && (
            <p className="text-xs text-amber-300">
              kind: {base.kind} → {target.kind}
            </p>
          )}
          {linktargetChanged && target && base && (
            <p className="text-xs text-amber-300">
              link target: {base.linktarget ?? "—"} → {target.linktarget ?? "—"}
            </p>
          )}
          {target && (
            <FilePreview view={target} rawSrc={rawSrc} locator={locator} showHeader={false} />
          )}
        </div>
      )}
    </div>
  );
}

type DiffSummary = { summary: string; summaryClass: string };

function summaryFor(
  query: { isPending: boolean; error: unknown },
  targetMissing: boolean,
  delta: number,
): DiffSummary {
  if (query.isPending) return { summary: "loading…", summaryClass: "text-slate-500" };
  if (query.error != null) return { summary: "failed", summaryClass: "text-amber-300" };
  if (targetMissing) {
    return {
      summary: "doesn't exist in this manifest version",
      summaryClass: "text-amber-300",
    };
  }
  return {
    summary: signedDelta(delta),
    summaryClass: delta === 0 ? "text-slate-500" : "text-amber-300",
  };
}

function signedDelta(delta: number): string {
  if (delta === 0) return "±0 B";
  const sign = delta > 0 ? "+" : "−";
  return `${sign}${formatBytes(Math.abs(delta))}`;
}
