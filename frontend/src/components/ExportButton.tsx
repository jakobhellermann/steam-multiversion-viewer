import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import {
  cancelExport,
  exportManifest,
  fetchConfig,
  fetchExportStatus,
  type AppId,
  type ExportStatus,
  type GameInfo,
} from "../data/api";
import { exportFolderName, joinExportPath } from "../lib/exportName";
import { formatBytes } from "../lib/format";
import { queryKeys } from "../data/queryKeys";
import { useServerEvent } from "../data/serverEvents";

export function ExportButton({
  appid,
  appName,
  depotId,
  manifestId,
  branch,
  gameInfo,
}: {
  appid: AppId;
  appName: string | undefined;
  depotId: number;
  manifestId: string;
  branch: string;
  gameInfo: GameInfo | undefined;
}) {
  const qc = useQueryClient();
  const status = useQuery({
    queryKey: queryKeys.exportStatus(),
    queryFn: fetchExportStatus,
    refetchInterval: (q) => (q.state.data?.state === "running" ? 1000 : false),
  });
  const config = useQuery({ queryKey: queryKeys.config(), queryFn: fetchConfig });
  const subdir = exportFolderName(appName, appid, gameInfo, manifestId);
  const destination = config.data ? joinExportPath(config.data.export_dir, subdir) : undefined;

  // The backend only reports `idle`/`running`; the outcome arrives once as `export_finished`.
  const [finished, setFinished] = useState<ExportStatus | null>(null);
  useServerEvent("export_finished", (data) => {
    setFinished(data as ExportStatus);
    void qc.invalidateQueries({ queryKey: queryKeys.exportStatus() });
  });

  const start = useMutation({
    mutationFn: () => exportManifest(appid, depotId, manifestId, { branch, subdir }),
    onSuccess: (s) => {
      setFinished(null);
      qc.setQueryData(queryKeys.exportStatus(), s);
    },
  });
  const cancel = useMutation({
    mutationFn: cancelExport,
    onSuccess: (s) => qc.setQueryData(queryKeys.exportStatus(), s),
  });

  // The backend runs one export at a time, so the status is global — only
  // treat it as ours when it names this manifest.
  const isMine = (s: ExportStatus | null | undefined) =>
    s?.target?.app_id === appid &&
    s.target.depot_id === depotId &&
    s.target.manifest_id === manifestId;
  const running = status.data?.state === "running";
  const blocked = running && !isMine(status.data);
  const current = running ? status.data : finished;
  const shown = isMine(current) ? current : null;

  // A failure stays put — its message is the whole point.
  useEffect(() => {
    if (finished?.state !== "done" && finished?.state !== "cancelled") return;
    const t = window.setTimeout(() => setFinished(null), 3000);
    return () => window.clearTimeout(t);
  }, [finished]);

  const error = shown?.error ?? (start.error as Error | null)?.message;

  return (
    <div className="ml-auto text-right">
      <div className="flex items-center gap-3">
        {shown && <ExportProgress status={shown} />}
        <button
          type="button"
          onClick={() => (running ? cancel.mutate() : start.mutate())}
          disabled={blocked || start.isPending || cancel.isPending}
          title={blocked ? "Another export is running" : running ? undefined : destination}
          className="rounded border border-emerald-800 bg-emerald-950/40 px-3 py-1.5 text-sm whitespace-nowrap hover:bg-emerald-900/40 disabled:opacity-50"
        >
          {running && !blocked ? "Cancel export" : start.isPending ? "Starting…" : "Export to disk"}
        </button>
      </div>
      {error && <p className="mt-2 max-w-96 text-xs wrap-break-word text-red-300">{error}</p>}
    </div>
  );
}

function ExportProgress({ status }: { status: ExportStatus }) {
  const pct =
    status.bytes_total === 0 ? 0 : Math.min(100, (status.bytes_written / status.bytes_total) * 100);
  const label = {
    idle: "",
    running: "Exporting",
    done: "Exported",
    failed: "Export failed",
    cancelled: "Export cancelled",
  }[status.state];

  return (
    <div className="w-56 text-xs text-slate-400">
      <div className="flex items-baseline gap-2">
        <span
          title={status.current_path ?? status.target_dir ?? undefined}
          className={status.state === "failed" ? "text-red-300" : ""}
        >
          {label}
        </span>
        <span className="ml-auto tabular-nums">
          {status.files_done.toLocaleString()} / {status.files_total.toLocaleString()} ·{" "}
          {formatBytes(status.bytes_written)}
        </span>
      </div>
      <div className="mt-1 h-1 overflow-hidden rounded bg-slate-800">
        <div
          className={`h-full transition-[width] ${status.state === "failed" ? "bg-red-500" : "bg-emerald-500"}`}
          style={{ width: `${status.state === "done" ? 100 : pct}%` }}
        />
      </div>
    </div>
  );
}
