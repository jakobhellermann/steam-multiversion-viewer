// TODO(ai-review): review for style and correctness
import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  downloadManifest,
  fetchAppInfo,
  fetchExtraManifests,
  fetchFileView,
  fetchFileViewOptional,
  fetchManifestStatuses,
  fetchTransformedFile,
  fileRawUrl,
  type FileView,
  type ManifestRef,
} from "../api";
import { isTransformablePath, mediaKindForPath } from "../mediaKind";
import { highlight, langForMime, langForPath } from "../syntax";
import { Bytes } from "../Bytes";
import { CompareMenu, diffTargetKey } from "../CompareMenu";
import { ErrorBox } from "../ErrorBox";
import { formatBytes, formatDate } from "../format";

type Search = {
  branch: string;
  path: string;
  /// Diff targets, mirrored from the manifest-detail page so the
  /// "compare to" filter survives navigation. Comma-separated list of
  /// "depot-manifest" or bare "manifest" entries.
  compare_to?: string;
};

export const Route = createFileRoute("/apps/$appid/depots/$depotId/manifests/$manifestId_/file")({
  validateSearch: (search: Record<string, unknown>): Search => ({
    branch: typeof search.branch === "string" ? search.branch : "public",
    path: typeof search.path === "string" ? search.path : "",
    compare_to:
      typeof search.compare_to === "string" && search.compare_to ? search.compare_to : undefined,
  }),
  component: FileViewPage,
});

