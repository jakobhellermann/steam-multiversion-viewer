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
  manifest_id: string;
  size: number;
  download_size: number;
};

async function extractErrorMessage(r: Response): Promise<string> {
  let message = `${r.status} ${r.statusText}`;
  try {
    const body = await r.json();
    if (body && typeof body.error === "string") message = body.error;
  } catch {
    const text = await r.text().catch(() => "");
    if (text) message = text;
  }
  return message;
}

async function getJson<T>(path: string): Promise<T> {
  const r = await fetch(path);
  if (!r.ok) {
    throw new Error(await extractErrorMessage(r));
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
  /** `"not cached"` if absent locally, or another diagnostic message. */
  error: string | null;
  chunks_total: number;
  chunks_missing: number;
  bytes_total: number;
  bytes_missing: number;
  bytes_missing_compressed: number;
  bytes_unique: number;
  /** Steam-side manifest creation time (unix seconds). 0 when `error` is set. */
  creation_time: number;
};

export type ManifestRef = { depot_id: number; manifest_id: string; branch: string };

export type ExtraManifestEntry = {
  depot_id: number;
  manifest_id: string;
  branch: string | null;
};

export async function fetchExtraManifests(appid: AppId): Promise<ExtraManifestEntry[]> {
  const r = await fetch(`/api/apps/${appid}/extra_manifests`);
  if (!r.ok) throw new Error(`${r.status} ${r.statusText}`);
  return r.json();
}

export async function putExtraManifests(
  appid: AppId,
  entries: ExtraManifestEntry[],
): Promise<ExtraManifestEntry[]> {
  const r = await fetch(`/api/apps/${appid}/extra_manifests`, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ entries }),
  });
  if (!r.ok) throw new Error(await extractErrorMessage(r));
  return r.json();
}

export async function deleteExtraManifest(
  appid: AppId,
  depotId: number,
  manifestId: string,
): Promise<ExtraManifestEntry[]> {
  const r = await fetch(`/api/apps/${appid}/extra_manifests/${depotId}/${manifestId}`, {
    method: "DELETE",
  });
  if (!r.ok) throw new Error(await extractErrorMessage(r));
  return r.json();
}

/// Fetch a unified diff between the same file in two manifests.
/// Backend resolves both sides through `/file/transformed` semantics —
/// .dll diffs are between two decompiled C# blobs. Returns the raw
/// unified-diff text (ready to feed into shiki with the `diff` lang)
/// or null when the file is binary and untransformable (HTTP 415).
export async function fetchFileDiff(
  appid: AppId,
  baseDepotId: number,
  baseManifestId: string,
  baseBranch: string,
  target: { depot_id: number; manifest_id: string; branch: string },
  path: string,
): Promise<string | null> {
  const qs = new URLSearchParams({ branch: baseBranch, path });
  const r = await fetch(
    `/api/apps/${appid}/depots/${baseDepotId}/manifests/${baseManifestId}/file/diff?${qs}`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ path, target }),
    },
  );
  if (r.status === 415) return null;
  if (!r.ok) throw new Error(await extractErrorMessage(r));
  return r.text();
}

export async function fetchManifestDiff(
  appid: AppId,
  base: ManifestRef,
  others: ManifestRef[],
): Promise<string[]> {
  if (others.length === 0) return [];
  const r = await fetch(`/api/apps/${appid}/manifests/diff`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ base, others }),
  });
  if (!r.ok) throw new Error(await extractErrorMessage(r));
  const body: { changed_paths: string[] } = await r.json();
  return body.changed_paths;
}

export type FileDiffStatus = "same" | "different" | "missing";

export type FileDiffTargetStatus = {
  depot_id: number;
  manifest_id: string;
  status: FileDiffStatus;
};

/// Ask the backend which of the candidate manifests have a *different*
/// (or missing) version of the file at `path`. Used by the compare-to
/// menu on the file-view page so the user only sees manifests where
/// the focused file actually changed.
export async function fetchFileDiffTargets(
  appid: AppId,
  base: ManifestRef,
  others: ManifestRef[],
  path: string,
): Promise<FileDiffTargetStatus[]> {
  if (others.length === 0) return [];
  const r = await fetch(`/api/apps/${appid}/file/diff-targets`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ base, others, path }),
  });
  if (!r.ok) throw new Error(await extractErrorMessage(r));
  const body: { statuses: FileDiffTargetStatus[] } = await r.json();
  return body.statuses;
}

export async function fetchManifestStatuses(
  appid: AppId,
  manifests: ManifestRef[],
): Promise<ManifestStatusEntry[]> {
  if (manifests.length === 0) return [];
  const r = await fetch(`/api/apps/${appid}/manifests/status`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ manifests }),
  });
  if (!r.ok) throw new Error(await extractErrorMessage(r));
  return r.json();
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

