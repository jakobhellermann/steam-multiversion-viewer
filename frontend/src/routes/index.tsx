import { createFileRoute, Link } from "@tanstack/react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { fetchLibrary, type OwnedGame } from "../api";

export const Route = createFileRoute("/")({ component: Home });

function Home() {
  const qc = useQueryClient();
  const query = useQuery({
    queryKey: ["library"],
    queryFn: fetchLibrary,
    initialData: () => qc.getQueryData<OwnedGame[]>(["library"]),
  });

  const [search, setSearch] = useState("");

  const rows = useMemo<OwnedGame[]>(() => {
    const all = query.data ?? [];
    const q = search.trim().toLowerCase();
    const filtered = q ? all.filter((g) => g.name.toLowerCase().includes(q)) : all;
    return [...filtered].sort((a, b) => b.playtime_minutes - a.playtime_minutes);
  }, [query.data, search]);

  return (
    <div className="p-8 max-w-4xl mx-auto">
      <input
        className="w-full px-3 py-2 mb-4 border border-slate-700 bg-slate-800 rounded placeholder-slate-400"
        placeholder="Search…"
        value={search}
        onChange={(e) => setSearch(e.target.value)}
      />
      <table className="w-full text-left">
        <thead>
          <tr className="border-b border-slate-700">
            <th className="px-3 py-2">Name</th>
            <th className="px-3 py-2 text-right">Playtime</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((g) => (
            <tr key={g.appid} className="border-b border-slate-800 hover:bg-slate-800">
              <td className="p-0">
                <Link
                  to="/apps/$appid"
                  params={{ appid: String(g.appid) }}
                  className="block px-3 py-2 text-sky-400"
                >
                  {g.name}
                </Link>
              </td>
              <td className="p-0">
                <Link
                  to="/apps/$appid"
                  params={{ appid: String(g.appid) }}
                  tabIndex={-1}
                  aria-hidden="true"
                  className="block px-3 py-2 text-right tabular-nums"
                >
                  {formatPlaytime(g.playtime_minutes)}
                </Link>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {query.error && (
        <div className="mt-4 p-4 border border-red-900 bg-red-950/40 rounded">
          <p className="text-red-400 font-medium mb-1">Failed to load library</p>
          <p className="text-red-300 text-sm font-mono break-words">
            {(query.error as Error).message}
          </p>
        </div>
      )}
    </div>
  );
}

function formatPlaytime(minutes: number): string {
  if (minutes < 60) return `${minutes} min`;
  return `${(minutes / 60).toFixed(1)} h`;
}