function FileViewPage() {
  const { appid: appidParam, depotId: depotIdParam, manifestId } = Route.useParams();
  const { branch, path, compare_to } = Route.useSearch();
  const navigate = useNavigate({ from: Route.fullPath });
  const appid = Number(appidParam);
  const depotId = Number(depotIdParam);
  const queryClient = useQueryClient();

  const view = useQuery({
    queryKey: ["file-view", appid, depotId, manifestId, branch, path],
    queryFn: () => fetchFileView(appid, depotId, manifestId, branch, path),
    enabled: path.length > 0,
    // chunks_present + auto-fetched content depend on what's on disk, so
    // re-run when navigated back to.
    refetchOnMount: "always",
  });

  const appInfoQuery = useQuery({
    queryKey: ["app", appid],
    queryFn: () => fetchAppInfo(appid),
  });
  const extraQuery = useQuery({
    queryKey: ["extra-manifests", appid],
    queryFn: () => fetchExtraManifests(appid),
  });

  const diffTargets = useMemo(
    () => new Set(compare_to ? compare_to.split(",").filter(Boolean) : []),
    [compare_to],
  );
  const setDiffTargets = useCallback(
    (next: Set<string>) => {
      navigate({
        search: (prev) => ({
          ...prev,
          compare_to: next.size === 0 ? undefined : [...next].join(","),
        }),
        replace: true,
      });
    },
    [navigate],
  );

  // Resolve URL diff-target keys back to concrete manifest refs.
  const diffRefs = useMemo<ManifestRef[]>(() => {
    if (!appInfoQuery.data || diffTargets.size === 0) return [];
    const all: { depot_id: number; manifest_id: string; branch: string }[] = [];
    const seenAll = new Set<string>();
    for (const d of appInfoQuery.data.depots) {
      for (const m of d.manifests) {
        const k = `${d.depot_id}-${m.manifest_id}`;
        if (seenAll.has(k)) continue;
        seenAll.add(k);
        all.push({ depot_id: d.depot_id, manifest_id: m.manifest_id, branch: m.branch });
      }
    }
    for (const e of extraQuery.data ?? []) {
      const k = `${e.depot_id}-${e.manifest_id}`;
      if (seenAll.has(k)) continue;
      seenAll.add(k);
      all.push({
        depot_id: e.depot_id,
        manifest_id: e.manifest_id,
        branch: e.branch ?? "public",
      });
    }
    const out: ManifestRef[] = [];
    const seenRef = new Set<string>();
    for (const entry of all) {
      const canonical = diffTargetKey(entry.depot_id, entry.manifest_id, depotId);
      const composite = `${entry.depot_id}-${entry.manifest_id}`;
      if (
        (diffTargets.has(canonical) ||
          diffTargets.has(entry.manifest_id) ||
          diffTargets.has(composite)) &&
        !seenRef.has(composite)
      ) {
        seenRef.add(composite);
        out.push(entry);
      }
    }
    return out;
  }, [appInfoQuery.data, extraQuery.data, diffTargets, depotId]);

  // Status query gives us creation_time per manifest for the menu's
  // chronological sort. Same key as elsewhere so the cache is shared.
  const compareRefs = useMemo<ManifestRef[]>(() => {
    if (!appInfoQuery.data) return [];
    const seen = new Set<string>();
    const refs: ManifestRef[] = [];
    for (const d of appInfoQuery.data.depots) {
      for (const m of d.manifests) {
        const key = `${d.depot_id}/${m.manifest_id}`;
        if (seen.has(key)) continue;
        seen.add(key);
        refs.push({ depot_id: d.depot_id, manifest_id: m.manifest_id, branch: m.branch });
      }
    }
    for (const e of extraQuery.data ?? []) {
      const key = `${e.depot_id}/${e.manifest_id}`;
      if (seen.has(key)) continue;
      seen.add(key);
      refs.push({
        depot_id: e.depot_id,
        manifest_id: e.manifest_id,
        branch: e.branch ?? "public",
      });
    }
    return refs;
  }, [appInfoQuery.data, extraQuery.data]);
  const statusQuery = useQuery({
    queryKey: ["manifest-statuses", appid, compareRefs],
    queryFn: () => fetchManifestStatuses(appid, compareRefs),
    enabled: compareRefs.length > 0,
  });

  // One file-view query per diff target. useQueries handles the dynamic
  // count safely; useQuery in a loop would break the hook rule.
  const targetViews = useQueries({
    queries: diffRefs.map((ref) => ({
      queryKey: ["file-view", appid, ref.depot_id, ref.manifest_id, ref.branch, path],
      queryFn: () => fetchFileViewOptional(appid, ref.depot_id, ref.manifest_id, ref.branch, path),
      enabled: path.length > 0,
    })),
  });

  const download = useMutation({
    mutationFn: () => downloadManifest(appid, depotId, manifestId, { branch, paths: [path] }),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["file-view", appid, depotId, manifestId, branch, path],
      });
    },
  });

  return (
    <div className="p-8 max-w-6xl mx-auto">
      <nav className="text-sm text-slate-400 mb-4">
        <Link
          to="/apps/$appid/depots/$depotId/manifests/$manifestId"
          params={{ appid: appidParam, depotId: depotIdParam, manifestId }}
          search={{ branch, compare_to }}
          className="hover:underline"
        >
          ← Manifest
        </Link>
      </nav>

      <div className="flex items-baseline gap-3">
        <h1 className="font-mono text-sm break-all flex-1">{path}</h1>
        {appInfoQuery.data && (
          <CompareMenu
            appInfo={appInfoQuery.data}
            extras={extraQuery.data ?? []}
            statuses={statusQuery.data}
            currentDepotId={depotId}
            currentManifestId={manifestId}
            selected={diffTargets}
            onChange={setDiffTargets}
          />
        )}
      </div>

      {view.isPending && <p className="mt-4 text-slate-400">Loading…</p>}
      {view.error && <ErrorBox title="Failed to load file" error={view.error as Error} />}

      {view.data && (
        <FileMeta
          view={view.data}
          onDownload={() => download.mutate()}
          downloadPending={download.isPending}
        />
      )}

      {diffRefs.length > 0 && (
        <section className="mt-8">
          <h2 className="text-sm font-semibold text-slate-400 mb-3">Compared to</h2>
          <div className="space-y-2">
            {diffRefs.map((ref, i) => {
              const status = (statusQuery.data ?? []).find(
                (s) => s.depot_id === ref.depot_id && s.manifest_id === ref.manifest_id,
              );
              return (
                <DiffTargetBlock
                  key={`${ref.depot_id}-${ref.manifest_id}`}
                  base={view.data ?? null}
                  ref_={ref}
                  creationTime={status?.creation_time ?? 0}
                  query={targetViews[i]}
                  rawSrc={fileRawUrl(appid, ref.depot_id, ref.manifest_id, ref.branch, path)}
                  locator={{
                    appid,
                    depotId: ref.depot_id,
                    manifestId: ref.manifest_id,
                    branch: ref.branch,
                    path,
                  }}
                />
              );
            })}
          </div>
        </section>
      )}

      {view.data && (
        <FilePreview
          view={view.data}
          rawSrc={fileRawUrl(appid, depotId, manifestId, branch, path)}
          locator={{ appid, depotId, manifestId, branch, path }}
        />
      )}
    </div>
  );
}