export type FileContentKind = "text" | "binary" | "unknown" | "too_large";

export type FileView = {
  path: string;
  size: number;
  kind: ManifestFileKind;
  chunk_count: number;
  chunks_present: number;
  linktarget: string | null;
  content_kind: FileContentKind;
  content: string | null;
  preview_cap_bytes: number;
  /// Set when the backend has a registered text transformer for this
  /// file's extension. The frontend decides "show a decompile-spinner"
  /// purely from this field — never from the file extension.
  transformer: TransformerInfo | null;
  /// Set when the backend can build a structured tree (e.g. unity
  /// serialized files). Frontend toggles to the tree renderer when set.
  structured: StructuredInfo | null;
};

export type TransformerInfo = {
  /// MIME type of the transformer's output — frontend uses it to pick
  /// a syntax highlighter for the result.
  mime: string;
};

export type StructuredInfo = {
  /// Renderer hint — currently always `"unity-serialized"`.
  kind: string;
};

export type StructuredNode = {
  id: string;
  label: string;
  kind: string;
  badge?: string;
  /// When true, the backend wants this node collapsed by default.
  /// Frontend honors the flag verbatim — never decides expansion
  /// based on `kind` or `id`.
  default_collapsed?: boolean;
  /// When true, the row is hidden unless it matches the active
  /// search / facet filter. Used for secondary entries (e.g. nested
  /// .NET types) that would otherwise clutter the listing.
  hide_unless_matched?: boolean;
  /// When true, every descendant of this row is also visible whenever
  /// the row itself matches a filter (otherwise only matches +
  /// ancestors stay visible). Set on container-shaped rows like
  /// gameobjects.
  include_descendants_on_match?: boolean;
  /// Faceted attributes — each key is a filter dimension the frontend
  /// surfaces as a dropdown (multi-select whitelist). Format-specific;
  /// the renderer doesn't interpret keys.
  facets?: Record<string, string>;
  children: StructuredNode[];
};

export type StructuredTree = {
  kind: string;
  root: StructuredNode;
};

export type NodeContent = {
  mime: string;
  text: string;
};

/// Fetch the structured tree for a file. The path/branch identify the
/// file; backend dispatches based on the file's type.
export async function fetchFileStructured(
  appid: AppId,
  depotId: number,
  manifestId: string,
  branch: string,
  path: string,
): Promise<StructuredTree> {
  const qs = new URLSearchParams({ branch, path });
  const r = await fetch(
    `/api/apps/${appid}/depots/${depotId}/manifests/${manifestId}/file/structured?${qs}`,
    { method: "POST" },
  );
  if (!r.ok) throw new Error(await extractErrorMessage(r));
  return r.json();
}

/// Fetch lazy content for a single node inside a structured tree.
export async function fetchStructuredNodeContent(
  appid: AppId,
  depotId: number,
  manifestId: string,
  branch: string,
  path: string,
  nodeId: string,
): Promise<NodeContent> {
  const qs = new URLSearchParams({ branch, path });
  const r = await fetch(
    `/api/apps/${appid}/depots/${depotId}/manifests/${manifestId}/file/structured/node?${qs}`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ node_id: nodeId }),
    },
  );
  if (!r.ok) throw new Error(await extractErrorMessage(r));
  return r.json();
}

export function fetchFileView(
  appid: AppId,
  depotId: number,
  manifestId: string,
  branch: string,
  path: string,
): Promise<FileView> {
  const qs = new URLSearchParams({ branch, path });
  return getJson(`/api/apps/${appid}/depots/${depotId}/manifests/${manifestId}/file?${qs}`);
}

/// URL to the raw bytes of a file in a manifest. Backend streams the
/// file with a best-effort Content-Type (mime_guess by extension), so
/// `<img src=…>` / `<audio src=…>` work for browser-known media types.
export function fileRawUrl(
  appid: AppId,
  depotId: number,
  manifestId: string,
  branch: string,
  path: string,
): string {
  const qs = new URLSearchParams({ branch, path });
  return `/api/apps/${appid}/depots/${depotId}/manifests/${manifestId}/file/raw?${qs}`;
}

export type TransformedFile = {
  text: string;
  /// MIME type the backend declared for the output. Frontend uses this
  /// to pick a syntax highlighter (e.g. `text/x-csharp` → csharp lang).
  mime: string;
};

