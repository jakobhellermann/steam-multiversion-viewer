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
  developer: string | null;
  publisher: string | null;
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
  from_app_id: number | null;
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

export type ManifestStatusEntry = {
  depot_id: number;
  manifest_id: string;
  branch: string;
  chunks_total: number;
  chunks_missing: number;
  bytes_total: number;
  bytes_missing: number;
  bytes_missing_compressed: number;
  bytes_unique: number;
};

export function fetchManifestStatuses(appid: AppId): Promise<ManifestStatusEntry[]> {
  return getJson(`/api/apps/${appid}/manifests/status`);
}

export type ManifestInfo = {
  depot_id: number;
  manifest_id: string;
  creation_time: number;
  size_uncompressed: number;
  size_compressed: number;
  file_count: number;
};

export type ManifestFileKind = "file" | "directory" | "symlink";

export type ManifestFile = {
  path: string;
  size: number;
  kind: ManifestFileKind;
  chunk_count: number;
  linktarget: string | null;
};

export type ManifestFilesPage = {
  depot_id: number;
  manifest_id: string;
  offset: number;
  limit: number;
  file_count: number;
  files: ManifestFile[];
};

export function fetchManifestInfo(
  appid: AppId,
  depotId: number,
  gid: string,
  branch: string,
): Promise<ManifestInfo> {
  const qs = new URLSearchParams({ branch });
  return getJson(`/api/apps/${appid}/depots/${depotId}/manifests/${gid}?${qs}`);
}

export type Config = {
  store_root: string;
  restart_required: boolean;
};

export function fetchConfig(): Promise<Config> {
  return getJson("/api/config");
}

export async function patchConfig(patch: { store_root?: string }): Promise<Config> {
  const r = await fetch("/api/config", {
    method: "PATCH",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(patch),
  });
  if (!r.ok) {
    let message = `${r.status} ${r.statusText}`;
    try {
      const body = await r.json();
      if (body && typeof body.error === "string") message = body.error;
    } catch {
      /* ignore */
    }
    throw new Error(message);
  }
  return r.json();
}

export function fetchManifestFiles(
  appid: AppId,
  depotId: number,
  gid: string,
  branch: string,
  offset: number,
  limit: number,
): Promise<ManifestFilesPage> {
  const qs = new URLSearchParams({
    branch,
    offset: String(offset),
    limit: String(limit),
  });
  return getJson(`/api/apps/${appid}/depots/${depotId}/manifests/${gid}/files?${qs}`);
}
