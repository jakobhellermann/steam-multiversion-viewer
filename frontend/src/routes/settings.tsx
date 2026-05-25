// TODO(ai-review): review for style and correctness
import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { fetchConfig, patchConfig, type Config } from "../api";

export const Route = createFileRoute("/settings")({ component: SettingsPage });

function SettingsPage() {
  const qc = useQueryClient();
  const query = useQuery({ queryKey: ["config"], queryFn: fetchConfig });

  return (
    <div className="mx-auto max-w-4xl p-8">
      <h1 className="mb-6 text-2xl font-bold">Settings</h1>
      {query.isPending && <p className="text-slate-400">Loading…</p>}
      {query.error && <p className="text-sm text-red-400">{(query.error as Error).message}</p>}
      {query.data && (
        <SettingsForm
          config={query.data}
          onSaved={() => qc.invalidateQueries({ queryKey: ["config"] })}
        />
      )}
    </div>
  );
}

function SettingsForm({ config, onSaved }: { config: Config; onSaved: () => void }) {
  const [storeRoot, setStoreRoot] = useState(config.store_root);
  const [mountpoint, setMountpoint] = useState(config.mountpoint);

  useEffect(() => {
    setStoreRoot(config.store_root);
    setMountpoint(config.mountpoint);
  }, [config.store_root, config.mountpoint]);

  const mutation = useMutation({
    mutationFn: () => patchConfig({ store_root: storeRoot, mountpoint }),
    onSuccess: () => onSaved(),
  });

  const dirty = storeRoot !== config.store_root || mountpoint !== config.mountpoint;

  return (
    <form
      className="space-y-4"
      onSubmit={(e) => {
        e.preventDefault();
        if (dirty) mutation.mutate();
      }}
    >
      <label className="block">
        <span className="text-sm text-slate-400">Chunk & manifest store root</span>
        <input
          type="text"
          value={storeRoot}
          onChange={(e) => setStoreRoot(e.target.value)}
          className="mt-1 block w-full rounded border border-slate-700 bg-slate-900 px-3 py-2 font-mono text-sm"
          spellCheck={false}
        />
        <p className="mt-1 text-xs text-slate-500">
          Directory holding <code>chunks/</code> and <code>manifests/</code>. Created on save if it
          doesn't exist.
        </p>
      </label>

      <label className="block">
        <span className="text-sm text-slate-400">FUSE mountpoint</span>
        <input
          type="text"
          value={mountpoint}
          onChange={(e) => setMountpoint(e.target.value)}
          className="mt-1 block w-full rounded border border-slate-700 bg-slate-900 px-3 py-2 font-mono text-sm"
          spellCheck={false}
        />
        <p className="mt-1 text-xs text-slate-500">
          Where the depot tree appears when you click the mount button. Created on first mount;
          changes take effect after the next stop/start.
        </p>
      </label>

      <div className="flex items-center gap-3">
        <button
          type="submit"
          disabled={!dirty || mutation.isPending}
          className="rounded bg-sky-700 px-4 py-2 text-sm font-medium hover:bg-sky-600 disabled:opacity-30 disabled:hover:bg-sky-700"
        >
          {mutation.isPending ? "Saving…" : "Save"}
        </button>
        {mutation.error && (
          <span className="text-sm text-red-400">{(mutation.error as Error).message}</span>
        )}
        {mutation.isSuccess && !dirty && <span className="text-sm text-slate-400">Saved.</span>}
      </div>

      {config.restart_required && (
        <div className="rounded border border-amber-700 bg-amber-950/40 p-3 text-sm">
          <span className="font-medium text-amber-300">Restart required.</span>
          <span className="ml-2 text-amber-200/70">
            The backend is still using the previous path; restart it to pick up the new value.
          </span>
        </div>
      )}
    </form>
  );
}
