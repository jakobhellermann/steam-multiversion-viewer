// TODO(ai-review): review for style and correctness
import { createFileRoute, Link } from "@tanstack/react-router";
import { useMutation, useQuery, keepPreviousData } from "@tanstack/react-query";
import {
  downloadManifest,
  fetchManifestFiles,
  fetchManifestInfo,
  type EnqueueSummary,
  type ManifestFile,
  type ManifestInfo,
} from "../api";
import { Bytes } from "../Bytes";
import { formatBytes } from "../format";

type Search = {
  branch: string;
  offset: number;
  limit: number;
};

export const Route = createFileRoute("/apps/$appid/depots/$depotId/manifests/$gid")({
  validateSearch: (search: Record<string, unknown>): Search => ({
    branch: typeof search.branch === "string" ? search.branch : "public",
    offset: typeof search.offset === "number" ? search.offset : 0,
    limit: typeof search.limit === "number" ? search.limit : 100,
  }),
  component: ManifestDetail,
});

function ManifestDetail() {
  const { appid: appidParam, depotId: depotIdParam, gid } = Route.useParams();
  const { branch, offset, limit } = Route.useSearch();
  const appid = Number(appidParam);
  const depotId = Number(depotIdParam);

  const info = useQuery({
    queryKey: ["manifest-info", appid, depotId, gid, branch],
    queryFn: () => fetchManifestInfo(appid, depotId, gid, branch),
  });
  const files = useQuery({
    queryKey: ["manifest-files", appid, depotId, gid, branch, offset, limit],
    queryFn: () => fetchManifestFiles(appid, depotId, gid, branch, offset, limit),
    placeholderData: keepPreviousData,
    // chunks_present changes as the download manager makes progress, so
    // we refetch every time we mount — `staleTime: Infinity` globally
    // would otherwise hide it.
    refetchOnMount: "always",
  });

  const downloadAll = useMutation({
    mutationFn: () => downloadManifest(appid, depotId, gid, { branch }),
  });

  return (
    <div className="p-8 max-w-6xl mx-auto">
      <nav className="text-sm text-slate-400 mb-4">
        <Link to="/apps/$appid" params={{ appid: appidParam }} className="hover:underline">
          ← App {appid}
        </Link>
      </nav>

      {info.isPending && <p className="text-slate-400">Loading manifest…</p>}
      {info.error && <ErrorBox title="Failed to load manifest" error={info.error as Error} />}
      {info.data && (
        <ManifestHeader
          info={info.data}
          branch={branch}
          onDownload={() => downloadAll.mutate()}
          downloadPending={downloadAll.isPending}
          downloadResult={downloadAll.data}
          downloadError={downloadAll.error as Error | null}
        />
      )}

      <section className="mt-8">
        <div className="flex items-baseline gap-4 mb-3">
          <h2 className="text-xl font-semibold">Files</h2>
          {info.data && (
            <span className="text-sm text-slate-400 tabular-nums">
              {info.data.file_count.toLocaleString()} total
            </span>
          )}
          {files.isFetching && !files.isPending && (
            <span className="text-xs text-slate-500">refreshing…</span>
          )}
        </div>

        {files.error && <ErrorBox title="Failed to load files" error={files.error as Error} />}

        {files.data && (
          <>
            <FilesTable
              files={files.data.files}
              appid={appidParam}
              depotId={depotIdParam}
              gid={gid}
              branch={branch}
            />
            <Pager
              offset={files.data.offset}
              limit={files.data.limit}
              total={files.data.file_count}
            />
          </>
        )}
      </section>
    </div>
  );
}

function ManifestHeader({
  info,
  onDownload,
  downloadPending,
  downloadResult,
  downloadError,
}: {
  info: ManifestInfo;
  branch: string;
  onDownload: () => void;
  downloadPending: boolean;
  downloadResult: EnqueueSummary | undefined;
  downloadError: Error | null;
}) {
  return (
    <div>
      <div className="flex items-start gap-4">
        <h1 className="text-2xl font-bold">Manifest</h1>
        <button
          type="button"
          onClick={onDownload}
          disabled={downloadPending}
          className="ml-auto px-3 py-1.5 text-sm border border-sky-700 bg-sky-950/40 rounded hover:bg-sky-900/40 disabled:opacity-50"
        >
          {downloadPending ? "Enqueuing…" : "Download all"}
        </button>
      </div>
      {downloadResult && (
        <p className="mt-2 text-xs text-slate-400">
          {downloadResult.enqueued_chunks > 0
            ? `Enqueued ${downloadResult.enqueued_chunks.toLocaleString()} chunks (${formatBytes(downloadResult.enqueued_bytes)} compressed).`
            : "Nothing to download — everything is already cached."}
          {downloadResult.already_present_chunks > 0 && (
            <span> {downloadResult.already_present_chunks.toLocaleString()} already on disk.</span>
          )}
        </p>
      )}
      {downloadError && (
        <p className="mt-2 text-xs text-red-300">Download failed: {downloadError.message}</p>
      )}
      <dl className="mt-4 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
        <dt className="text-slate-400">Manifest ID</dt>
        <dd className="font-mono tabular-nums break-all">{info.manifest_id}</dd>
        <dt className="text-slate-400">Depot</dt>
        <dd className="tabular-nums">{info.depot_id}</dd>
        <dt className="text-slate-400">Created</dt>
        <dd className="tabular-nums">{formatTime(info.creation_time)}</dd>
        <dt className="text-slate-400">Files</dt>
        <dd className="tabular-nums">{info.file_count.toLocaleString()}</dd>
        <dt className="text-slate-400">Size (uncompressed)</dt>
        <dd className="tabular-nums">
          <Bytes value={info.size_uncompressed} />
        </dd>
        <dt className="text-slate-400">Size (compressed)</dt>
        <dd className="tabular-nums">
          <Bytes value={info.size_compressed} />
        </dd>
      </dl>
    </div>
  );
}