/// Fetch the transformer output (decompiled / disassembled text) for a
/// file. Backend caches the result on disk keyed by content sha, so
/// repeats are instant; the first call can take seconds to minutes
/// depending on the tool (ilspycmd on a big assembly takes ~30s+).
/// Returns null when the backend has no transformer for this extension.
export async function fetchTransformedFile(
  appid: AppId,
  depotId: number,
  manifestId: string,
  branch: string,
  path: string,
): Promise<TransformedFile | null> {
  const qs = new URLSearchParams({ branch, path });
  const r = await fetch(
    `/api/apps/${appid}/depots/${depotId}/manifests/${manifestId}/file/transformed?${qs}`,
  );
  if (r.status === 415) return null;
  if (!r.ok) throw new Error(await extractErrorMessage(r));
  // content-type comes back as e.g. "text/x-csharp; charset=utf-8";
  // strip the params so callers can match on the bare type.
  const mime = (r.headers.get("content-type") ?? "text/plain").split(";")[0].trim();
  const text = await r.text();
  return { text, mime };
}

/// Like fetchFileView but returns null when the file doesn't exist in
/// the target manifest (404) instead of throwing — useful for the
/// "compare to" diff view where "not present" is meaningful.
export async function fetchFileViewOptional(
  appid: AppId,
  depotId: number,
  manifestId: string,
  branch: string,
  path: string,
): Promise<FileView | null> {
  const qs = new URLSearchParams({ branch, path });
  const r = await fetch(`/api/apps/${appid}/depots/${depotId}/manifests/${manifestId}/file?${qs}`);
  if (r.status === 404) return null;
  if (!r.ok) throw new Error(await extractErrorMessage(r));
  return r.json();
}

export type ManifestFiles = {
  depot_id: number;
  manifest_id: string;
  files: ManifestFile[];
};

export function fetchManifestInfo(
  appid: AppId,
  depotId: number,
  manifestId: string,
  branch: string,
): Promise<ManifestInfo> {
  const qs = new URLSearchParams({ branch });
  return getJson(`/api/apps/${appid}/depots/${depotId}/manifests/${manifestId}?${qs}`);
}

export type Config = {
  store_root: string;
  mountpoint: string;
  restart_required: boolean;
};

export function fetchConfig(): Promise<Config> {
  return getJson("/api/config");
}

export async function patchConfig(patch: {
  store_root?: string;
  mountpoint?: string;
}): Promise<Config> {
  const r = await fetch("/api/config", {
    method: "PATCH",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(patch),
  });
  if (!r.ok) throw new Error(await extractErrorMessage(r));
  return r.json();
}

export type MountStatus = { state: "idle" } | { state: "mounted"; mountpoint: string };

export function fetchMountStatus(): Promise<MountStatus> {
  return getJson("/api/mount/status");
}

export async function startMount(): Promise<MountStatus> {
  const r = await fetch("/api/mount/start", { method: "POST" });
  if (!r.ok) throw new Error(await extractErrorMessage(r));
  return r.json();
}

export async function stopMount(): Promise<MountStatus> {
  const r = await fetch("/api/mount/stop", { method: "POST" });
  if (!r.ok) throw new Error(await extractErrorMessage(r));
  return r.json();
}

export type DownloadStats = {
  chunks_total: number;
  chunks_completed: number;
  chunks_failed: number;
  bytes_total: number;
  bytes_completed: number;
  last_error: string | null;
};

/// SSE `event: chunks` payload. The backend coalesces per-chunk landings
/// into per-file `chunks_present` updates so the frontend can patch its
/// `manifest-files` query cache without a refetch.
export type ChunkUpdate = {
  depot_id: number;
  manifest_id: string;
  files: { path: string; chunks_present: number }[];
};

export type EnqueueSummary = {
  enqueued_chunks: number;
  enqueued_bytes: number;
  already_present_chunks: number;
};

export async function downloadManifest(
  appid: AppId,
  depotId: number,
  manifestId: string,
  body: { branch: string; paths?: string[] },
): Promise<EnqueueSummary> {
  const r = await fetch(`/api/apps/${appid}/depots/${depotId}/manifests/${manifestId}/download`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!r.ok) {
    let message = `${r.status} ${r.statusText}`;
    try {
      const j = await r.json();
      if (j && typeof j.error === "string") message = j.error;
    } catch {
      /* ignore */
    }
    throw new Error(message);
  }
  return r.json();
}

export function fetchDownloads(): Promise<DownloadStats> {
  return getJson("/api/downloads");
}

export async function cancelDownloads(): Promise<DownloadStats> {
  const r = await fetch("/api/downloads/cancel", { method: "POST" });
  if (!r.ok) throw new Error(`${r.status} ${r.statusText}`);
  return r.json();
}

export function fetchManifestFiles(
  appid: AppId,
  depotId: number,
  manifestId: string,
  branch: string,
): Promise<ManifestFiles> {
  const qs = new URLSearchParams({ branch });
  return getJson(`/api/apps/${appid}/depots/${depotId}/manifests/${manifestId}/files?${qs}`);
}
