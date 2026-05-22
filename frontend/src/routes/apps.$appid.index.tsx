import { createFileRoute, Link } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import {
  fetchAppInfo,
  fetchManifestStatuses,
  type AppInfo,
  type DepotEntry,
  type ManifestStatusEntry,
} from "../api";

export const Route = createFileRoute("/apps/$appid/")({ component: AppDetail });

function AppDetail() {
  const { appid: appidParam } = Route.useParams();
  const appid = Number(appidParam);
  const query = useQuery({
    queryKey: ["app", appid],
    queryFn: () => fetchAppInfo(appid),
  });
  const statusQuery = useQuery({
    queryKey: ["manifest-statuses", appid],
    queryFn: () => fetchManifestStatuses(appid),
  });

  return (
    <div className="p-8 max-w-4xl mx-auto">
      {query.isPending && <p className="text-slate-400">Loading…</p>}
      {query.error && (
        <div className="mt-4 p-4 border border-red-900 bg-red-950/40 rounded">
          <p className="text-red-400 font-medium mb-1">Failed to load app info</p>
          <p className="text-red-300 text-sm font-mono break-words">
            {(query.error as Error).message}
          </p>
        </div>
      )}
      {query.data && (
        <AppDetailBody
          info={query.data}
          statuses={statusQuery.data}
          statusError={statusQuery.error as Error | null}
        />
      )}
    </div>
  );
}

function AppDetailBody({
  info,
  statuses,
  statusError,
}: {
  info: AppInfo;
  statuses: ManifestStatusEntry[] | undefined;
  statusError: Error | null;
}) {
  const statusByKey = new Map<string, ManifestStatusEntry>();
  for (const s of statuses ?? []) {
    statusByKey.set(`${s.depot_id}/${s.manifest_id}/${s.branch}`, s);
  }
  return (
    <>
      <div className="flex gap-6">
        <img
          src={`https://cdn.cloudflare.steamstatic.com/steam/apps/${info.appid}/header.jpg`}
          alt=""
          className="w-[460px] h-[215px] rounded"
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
        <h2 className="text-xl font-semibold mb-3">Branches</h2>
        {info.branches.length === 0 ? (
          <p className="text-slate-500 text-sm">No branches.</p>
        ) : (
          <table className="w-full text-left text-sm">
            <thead>
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
        )}
        {info.private_branches && (
          <p className="mt-2 text-xs text-slate-500">
            This app has private branches not visible to your account.
          </p>
        )}
      </section>

      <section className="mt-8">
        <h2 className="text-xl font-semibold mb-3">Depots</h2>
        {statusError && (
          <p className="mb-2 text-xs text-red-400">
            Failed to load manifest status: {statusError.message}
          </p>
        )}
        {info.depots.length === 0 ? (
          <p className="text-slate-500 text-sm">No depots.</p>
        ) : (
          <div className="space-y-4">
            {info.depots.map((d) => (
              <DepotCard key={d.depot_id} depot={d} appid={info.appid} statuses={statusByKey} />
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
}: {
  depot: DepotEntry;
  appid: number;
  statuses: Map<string, ManifestStatusEntry>;
}) {
  const tags = [depot.oslist, depot.osarch, depot.language].filter(Boolean) as string[];

  return (
    <div className="border border-slate-800 rounded">
      <div className="px-4 py-2 border-b border-slate-800 flex items-baseline gap-3">
        <span className="font-mono tabular-nums">{depot.depot_id}</span>
        <span className="text-xs text-slate-400">{tags.join(" · ")}</span>
        <a
          href={`https://steamdb.info/depot/${depot.depot_id}/manifests/`}
          target="_blank"
          rel="noreferrer"
          className="ml-auto text-xs text-slate-500 hover:text-sky-400 hover:underline"
        >
          history on SteamDB ↗
        </a>
      </div>
      {depot.manifests.length === 0 ? (
        <p className="px-4 py-3 text-sm text-slate-500">No manifests in this depot.</p>
      ) : (
        <table className="w-full text-left text-sm">
          <thead>
            <tr className="border-b border-slate-800 text-slate-400">
              <th className="px-4 py-2">Branch</th>
              <th className="px-4 py-2">Manifest ID</th>
              <th className="px-4 py-2 text-right">Size</th>
              <th className="px-4 py-2 text-right">Missing</th>
              <th className="px-4 py-2 text-right">Unique</th>
            </tr>
          </thead>
          <tbody>
            {depot.manifests.map((m) => {
              const status = statuses.get(`${depot.depot_id}/${m.gid}/${m.branch}`);
              return (
                <tr
                  key={`${m.branch}-${m.gid}`}
                  className="border-b border-slate-800 last:border-b-0 hover:bg-slate-800/40"
                >
                  <td className="px-4 py-2 font-medium whitespace-nowrap">{m.branch}</td>
                  <td className="px-4 py-2 font-mono tabular-nums text-xs">
                    <Link
                      to="/apps/$appid/depots/$depotId/manifests/$gid"
                      params={{
                        appid: String(appid),
                        depotId: String(depot.depot_id),
                        gid: m.gid,
                      }}
                      search={{ branch: m.branch, offset: 0, limit: 100 }}
                      className="text-sky-400 hover:underline"
                    >
                      {m.gid}
                    </Link>
                  </td>
                  <td className="px-4 py-2 text-right tabular-nums">{formatBytes(m.size)}</td>
                  <td className="px-4 py-2 text-right tabular-nums">
                    {status ? (
                      status.bytes_missing === 0 ? (
                        <span className="text-slate-600">—</span>
                      ) : (
                        <span className="text-amber-300">
                          {formatBytes(status.bytes_missing)}{" "}
                          <span className="text-slate-500">
                            ({formatBytes(status.bytes_missing_compressed)})
                          </span>
                        </span>
                      )
                    ) : (
                      <Skeleton />
                    )}
                  </td>
                  <td className="px-4 py-2 text-right tabular-nums text-slate-400">
                    {status ? (
                      status.bytes_unique === 0 ? (
                        <span className="text-slate-600">—</span>
                      ) : (
                        formatBytes(status.bytes_unique)
                      )
                    ) : (
                      <Skeleton />
                    )}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </div>
  );
}

function Skeleton() {
  return <span className="inline-block h-3 w-16 bg-slate-800 rounded animate-pulse align-middle" />;
}

function formatTime(unix: number | null): string {
  if (!unix) return "—";
  return new Date(unix * 1000).toISOString().slice(0, 10);
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