function FileMeta({
  view,
  onDownload,
  downloadPending,
}: {
  view: FileView;
  onDownload: () => void;
  downloadPending: boolean;
}) {
  const fullyOnDisk = view.chunks_present === view.chunk_count;
  const missingChunks = view.chunk_count - view.chunks_present;
  return (
    <p className="mt-1 flex items-baseline gap-3 text-sm text-slate-400">
      <span>
        <Bytes value={view.size} />
      </span>
      {view.kind === "symlink" && view.linktarget && (
        <span className="text-slate-500">
          → <span className="font-mono text-slate-300">{view.linktarget}</span>
        </span>
      )}
      {view.kind === "file" &&
        view.chunk_count > 0 &&
        (fullyOnDisk ? (
          <span className="text-emerald-400">on disk ✓</span>
        ) : (
          <button
            type="button"
            onClick={onDownload}
            disabled={downloadPending}
            className="text-xs px-2 py-0.5 border border-sky-700 bg-sky-950/40 rounded hover:bg-sky-900/40 disabled:opacity-40"
          >
            {downloadPending
              ? "enqueuing…"
              : `download ${missingChunks} missing chunk${missingChunks === 1 ? "" : "s"}`}
          </button>
        ))}
    </p>
  );
}

function MediaPlayer({ kind, src }: { kind: "audio" | "video"; src: string }) {
  const ref = useRef<HTMLMediaElement>(null);
  // Auto-focus the player on mount so space immediately toggles play.
  // Without focus, space scrolls the page instead.
  useEffect(() => {
    ref.current?.focus();
  }, [src]);
  const className = kind === "audio" ? "w-full" : "max-w-full bg-slate-950 rounded";
  if (kind === "audio") {
    return (
      <audio
        ref={ref as React.RefObject<HTMLAudioElement>}
        src={src}
        controls
        className={className}
      />
    );
  }
  return (
    <video
      ref={ref as React.RefObject<HTMLVideoElement>}
      src={src}
      controls
      className={className}
    />
  );
}

type FileLocator = {
  appid: number;
  depotId: number;
  manifestId: string;
  branch: string;
  path: string;
};

function TransformedPreview({ locator }: { locator: FileLocator }) {
  const query = useQuery({
    queryKey: [
      "file-transformed",
      locator.appid,
      locator.depotId,
      locator.manifestId,
      locator.branch,
      locator.path,
    ],
    queryFn: () =>
      fetchTransformedFile(
        locator.appid,
        locator.depotId,
        locator.manifestId,
        locator.branch,
        locator.path,
      ),
  });
  if (query.isPending) {
    return <p className="text-sm text-slate-500">Decompiling… (first run can take a while)</p>;
  }
  if (query.error) {
    return (
      <p className="text-sm text-red-300">
        Decompile failed: {query.error instanceof Error ? query.error.message : String(query.error)}
      </p>
    );
  }
  if (query.data == null) {
    return <p className="text-sm text-slate-500">Not transformable.</p>;
  }
  return <HighlightedPre code={query.data.text} lang={langForMime(query.data.mime)} />;
}

