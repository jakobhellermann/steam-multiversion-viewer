// TODO(ai-review): review for style and correctness
import { createFileRoute, Link } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { downloadManifest, fetchFileView, type FileView } from "../api";

type Search = {
  branch: string;
  path: string;
};

export const Route = createFileRoute("/apps/$appid/depots/$depotId/manifests/$gid_/file")({
  validateSearch: (search: Record<string, unknown>): Search => ({
    branch: typeof search.branch === "string" ? search.branch : "public",
    path: typeof search.path === "string" ? search.path : "",
  }),
  component: FileViewPage,
});

function FileViewPage() {
  const { appid: appidParam, depotId: depotIdParam, gid } = Route.useParams();
  const { branch, path } = Route.useSearch();
  const appid = Number(appidParam);
  const depotId = Number(depotIdParam);
  const queryClient = useQueryClient();

  const view = useQuery({
    queryKey: ["file-view", appid, depotId, gid, branch, path],
    queryFn: () => fetchFileView(appid, depotId, gid, branch, path),
    enabled: path.length > 0,
    // chunks_present + auto-fetched content depend on what's on disk, so
    // re-run when navigated back to.
    refetchOnMount: "always",
  });

  const download = useMutation({
    mutationFn: () => downloadManifest(appid, depotId, gid, { branch, paths: [path] }),
    onSuccess: () => {
      // After enqueuing, the chunks land asynchronously. Don't auto-refetch
      // here; the user can hit Reload once the drawer settles.
      queryClient.invalidateQueries({ queryKey: ["file-view", appid, depotId, gid, branch, path] });
    },
  });

  return (
    <div className="p-8 max-w-6xl mx-auto">
      <nav className="text-sm text-slate-400 mb-4">
        <Link
          to="/apps/$appid/depots/$depotId/manifests/$gid"
          params={{ appid: appidParam, depotId: depotIdParam, gid }}
          search={{ branch, offset: 0, limit: 100 }}
          className="hover:underline"
        >
          ← Manifest
        </Link>
      </nav>

      <h1 className="font-mono text-sm break-all">{path}</h1>

      {view.isPending && <p className="mt-4 text-slate-400">Loading…</p>}
      {view.error && (
        <div className="mt-4 p-4 border border-red-900 bg-red-950/40 rounded">
          <p className="text-red-400 font-medium mb-1">Failed to load file</p>
          <p className="text-red-300 text-sm font-mono break-words">
            {(view.error as Error).message}
          </p>
        </div>
      )}

      {view.data && (
        <FileBody
          view={view.data}
          onDownload={() => download.mutate()}
          downloadPending={download.isPending}
        />
      )}
    </div>
  );
}

function FileBody({
  view,
  onDownload,
  downloadPending,
}: {
  view: FileView;
  onDownload: () => void;
  downloadPending: boolean;
}) {
  const fullyOnDisk = view.chunks_present === view.chunk_count;
  return (
    <>
      <dl className="mt-4 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
        <dt className="text-slate-400">Size</dt>
        <dd className="tabular-nums">{formatBytes(view.size)}</dd>
        <dt className="text-slate-400">Chunks on disk</dt>
        <dd className="tabular-nums">
          {view.chunks_present}/{view.chunk_count}
          {fullyOnDisk && view.chunk_count > 0 && <span className="text-emerald-400 ml-2">✓</span>}
        </dd>
        {view.linktarget && (
          <>
            <dt className="text-slate-400">Link target</dt>
            <dd className="font-mono break-all">{view.linktarget}</dd>
          </>
        )}
      </dl>

      <div className="mt-4 flex items-center gap-3">
        <button
          type="button"
          onClick={onDownload}
          disabled={
            downloadPending || fullyOnDisk || view.kind !== "file" || view.chunk_count === 0
          }
          className="px-3 py-1.5 text-sm border border-sky-700 bg-sky-950/40 rounded hover:bg-sky-900/40 disabled:opacity-40"
        >
          {fullyOnDisk
            ? "On disk"
            : downloadPending
              ? "Enqueuing…"
              : `Download ${view.chunk_count - view.chunks_present} missing chunk${view.chunk_count - view.chunks_present === 1 ? "" : "s"}`}
        </button>
      </div>

      <FilePreview view={view} />
    </>
  );
}

function FilePreview({ view }: { view: FileView }) {
  if (view.kind !== "file") {
    return null;
  }
  if (view.content_kind === "binary") {
    return (
      <section className="mt-6">
        <h2 className="text-sm font-semibold text-slate-400 mb-2">Preview</h2>
        <p className="text-sm text-slate-500">
          Binary file — {formatBytes(view.size)}. No inline preview.
        </p>
      </section>
    );
  }
  if (view.content_kind === "too_large") {
    return (
      <section className="mt-6">
        <h2 className="text-sm font-semibold text-slate-400 mb-2">Preview</h2>
        <p className="text-sm text-slate-500">
          File is {formatBytes(view.size)}; preview cap is {formatBytes(view.preview_cap_bytes)}.
        </p>
      </section>
    );
  }
  if (view.content_kind === "text" && view.content != null) {
    return (
      <section className="mt-6">
        <h2 className="text-sm font-semibold text-slate-400 mb-2">Preview</h2>
        <pre className="p-3 bg-slate-950 border border-slate-800 rounded text-xs whitespace-pre-wrap break-words font-mono overflow-x-auto">
          {view.content}
        </pre>
      </section>
    );
  }
  return null;
}

function formatBytes(n: number): string {
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return i === 0 ? `${n} ${units[0]}` : `${v.toFixed(1)} ${units[i]}`;
}
