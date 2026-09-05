import { createFileRoute, Link } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  fetchAppInfo,
  fetchExtraManifests,
  fetchManifestStatuses,
  putExtraManifests,
  type AppInfo,
  type DepotEntry,
  type DepotManifest,
  type ExtraManifestEntry,
  type ManifestRef,
  type ManifestStatusEntry,
} from "../api";
import { BranchFilterList } from "../components/BranchFilterList";
import { Bytes } from "../components/Bytes";
import { ErrorBox } from "../components/ErrorBox";
import { parseSteamDbPaste, steamDbSignInGated, type ParsedExtra } from "../lib/parseSteamDbPaste";
import { pinScroll } from "../lib/pinScroll";
import { useBranchFilter } from "../lib/useBranchFilter";

export const Route = createFileRoute("/apps/$appid/")({ component: AppDetail });

function AppDetail() {
  const { appid: appidParam } = Route.useParams();
  const appid = Number(appidParam);
  const query = useQuery({
    queryKey: ["app", appid],
    queryFn: () => fetchAppInfo(appid),
  });
  const extraQuery = useQuery({
    queryKey: ["extra-manifests", appid],
    queryFn: () => fetchExtraManifests(appid),
  });
  // (depot_id, manifest_id, branch) refs from app_info + extras. Dedup
  // by (depot_id, manifest_id) — branches that share a gid don't add
  // work, and the chosen branch only matters for the cache-miss path
  // anyway. Extras with no branch fall back to "public".
  const manifestRefs = useMemo<ManifestRef[]>(() => {
    if (!query.data) return [];
    const seen = new Set<string>();
    const refs: ManifestRef[] = [];
    for (const d of query.data.depots) {
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
  }, [query.data, extraQuery.data]);
  const statusQuery = useQuery({
    queryKey: ["manifest-statuses", appid, manifestRefs],
    queryFn: () => fetchManifestStatuses(appid, manifestRefs),
    // Don't fire on the half-populated ref list before extras land.
    enabled: manifestRefs.length > 0 && extraQuery.isSuccess,
    // Per-manifest missing-bytes change as the download manager makes
    // progress; refetch on every mount.
    refetchOnMount: "always",
  });

  return (
    <div className="mx-auto max-w-4xl p-8">
      {query.isPending && <p className="text-slate-400">Loading…</p>}
      {query.error && <ErrorBox title="Failed to load app info" error={query.error as Error} />}
      {query.data && (
        <AppDetailBody
          info={query.data}
          extras={extraQuery.data ?? []}
          statuses={statusQuery.data}
          statusError={statusQuery.error as Error | null}
        />
      )}
    </div>
  );
}

function AppDetailBody({
  info,
  extras,
  statuses,
  statusError,
}: {
  info: AppInfo;
  extras: ExtraManifestEntry[];
  statuses: ManifestStatusEntry[] | undefined;
  statusError: Error | null;
}) {
  const extrasByDepot = useMemo(() => {
    const map = new Map<number, ExtraManifestEntry[]>();
    for (const e of extras) {
      const list = map.get(e.depot_id);
      if (list) list.push(e);
      else map.set(e.depot_id, [e]);
    }
    return map;
  }, [extras]);
  const statusByKey = new Map<string, ManifestStatusEntry>();
  for (const s of statuses ?? []) {
    statusByKey.set(`${s.depot_id}/${s.manifest_id}`, s);
  }
  // Branch names that actually occur across depots + extras, in
  // info.branches order first (public usually leads), then any extra
  // ones. This is the togglable set in the Depots filter below.
  const allBranches = useMemo(() => {
    const ordered: string[] = [];
    const seen = new Set<string>();
    const add = (name: string) => {
      if (!seen.has(name)) {
        seen.add(name);
        ordered.push(name);
      }
    };
    for (const b of info.branches) add(b.name);
    for (const d of info.depots) for (const m of d.manifests) add(m.branch);
    for (const e of extras) add(e.branch ?? "public");
    return ordered;
  }, [info, extras]);
  // Branch filter, shared with the compare menu (persisted per appid) and
  // defaulting to public-only. Filtering rows changes the depot list
  // height, so pin the Depots header (where the filter button lives) on
  // every change to keep the viewport from jumping.
  const depotsHeaderRef = useRef<HTMLDivElement>(null);
  const {
    hidden: hiddenBranches,
    toggle: toggleBranch,
    showAll: showAllBranches,
    hideAll: hideAllBranches,
    only: onlyBranch,
  } = useBranchFilter(info.appid, allBranches, () => pinScroll(depotsHeaderRef.current));
  const branchDescriptions = new Map<string, string>();
  for (const b of info.branches) {
    if (b.description) branchDescriptions.set(b.name, b.description);
  }
  // Re-honor the URL hash once the depot cards are in the DOM — on first
  // navigation the browser tries to scroll before our data has loaded.
  useEffect(() => {
    const hash = window.location.hash.slice(1);
    if (!hash) return;
    document.getElementById(hash)?.scrollIntoView({ block: "start" });
  }, []);
  return (
    <>
      <div className="flex gap-6">
        <img
          src={`https://cdn.cloudflare.steamstatic.com/steam/apps/${info.appid}/header.jpg`}
          alt=""
          className="h-54 w-115 rounded"
        />
        <div>
          <h1 className="text-3xl font-bold">{info.name}</h1>
          <dl className="mt-4 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
            <dt className="text-slate-400">Type</dt>
            <dd>{info.type}</dd>
            <dt className="text-slate-400">Developer</dt>
            <dd>{info.developer}</dd>
            <dt className="text-slate-400">Publisher</dt>
            <dd>{info.publisher}</dd>
            <dt className="text-slate-400">App ID</dt>
            <dd className="tabular-nums">{info.appid}</dd>
            <dt className="text-slate-400">Homepage</dt>
            <dd>
              {info.homepage ? (
                <a
                  href={info.homepage}
                  target="_blank"
                  rel="noreferrer"
                  className="text-sky-400 hover:underline"
                >
                  {info.homepage}
                </a>
              ) : (
                <span className="text-slate-500">—</span>
              )}
            </dd>
          </dl>
        </div>
      </div>

      <section className="mt-8">
        <h2 className="mb-3 text-xl font-semibold">Branches</h2>
        {info.branches.length === 0 ? (
          <p className="text-sm text-slate-500">No branches.</p>
        ) : (
          <div className="max-h-80 overflow-y-auto">
            <table className="w-full text-left text-sm">
              <thead className="sticky top-0">
                <tr className="border-b border-slate-700">
                  <th className="px-3 py-2">Name</th>
                  <th className="px-3 py-2 text-right">Build ID</th>
                  <th className="px-3 py-2">Updated</th>
                  <th className="px-3 py-2">Description</th>
                </tr>
              </thead>
              <tbody>
                {info.branches.map((b) => (
                  <tr key={b.name} className="border-b border-slate-800">
                    <td className="px-3 py-2 font-medium">{b.name}</td>
                    <td className="px-3 py-2 text-right tabular-nums">{b.build_id}</td>
                    <td className="px-3 py-2 tabular-nums">{formatTime(b.time_updated)}</td>
                    <td className="px-3 py-2 text-slate-400">
                      {b.description || <span className="text-slate-600">—</span>}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
        {info.private_branches && (
          <p className="mt-2 text-xs text-slate-500">
            This app has private branches not visible to your account.
          </p>
        )}
      </section>

      <section className="mt-8">
        <div ref={depotsHeaderRef} className="mb-3 flex items-center justify-between gap-4">
          <h2 className="text-xl font-semibold">Depots</h2>
          {allBranches.length > 1 && (
            <BranchFilter
              branches={allBranches}
              hidden={hiddenBranches}
              onToggle={toggleBranch}
              onShowAll={showAllBranches}
              onHideAll={hideAllBranches}
              onOnly={onlyBranch}
            />
          )}
        </div>
        {statusError && (
          <p className="mb-2 text-xs text-red-400">
            Failed to load manifest status: {statusError.message}
          </p>
        )}
        {info.depots.length === 0 ? (
          <p className="text-sm text-slate-500">No depots.</p>
        ) : (
          <div className="grid grid-cols-[max-content_1fr_max-content_max-content_max-content] gap-y-3 text-sm">
            {info.depots.map((depot) => (
              <DepotCard
                key={depot.depot_id}
                depot={depot}
                appid={info.appid}
                statuses={statusByKey}
                branchDescriptions={branchDescriptions}
                extras={extrasByDepot.get(depot.depot_id) ?? []}
                allExtras={extras}
                hiddenBranches={hiddenBranches}
              />
            ))}
          </div>
        )}
      </section>
    </>
  );
}

function DepotCard({
  depot,
  appid,
  statuses,
  branchDescriptions,
  extras,
  allExtras,
  hiddenBranches,
}: {
  depot: DepotEntry;
  appid: number;
  statuses: Map<string, ManifestStatusEntry>;
  branchDescriptions: Map<string, string>;
  extras: ExtraManifestEntry[];
  allExtras: ExtraManifestEntry[];
  hiddenBranches: Set<string>;
}) {
  const [importOpen, setImportOpen] = useState(false);
  const tags = [depot.oslist, depot.osarch, depot.language].filter(Boolean) as string[];
  const visibleManifests = depot.manifests.filter((m) => !hiddenBranches.has(m.branch));
  const visibleExtras = extras.filter((e) => !hiddenBranches.has(e.branch ?? "public"));
  const sharedFromLink =
    depot.from_app_id != null ? (
      <Link
        to="/apps/$appid"
        params={{ appid: String(depot.from_app_id) }}
        hash={`depot-${depot.depot_id}`}
        className="text-sky-400 tabular-nums hover:underline"
      >
        app {depot.from_app_id}
      </Link>
    ) : null;
  // A depot with manifests upstream but all of them filtered out renders
  // an empty body — don't draw the header's bottom border then.
  const showNoManifestsMsg = depot.manifests.length === 0 && !sharedFromLink;
  const hasBody = visibleManifests.length > 0 || showNoManifestsMsg || visibleExtras.length > 0;
  // Extras only make sense for depots whose content lives here; shared
  // depots belong to another app entirely.
  const canTrackExtras = depot.from_app_id == null;
  return (
    <div
      id={`depot-${depot.depot_id}`}
      className="col-span-full grid scroll-mt-4 grid-cols-subgrid rounded border border-slate-800"
    >
      <div
        className={`col-span-full flex items-baseline gap-3 px-4 py-2 ${
          hasBody ? "border-b border-slate-800" : ""
        }`}
      >
        <span className="font-mono tabular-nums">{depot.depot_id}</span>
        <span className="text-xs text-slate-400">{tags.join(" · ")}</span>
        {sharedFromLink && (
          <span className="text-xs text-slate-500">shared from {sharedFromLink}</span>
        )}
        {canTrackExtras && (
          <button
            type="button"
            onClick={() => setImportOpen(true)}
            className="ml-auto text-xs text-slate-500 hover:text-sky-400 hover:underline"
          >
            import history from SteamDB <span className="font-glyph">↗</span>
          </button>
        )}
      </div>
      {depot.manifests.length === 0 ? (
        sharedFromLink ? null : (
          <div className="col-span-full px-4 py-2 text-sm text-slate-500">
            No manifests in this depot.
          </div>
        )
      ) : visibleManifests.length === 0 ? null : (
        <>
          <div className="col-span-full grid grid-cols-subgrid border-b border-slate-800 text-slate-400">
            <div className="px-4 py-1.5 font-semibold">Branch</div>
            <div className="px-4 py-1.5 font-semibold">Manifest ID</div>
            <div className="px-4 py-1.5 text-right font-semibold">Size</div>
            <div className="px-4 py-1.5 text-right font-semibold">Download Size</div>
            <div className="px-4 py-1.5 text-right font-semibold">Missing</div>
          </div>
          {(() => {
            // Two branches often point at the same manifest gid (public ==
            // public-beta when no beta is active). Mute the second+ occurrence
            // so the duplicate doesn't draw the eye.
            const seenManifests = new Set<string>();
            return visibleManifests.map((m) => {
              const status = statuses.get(`${depot.depot_id}/${m.manifest_id}`);
              const duplicate = seenManifests.has(m.manifest_id);
              seenManifests.add(m.manifest_id);
              return (
                <ManifestRow
                  key={`${m.branch}-${m.manifest_id}`}
                  depotId={depot.depot_id}
                  appid={appid}
                  manifest={m}
                  status={status}
                  duplicate={duplicate}
                  branchDescription={branchDescriptions.get(m.branch)}
                />
              );
            });
          })()}
        </>
      )}
      {visibleExtras.length > 0 && (
        <ExtrasSection
          appid={appid}
          depotId={depot.depot_id}
          extras={visibleExtras}
          allExtras={allExtras}
          statuses={statuses}
        />
      )}
      {importOpen && (
        <ImportExtrasModal
          appid={appid}
          depotId={depot.depot_id}
          existing={allExtras}
          officialManifests={depot.manifests}
          onClose={() => setImportOpen(false)}
        />
      )}
    </div>
  );
}

function ExtrasSection({
  appid,
  depotId,
  extras,
  allExtras,
  statuses,
}: {
  appid: number;
  depotId: number;
  extras: ExtraManifestEntry[];
  allExtras: ExtraManifestEntry[];
  statuses: Map<string, ManifestStatusEntry>;
}) {
  const queryClient = useQueryClient();
  // Persist the open/closed state across navigation. Without this the
  // user clicks into a manifest, comes back, and the "Additional
  // manifests" dropdown they had open is collapsed again. Hydrate from
  // the query cache on mount, write through on every toggle so the
  // next mount sees the latest value.
  const expandedKey = useMemo(() => ["extras-expanded", appid, depotId] as const, [appid, depotId]);
  const [expanded, setExpandedLocal] = useState<boolean>(
    () => queryClient.getQueryData<boolean>(expandedKey) ?? false,
  );
  const headerRef = useRef<HTMLDivElement>(null);
  const toggle = () => {
    // Only collapse shrinks the page — that's the case where scrollY
    // gets yanked, so only run pinScroll then. We measure now, before
    // React updates the DOM, and restore once the new layout is in.
    const restore = expanded ? pinScroll(headerRef.current) : null;
    const next = !expanded;
    setExpandedLocal(next);
    queryClient.setQueryData(expandedKey, next);
    if (restore) requestAnimationFrame(restore);
  };
  const clearAll = useMutation({
    mutationFn: () =>
      putExtraManifests(
        appid,
        allExtras.filter((e) => e.depot_id !== depotId),
      ),
    onSuccess: (next) => {
      queryClient.setQueryData(["extra-manifests", appid], next);
    },
  });
  return (
    <>
      <div
        ref={headerRef}
        role="button"
        tabIndex={0}
        onClick={toggle}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            toggle();
          }
        }}
        aria-expanded={expanded}
        className="col-span-full grid cursor-pointer grid-cols-subgrid border-t border-slate-800 text-xs text-slate-500 select-none hover:bg-slate-800/40 hover:text-slate-300"
      >
        <div className="col-span-4 flex items-center gap-1 px-4 py-1.5">
          <span className="inline-block w-3 font-glyph text-slate-500">{expanded ? "▼" : "▶"}</span>
          <span>
            Additional manifests <span className="tabular-nums">({extras.length})</span>
          </span>
        </div>
        <div className="px-4 py-1.5 text-right">
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation();
              clearAll.mutate();
            }}
            disabled={clearAll.isPending}
            aria-label="Remove all tracked manifests in this depot"
            title="Remove all tracked manifests in this depot"
            className="text-lg leading-none text-slate-500 hover:text-red-400 disabled:opacity-40"
          >
            ×
          </button>
        </div>
      </div>
      {expanded &&
        extras.map((e) => {
          const status = statuses.get(`${depotId}/${e.manifest_id}`);
          return (
            <ExtraManifestRow
              key={e.manifest_id}
              appid={appid}
              depotId={depotId}
              extra={e}
              status={status}
            />
          );
        })}
    </>
  );
}

function ManifestRow({
  depotId,
  appid,
  manifest: m,
  status,
  duplicate,
  branchDescription,
}: {
  depotId: number;
  appid: number;
  manifest: DepotManifest;
  status: ManifestStatusEntry | undefined;
  duplicate: boolean;
  branchDescription: string | undefined;
}) {
  const linkProps = {
    to: "/apps/$appid/depots/$depotId/manifests/$manifestId",
    params: { appid: String(appid), depotId: String(depotId), manifestId: m.manifest_id },
    // Drop ?branch=public from the URL — it's the default everywhere.
    search: { branch: m.branch === "public" ? undefined : m.branch },
    draggable: false,
    onClick: (e: React.MouseEvent) => {
      // Don't navigate if the click ended a drag-to-select.
      if (window.getSelection()?.toString()) {
        e.preventDefault();
      }
    },
  } as const;
  const cell = "px-4 py-1.5";
  // Mute repeat occurrences of the same manifest_id within a depot
  // (e.g. public and public-beta share a gid when no beta is active).
  const rowMute = duplicate ? "opacity-50" : "";
  return (
    <div
      className={`group col-span-full grid grid-cols-subgrid items-baseline hover:bg-slate-800/40 ${rowMute}`}
    >
      <Link
        {...linkProps}
        title={branchDescription}
        className={`${cell} font-medium whitespace-nowrap`}
      >
        {m.branch}
      </Link>
      <Link
        {...linkProps}
        tabIndex={-1}
        aria-hidden="true"
        className={`${cell} font-mono text-xs text-sky-400 tabular-nums`}
      >
        {m.manifest_id}
      </Link>
      <Link
        {...linkProps}
        tabIndex={-1}
        aria-hidden="true"
        className={`${cell} text-right whitespace-nowrap tabular-nums`}
      >
        <Bytes value={m.size} />
      </Link>
      <Link
        {...linkProps}
        tabIndex={-1}
        aria-hidden="true"
        className={`${cell} text-right whitespace-nowrap tabular-nums`}
      >
        <DownloadCell status={status} appinfoDownloadSize={m.download_size} />
      </Link>
      <Link
        {...linkProps}
        tabIndex={-1}
        aria-hidden="true"
        className={`${cell} text-right whitespace-nowrap tabular-nums`}
      >
        <MissingCell status={status} />
      </Link>
    </div>
  );
}

function ExtraManifestRow({
  appid,
  depotId,
  extra,
  status,
}: {
  appid: number;
  depotId: number;
  extra: ExtraManifestEntry;
  status: ManifestStatusEntry | undefined;
}) {
  const branch = extra.branch ?? "public";
  const linkProps = {
    to: "/apps/$appid/depots/$depotId/manifests/$manifestId",
    params: {
      appid: String(appid),
      depotId: String(depotId),
      manifestId: extra.manifest_id,
    },
    search: { branch: branch === "public" ? undefined : branch },
    draggable: false,
    onClick: (e: React.MouseEvent) => {
      if (window.getSelection()?.toString()) {
        e.preventDefault();
      }
    },
  } as const;
  const cell = "px-4 py-1.5";
  return (
    <div className="group col-span-full grid grid-cols-subgrid items-baseline hover:bg-slate-800/40">
      <Link {...linkProps} className={`${cell} font-medium whitespace-nowrap text-slate-400`}>
        {extra.branch ?? <span className="text-slate-600 italic">unknown</span>}
      </Link>
      <Link
        {...linkProps}
        tabIndex={-1}
        aria-hidden="true"
        className={`${cell} font-mono text-xs text-sky-400 tabular-nums`}
      >
        {extra.manifest_id}
      </Link>
      <Link
        {...linkProps}
        tabIndex={-1}
        aria-hidden="true"
        className={`${cell} text-right whitespace-nowrap text-slate-500 tabular-nums`}
      >
        {status?.error ? (
          <span className="text-slate-600">—</span>
        ) : (
          <Bytes value={status?.bytes_total ?? 0} />
        )}
      </Link>
      <Link
        {...linkProps}
        tabIndex={-1}
        aria-hidden="true"
        className={`${cell} text-right whitespace-nowrap tabular-nums`}
      >
        <DownloadCell status={status} />
      </Link>
      <Link
        {...linkProps}
        tabIndex={-1}
        aria-hidden="true"
        className={`${cell} text-right whitespace-nowrap tabular-nums`}
      >
        <MissingCell status={status} />
      </Link>
    </div>
  );
}

function ImportExtrasModal({
  appid,
  depotId,
  existing,
  officialManifests,
  onClose,
}: {
  appid: number;
  depotId: number;
  existing: ExtraManifestEntry[];
  officialManifests: { branch: string; manifest_id: string }[];
  onClose: () => void;
}) {
  const [text, setText] = useState("");
  const queryClient = useQueryClient();
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    textareaRef.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    // When the user comes back from another tab (where they copied
    // text), put focus back on the textarea so ctrl-v lands.
    const onWindowFocus = () => textareaRef.current?.focus();
    document.addEventListener("keydown", onKey);
    window.addEventListener("focus", onWindowFocus);
    return () => {
      document.removeEventListener("keydown", onKey);
      window.removeEventListener("focus", onWindowFocus);
    };
  }, [onClose]);
  const parsed = useMemo(() => parseSteamDbPaste(text), [text]);
  // The paste carries SteamDB's "sign in to view more" gate → the history
  // was truncated to the most recent rows. Warn so the user logs in and
  // re-copies instead of importing a partial list.
  const signInGated = useMemo(() => steamDbSignInGated(text), [text]);
  // Identity = manifest_id + branch. We treat a parsed entry as a
  // duplicate iff the same (manifest_id, branch) already exists on this
  // depot — either as an official manifest or a tracked extra. Same
  // manifest_id on a *different* branch is still worth adding (it's a
  // new (gid, branch) tuple).
  const existingKeys = useMemo(() => {
    const s = new Set<string>();
    for (const m of officialManifests) s.add(`${m.manifest_id}|${m.branch}`);
    for (const e of existing) {
      if (e.depot_id === depotId) s.add(`${e.manifest_id}|${e.branch ?? "public"}`);
    }
    return s;
  }, [existing, officialManifests, depotId]);
  // depotdownloader / download_depot lines carry their own app_id and
  // depot_id; reject ones that reference a different depot so we don't
  // silently file them under the wrong card.
  const matchesDepot = (p: ParsedExtra) =>
    (p.app_id == null || p.app_id === appid) && (p.depot_id == null || p.depot_id === depotId);
  const eligible = parsed.filter(matchesDepot);
  const mismatches = parsed.filter((p) => !matchesDepot(p));
  const isExisting = (p: ParsedExtra) => existingKeys.has(`${p.manifest_id}|${p.branch}`);
  const newOnly = eligible.filter((p) => !isExisting(p));
  const save = useMutation({
    mutationFn: async () => {
      // Replace-all endpoint: union of existing + new for this depot,
      // keeping other depots' entries untouched. Identity is
      // (manifest_id, branch) so same gid on a different branch can
      // still be added.
      const otherDepots = existing.filter((e) => e.depot_id !== depotId);
      const sameDepot = existing.filter((e) => e.depot_id === depotId);
      const seen = new Set(sameDepot.map((e) => `${e.manifest_id}|${e.branch ?? "public"}`));
      for (const p of newOnly) {
        const key = `${p.manifest_id}|${p.branch}`;
        if (!seen.has(key)) {
          sameDepot.push({
            depot_id: depotId,
            manifest_id: p.manifest_id,
            branch: p.branch,
          });
          seen.add(key);
        }
      }
      return putExtraManifests(appid, [...otherDepots, ...sameDepot]);
    },
    onSuccess: (next) => {
      queryClient.setQueryData(["extra-manifests", appid], next);
      onClose();
    },
  });
  return (
    <div
      className="fixed inset-0 z-50 overflow-y-auto bg-black/60"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="mx-auto mt-32 mb-16 w-160 max-w-[95vw] rounded border border-slate-700 bg-slate-900 shadow-xl">
        <header className="flex items-center justify-between border-b border-slate-800 px-4 py-3">
          <h3 className="text-base font-semibold">Track manifests in depot {depotId}</h3>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close"
            className="px-1 text-lg leading-none text-slate-500 hover:text-slate-200"
          >
            ×
          </button>
        </header>
        <div className="space-y-3 px-4 py-3 text-sm text-slate-400">
          <p>
            SteamDB doesn't expose an API. Open{" "}
            <a
              href={`https://steamdb.info/depot/${depotId}/manifests/`}
              target="_blank"
              rel="noreferrer"
              className="text-sky-400 hover:underline"
            >
              the depot's manifest history
            </a>{" "}
            and log in to your steam account.
          </p>
          <p>
            Then{" "}
            <kbd className="rounded border border-slate-700 bg-slate-800 px-1 py-0.5 text-slate-300">
              Ctrl+A
            </kbd>{" "}
            <kbd className="rounded border border-slate-700 bg-slate-800 px-1 py-0.5 text-slate-300">
              Ctrl+C
            </kbd>{" "}
            the whole page and{" "}
            <kbd className="rounded border border-slate-700 bg-slate-800 px-1 py-0.5 text-slate-300">
              Ctrl+V
            </kbd>{" "}
            in here.
          </p>
          <textarea
            ref={textareaRef}
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
                e.preventDefault();
                if (newOnly.length > 0 && !save.isPending) save.mutate();
              }
            }}
            spellCheck={false}
            rows={10}
            placeholder={
              "Seen Date     Relative Date     ManifestID\n" +
              "1 January 2025 – 12:00:00 UTC    1 year ago     1111111111111111111\n" +
              "2 January 2025 – 12:00:00 UTC    1 year ago     2222222222222222222 public-beta"
            }
            className="w-full rounded border border-slate-700 bg-slate-950 px-3 py-2 font-mono text-xs focus:border-sky-700 focus:outline-none"
          />
          {signInGated && (
            <p className="rounded border border-amber-800/60 bg-amber-950/40 px-3 py-2 text-xs text-amber-300">
              SteamDB only shows the most recent manifests to logged-out visitors. Log in for the
              full history.
            </p>
          )}
          {parsed.length > 0 && (
            <div className="overflow-hidden rounded border border-slate-800">
              <div className="flex justify-between border-b border-slate-800 bg-slate-900/60 px-3 py-1.5 text-xs text-slate-400">
                <span>{parsed.length} parsed</span>
                <span className="text-slate-500">
                  {newOnly.length} new · {eligible.length - newOnly.length} already tracked
                  {mismatches.length > 0 && (
                    <>
                      {" · "}
                      <span className="text-amber-400">{mismatches.length} wrong depot</span>
                    </>
                  )}
                </span>
              </div>
              <ul tabIndex={-1} className="max-h-48 overflow-auto text-sm">
                {parsed.map((p) => {
                  const dup = isExisting(p);
                  const wrongDepot = !matchesDepot(p);
                  const mismatchLabel = wrongDepot
                    ? p.app_id != null && p.app_id !== appid
                      ? `app ${p.app_id}`
                      : `depot ${p.depot_id}`
                    : null;
                  return (
                    <li
                      key={p.manifest_id}
                      className={`flex items-baseline gap-3 px-3 py-1 ${
                        wrongDepot ? "text-amber-400/70" : dup ? "text-slate-600" : "text-slate-200"
                      }`}
                    >
                      <span className="font-mono text-xs whitespace-pre tabular-nums">
                        {p.manifest_id.padStart(20, " ")}
                      </span>
                      <span className="text-xs text-slate-400">{p.branch}</span>
                      {mismatchLabel && (
                        <span className="ml-auto text-xs text-amber-400">
                          skipped ({mismatchLabel})
                        </span>
                      )}
                      {!mismatchLabel && dup && (
                        <span className="ml-auto text-xs text-slate-500">already tracked</span>
                      )}
                    </li>
                  );
                })}
              </ul>
            </div>
          )}
          {save.error && (
            <p className="text-xs text-red-300">Save failed: {(save.error as Error).message}</p>
          )}
        </div>
        <footer className="flex items-center gap-2 border-t border-slate-800 px-4 py-3">
          <span className="text-xs text-slate-500">
            {newOnly.length === 0
              ? "Nothing new to add."
              : `${newOnly.length} new manifest${newOnly.length === 1 ? "" : "s"} ready.`}
          </span>
          <button
            type="button"
            onClick={onClose}
            className="ml-auto px-3 py-1.5 text-sm text-slate-300 hover:text-slate-100"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={() => save.mutate()}
            disabled={newOnly.length === 0 || save.isPending}
            className="rounded border border-sky-700 bg-sky-950/40 px-3 py-1.5 text-sm hover:bg-sky-900/40 disabled:opacity-40"
          >
            {save.isPending ? "Saving…" : "Add"}
          </button>
        </footer>
      </div>
    </div>
  );
}

