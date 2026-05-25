// TODO(ai-review): review for style and correctness
import { useEffect, useState } from "react";

import type { FileView, ManifestRef } from "../../api";
import { formatBytes, formatDate } from "../../lib/format";
import { DiffView } from "./DiffView";
import { FilePreview } from "./FilePreview";
import type { FileLocator } from "./types";

/// Collapsible "diff against another manifest" row. Header summarises
/// the size delta; body shows the actual diff for text-like files, or
/// the target's preview otherwise.
export function DiffTargetBlock({
  base,
  baseLocator,
  ref_,
  creationTime,
  query,
  rawSrc,
  locator,
}: {
  base: FileView | null;
  /// Identity of the base file — needed so we can ask the backend for a
  /// unified diff against `locator` (the target).
  baseLocator: FileLocator;
  ref_: ManifestRef;
  creationTime: number;
  query: { data: FileView | null | undefined; isPending: boolean; error: unknown };
  rawSrc: string;
  locator: FileLocator;
}) {
  const target = query.data;
  // Auto-expand for plain text files (no transformer)
  const autoOpen =
    target != null &&
    base != null &&
    canDiffText(base) &&
    canDiffText(target) &&
    base.transformer == null &&
    target.transformer == null;
  const [open, setOpen] = useState(false);
  // `useState(autoOpen)` would only see the initial render's value;
  // open it once the file-view query lands.
  useEffect(() => {
    if (autoOpen) setOpen(true);
  }, [autoOpen]);
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
          {target &&
            (canDiffText(base) && canDiffText(target) ? (
              <DiffView
                appid={baseLocator.appid}
                base={{
                  depotId: baseLocator.depotId,
                  manifestId: baseLocator.manifestId,
                  branch: baseLocator.branch,
                }}
                target={locator}
                path={baseLocator.path}
              />
            ) : (
              <FilePreview view={target} rawSrc={rawSrc} locator={locator} showHeader={false} />
            ))}
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

/// True when this file resolves to a text blob the backend can diff —
/// either it's already inline text, or there's a registered transformer
/// that produces text (decompile / disassembly). Bytes-only files
/// without a transformer fall through to the plain preview.
function canDiffText(view: FileView | null): boolean {
  if (view == null) return false;
  if (view.kind !== "file") return false;
  return view.content_kind === "text" || view.transformer != null;
}

function signedDelta(delta: number): string {
  if (delta === 0) return "±0 B";
  const sign = delta > 0 ? "+" : "−";
  return `${sign}${formatBytes(Math.abs(delta))}`;
}
