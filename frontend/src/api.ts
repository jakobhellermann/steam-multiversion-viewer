export type AppId = number;

export type OwnedGame = {
  appid: AppId;
  name: string;
  playtime_minutes: number;
};

export type AppInfo = {
  appid: AppId;
  name: string;
  type: string;
  developer: string;
  publisher: string;
  homepage: string | null;
  logo_url: string | null;
  icon_url: string;
  branches: BranchInfo[];
  depots: DepotEntry[];
  private_branches: boolean;
};

export type BranchInfo = {
  name: string;
  build_id: number;
  time_updated: number | null;
  description: string | null;
};

export type DepotEntry = {
  depot_id: number;
  oslist: string | null;
  osarch: string | null;
  language: string | null;
  manifests: DepotManifest[];
};

export type DepotManifest = {
  branch: string;
  gid: string;
  size: number;
  download_size: number;
};

async function getJson<T>(path: string): Promise<T> {
  const r = await fetch(path);
  if (!r.ok) {
    let message = `${r.status} ${r.statusText}`;
    try {
      const body = await r.json();
      if (body && typeof body.error === "string") message = body.error;
    } catch {
      const text = await r.text().catch(() => "");
      if (text) message = text;
    }
    throw new Error(message);
  }
  return r.json();
}

export function fetchLibrary(): Promise<OwnedGame[]> {
  return getJson("/api/library");
}

export function fetchAppInfo(appid: AppId): Promise<AppInfo> {
  return getJson(`/api/apps/${appid}`);
}