function BranchFilter({
  branches,
  hidden,
  onToggle,
  onShowAll,
  onHideAll,
  onOnly,
}: {
  branches: string[];
  hidden: Set<string>;
  onToggle: (name: string) => void;
  onShowAll: () => void;
  onHideAll: () => void;
  onOnly: (name: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  // Close on outside click / Escape — same pattern as CompareMenu.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);
  const visibleCount = branches.length - hidden.size;
  const label =
    hidden.size === 0
      ? "all"
      : visibleCount === 0
        ? "none"
        : `${visibleCount} of ${branches.length}`;
  return (
    <div ref={rootRef} className="relative inline-block">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        className="flex items-center gap-1.5 rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 hover:border-slate-600"
      >
        <span className="text-slate-500">Branches:</span>
        <span>{label}</span>
        <span className="text-slate-500">{open ? "▲" : "▼"}</span>
      </button>
      {open && (
        <div className="absolute top-full right-0 z-10 mt-1 flex max-h-96 w-64 flex-col overflow-hidden rounded border border-slate-700 bg-slate-900 shadow-lg">
          <BranchFilterList
            branches={branches}
            hidden={hidden}
            onToggle={onToggle}
            onShowAll={onShowAll}
            onHideAll={onHideAll}
            onOnly={onOnly}
          />
        </div>
      )}
    </div>
  );
}

// Total compressed download size
function DownloadCell({
  status,
  appinfoDownloadSize,
}: {
  status: ManifestStatusEntry | undefined;
  appinfoDownloadSize?: number;
}) {
  if (appinfoDownloadSize !== undefined) {
    return <Bytes value={appinfoDownloadSize} />;
  }
  if (!status) {
    return appinfoDownloadSize !== undefined ? <Bytes value={appinfoDownloadSize} /> : <Skeleton />;
  }
  if (status.error) {
    return <span className="text-slate-600">—</span>;
  }
  return <Bytes value={status.bytes_total_compressed} />;
}
function MissingCell({ status }: { status: ManifestStatusEntry | undefined }) {
  if (!status) {
    return <Skeleton />;
  }
  if (status.error) {
    return (
      <span className="text-red-400" title={status.error}>
        inaccessible
      </span>
    );
  }
  return <Bytes value={status.bytes_missing_compressed} />;
}

function Skeleton() {
  return <span className="inline-block h-3 w-16 animate-pulse rounded bg-slate-800 align-middle" />;
}

function formatTime(unix: number | null): string {
  if (!unix) return "—";
  return new Date(unix * 1000).toISOString().slice(0, 10);
}