function FilesTable({
  files,
  appid,
  depotId,
  gid,
  branch,
}: {
  files: ManifestFile[];
  appid: string;
  depotId: string;
  gid: string;
  branch: string;
}) {
  if (files.length === 0) {
    return <p className="text-slate-500 text-sm">No files in this page.</p>;
  }
  return (
    <table className="w-full text-left text-sm">
      <thead>
        <tr className="border-b border-slate-700 text-slate-400">
          <th className="px-3 py-2">Path</th>
          <th className="px-3 py-2 text-right">Size</th>
          <th className="px-3 py-2 text-right">Chunks</th>
        </tr>
      </thead>
      <tbody>
        {files.map((f) => (
          <tr
            key={f.path}
            className="border-b border-slate-800 last:border-b-0 hover:bg-slate-900/40"
          >
            <td className="px-3 py-1.5 font-mono text-xs break-all">
              <Link
                to="/apps/$appid/depots/$depotId/manifests/$gid/file"
                params={{ appid, depotId, gid }}
                search={{ branch, path: f.path }}
                className="text-sky-400 hover:underline"
              >
                {f.path}
              </Link>
              {f.linktarget && <span className="text-slate-500"> → {f.linktarget}</span>}
            </td>
            <td className="px-3 py-1.5 text-right tabular-nums">
              <Bytes value={f.size} />
            </td>
            <td className="px-3 py-1.5 text-right tabular-nums text-slate-400">
              <ChunkPresence present={f.chunks_present} total={f.chunk_count} />
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function ChunkPresence({ present, total }: { present: number; total: number }) {
  if (total === 0) return <span className="text-slate-600">—</span>;
  if (present === total)
    return (
      <span className="text-emerald-400">
        {total}/{total}
      </span>
    );
  if (present === 0) return <span className="text-slate-500">0/{total}</span>;
  return (
    <span className="text-amber-300">
      {present}/{total}
    </span>
  );
}

function Pager({ offset, limit, total }: { offset: number; limit: number; total: number }) {
  const navigate = Route.useNavigate();
  const end = offset + limit;
  const hasPrev = offset > 0;
  const hasNext = end < total;

  const go = (nextOffset: number) =>
    navigate({
      search: (s) => ({ ...s, offset: Math.max(0, nextOffset) }),
    });

  return (
    <div className="mt-3 flex items-center gap-3 text-sm text-slate-400">
      <button
        type="button"
        onClick={() => go(offset - limit)}
        disabled={!hasPrev}
        className="px-3 py-1 border border-slate-700 rounded disabled:opacity-30 hover:bg-slate-800 disabled:hover:bg-transparent"
      >
        ← Prev
      </button>
      <span className="tabular-nums">
        {offset + 1}–{Math.min(end, total)} of {total.toLocaleString()}
      </span>
      <button
        type="button"
        onClick={() => go(offset + limit)}
        disabled={!hasNext}
        className="px-3 py-1 border border-slate-700 rounded disabled:opacity-30 hover:bg-slate-800 disabled:hover:bg-transparent"
      >
        Next →
      </button>
      <label className="ml-auto flex items-center gap-2">
        <span>Per page</span>
        <select
          value={limit}
          onChange={(e) =>
            navigate({
              search: (s) => ({ ...s, limit: Number(e.target.value), offset: 0 }),
            })
          }
          className="bg-slate-900 border border-slate-700 rounded px-2 py-1"
        >
          {[100, 250, 500, 1000, 2500].map((n) => (
            <option key={n} value={n}>
              {n}
            </option>
          ))}
        </select>
      </label>
    </div>
  );
}

function ErrorBox({ title, error }: { title: string; error: Error }) {
  return (
    <div className="mt-4 p-4 border border-red-900 bg-red-950/40 rounded">
      <p className="text-red-400 font-medium mb-1">{title}</p>
      <p className="text-red-300 text-sm font-mono break-words">{error.message}</p>
    </div>
  );
}

function formatTime(unix: number): string {
  if (!unix) return "—";
  return new Date(unix * 1000).toISOString().replace("T", " ").slice(0, 19) + " UTC";
}
