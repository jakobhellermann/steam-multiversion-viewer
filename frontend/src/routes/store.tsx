// TODO(ai-review): review for style and correctness
import { createFileRoute, Link } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useState } from "react";

import {
  fetchLibrary,
  fetchStore,
  prune,
  prunePreview,
  type OwnedGame,
  type StoreApp,
  type StoreManifest,
  type StoreManifestRef,
} from "../api";
import { Bytes } from "../components/Bytes";
import { ErrorBox } from "../components/ErrorBox";
import { formatBytes, formatDate } from "../lib/format";

export const Route = createFileRoute("/store")({ component: StorePage });

function keyOf(appId: number, depotId: number, manifestId: string): string {
  return `${appId}/${depotId}/${manifestId}`;
}

// Shared column layout for the manifest header row and each manifest row, so
// their cells stay aligned.
const ROW_GRID = "grid grid-cols-[auto_1fr_7rem_4rem_7rem_7rem] items-center gap-3 px-3";

function refFromKey(key: string): StoreManifestRef {
  const [app_id, depot_id, manifest_id] = key.split("/");
  return { app_id: Number(app_id), depot_id: Number(depot_id), manifest_id };
}

function StorePage() {
  const qc = useQueryClient();
  const store = useQuery({ queryKey: ["store"], queryFn: fetchStore });
  const library = useQuery({ queryKey: ["library"], queryFn: fetchLibrary });

  const names = useMemo(() => {
    const map = new Map<number, string>();
    for (const g of (library.data ?? []) as OwnedGame[]) map.set(g.appid, g.name);
    return map;
  }, [library.data]);

  // Per-manifest exclusive bytes, i.e. what deleting that manifest alone frees.
  const uniqueByKey = useMemo(() => {
    const map = new Map<string, number>();
    for (const app of store.data?.apps ?? []) {
      for (const depot of app.depots) {
        for (const m of depot.manifests) {
          map.set(keyOf(app.app_id, depot.depot_id, m.manifest_id), m.bytes_unique);
        }
      }
    }
    return map;
  }, [store.data]);

  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [includeUnreferenced, setIncludeUnreferenced] = useState(false);
  const [clearEmptyMeta, setClearEmptyMeta] = useState(false);
  const [clearAllMeta, setClearAllMeta] = useState(false);
  const [confirming, setConfirming] = useState(false);

  // The list is the reclaim view: only manifests with chunks on disk. Empty
  // ones (just cached postcards from browsing an app) live in the metadata
  // cleanup cards instead.
  const appsWithChunks = useMemo(
    () =>
      (store.data?.apps ?? [])
        .map((app) => ({
          ...app,
          depots: app.depots
            .map((d) => ({ ...d, manifests: d.manifests.filter((m) => m.bytes_on_disk > 0) }))
            .filter((d) => d.manifests.length > 0),
        }))
        .filter((app) => app.depots.length > 0),
    [store.data],
  );

  // All manifests as prune refs, and those with no chunks on disk.
  const meta = useMemo(() => {
    const all: StoreManifestRef[] = [];
    const empty: StoreManifestRef[] = [];
    for (const app of store.data?.apps ?? []) {
      for (const d of app.depots) {
        for (const m of d.manifests) {
          const ref = { app_id: app.app_id, depot_id: d.depot_id, manifest_id: m.manifest_id };
          all.push(ref);
          if (m.bytes_on_disk === 0) empty.push(ref);
        }
      }
    }
    return { all, empty };
  }, [store.data]);

  const toggle = (key: string) =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  const setMany = (keys: string[], on: boolean) =>
    setSelected((prev) => {
      const next = new Set(prev);
      for (const k of keys) {
        if (on) next.add(k);
        else next.delete(k);
      }
      return next;
    });

  // Single selection frees exactly that manifest's exclusive bytes, already
  // known client-side. Multiple selections are non-additive (shared chunks),
  // so only those need the server.
  const [manifestFreed, setManifestFreed] = useState(0);
  useEffect(() => {
    if (selected.size === 0) {
      setManifestFreed(0);
      return;
    }
    if (selected.size === 1) {
      const [key] = selected;
      setManifestFreed(uniqueByKey.get(key) ?? 0);
      return;
    }
    const free_chunks = [...selected].map(refFromKey);
    let cancelled = false;
    const t = setTimeout(() => {
      prunePreview({ free_chunks }).then((r) => {
        if (!cancelled) setManifestFreed(r.freed_bytes);
      });
    }, 200);
    return () => {
      cancelled = true;
      clearTimeout(t);
    };
  }, [selected, uniqueByKey]);

  const unrefBytes = store.data?.unreferenced.bytes ?? 0;
  const totalFreed = manifestFreed + (includeUnreferenced ? unrefBytes : 0);

  const deleteMetadata = clearAllMeta ? meta.all : clearEmptyMeta ? meta.empty : [];

  const mutation = useMutation({
    mutationFn: () =>
      prune({
        free_chunks: [...selected].map(refFromKey),
        delete_metadata: deleteMetadata,
        include_unreferenced: includeUnreferenced,
      }),
    onSuccess: async () => {
      setSelected(new Set());
      setConfirming(false);
      setIncludeUnreferenced(false);
      setClearEmptyMeta(false);
      setClearAllMeta(false);
      await qc.invalidateQueries({ queryKey: ["store"] });
    },
  });

  const nothingToDo =
    selected.size === 0 && !(includeUnreferenced && unrefBytes > 0) && deleteMetadata.length === 0;

  return (
    <div className="mx-auto max-w-4xl p-8 pb-28">
      <div className="mb-6 flex items-baseline justify-between">
        <h1 className="text-2xl font-bold">Store</h1>
        {store.data && (
          <span className="text-sm text-slate-400">
            <Bytes value={store.data.total_bytes_on_disk} /> on disk ·{" "}
            {store.data.total_chunks_on_disk.toLocaleString()} chunks
          </span>
        )}
      </div>

      {store.isPending && <p className="text-slate-400">Loading…</p>}
      {store.error && <ErrorBox title="Failed to load store" error={store.error as Error} />}

      {store.data && appsWithChunks.length === 0 && (
        <p className="text-slate-400">No downloaded content.</p>
      )}

      {appsWithChunks.map((app) => (
        <AppSection
          key={app.app_id}
          app={app}
          name={names.get(app.app_id) ?? `App ${app.app_id}`}
          selected={selected}
          onToggle={toggle}
          onToggleMany={setMany}
        />
      ))}

      {store.data && store.data.unreferenced.chunks > 0 && (
        <CleanupCard
          checked={includeUnreferenced}
          onChange={setIncludeUnreferenced}
          label={`Unreferenced chunks (${store.data.unreferenced.chunks.toLocaleString()})`}
          bytes={store.data.unreferenced.bytes}
          note="On disk but referenced by no manifest — leftovers from aborted downloads or dropped metadata."
        />
      )}

      {meta.empty.length > 0 && (
        <CleanupCard
          checked={clearEmptyMeta || clearAllMeta}
          disabled={clearAllMeta}
          onChange={setClearEmptyMeta}
          label={`Manifests without chunks (${meta.empty.length})`}
          note="Cached version metadata with nothing downloaded — e.g. from opening an app page."
        />
      )}

      {meta.all.length > 0 && (
        <CleanupCard
          checked={clearAllMeta}
          onChange={setClearAllMeta}
          label={`All manifests (${meta.all.length})`}
          note="Drop every manifest's metadata. Downloaded chunks stay and become unreferenced."
        />
      )}

      {mutation.error && (
        <div className="mt-4">
          <ErrorBox title="Prune failed" error={mutation.error as Error} />
        </div>
      )}

      <div className="fixed inset-x-0 bottom-0 border-t border-slate-800 surface-float">
        <div className="mx-auto flex max-w-4xl flex-wrap items-center gap-x-6 gap-y-2 px-8 py-3">
          <div className="text-sm">
            <span className="text-slate-400">Frees </span>
            <span className="font-medium text-emerald-400 tabular-nums">
              <Bytes value={totalFreed} />
            </span>
            <span className="ml-2 text-slate-500">
              ({selected.size} manifest{selected.size === 1 ? "" : "s"})
            </span>
          </div>

          <div className="ml-auto flex items-center gap-3">
            {confirming && (
              <button
                type="button"
                onClick={() => setConfirming(false)}
                className="text-sm text-slate-400 hover:text-slate-200"
              >
                Cancel
              </button>
            )}
            <button
              type="button"
              disabled={nothingToDo || mutation.isPending}
              onClick={() => (confirming ? mutation.mutate() : setConfirming(true))}
              className={
                confirming
                  ? "rounded bg-red-700 px-4 py-2 text-sm font-medium hover:bg-red-600 disabled:opacity-30"
                  : "rounded bg-slate-700 px-4 py-2 text-sm font-medium hover:bg-slate-600 disabled:opacity-30"
              }
            >
              {mutation.isPending
                ? "Pruning…"
                : confirming
                  ? totalFreed > 0
                    ? `Delete — free ${formatBytes(totalFreed)}`
                    : "Delete"
                  : "Prune"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

function CleanupCard({
  checked,
  onChange,
  label,
  bytes,
  note,
  disabled,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label: string;
  bytes?: number;
  note?: string;
  disabled?: boolean;
}) {
  return (
    <label
      className={`mt-4 block rounded border border-slate-800 p-3 ${
        disabled ? "opacity-50" : "cursor-pointer hover:bg-slate-800/30"
      }`}
    >
      <div className="flex items-center gap-3 text-sm">
        <input
          type="checkbox"
          checked={checked}
          disabled={disabled}
          onChange={(e) => onChange(e.target.checked)}
        />
        <span className="text-slate-300">{label}</span>
        {bytes !== undefined && (
          <span className="ml-auto text-slate-400 tabular-nums">
            <Bytes value={bytes} />
          </span>
        )}
      </div>
      {note && <p className="mt-1 pl-7 text-xs text-slate-500">{note}</p>}
    </label>
  );
}

function AppSection({
  app,
  name,
  selected,
  onToggle,
  onToggleMany,
}: {
  app: StoreApp;
  name: string;
  selected: Set<string>;
  onToggle: (key: string) => void;
  onToggleMany: (keys: string[], on: boolean) => void;
}) {
  const keys = useMemo(
    () =>
      app.depots.flatMap((d) =>
        d.manifests.map((m) => keyOf(app.app_id, d.depot_id, m.manifest_id)),
      ),
    [app],
  );
  const allSelected = keys.length > 0 && keys.every((k) => selected.has(k));
  const someSelected = keys.some((k) => selected.has(k));

  return (
    <section className="mb-4 rounded border border-slate-800">
      <header className="flex items-center gap-3 border-b border-slate-800 bg-slate-900/50 px-3 py-2">
        <input
          type="checkbox"
          checked={allSelected}
          ref={(el) => {
            if (el) el.indeterminate = someSelected && !allSelected;
          }}
          onChange={(e) => onToggleMany(keys, e.target.checked)}
        />
        <Link
          to="/apps/$appid"
          params={{ appid: String(app.app_id) }}
          className="font-medium text-sky-400 hover:underline"
        >
          {name}
        </Link>
        <span className="text-xs text-slate-500">{app.app_id}</span>
        <span className="ml-auto text-sm text-slate-400 tabular-nums">
          <Bytes value={app.bytes_on_disk} />
        </span>
      </header>
      <div className={`${ROW_GRID} border-b border-slate-800 py-1 text-xs text-slate-500`}>
        <span />
        <span>Manifest</span>
        <span>Created</span>
        <span>Status</span>
        <span className="text-right">On disk</span>
        <span className="text-right" title="Freed by deleting only this manifest">
          Exclusive
        </span>
      </div>
      <div>
        {app.depots.map((depot) => (
          <div key={depot.depot_id}>
            {app.depots.length > 1 && (
              <div className="px-3 pt-2 text-xs text-slate-500">Depot {depot.depot_id}</div>
            )}
            {depot.manifests.map((m) => (
              <ManifestRow
                key={m.manifest_id}
                appId={app.app_id}
                depotId={depot.depot_id}
                manifest={m}
                selected={selected.has(keyOf(app.app_id, depot.depot_id, m.manifest_id))}
                onToggle={() => onToggle(keyOf(app.app_id, depot.depot_id, m.manifest_id))}
              />
            ))}
          </div>
        ))}
      </div>
    </section>
  );
}

function ManifestRow({
  appId,
  depotId,
  manifest,
  selected,
  onToggle,
}: {
  appId: number;
  depotId: number;
  manifest: StoreManifest;
  selected: boolean;
  onToggle: () => void;
}) {
  const complete = manifest.chunks_present === manifest.chunks_total;
  return (
    <label
      className={`${ROW_GRID} cursor-pointer py-2 text-sm select-none hover:bg-slate-800/50 ${
        selected ? "bg-red-950/30" : ""
      }`}
    >
      <input type="checkbox" checked={selected} onChange={onToggle} />
      <span className="truncate">
        <Link
          to="/apps/$appid/depots/$depotId/manifests/$manifestId"
          params={{
            appid: String(appId),
            depotId: String(depotId),
            manifestId: manifest.manifest_id,
          }}
          className="font-mono text-xs text-sky-400 hover:underline"
        >
          {manifest.manifest_id}
        </Link>
      </span>
      <span className="text-xs text-slate-500">{formatDate(manifest.creation_time)}</span>
      <span
        className={`text-xs tabular-nums ${complete ? "text-slate-500" : "text-amber-500"}`}
        title={`${manifest.chunks_present} / ${manifest.chunks_total} chunks on disk`}
      >
        {complete
          ? "complete"
          : `${Math.round((100 * manifest.chunks_present) / Math.max(1, manifest.chunks_total))}%`}
      </span>
      <span className="text-right text-slate-300 tabular-nums">
        <Bytes value={manifest.bytes_on_disk} />
      </span>
      <span className="text-right text-xs text-slate-500 tabular-nums">
        <Bytes value={manifest.bytes_unique} />
      </span>
    </label>
  );
}