function FilePreview({
  view,
  rawSrc,
  locator,
  showHeader = true,
}: {
  view: FileView;
  /// URL to the raw bytes for media rendering. When the file extension
  /// looks like an image/audio/video, this is what the browser fetches
  /// directly. Omitted on the base preview during loading.
  rawSrc?: string;
  /// Identity passed to the transformer query so a .dll can be
  /// auto-decompiled (and a spinner shown while we wait).
  locator?: FileLocator;
  showHeader?: boolean;
}) {
  if (view.kind !== "file") {
    return null;
  }
  const header = showHeader ? (
    <h2 className="text-sm font-semibold text-slate-400 mb-2">Preview</h2>
  ) : null;
  const sectionClass = showHeader ? "mt-6" : "";
  const media = rawSrc ? mediaKindForPath(view.path) : null;
  if (media === "image") {
    return (
      <section className={sectionClass}>
        {header}
        <img
          src={rawSrc}
          alt={view.path}
          className="max-w-full bg-slate-950 border border-slate-800 rounded"
        />
      </section>
    );
  }
  if (media === "audio" && rawSrc) {
    return (
      <section className={sectionClass}>
        {header}
        <MediaPlayer kind="audio" src={rawSrc} />
      </section>
    );
  }
  if (media === "video" && rawSrc) {
    return (
      <section className={sectionClass}>
        {header}
        <MediaPlayer kind="video" src={rawSrc} />
      </section>
    );
  }
  if (view.content_kind === "binary") {
    if (locator && isTransformablePath(view.path)) {
      return (
        <section className={sectionClass}>
          {header}
          <TransformedPreview locator={locator} />
        </section>
      );
    }
    return (
      <section className={sectionClass}>
        {header}
        <p className="text-sm text-slate-500">
          Binary file — {formatBytes(view.size)}. No inline preview.
        </p>
      </section>
    );
  }
  if (view.content_kind === "too_large") {
    return (
      <section className={sectionClass}>
        {header}
        <p className="text-sm text-slate-500">
          File is {formatBytes(view.size)}; preview cap is {formatBytes(view.preview_cap_bytes)}.
        </p>
      </section>
    );
  }
  if (view.content_kind === "text" && view.content != null) {
    return (
      <section className={sectionClass}>
        {header}
        <HighlightedPre code={view.content} lang={langForPath(view.path)} />
      </section>
    );
  }
  return null;
}

function HighlightedPre({ code, lang }: { code: string; lang: ReturnType<typeof langForPath> }) {
  const html = useQuery({
    queryKey: ["syntax-highlight", lang, code.length, code.slice(0, 64)],
    queryFn: () => (lang ? highlight(code, lang) : Promise.resolve(null)),
    enabled: lang != null,
    staleTime: Infinity,
    gcTime: Infinity,
  });
  // Shiki emits its own <pre> with the theme background; wrap so our
  // own padding/border/scroll behavior stays consistent.
  if (html.data) {
    return (
      <div
        className="text-xs overflow-x-auto rounded border border-slate-800 [&_pre]:!bg-slate-950 [&_pre]:!p-3 [&_pre]:!m-0"
        dangerouslySetInnerHTML={{ __html: html.data }}
      />
    );
  }
  return (
    <pre className="p-3 bg-slate-950 border border-slate-800 rounded text-xs whitespace-pre-wrap wrap-break-word font-mono overflow-x-auto">
      {code}
    </pre>
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

function DiffTargetBlock({
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
    <div className="border border-slate-800 rounded">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        disabled={query.isPending || targetMissing}
        aria-expanded={open}
        className="w-full flex items-baseline gap-2 px-3 py-1.5 text-sm text-left hover:bg-slate-800/40 disabled:cursor-default disabled:hover:bg-transparent"
      >
        <span aria-hidden="true" className="inline-block w-3 text-slate-500">
          {query.isPending || targetMissing ? "" : open ? "▼︎" : "▶︎"}
        </span>
        <span className="font-medium text-slate-200">{ref_.branch}</span>
        {creationTime > 0 && (
          <span className="text-xs text-slate-500 tabular-nums">{formatDate(creationTime)}</span>
        )}
        <span className="font-mono tabular-nums text-xs text-slate-500">
          depot {ref_.depot_id} · {ref_.manifest_id.slice(0, 12)}…
        </span>
        <span className={`ml-auto text-xs tabular-nums ${summaryClass}`}>{summary}</span>
      </button>
      {open && (
        <div className="px-3 py-2 border-t border-slate-800 space-y-2">
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
