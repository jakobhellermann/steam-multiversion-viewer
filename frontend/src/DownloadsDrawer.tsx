// TODO(ai-review): review for style and correctness
import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import type { ManifestFilesPage } from "./api";
import { cancelDownloads, type ChunkUpdate, type DownloadStats } from "./api";
import { formatBytes } from "./format";

/// Live download progress panel, anchored top-right. Visible whenever
/// stats.chunks_total > 0; dismissed by clicking outside (only when
/// idle) or via the cancel/× button. There is no collapsed state — the
/// whole thing is small enough to leave open.
export function DownloadsDrawer() {
  const [stats, setStats] = useState<DownloadStats | null>(null);
  // Sliding window of (timestamp, bytes_completed) samples. Rate is
  // computed as the slope across the window — smooth in the face of
  // bursty chunk completions and decays toward 0 if the queue stalls.
  const samples = useRef<{ ts: number; bytes: number }[]>([]);
  const RATE_WINDOW_MS = 3000;
  const [rate, setRate] = useState(0);
  const queryClient = useQueryClient();
  // Throttle `manifest-statuses` / `file-view` invalidations — these
  // still go through the refetch path. `manifest-files` is patched
  // directly via the SSE chunks event instead (see the listener below).
  const lastInvalidate = useRef(0);
  const lastCompletedSeen = useRef(0);
  const rootRef = useRef<HTMLDivElement>(null);

  const activeNow =
    stats != null && stats.chunks_completed + stats.chunks_failed < stats.chunks_total;
  // Click outside (when settled) dismisses the panel entirely by
  // resetting backend stats. Active downloads ignore this so live
  // progress + cancel UI persist as the user clicks around the app.
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (activeNow) return;
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) {
        cancelDownloads().catch(() => {
          /* SSE will redeliver state regardless */
        });
      }
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [activeNow]);

  useEffect(() => {
    const es = new EventSource("/api/downloads/events");
    const onStats = (e: MessageEvent) => {
      const s = JSON.parse(e.data) as DownloadStats;
      setStats(s);
      const now = performance.now();
      // Drop the buffer on reset (cancel zeroes bytes_completed); a
      // shrinking value would otherwise produce a negative slope.
      if (
        samples.current.length > 0 &&
        s.bytes_completed < samples.current[samples.current.length - 1].bytes
      ) {
        samples.current = [];
      }
      samples.current.push({ ts: now, bytes: s.bytes_completed });
      const cutoff = now - RATE_WINDOW_MS;
      while (samples.current.length > 2 && samples.current[0].ts < cutoff) {
        samples.current.shift();
      }
      if (samples.current.length >= 2) {
        const first = samples.current[0];
        const last = samples.current[samples.current.length - 1];
        const dt = (last.ts - first.ts) / 1000;
        if (dt > 0.1) {
          setRate(Math.max(0, (last.bytes - first.bytes) / dt));
        }
      }
      // file-view + manifest-statuses still go through refetch on
      // change. manifest-files is patched directly by the `chunks`
      // listener below.
      const completedDelta = s.chunks_completed + s.chunks_failed - lastCompletedSeen.current;
      const idle = s.chunks_completed + s.chunks_failed === s.chunks_total && s.chunks_total > 0;
      if (completedDelta > 0 && (now - lastInvalidate.current > 500 || idle)) {
        lastInvalidate.current = now;
        lastCompletedSeen.current = s.chunks_completed + s.chunks_failed;
        queryClient.invalidateQueries({ queryKey: ["file-view"] });
        queryClient.invalidateQueries({ queryKey: ["manifest-statuses"] });
      }
    };
    const onChunks = (e: MessageEvent) => {
      const evt = JSON.parse(e.data) as ChunkUpdate;
      // Patch every currently-cached `manifest-files` page that matches
      // this manifest. setQueryData returns undefined when nothing is
      // cached for the key — no-op cost when no manifest page is open.
      const updates = new Map(evt.files.map((f) => [f.path, f.chunks_present]));
      queryClient
        .getQueryCache()
        .findAll({ queryKey: ["manifest-files"] })
        .forEach((query) => {
          const key = query.queryKey as unknown[];
          const depotId = key[2];
          const gid = key[3];
          if (depotId !== evt.depot_id || gid !== evt.manifest_id) return;
          queryClient.setQueryData<ManifestFilesPage>(query.queryKey, (old) => {
            if (!old) return old;
            let changed = false;
            const files = old.files.map((f) => {
              const next = updates.get(f.path);
              if (next != null && next !== f.chunks_present) {
                changed = true;
                return { ...f, chunks_present: next };
              }
              return f;
            });
            return changed ? { ...old, files } : old;
          });
        });
    };
    es.addEventListener("stats", onStats);
    es.addEventListener("chunks", onChunks);
    es.onerror = () => {
      // EventSource auto-reconnects; just leave it alone.
    };
    return () => {
      es.removeEventListener("stats", onStats);
      es.removeEventListener("chunks", onChunks);
      es.close();
    };
  }, [queryClient]);

  if (!stats || stats.chunks_total === 0) return null;

  const done = stats.chunks_completed + stats.chunks_failed;
  const pct = Math.min(100, (done / stats.chunks_total) * 100);
  const bytesPct =
    stats.bytes_total === 0 ? 0 : Math.min(100, (stats.bytes_completed / stats.bytes_total) * 100);
  const remainingBytes = Math.max(0, stats.bytes_total - stats.bytes_completed);
  const etaSecs = rate > 0 ? remainingBytes / rate : null;
  const active = done < stats.chunks_total;

  return (
    <div ref={rootRef} className="fixed top-3 right-3 z-50 w-80">
      <div className="flex items-stretch bg-slate-900 border border-slate-700 rounded-t shadow px-3 py-2 gap-3">
        <span
          className={`self-center inline-block h-2 w-2 rounded-full ${active ? "bg-sky-400 animate-pulse" : stats.chunks_failed > 0 ? "bg-amber-400" : "bg-emerald-400"}`}
        />
        <span className="text-sm font-medium self-center">
          {active ? "Downloading" : stats.chunks_failed > 0 ? "Done (errors)" : "Done"}
        </span>
        <span className="ml-auto text-xs text-slate-400 tabular-nums self-center">
          {formatBytes(stats.bytes_completed)} / {formatBytes(stats.bytes_total)}
        </span>
        {!active && (
          <button
            type="button"
            onClick={() => {
              cancelDownloads().catch(() => {
                /* SSE will redeliver state regardless */
              });
            }}
            className="self-center px-1.5 text-slate-500 hover:text-slate-200"
            aria-label="Dismiss"
            title="Dismiss"
          >
            ×
          </button>
        )}
      </div>
      <div className="h-1 bg-slate-800 overflow-hidden">
        <div
          className="h-full bg-sky-500 transition-[width] duration-300"
          style={{ width: `${bytesPct}%` }}
        />
      </div>
      <div className="px-3 py-3 bg-slate-900 border-x border-b border-slate-700 rounded-b shadow text-sm space-y-1.5">
        <Row label="Chunks">
          <span className="tabular-nums">
            {stats.chunks_completed.toLocaleString()} / {stats.chunks_total.toLocaleString()}
            {stats.chunks_failed > 0 && (
              <span className="text-amber-400"> · {stats.chunks_failed} failed</span>
            )}
          </span>
        </Row>
        <Row label="Progress">
          <span className="tabular-nums">{pct.toFixed(1)}%</span>
        </Row>
        <Row label="Rate">
          <span className="tabular-nums">
            {active ? `${formatBytes(rate)}/s` : <span className="text-slate-500">—</span>}
          </span>
        </Row>
        <Row label="ETA">
          <span className="tabular-nums">
            {active && etaSecs != null && isFinite(etaSecs) ? (
              formatEta(etaSecs)
            ) : (
              <span className="text-slate-500">—</span>
            )}
          </span>
        </Row>
        {active && (
          <div className="pt-1.5">
            <button
              type="button"
              onClick={() => {
                cancelDownloads().catch(() => {
                  /* SSE will redeliver state regardless */
                });
              }}
              className="w-full px-3 py-1 text-xs border border-red-900 bg-red-950/40 text-red-300 rounded hover:bg-red-900/40"
            >
              Cancel
            </button>
          </div>
        )}
        {stats.last_error && (
          <p className="mt-2 text-xs text-red-300 break-words">
            last error: <span className="font-mono">{stats.last_error}</span>
          </p>
        )}
      </div>
    </div>
  );
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-baseline">
      <span className="text-slate-400">{label}</span>
      <span className="ml-auto">{children}</span>
    </div>
  );
}

function formatEta(secs: number): string {
  if (secs < 60) return `${Math.ceil(secs)}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${Math.ceil(secs % 60)}s`;
  return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
}
