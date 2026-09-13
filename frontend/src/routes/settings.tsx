// TODO(ai-review): review for style and correctness
import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { fetchConfig, patchConfig, type Config, type VibrancyEffect } from "../api";
import { applyVibrancy, vibrancyActive, vibrancySupported } from "../vibrancy";

export const Route = createFileRoute("/settings")({ component: SettingsPage });

function SettingsPage() {
  const qc = useQueryClient();
  const query = useQuery({ queryKey: ["config"], queryFn: fetchConfig });
  const onSaved = () => qc.invalidateQueries({ queryKey: ["config"] });

  return (
    <div className="mx-auto max-w-4xl p-8">
      <h1 className="mb-6 text-2xl font-bold">Settings</h1>
      {query.isPending && <p className="text-slate-400">Loading…</p>}
      {query.error && <p className="text-sm text-red-400">{(query.error as Error).message}</p>}
      {query.data && (
        <div className="space-y-8">
          <SettingsForm config={query.data} onSaved={onSaved} />
          {vibrancySupported() && <VibrancySection config={query.data} onSaved={onSaved} />}
        </div>
      )}
    </div>
  );
}

function VibrancySection({ config, onSaved }: { config: Config; onSaved: () => void }) {
  const [effect, setEffect] = useState<VibrancyEffect>(config.vibrancy_effect);
  const [tint, setTint] = useState(config.vibrancy_tint);

  useEffect(() => {
    setEffect(config.vibrancy_effect);
    setTint(config.vibrancy_tint);
  }, [config.vibrancy_effect, config.vibrancy_tint]);

  const mutation = useMutation({
    mutationFn: (patch: { vibrancy_effect?: VibrancyEffect; vibrancy_tint?: number }) =>
      patchConfig(patch),
    onSuccess: onSaved,
  });

  // Turning the backdrop on can't happen mid-session — the window's transparency
  // is fixed at creation — so switching in/out of "none" needs a restart.
  const needsRestart = effect !== "none" && !vibrancyActive();

  function changeEffect(next: VibrancyEffect) {
    setEffect(next);
    applyVibrancy({ vibrancy_effect: next, vibrancy_tint: tint });
    mutation.mutate({ vibrancy_effect: next });
  }

  function changeTint(next: number) {
    setTint(next);
    applyVibrancy({ vibrancy_effect: effect, vibrancy_tint: next });
  }

  return (
    <section className="space-y-3 border-t border-slate-800 pt-6">
      <h2 className="text-lg font-semibold">Window backdrop</h2>

      <div className="flex items-end gap-6">
        <label className="block">
          <span className="text-sm text-slate-400">Effect</span>
          <select
            value={effect}
            onChange={(e) => changeEffect(e.target.value as VibrancyEffect)}
            className="mt-1 block w-40 field bg-slate-900 px-3 py-2 text-sm"
          >
            <option value="none">None</option>
            <option value="mica">Mica</option>
            <option value="acrylic">Acrylic</option>
          </select>
        </label>

        <label className="block flex-1">
          <span className="text-sm text-slate-400">Tint {tint}%</span>
          <input
            type="range"
            min={0}
            max={100}
            value={tint}
            disabled={effect === "none"}
            onChange={(e) => changeTint(Number(e.target.value))}
            onPointerUp={() => mutation.mutate({ vibrancy_tint: tint })}
            onBlur={() => mutation.mutate({ vibrancy_tint: tint })}
            className="mt-2 block w-full disabled:opacity-40"
          />
        </label>
      </div>

      {needsRestart && (
        <p className="text-sm text-amber-300">Restart required to turn the backdrop on.</p>
      )}
    </section>
  );
}

function SettingsForm({ config, onSaved }: { config: Config; onSaved: () => void }) {
  const [storeRoot, setStoreRoot] = useState(config.store_root);
  const [mountpoint, setMountpoint] = useState(config.mountpoint);
  const [exportDir, setExportDir] = useState(config.export_dir);

  useEffect(() => {
    setStoreRoot(config.store_root);
    setMountpoint(config.mountpoint);
    setExportDir(config.export_dir);
  }, [config.store_root, config.mountpoint, config.export_dir]);

  const mutation = useMutation({
    mutationFn: () => patchConfig({ store_root: storeRoot, mountpoint, export_dir: exportDir }),
    onSuccess: () => onSaved(),
  });

  const dirty =
    storeRoot !== config.store_root ||
    mountpoint !== config.mountpoint ||
    exportDir !== config.export_dir;

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
          className="mt-1 block w-full field bg-slate-900 px-3 py-2 font-mono text-sm"
          spellCheck={false}
        />
        <p className="mt-1 text-xs text-slate-500">
          Directory holding <code>chunks/</code> and <code>manifests/</code>. Created on save if it
          doesn't exist.
        </p>
      </label>

      <label className="block">
        <span className="text-sm text-slate-400">Mountpoint</span>
        <input
          type="text"
          value={mountpoint}
          onChange={(e) => setMountpoint(e.target.value)}
          className="mt-1 block w-full field bg-slate-900 px-3 py-2 font-mono text-sm"
          spellCheck={false}
        />
        <p className="mt-1 text-xs text-slate-500">
          Where the depot tree appears when you click the mount button. Created on first mount;
          changes take effect after the next stop/start.
        </p>
      </label>

      <label className="block">
        <span className="text-sm text-slate-400">Export directory</span>
        <input
          type="text"
          value={exportDir}
          onChange={(e) => setExportDir(e.target.value)}
          className="mt-1 block w-full field bg-slate-900 px-3 py-2 font-mono text-sm"
          spellCheck={false}
        />
        <p className="mt-1 text-xs text-slate-500">
          Root for "Export" on a manifest page. Each export lands in its own{" "}
          <code>&lt;game&gt;-&lt;version&gt;</code> subdirectory below it.
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
