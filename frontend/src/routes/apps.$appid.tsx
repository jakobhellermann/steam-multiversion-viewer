import { createFileRoute } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { fetchAppInfo } from "../api";

export const Route = createFileRoute("/apps/$appid")({ component: AppDetail });

function AppDetail() {
  const { appid: appidParam } = Route.useParams();
  const appid = Number(appidParam);
  const query = useQuery({
    queryKey: ["app", appid],
    queryFn: () => fetchAppInfo(appid),
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
        <div className="mt-4 flex gap-6">
          <img
            src={`https://cdn.cloudflare.steamstatic.com/steam/apps/${query.data.appid}/header.jpg`}
            alt=""
            className="w-[460px] h-[215px] rounded"
          />
          <div>
            <h1 className="text-3xl font-bold">{query.data.name}</h1>
            <dl className="mt-4 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
              <dt className="text-slate-400">Type</dt>
              <dd>{query.data.type}</dd>
              <dt className="text-slate-400">Developer</dt>
              <dd>{query.data.developer}</dd>
              <dt className="text-slate-400">Publisher</dt>
              <dd>{query.data.publisher}</dd>
              <dt className="text-slate-400">App ID</dt>
              <dd className="tabular-nums">{query.data.appid}</dd>
              <dt className="text-slate-400">Homepage</dt>
              <dd>
                {query.data.homepage ? (
                  <a
                    href={query.data.homepage}
                    target="_blank"
                    rel="noreferrer"
                    className="text-sky-400 hover:underline"
                  >
                    {query.data.homepage}
                  </a>
                ) : (
                  <span className="text-slate-500">—</span>
                )}
              </dd>
            </dl>
          </div>
        </div>
      )}
    </div>
  );
}
